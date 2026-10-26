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
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified retained fake before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const binary_sha256 = sha(await readFile(binary))
const directory = await mkdtemp(join(root, '.audit/provider-evidence-fixture-'))
const test = 'server::accounts::provider_evidence_server_fixture'
let server, token
async function start() {
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_PROVIDER_EVIDENCE_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 300; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await new Promise(r => setTimeout(r, 40))
  }
  throw Error('Local provider fixture startup timed out')
}
async function stop() {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  assert.equal(sha(await readFile(binary)), binary_sha256)
  const done = once(server, 'exit'); server.kill('SIGTERM'); await done; server = undefined
}
async function get(path) { return fetch(base + path, { headers: { 'X-Mailroom-Token': token } }) }
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `Unexpected status for ${path}`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
async function terminal(expected = 'complete') {
  for (let i = 0; i < 1500; i++) {
    const s = await (await get('/api/accounts/status')).json()
    if (s.job?.progress.state !== 'running') { assert.equal(s.job?.progress.state, expected); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Local protocol acquisition did not finish')
}
async function scan(account, mode, expected = 'complete') {
  await writeFile(join(directory, 'provider-mode'), mode)
  await post('scan', { account }); await terminal(expected)
  return post('open', { account })
}
async function reopen(account) {
  execFileSync(binary, ['--exact', 'acquisition::imap::tests::provider_evidence_reopen_worker'], { cwd: root, env: { ...process.env, MAILROOM_PROVIDER_EVIDENCE_FIXTURE: directory, MAILROOM_PROVIDER_EVIDENCE_ACCOUNT: account }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'provider-reopen.json'), 'utf8'))
}
await start()
try {
  assert.equal((await fetch(`${base}/api/accounts/open`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.example' }, { 'Sec-Fetch-Site': 'cross-site' }]) {
    assert.equal((await fetch(`${base}/api/accounts/open`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...headers }, body: '{}' })).status, 403)
  }
  const badHost = await new Promise((resolve, reject) => {
    const req = request(`${base}/api/session`, { headers: { Host: 'foreign.example' } }, response => { response.resume(); resolve(response.statusCode) })
    req.on('error', reject); req.end()
  })
  assert.equal(badHost, 403)
  await post('synthetic-gmail', {})
  const account = await terminal()
  for (const injection of [{ trusted: true }, { capabilities: ['fileinto'] }, { endpoint: 'localhost' }, { synthetic: true }, { adapter: 'trusted' }, { recipient_evidence: 'verified' }, { dkim_pass: true }]) {
    await post('scan', { account, ...injection }, 422)
  }
  let view = await scan(account, 'normal')
  assert.equal(view.evidence.current, true)
  const good = view.evidence.last_successful
  assert.equal(good.adapter, 'mailroom/read-only-imap/v1')
  assert.deepEqual(good.imap_capabilities, ['IMAP4REV1', 'SPECIAL-USE', 'X-GM-EXT-1'])
  assert.equal(good.folders.length, 7)
  assert.equal(good.folders.filter(f => f.selection === 'selected').length, 1)
  assert.equal(view.evidence.spam.messages, 3)
  assert.equal(Object.values(view.census.folders)[0].messages['1'].headers.dkim_coverage.status, 'unknown')
  assert.equal(view.census.winddown_evidence.complete_since, null)
  assert.deepEqual(view.census.winddown_evidence.deliveries, {})
  const independent = await reopen(account)
  assert.equal(independent.schema, 17)
  assert.deepEqual(independent.evidence.last_successful, good)
  assert.equal(independent.history_unknown && independent.delivery_unknown, true)
  const bytes = await readFile(join(directory, 'data', `${account}.census.enc`))
  assert.equal(bytes.subarray(0, 16).toString(), 'MAILROOM-CENSUS\x01')
  assert.equal(bytes.includes(Buffer.from('Alle Nachrichten')), false)
  const oldToken = token
  await stop(); await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  view = await post('open', { account })
  assert.deepEqual(view.evidence.last_successful, good)
  assert.equal(sha(await readFile(join(directory, 'data', `${account}.census.enc`))), sha(bytes))
  const as_of = Math.floor(Date.now() / 1000) + 1
  const report = await post('report', { account, as_of })
  const summary = await post('summary/review', { account, as_of, version: report.version, approved: [] })
  const winddown = await post('winddown', { account, as_of, command: { action: 'open' } })
  assert.equal(winddown.report.sources[0].spam_count, 3)
  assert.equal(winddown.report.sources[0].spam_observation.source, 'status_messages')
  assert.ok(winddown.report.blockers.includes('historical_coverage_unknown'))
  assert.equal(winddown.report.sources[0].coverage_since, null)
  view = await scan(account, 'partial_list', 'failed')
  assert.deepEqual(view.evidence.last_successful, good)
  assert.equal(view.evidence.current, false)
  assert.equal(view.evidence.latest.checkpoint, null)
  assert.equal(view.evidence.spam.kind, 'unknown')
  assert.deepEqual(view.census.folder_roles, Object.fromEntries(good.folders.map(f => [f.name, f.roles])))
  const old = await reopen(account)
  assert.deepEqual(old.evidence.last_successful, good)
  await post('summary/export', { account, as_of, version: report.version, approved: [], receipt: summary.receipt, confirmed: true }, 422)
  const failedWinddown = await post('winddown', { account, as_of, command: { action: 'open' } })
  assert.equal(failedWinddown.report.sources[0].spam_count, null)
  assert.equal(failedWinddown.report.sources[0].spam_observation.reason, 'older_snapshot')
  view = await scan(account, 'changed')
  assert.equal(view.census.folder_roles['Opaque mailbox'], undefined)
  assert.equal(view.evidence.spam.reason, 'no_junk_role')
  view = await scan(account, 'zero')
  assert.equal(view.evidence.spam.messages, 0)
  view = await scan(account, 'ambiguous')
  assert.equal(view.evidence.spam.reason, 'ambiguous')
  view = await scan(account, 'zero')
  await post('synthetic', {})
  const mailbox = await terminal()
  await scan(mailbox, 'normal')
  const sample = await (await get('/api/sample')).json()
  let workspace = await post('workspace/open', { account: mailbox })
  workspace = await post('workspace/save', { account: mailbox, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  const proof = await post('proof/preview', { account: mailbox })
  assert.equal(proof.download_available, true)
  assert.equal(proof.proof.artifact.context.recipient_evidence, 'unknown')
  assert.deepEqual(proof.proof.artifact.context.capabilities, [])
  assert.ok(proof.proof.decisions.length > 0)
  assert.ok(proof.proof.decisions.every(d => d.destination.kind === 'inbox' && d.copies.length === 0))
  const before = await reopen(mailbox)
  await scan(mailbox, 'partial_list', 'failed')
  await post('proof/download', { account: mailbox, receipt: proof.receipt, confirmed: true }, 422)
  const after = await reopen(mailbox)
  assert.equal(before.other_state_sha256, after.other_state_sha256)
  await scan(account, 'zero')
  const wire = (await readFile(join(directory, 'provider-wire.log'), 'utf8')).trim().split('\n')
  let selected
  for (const command of wire) {
    assert.ok(command === 'LOGIN [REDACTED]' || command === 'CAPABILITY' || command.startsWith('LIST ') || command.startsWith('STATUS ') || command.startsWith('EXAMINE ') || command === 'UID SEARCH ALL' || command.startsWith('UID FETCH '))
    if (command.startsWith('EXAMINE ')) { selected = command; assert.ok(!/Indesirable|Other Junk|Namespace/.test(command)) }
    if (command.startsWith('UID FETCH ')) assert.ok(selected && !/Indesirable|Other Junk|Namespace/.test(selected))
    assert.ok(!/STORE|MOVE|COPY|APPEND|CREATE|DELETE|EXPUNGE|CLOSE|SELECT |BODY\[\]|password|invented/.test(command))
  }
  const metadata = { base, pid: server.pid, binary, binary_sha256, directory, account, mailbox, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(), test }
  await writeFile(join(root, '.audit/provider-evidence-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/provider-evidence-runtime.json'), JSON.stringify({ ...metadata, checks: 16, commands: wire.length, encrypted_reopen: true, restart: true, stale_report_rejected: true, stale_proof_rejected: true, conservative_context: true, preserved_aggregates: true }, null, 2) + '\n')
  server.unref()
  console.log(`PASS: 16 actual TLS acquisition, encrypted reopen, process restart, freshness, topology, count uncertainty, stale report/proof, authority and protected HTTP checks; ${wire.length} redacted read-only commands`)
} catch (error) {
  if (server) await stop()
  throw error
}
