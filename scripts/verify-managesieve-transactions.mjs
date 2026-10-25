import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
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
const test = 'server::accounts::managesieve_transactions_server_fixture'
const worker = 'acquisition::sieve_transaction::tests::transaction_fixture_worker'
let server, directory, token, account
const checks = [], crashes = []
const pause = ms => new Promise(r => setTimeout(r, ms))
async function start(fresh = false) {
  if (fresh) directory = await mkdtemp(join(root, '.audit/managesieve-transactions-fixture-'))
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_TRANSACTIONS_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 400; i++) {
    assert.equal(server.exitCode, null, 'Fixture exited during startup')
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; account = (await observe()).account; return }
    await pause(20)
  }
  throw Error('Transaction fixture startup timed out')
}
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
}
async function stop(signal = 'SIGTERM') { identity(); assert.equal(sha(await readFile(binary)), binary_sha256); const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function observe(action = '') {
  execFileSync(binary, ['--exact', worker], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_TRANSACTIONS_FIXTURE: directory, MAILROOM_SIEVE_WORKER_ACTION: action }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'transaction-observations.json'), 'utf8'))
}
async function post(path, body, expected = 200, extraHeaders = {}) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...extraHeaders }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `Unexpected status for ${path}`); assert.equal(response.headers.get('cache-control'), 'no-store'); return response.json()
}
const command = (action, fields = {}, expected = 200) => post('sieve-transactions', { account, command: { action, ...fields } }, expected)
async function terminal(expected = 'complete') {
  for (let i = 0; i < 1000; i++) {
    const response = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }); const s = await response.json()
    if (s.job?.progress.state !== 'running') { assert.equal(s.job?.progress.state, expected, 'Unexpected transaction worker outcome'); return }
    await pause(10)
  }
  throw Error('Transaction job did not finish')
}
async function confirm(preview, expected = 'complete') {
  const comp = preview.transaction.compensates
  await command(comp ? 'compensation_confirm' : 'confirm', { ...(comp ? { operation: comp } : {}), receipt: preview.receipt, confirmed: true }); await terminal(expected)
}
function effects(snapshot) { return snapshot.commands.filter(c => ['PUTSCRIPT', 'SETACTIVE'].includes(c)).join(',') }
async function boundary(name) {
  await writeFile(join(directory, 'transaction-boundary'), name)
}
async function reached(name) {
  for (let i = 0; i < 1200; i++) { if (await readFile(join(directory, `reached-${name}`)).then(() => true, () => false)) return; await pause(10) }
  throw Error(`Crash boundary not reached: ${name}`)
}
async function crash(name, compensation = false) {
  await start(true)
  let preview = await command('preview')
  const initial = await observe()
  if (compensation) { await confirm(preview); preview = await command('compensation_preview', { operation: preview.transaction.id }) }
  await boundary(name)
  const comp = preview.transaction.compensates
  await command(comp ? 'compensation_confirm' : 'confirm', { ...(comp ? { operation: comp } : {}), receipt: preview.receipt, confirmed: true })
  await reached(name)
  const oldToken = token
  await stop('SIGKILL')
  await writeFile(join(directory, 'transaction-boundary'), '')
  const beforeRestart = await observe()
  await start()
  assert.notEqual(token, oldToken)
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  const reopened = await command('status')
  const last = reopened.journal.history.at(-1)
  assert.ok(['uncertain', 'stopped'].includes(last.status), 'Restart cannot complete or resume an unfinished operation')
  assert.equal((await observe()).provider_image, beforeRestart.provider_image, 'Restart preserves independent provider state')
  await command('reconcile', { operation: last.id })
  const first = await observe()
  await command('reconcile', { operation: last.id })
  await command('status')
  const second = await observe()
  assert.equal(effects(second), effects(beforeRestart), 'Reconciliation never resends an effect')
  assert.equal(first.revision, second.revision, 'Repeated reconciliation and status do not persist again')
  assert.equal(JSON.stringify(first.transactions), JSON.stringify(second.transactions), 'Repeated reconciliation converges without changing journal semantics')
  assert.equal(second.unrelated, initial.unrelated, 'Crash/reopen/reconcile preserves every unrelated aggregate')
  assert.deepEqual(second.external_unrelated, initial.external_unrelated, 'Crash/reopen/reconcile preserves independent moves, sends and unsubscribe effects')
  if (compensation && (await command('status')).journal.history.at(-1).status === 'fresh_review') {
    const fresh = await command('compensation_preview', { operation: comp }); await confirm(fresh)
    assert.equal((await observe()).provider_image, initial.provider_image, 'Interrupted compensation needs a fresh exact approval to finish')
    assert.deepEqual((await observe()).external_unrelated, initial.external_unrelated)
  }
  crashes.push({ boundary: name, compensation, effects_before_reconcile: effects(beforeRestart), effects_after_reconcile: effects(second), initial_provider: initial.provider_image, reopened_provider: beforeRestart.provider_image })
  await stop()
}
try {
  await start(true)
  assert.equal((await fetch(`${base}/api/accounts/sieve-transactions`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.example' }, { 'Sec-Fetch-Site': 'cross-site' }]) await post('sieve-transactions', {}, 403, headers)
  for (const field of ['endpoint', 'script', 'name', 'ownership', 'permit', 'source', 'trust', 'baseline']) {
    await post('sieve-transactions', { account, [field]: 'caller-claim', command: { action: 'preview' } }, 422)
    await command('preview', { [field]: 'caller-claim' }, 422)
  }
  checks.push('Protected HTTP and unknown-field rejection without caller trust or script input')
  const p = await command('preview')
  const initial = await observe()
  await post('sieve-transactions', { account: 'other-fixture', command: { action: 'confirm', receipt: p.receipt, confirmed: true } }, 422)
  assert.equal(effects(await observe()), '')
  assert.equal(p.transaction.before.names.find(n => n.active), undefined)
  assert.ok(p.effects.includes('upload') && p.effects.includes('activate'))
  const exportProof = await post('proof/preview', { account })
  await command('confirm', { receipt: exportProof.receipt, confirmed: true }); await terminal('failed')
  assert.equal(effects(await observe()), '')
  await command('confirm', { receipt: `${p.receipt.slice(0, -1)}${p.receipt.endsWith('0') ? '1' : '0'}`, confirmed: true }); await terminal('failed')
  await confirm(p)
  const deployed = await observe()
  assert.equal(deployed.schema, 15); assert.equal(deployed.provider_target, p.transaction.after.bodies.find(b => b.name === p.transaction.target).sha256)
  assert.equal(deployed.active, 'Open-Xchange'); assert.equal(deployed.unrelated, initial.unrelated)
  assert.deepEqual(deployed.external_unrelated, initial.external_unrelated)
  assert.ok((await command('status')).journal.history[0].steps.every(s => s.acknowledgment && s.outcome === 'observed'))
  await confirm(p, 'failed'); assert.equal(effects(await observe()), effects(deployed))
  checks.push('Exact conservative artifact, distinct acknowledgments/readback, purpose/alteration/replay denial and unrelated aggregate preservation')
  const c = await command('compensation_preview', { operation: p.transaction.id }); assert.ok(c.effects.includes('disable'))
  await command('confirm', { receipt: c.receipt, confirmed: true }); await terminal('failed')
  await confirm(c)
  assert.equal((await observe()).provider_image, initial.provider_image)
  assert.deepEqual((await observe()).external_unrelated, initial.external_unrelated)
  checks.push('Separate conditional compensation restores exact bytes and explicitly confirmed empty active selection')
  await stop()
  await start(true)
  const changed = await command('preview'); await observe('external_target'); await confirm(changed, 'failed')
  assert.equal(effects(await observe()), '')
  await observe('external_active'); await command('preview', {}, 422)
  await observe('missing_target'); await command('preview', {}, 422)
  checks.push('External target edits, foreign active selection and missing target refuse without effects')
  await stop()
  await start(true)
  const ledgerPreview = await command('preview')
  const ledgerPath = join(directory, 'ledger/addresses.yaml')
  await writeFile(ledgerPath, (await readFile(ledgerPath, 'utf8')) + '\n# invented external authority change\n')
  await confirm(ledgerPreview, 'failed'); assert.equal(effects(await observe()), '')
  checks.push('External authoritative ledger changes invalidate approval without effects')
  await stop()
  for (const external of ['external_target', 'external_active']) {
    await start(true)
    const compensationConflict = await command('preview'); await confirm(compensationConflict)
    await observe(external)
    await command('compensation_preview', { operation: compensationConflict.transaction.id }, 422)
    await stop()
  }
  checks.push('Conditional compensation refuses external target edits and changed active selection')
  for (const external of ['external_target', 'external_active', 'external_epoch']) {
    await start(true)
    const interrupted = await command('preview'); await boundary('provider_upload_effect')
    await command('confirm', { receipt: interrupted.receipt, confirmed: true }); await reached('provider_upload_effect'); await stop('SIGKILL')
    await observe(external); const prior = await observe()
    await writeFile(join(directory, 'transaction-boundary'), ''); await start()
    await command('reconcile', { operation: interrupted.transaction.id }, external === 'external_epoch' ? 422 : 200)
    assert.equal((await command('status')).journal.history[0].status, 'conflict')
    const first = await observe()
    await command('reconcile', { operation: interrupted.transaction.id }, external === 'external_epoch' ? 422 : 200)
    const second = await observe()
    assert.equal(effects(second), effects(prior)); assert.equal(second.provider_image, prior.provider_image); assert.equal(first.revision, second.revision)
    await stop()
  }
  checks.push('Recovery blocks third-state target/active edits and changed installed ownership epoch without repeating effects')
  await start(true)
  const raced = await command('preview'); await boundary('upload_intent')
  await command('confirm', { receipt: raced.receipt, confirmed: true }); await reached('upload_intent')
  await observe('external_target')
  await writeFile(join(directory, 'continue-upload_intent'), 'continue'); await terminal()
  const raceResult = await observe()
  assert.equal(raceResult.provider_target, raced.transaction.after.bodies.find(b => b.name === 'Open-Xchange').sha256)
  checks.push('An injected edit after the final read is overwritten, exposing the unavoidable non-CAS race')
  await stop()
  for (const name of ['transaction_consumed', 'upload_intent', 'provider_upload_effect', 'upload_acknowledged', 'upload_observed', 'activation_intent', 'provider_activation_effect', 'activation_acknowledged']) await crash(name)
  for (const name of ['compensation_upload_intent', 'provider_upload_effect', 'compensation_upload_acknowledged', 'compensation_upload_observed', 'compensation_activation_intent', 'provider_activation_effect', 'compensation_activation_acknowledged']) await crash(name, true)
  checks.push('Actual SIGKILL/replacement around intents, provider effects, acknowledgments and between upload/activation including compensation')
  await start(true)
  await boundary('upload_observed')
  const cancelPreview = await command('preview')
  await command('confirm', { receipt: cancelPreview.receipt, confirmed: true }); await reached('upload_observed')
  await post('scan', { account }, 422)
  await command('confirm', { receipt: cancelPreview.receipt, confirmed: true }, 422)
  await command('cancel'); await writeFile(join(directory, 'continue-upload_observed'), 'continue'); await terminal('cancelled')
  const canceled = await observe(); assert.equal(effects(canceled), 'PUTSCRIPT'); assert.equal(canceled.active, null)
  await command('reconcile', { operation: cancelPreview.transaction.id })
  const compensate = await command('compensation_preview', { operation: cancelPreview.transaction.id }); await confirm(compensate)
  checks.push('Stop preserves completed upload, rejects competing scan and offers separately confirmed compensation')
  await stop()
  await start(true)
  identity()
  const metadata = { base, pid: server.pid, binary, binary_sha256, directory, account, test, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/managesieve-transactions-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/managesieve-transactions-runtime.json'), JSON.stringify({ ...metadata, checks, crashes, synthetic_only: true }, null, 2) + '\n')
  server.unref()
  console.log(`PASS: ${checks.length} transaction HTTP groups and ${crashes.length} actual SIGKILL/replacement boundaries against durable local TLS state`)
} catch (error) { if (server) await stop(); throw error }
