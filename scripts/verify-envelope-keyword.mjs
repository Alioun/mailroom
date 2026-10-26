import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { request } from 'node:http'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
const base = 'http://127.0.0.1:4173'
const metadataPath = join(root, '.audit/envelope-keyword-retained.json')
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const pause = ms => new Promise(r => setTimeout(r, ms))
function identity(m) {
  assert.equal(execFileSync('ps', ['-p', String(m.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${m.binary} --exact ${m.test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(m.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(m.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
}
if (process.argv.includes('--stop')) {
  const m = JSON.parse(await readFile(metadataPath, 'utf8')); identity(m); assert.equal(sha(await readFile(m.binary)), m.binary_sha256)
  process.kill(m.pid, 'SIGTERM')
  for (let i = 0; i < 300; i++) { try { process.kill(m.pid, 0) } catch (error) { if (error.code !== 'ESRCH') throw error; await writeFile(join(root, '.audit/envelope-keyword-stopped.json'), JSON.stringify({ ...m, stopped: new Date().toISOString() }, null, 2)); process.exit(0) }; await pause(20) }
  throw Error('Verified fixture did not stop')
}
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Identity-stop the retained fixture before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const binary_sha256 = sha(await readFile(binary))
const test = 'server::accounts::envelope_keyword_server_fixture'
const observer = 'acquisition::workspace_authority::envelope_tests::envelope_fixture_observer'
const directory = await mkdtemp(join(root, '.audit/envelope-keyword-fixture-'))
let server, token, account
const checks = [], processes = []
async function observe() {
  execFileSync(binary, ['--exact', observer], { cwd: root, env: { ...process.env, MAILROOM_ENVELOPE_FIXTURE: directory }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'envelope-observations.json'), 'utf8'))
}
async function start() {
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_ENVELOPE_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 500; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; account = (await observe()).account; processes.push({ event: 'started', pid: server.pid, argv: `${binary} --exact ${test} --nocapture`, cwd: root, binary_sha256, time: new Date().toISOString() }); return }
    await pause(20)
  }
  throw Error('Envelope fixture startup timed out')
}
async function stop() { identity({ pid: server.pid, binary, test }); assert.equal(sha(await readFile(binary)), binary_sha256); const pid = server.pid; const done = once(server, 'exit'); server.kill('SIGTERM'); await done; processes.push({ event: 'identity_verified_stopped', pid, argv: `${binary} --exact ${test} --nocapture`, cwd: root, binary_sha256, signal: 'SIGTERM', time: new Date().toISOString() }); server = undefined }
async function post(path, body, expected = 200, headers = {}) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...headers }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, path); assert.equal(response.headers.get('cache-control'), 'no-store'); return response.json()
}
async function terminal(expected = 'complete') {
  for (let i = 0; i < 1200; i++) {
    const value = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (value.job?.progress.state !== 'running') { assert.equal(value.job?.progress.state, expected); return }
    await pause(10)
  }
  throw Error('Fixture operation timeout')
}
const command = (path, action, fields = {}) => post(path, { account, command: { action, ...fields } })
try {
  await start()
  assert.equal((await fetch(`${base}/api/accounts/proof/preview`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.invalid' }, { 'Sec-Fetch-Site': 'cross-site' }]) await post('proof/preview', { account }, 403, headers)
  for (const field of ['context', 'delivery', 'recipient_evidence', 'capabilities', 'source', 'endpoint', 'trust']) await post('proof/preview', { account, [field]: 'claim' }, 422)
  const hostileHost = await new Promise((resolve, reject) => { const req = request(`${base}/api/session`, { headers: { Host: 'foreign.invalid' } }, res => { res.resume(); resolve(res.statusCode) }); req.on('error', reject); req.end() }); assert.equal(hostileHost, 403)
  const page = await fetch(base); assert.match(page.headers.get('content-security-policy'), /default-src 'none'/); assert.equal(page.headers.get('cache-control'), 'no-store')
  const view = await post('workspace/open', { account })
  const initial = await observe()
  const before = await post('proof/preview', { account })
  const input = structuredClone(view.workspace.input)
  input.documents.override_yaml += `\ndelivery_tags:\n  - id: user.route\n    enabled: true\n    accounts: [${account}]\n    envelope_to: route@business.test\n    add_keyword: $route\n`
  await post('workspace/save', { account, revision: view.revision, input })
  await post('proof/download', { account, receipt: before.receipt, confirmed: true }, 422)
  let proof = await post('proof/preview', { account })
  assert.deepEqual(proof.proof.artifact.folders, before.proof.artifact.folders)
  assert.notEqual(proof.proof.artifact.identity, before.proof.artifact.identity)
  assert.notEqual(proof.receipt, before.receipt)
  assert.deepEqual(proof.proof.decisions[0].keyword_additions, ['$route'])
  assert.equal(proof.proof.decisions[0].delivery_tags[0].status, 'match')
  assert.equal(proof.proof.decisions[1].delivery_tags[0].status, 'no_match')
  for (const d of proof.proof.decisions.slice(2)) { assert.equal(d.review, true); assert.deepEqual(d.keyword_additions, []); assert.deepEqual(d.copies, []); assert.equal(d.mark_read, false) }
  assert.equal(proof.proof.decisions[5].delivery_tags[0].status, 'no_match')
  const heldInput = structuredClone(input)
  heldInput.documents.override_yaml = heldInput.documents.override_yaml.replace('business.test: business', 'business.test: unknown')
  assert.notEqual(heldInput.documents.override_yaml, input.documents.override_yaml)
  await post('workspace/save', { account, revision: (await post('workspace/open', { account })).revision, input: heldInput })
  const held = await post('proof/preview', { account })
  assert.equal(held.proof.decisions[0].delivery_tags[0].status, 'held_tagged')
  assert.equal(held.proof.decisions[0].review, true)
  assert.deepEqual(held.proof.decisions[0].keyword_additions, ['$route'])
  assert.deepEqual(held.proof.decisions[0].copies, [])
  assert.equal(held.proof.decisions[0].mark_read, false)
  assert.equal(held.proof.decisions[0].destination.kind, 'inbox')
  assert.equal(held.proof.decisions[0].matched_rule, 'system.role_safety')
  assert.deepEqual(held.proof.decisions.map(d => d.delivery_tags[0].status), ['held_tagged', 'no_match', 'unknown', 'unknown', 'unknown', 'no_match'])
  assert.deepEqual(held.proof.decisions.map(d => d.delivery_status), ['known', 'known', 'unavailable', 'unsupported', 'ambiguous', 'known'])
  for (const d of held.proof.decisions.slice(1)) { assert.equal(d.review, true); assert.deepEqual(d.keyword_additions, []); assert.deepEqual(d.copies, []); assert.equal(d.mark_read, false) }
  assert.deepEqual(held.proof.artifact.folders, before.proof.artifact.folders)
  assert.ok(held.proof.artifact.executable.indexOf('addflag "$route"') < held.proof.artifact.executable.indexOf('#@ system.role_safety'))
  const heldDownload = await post('proof/download', { account, receipt: held.receipt, confirmed: true })
  assert.equal(heldDownload.sieve, held.proof.artifact.executable); assert.equal(heldDownload.identity, held.proof.artifact.identity)
  checks.push('Held saved proof: verified envelope match tagged and held in Inbox without read change or copies; different, unavailable, unsupported and ambiguous evidence keep distinct untagged outcomes; held tag precedes role-safety keep; exact held download')
  await post('workspace/save', { account, revision: (await post('workspace/open', { account })).revision, input })
  proof = await post('proof/preview', { account })
  assert.equal((await observe()).save_preserved, initial.save_preserved, 'Complete canonical aggregate excludes only intended workspace/export/revision changes during save and proof')
  await post('proof/download', { account, receipt: proof.receipt, confirmed: false }, 422)
  const downloaded = await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  assert.equal(downloaded.sieve, proof.proof.artifact.executable); assert.equal(downloaded.identity, proof.proof.artifact.identity)
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true }, 422)
  checks.push('Protected saved HTTP save/proof/fresh exact download; known envelope tags once, different envelope no-match, unavailable/unsupported/ambiguous held; tag-only folders unchanged')
  const fresh = await post('proof/preview', { account })
  const ledger = await command('ledger', 'open')
  const ledgerReview = await command('ledger', 'preview', { batch: { content_version: ledger.snapshot.content_version, revision: ledger.revision, enabled: ledger.snapshot.document.enabled, entries: [] } })
  assert.equal(ledgerReview.proof.proof.artifact.identity, fresh.proof.artifact.identity)
  assert.deepEqual(ledgerReview.proof.proof.decisions, fresh.proof.decisions)
  const proposals = await command('proposals', 'open')
  assert.equal(proposals.current.proof.artifact.identity, fresh.proof.artifact.identity)
  assert.deepEqual(proposals.current.proof.decisions, fresh.proof.decisions)
  checks.push('Saved ledger review and proposal current proof resolve the same installed envelope context, immutable artifact and parsed message decisions')
  const folders = await command('folders', 'preview')
  assert.equal(folders.operation.artifact, fresh.proof.artifact.identity); assert.equal(folders.operation.proof_binding, fresh.receipt); assert.deepEqual(folders.operation.plan.logical_leaves, fresh.proof.artifact.folders)
  const transaction = await command('sieve-transactions', 'preview')
  assert.equal(transaction.transaction.artifact, fresh.proof.artifact.identity)
  assert.equal(transaction.transaction.proof_binding, fresh.receipt)
  const candidate = transaction.transaction.after.bodies.find(b => b.name === transaction.transaction.target)
  assert.equal(Buffer.from(candidate.bytes).toString(), downloaded.sieve)
  await command('sieve-transactions', 'confirm', { receipt: fresh.receipt, confirmed: true }); await terminal('failed')
  await post('sieve-transactions', { account: 'wrong-account', command: { action: 'confirm', receipt: transaction.receipt, confirmed: true } }, 422)
  await command('sieve-transactions', 'confirm', { receipt: transaction.receipt, confirmed: true }); await terminal()
  const deployed = await observe()
  assert.equal(deployed.provider_bytes, downloaded.sieve); assert.equal(deployed.provider_target, sha(Buffer.from(downloaded.sieve)))
  assert.equal(deployed.preserved, initial.preserved); assert.deepEqual(deployed.independent_effects, initial.independent_effects)
  assert.deepEqual(deployed.exports.slice(0, initial.exports.length), initial.exports)
  await command('sieve-transactions', 'confirm', { receipt: transaction.receipt, confirmed: true }); await terminal('failed')
  const currentFolders = await command('folders', 'preview')
  assert.equal(currentFolders.operation.artifact, downloaded.identity)
  await command('folders', 'confirm', { receipt: currentFolders.receipt, confirmed: true }); await terminal()
  const created = await observe(); assert.ok(created.folder_commands.includes('CREATE')); assert.equal(created.preserved, initial.preserved); assert.deepEqual(created.independent_effects, initial.independent_effects)
  checks.push('Actual verified TLS transaction candidate and stored bytes equal confirmed download and artifact; same-artifact real TLS folder CREATE; purpose/replay/account denials; complete unrelated digest, exact old exports and independent effects preserved')
  const stable = await post('proof/preview', { account }); const stableState = await observe(); const oldToken = token
  await stop(); await start(); assert.notEqual(token, oldToken)
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  const reopened = await post('proof/preview', { account }); assert.equal(reopened.proof.artifact.identity, stable.proof.artifact.identity); assert.equal(reopened.receipt, stable.receipt); assert.equal((await observe()).complete, stableState.complete)
  await stop(); await writeFile(join(directory, 'envelope-authority'), 'changed-delivery-epoch'); await start()
  const changed = await post('proof/preview', { account }); assert.ok(changed.proof.decisions.every(d => d.review && d.keyword_additions.length === 0)); assert.notEqual(changed.receipt, stable.receipt)
  await post('proof/download', { account, receipt: stable.receipt, confirmed: true }, 422)
  await stop(); await writeFile(join(directory, 'envelope-authority'), ''); await start()
  const removed = await post('proof/preview', { account }); assert.ok(removed.proof.artifact.omissions.some(o => o.rule === 'user.route')); assert.ok(removed.proof.decisions.every(d => d.review && d.keyword_additions.length === 0))
  assert.equal((await observe()).complete, stableState.complete)
  checks.push('Actual process replacement preserves complete canonical saved state and exact export history; changed source downgrades persisted evidence; removal omits whole tag; old session and stale proof denied')
  await stop(); await writeFile(join(directory, 'envelope-authority'), 'delivery-fixture-v1'); await start()
  const metadata = { pid: server.pid, binary, binary_sha256, test, directory, base, account, cwd: root }
  identity(metadata); await writeFile(metadataPath, JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/envelope-keyword-http.json'), JSON.stringify({ checks, artifact: downloaded.identity, bytes_sha256: sha(Buffer.from(downloaded.sieve)), schema: created.schema, metadata: metadataPath, processes, preservation: { complete_across_restart_and_source_changes: stableState.complete, unrelated_across_transaction_and_folders: initial.preserved, save_except_workspace_export_revision: initial.save_preserved, independent_effects: sha(Buffer.from(JSON.stringify(initial.independent_effects))), historical_exports: initial.exports }, time: new Date().toISOString(), independent_sieve: 'blocked; TLS fixture stores bytes without executing Sieve', held_tagging: 'unresolved' }, null, 2) + '\n')
  server.unref(); console.log(JSON.stringify({ checks, retained: metadataPath }, null, 2))
} catch (error) { if (server) await stop(); throw error }
