import assert from 'node:assert/strict'
import { request as httpRequest } from 'node:http'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile, readdir, stat, unlink } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the directly verified fake server first')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const fixtureBinaryHash = createHash('sha256').update(await readFile(binary)).digest('hex')
const directory = await mkdtemp(join(root, '.audit/account-lifecycle-fixture-'))
let server, token, sample
const results = []
async function start(boundary) {
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory }
  delete env.MAILROOM_WINDDOWN_FIXTURE
  delete env.MAILROOM_LIFECYCLE_BOUNDARY
  if (boundary) env.MAILROOM_LIFECYCLE_BOUNDARY = boundary
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env, stdio: 'ignore', detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const r = await fetch(`${base}/api/session`).catch(() => null)
    if (r?.ok) { token = (await r.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw Error('Fake lifecycle startup timeout')
}
async function stop(signal = 'SIGTERM') {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact server::accounts::durable_browser_server_fixture --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root.replace(/\/$/, '')}\n`))
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(server.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  assert.equal(createHash('sha256').update(await readFile(binary)).digest('hex'), fixtureBinaryHash)
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  const value = await response.json()
  assert.equal(response.status, expected, `${path} ${body.action ?? body.command?.action ?? ''} HTTP ${expected}, ${value.error ?? ''}`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return value
}
const lifecycle = (body, expected) => post('lifecycle', body, expected)
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Fake census timeout')
}
async function newAccount(gmail = false) {
  await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
  const account = await terminal()
  await post('scan', { account }); await terminal()
  const ws = await post('workspace/open', { account })
  const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test') }
  await post('workspace/save', { account, revision: ws.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  await post('open', { account })
  return account
}
function subprocess(action, account) {
  execFileSync(binary, ['--exact', 'acquisition::accounts::lifecycle::tests::lifecycle_process_worker'], { cwd: root, env: { ...process.env, MAILROOM_LIFECYCLE_PROCESS_DIR: directory, MAILROOM_LIFECYCLE_PROCESS_ACTION: action }, stdio: 'ignore' })
}
async function snapshot(account) {
  execFileSync(binary, ['--exact', 'acquisition::accounts::lifecycle::tests::lifecycle_snapshot_worker'], { cwd: root, env: { ...process.env, MAILROOM_LIFECYCLE_SNAPSHOT_DIR: directory, MAILROOM_LIFECYCLE_ACCOUNT: account }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'lifecycle-observation.json'), 'utf8'))
}
async function metadata(account) { return JSON.parse(await readFile(join(directory, 'config', `${account}.json`), 'utf8')) }
async function reached(phase) {
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(directory, `reached-${phase}`)).then(() => true, () => false)) return
    assert.equal(server.exitCode, null)
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error(`Lifecycle boundary ${phase} was not reached`)
}
async function crashAction(phase, action, account) {
  await stop()
  await unlink(join(directory, `reached-${phase}`)).catch(e => { if (e.code !== 'ENOENT') throw e })
  await start(phase)
  const pending = lifecycle(action).catch(() => null)
  await reached(phase)
  if (account) {
    await writeFile(join(directory, 'id'), account)
    subprocess('excluded', account)
  }
  await stop('SIGKILL'); await pending
  const oldToken = token; await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
}
async function persistResults() { await writeFile(join(root, '.audit/account-lifecycle-runtime-results.json'), JSON.stringify({ directory, results }, null, 2) + '\n') }
try {
  await start()
  sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const account = await newAccount(), comparison = await newAccount(true)
  const command = (path, action, extra = {}, code = 200) => post(path, { account, command: { action, ...extra } }, code)
  const proof = await post('proof/preview', { account })
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  let invoice = await command('invoices', 'open')
  const selection = invoice.report.rows.find(r => r.send_available).id, target = 'accounting'
  const invoicePlan = await command('invoices', 'preview', { selection, target })
  invoice = await command('invoices', 'send', { selection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action })
  assert.equal(invoice.sends.length, 1); assert.equal(Object.keys(invoice.received).length, 1)
  let unsubscribe = await command('unsubscribe', 'open', { as_of: sample.as_of })
  const subscription = unsubscribe.report.rows.find(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test' && r.list_id === '<offers.shop.example>')
  const unsubscribeInput = { as_of: sample.as_of, selections: [{ subscription: subscription.id, method: 'one_click' }], folders: { personal: 'Muted', business: 'Quiet' } }
  const unsubscribePlan = await command('unsubscribe', 'preview', { input: unsubscribeInput })
  unsubscribe = await command('unsubscribe', 'confirm', { input: unsubscribeInput, receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation })
  assert.equal(unsubscribe.history.length, 1); assert.equal(Object.keys(unsubscribe.received).length, 1)
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const backlogPlan = await command('backlog', 'preview', { options })
  assert.equal(backlogPlan.execution_available, true)
  let backlog = await command('backlog', 'start', { options, receipt: backlogPlan.receipt, confirmed: true })
  const run = backlog.runs.at(-1).id
  backlog = await command('backlog', 'advance', { run }); assert.equal(backlog.runs.at(-1).status, 'completed')
  const wind = (action, extra = {}) => post('winddown', { account, as_of: sample.as_of, command: { action, ...extra } })
  await post('confirm', { account: comparison, address: 'owner@example.test', confirmed: true })
  const winddown = await wind('open')
  const change = { kind: 'settings', settings: { winding_down: true, comparisons: { [comparison]: ['owner@example.test'] } } }
  const decision = await wind('preview', { version: winddown.report.version, change })
  await wind('confirm', { version: winddown.report.version, change, receipt: decision.receipt, confirmed: true })
  const baseline = await snapshot(account), unrelated = await snapshot(comparison)
  assert.equal(Object.keys(baseline.hashes).length, 4)
  assert.ok(baseline.histories[0].exports > 0 && baseline.histories[0].backlog > 0 && baseline.histories[0].invoice_sends > 0 && baseline.histories[0].unsubscribe > 0 && baseline.histories[0].winddown_decisions > 0)
  assert.ok(baseline.histories[1].effects > 0 && baseline.histories[2].invoice_effects > 0 && baseline.histories[3].unsubscribe_effects > 0)
  const beforePassword = await metadata(account)
  await lifecycle({ action: 'replace_password', account, password: 'synthetic-replacement-only' })
  const afterPassword = await metadata(account)
  assert.notEqual(beforePassword.password_ref, afterPassword.password_ref)
  assert.equal(beforePassword.local_secret_ref, afterPassword.local_secret_ref)
  assert.equal(afterPassword.connection, 'unverified')
  assert.deepEqual(await snapshot(account), baseline)
  results.push({ check: 'completed password replacement preserves all four aggregates and identities', pass: true })
  for (const phase of ['replace_password', 'metadata']) {
    const before = await metadata(account)
    await crashAction(phase, { action: 'replace_password', account, password: 'another-synthetic-replacement-only' }, account)
    await post('open', { account }, 422)
    await lifecycle({ action: 'recover', account }); await lifecycle({ action: 'recover', account })
    const current = await metadata(account)
    assert.equal(current.password_ref === before.password_ref, phase === 'replace_password')
    assert.deepEqual(await snapshot(account), baseline)
    results.push({ check: `SIGKILL password ${phase}, reconstructed backend, selected reference and retry convergence`, pass: true }); await persistResults()
  }
  for (const phase of ['rotate_primary', 'rotate_invoice', 'metadata', 'rotate_retire_data']) {
    const old = await metadata(account)
    const outstanding = await post('proof/preview', { account })
    await crashAction(phase, { action: 'rotate', account, confirmed: true }, account)
    const selected = await metadata(account)
    assert.equal(selected.generation, phase.startsWith('rotate_') && phase !== 'rotate_retire_data' ? old.generation : old.generation + 1)
    assert.deepEqual(await snapshot(account), baseline, 'Every selected generation is still decryptable before recovery')
    await post('open', { account }, 422)
    await lifecycle({ action: 'recover', account }); await lifecycle({ action: 'recover', account })
    // Unpublished rotations roll back; published ones finish under the keys their data authenticates.
    const published = phase === 'metadata' || phase === 'rotate_retire_data'
    assert.equal((await metadata(account)).generation, published ? old.generation + 1 : old.generation)
    assert.deepEqual(await snapshot(account), baseline)
    assert.deepEqual(await snapshot(comparison), unrelated)
    if (!published) {
      results.push({ check: `SIGKILL ${phase}, unpublished rotation rolled back, four-store preservation`, pass: true }); await persistResults()
      continue
    }
    await post('proof/download', { account, receipt: outstanding.receipt, confirmed: true }, 422)
    await command('invoices', 'send', { selection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action }, 422)
    await command('unsubscribe', 'confirm', { input: unsubscribeInput, receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation }, 422)
    const replay = await command('backlog', 'advance', { run }); assert.equal(replay.runs.at(-1).id, run)
    assert.deepEqual(replay.runs.at(-1).outcomes, backlog.runs.at(-1).outcomes)
    assert.deepEqual(await snapshot(account), baseline)
    results.push({ check: `SIGKILL ${phase}, mixed-generation authentication, four-store preservation, approval invalidation, no effect replay`, pass: true }); await persistResults()
  }
  for (const phase of ['credential_staged', 'create_password', 'create_data']) {
    await stop(); await start(phase); await post('synthetic', {}); await reached(phase)
    const names = (await readdir(join(directory, 'config/lifecycle'))).filter(n => n.endsWith('.json'))
    const pending = []
    for (const name of names) { const value = JSON.parse(await readFile(join(directory, 'config/lifecycle', name), 'utf8')); if (value.intent.state === 'creating') pending.push(value.intent.account.id) }
    assert.equal(pending.length, 1); const interrupted = pending[0]
    assert.equal(await stat(join(directory, 'config', `${interrupted}.json`)).then(() => true, () => false), false)
    await stop('SIGKILL'); await start()
    await lifecycle({ action: 'recover', account: interrupted }); await lifecycle({ action: 'recover', account: interrupted })
    assert.equal((await post('list', {})).some(a => a.id === interrupted), false)
    assert.equal((await readdir(join(directory, 'fake-credentials'))).some(n => n.startsWith(interrupted) || n.startsWith(`.${interrupted}-`)), false)
    assert.equal(await stat(join(directory, 'data', `${interrupted}.census.enc`)).then(() => true, () => false), false)
    results.push({ check: `SIGKILL ${phase}, unpublished creation cleanup without resurrection`, pass: true }); await persistResults()
  }
  const preview = await lifecycle({ action: 'forget_preview', account })
  assert.equal(preview.receipt.length, 64)
  assert.equal(preview.resources.length, 5)
  for (const alteration of [{ ...preview, consequences: { ...preview.consequences, en: 'altered' } }, { ...preview, revision: preview.revision + 1 }, { ...preview, resources: [] }, { ...preview, receipt: proof.receipt }]) await lifecycle({ action: 'forget', account, preview: alteration, confirmed: true }, 422)
  await lifecycle({ action: 'forget', account: comparison, preview, confirmed: true }, 422)
  await lifecycle({ action: 'forget', account, preview, confirmed: false }, 422)
  await post('scan', { account: comparison })
  const during = await lifecycle({ action: 'replace_password', account: comparison, password: 'synthetic-worker-cancellation' })
  assert.equal(during.saved, true)
  const status = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json()); assert.equal(status.job, null)
  results.push({ check: 'acquisition admission closed, cancelled worker joined before replacement; no cached job resurrection', pass: true })
  const unknown = join(directory, 'mailboxes/unknown-retained.txt'), ledger = join(directory, 'ledger/addresses.yaml')
  await writeFile(unknown, 'unrelated synthetic file')
  const ledgerBytes = await readFile(ledger)
  const lockPath = join(directory, 'data', `${account}.lock`), inode = (await stat(lockPath)).ino
  const finalPreview = await lifecycle({ action: 'forget_preview', account })
  await crashAction('forget_credential', { action: 'forget', account, preview: finalPreview, confirmed: true }, account)
  await post('open', { account }, 422)
  await lifecycle({ action: 'recover', account }); await lifecycle({ action: 'recover', account })
  for (const resource of finalPreview.resources) assert.equal(await stat(resource.path).then(() => true, () => false), false)
  for (const reference of finalPreview.credential_references) assert.equal(await stat(join(directory, 'fake-credentials', reference)).then(() => true, () => false), false)
  assert.equal((await stat(lockPath)).ino, inode)
  assert.equal(await readFile(unknown, 'utf8'), 'unrelated synthetic file')
  assert.deepEqual(await readFile(ledger), ledgerBytes)
  await lifecycle({ action: 'forget', account, preview: finalPreview, confirmed: true }, 422)
  assert.equal((await post('list', {})).some(a => a.id === account), false)
  results.push({ check: 'SIGKILL partial forget converges exactly, no resurrection, credentials/stores gone, unknown/shared ledger/lock inode retained', pass: true })
  const survivor = await newAccount()
  const survivorReport = await post('winddown', { account: survivor, as_of: sample.as_of, command: { action: 'open' } })
  const missingChange = { kind: 'settings', settings: { winding_down: true, comparisons: { [comparison]: ['owner@example.test'] } } }
  const survivorReview = await post('winddown', { account: survivor, as_of: sample.as_of, command: { action: 'preview', version: survivorReport.report.version, change: missingChange } })
  await post('winddown', { account: survivor, as_of: sample.as_of, command: { action: 'confirm', version: survivorReport.report.version, change: missingChange, receipt: survivorReview.receipt, confirmed: true } })
  const forgetComparison = await lifecycle({ action: 'forget_preview', account: comparison })
  await lifecycle({ action: 'forget', account: comparison, preview: forgetComparison, confirmed: true })
  const missingReport = await post('winddown', { account: survivor, as_of: sample.as_of, command: { action: 'open' } })
  assert.ok(missingReport.report.blockers.some(b => b.includes('comparison')))
  assert.ok(!missingReport.available.some(a => a.account === comparison))
  results.push({ check: 'forget invalidates cross-account report dependency without deleting survivor owner decisions or shared YAML', pass: true })
  const joinAccount = await newAccount()
  const unaffected = await snapshot(survivor)
  await stop(); await start('acquisition_running')
  const scanning = await post('scan', { account: joinAccount }); assert.equal(scanning.job.progress.state, 'running')
  await reached('acquisition_running')
  let joined = false
  const joining = lifecycle({ action: 'replace_password', account: joinAccount, password: 'synthetic-join-check' }).then(value => { joined = true; return value })
  await new Promise(r => setTimeout(r, 150))
  assert.equal(joined, false, 'Lifecycle cannot complete before the acquisition worker joins')
  assert.equal(await stat(join(directory, 'config/lifecycle', `${joinAccount}.json`)).then(() => true, () => false), false, 'No lifecycle intent is published while acquisition is still active')
  await post('scan', { account: joinAccount }, 422)
  await writeFile(join(directory, 'continue-acquisition_running'), 'release synthetic worker')
  await joining
  assert.equal((await post('open', { account: joinAccount })).census.acquisition_state, 'cancelled')
  assert.deepEqual(await snapshot(survivor), unaffected)
  await stop(); await start()
  results.push({ check: 'deterministic active-worker cancellation and join precede durable intent, closed admission rejects new work, unrelated account retained', pass: true })
  await stop()
  for (const marker of ['reached-acquisition_running', 'continue-acquisition_running']) await unlink(join(directory, marker))
  await start('acquisition_running')
  await post('scan', { account: joinAccount }); await reached('acquisition_running')
  let unrelatedJoined = false
  const waitingForOtherAccount = lifecycle({ action: 'replace_password', account: survivor, password: 'synthetic-other-account-isolation' }).then(() => { unrelatedJoined = true })
  await new Promise(r => setTimeout(r, 150)); assert.equal(unrelatedJoined, false)
  await writeFile(join(directory, 'continue-acquisition_running'), 'release unrelated synthetic worker')
  await waitingForOtherAccount
  assert.equal((await post('open', { account: joinAccount })).census.acquisition_state, 'complete', 'An unrelated acquisition finishes without being cancelled')
  assert.deepEqual(await snapshot(survivor), unaffected)
  await stop(); await start()
  results.push({ check: 'lifecycle waits for an unrelated active worker without cancelling or changing its completed census', pass: true })
  const protections = [{ Origin: 'https://outside.invalid' }, { Origin: 'null' }, { 'Sec-Fetch-Site': 'cross-site' }]
  for (const extra of protections) assert.equal((await fetch(`${base}/api/accounts/lifecycle`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...extra }, body: JSON.stringify({ action: 'status' }) })).status, 403)
  assert.equal((await fetch(`${base}/api/accounts/lifecycle`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'status' }) })).status, 403)
  const hostCode = await new Promise((resolve, reject) => { const req = httpRequest(`${base}/api/accounts/lifecycle`, { method: 'POST', headers: { Host: 'outside.invalid', 'Content-Type': 'application/json', 'X-Mailroom-Token': token } }, r => { r.resume(); resolve(r.statusCode) }); req.on('error', reject); req.end(JSON.stringify({ action: 'status' })) })
  assert.equal(hostCode, 403)
  await lifecycle({ action: 'status', path: '/forbidden' }, 422)
  assert.equal((await fetch(`${base}/api/accounts/lifecycle`, { method: 'POST', headers: { 'Content-Type': 'text/plain', 'X-Mailroom-Token': token }, body: '{}' })).status, 415)
  assert.equal((await fetch(`${base}/api/accounts/lifecycle`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: ' '.repeat(262145) + JSON.stringify({ action: 'status' }) })).status, 413)
  const csp = (await fetch(base)).headers.get('content-security-policy')
  assert.ok(csp && !csp.includes('unsafe-inline') && !csp.includes('unsafe-eval'))
  results.push({ check: 'Host/Origin/session/cross-site/strict request protections', pass: true })
  await persistResults()
  const retained = { pid: server.pid, binary, binary_sha256: createHash('sha256').update(await readFile(binary)).digest('hex'), directory, base, account: survivor, forgotten: [account, comparison], branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/account-lifecycle-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  server.unref(); server = undefined
  console.log(`PASS lifecycle protected HTTP, ${results.length} checks, actual SIGKILL/replacement, four aggregate/effect stores and durable fake credential reconstruction. Retained ${directory}.`)
} finally { if (server) await stop() }
