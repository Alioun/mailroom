import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile, rename, mkdir, stat, readdir } from 'node:fs/promises'
import { createHash, createDecipheriv } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified fake server before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const binary_sha256 = createHash('sha256').update(await readFile(binary)).digest('hex')
const directory = await mkdtemp(join(root, '.audit/recovery-fixture-'))
let installation = directory
let server, token, sample
const checks = []
const sha = b => createHash('sha256').update(b).digest('hex')
async function start(boundary = '') {
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_RECOVERY_INSTALLATION: installation }
  delete env.MAILROOM_LIFECYCLE_BOUNDARY
  delete env.MAILROOM_INVOICE_BOUNDARY
  delete env.MAILROOM_UNSUBSCRIBE_BOUNDARY
  if (boundary) env.MAILROOM_LIFECYCLE_BOUNDARY = boundary
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env, stdio: 'ignore', detached: true })
  for (let i = 0; i < 300; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await new Promise(r => setTimeout(r, 40))
  }
  throw Error('Synthetic recovery startup timeout')
}
async function stop(signal = 'SIGTERM') {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact server::accounts::durable_browser_server_fixture --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root.replace(/\/$/, '')}\n`))
  assert.equal(sha(await readFile(binary)), binary_sha256)
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `${path} ${body.action ?? body.command?.action ?? ''} expected ${expected}, got ${response.status}`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
const recovery = (body, expected) => post('recovery', body, expected)
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Synthetic acquisition timeout')
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
async function snapshot(account) {
  execFileSync(binary, ['--exact', 'acquisition::accounts::recovery::tests::recovery_snapshot_worker'], { cwd: root, env: { ...process.env, MAILROOM_RECOVERY_INSTALLATION: installation, MAILROOM_RECOVERY_REALITY: join(directory, 'mailboxes'), MAILROOM_RECOVERY_ACCOUNT: account }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(installation, 'recovery-observation.json'), 'utf8'))
}
async function reached(phase) {
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(directory, `reached-${phase}`)).then(() => true, () => false)) return
    assert.equal(server.exitCode, null)
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error(`Recovery boundary not reached: ${phase}`)
}
async function effectBytes(account) {
  const result = {}
  for (const part of ['mailboxes', 'mailboxes/invoice-transport', 'mailboxes/unsubscribe-transport']) {
    for (const name of await readdir(join(directory, part))) if (name.startsWith(account) && name.endsWith('.enc')) result[`${part}/${name}`] = sha(await readFile(join(directory, part, name)))
  }
  return result
}
try {
  await start()
  sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const account = await newAccount(), comparison = await newAccount(true)
  const command = (path, action, extra = {}, code = 200) => post(path, { account, command: { action, ...extra } }, code)
  const proof = await post('proof/preview', { account })
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  let invoice = await command('invoices', 'open')
  const selection = invoice.report.rows.find(r => r.send_available).id, target = 'accounting'
  let invoicePlan = await command('invoices', 'preview', { selection, target })
  invoice = await command('invoices', 'send', { selection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action })
  let unsub = await command('unsubscribe', 'open', { as_of: sample.as_of })
  const rows = unsub.report.rows.filter(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test')
  assert.ok(rows.length >= 2)
  const unsubscribeInput = id => ({ as_of: sample.as_of, selections: [{ subscription: id, method: 'one_click' }], folders: { personal: 'Muted', business: 'Quiet' } })
  let unsubscribePlan = await command('unsubscribe', 'preview', { input: unsubscribeInput(rows[0].id) })
  unsub = await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[0].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation })
  const wind = (action, extra = {}) => post('winddown', { account, as_of: sample.as_of, command: { action, ...extra } })
  await post('confirm', { account: comparison, address: 'owner@example.test', confirmed: true })
  const winddown = await wind('open')
  const change = { kind: 'settings', settings: { winding_down: true, comparisons: { [comparison]: ['owner@example.test'] } } }
  const decision = await wind('preview', { version: winddown.report.version, change })
  await wind('confirm', { version: winddown.report.version, change, receipt: decision.receipt, confirmed: true })
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const backlogPlan = await command('backlog', 'preview', { options })
  const backlog = await command('backlog', 'start', { options, receipt: backlogPlan.receipt, confirmed: true })
  const run = backlog.runs.at(-1).id
  const stale = await recovery({ action: 'capture_preview', account })
  const ws = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: ws.revision, input: ws.workspace.input })
  await recovery({ action: 'capture', preview: stale.preview, receipt: stale.receipt }, 422)
  const cross = await recovery({ action: 'capture_preview', account })
  await recovery({ action: 'capture', preview: { ...cross.preview, account: comparison }, receipt: cross.receipt }, 422)
  const before = await snapshot(account)
  const sourceMetadata = JSON.parse(await readFile(join(installation, 'config', `${account}.json`), 'utf8'))
  const cipherName = `${account}${sourceMetadata.generation ? `.g${sourceMetadata.generation}` : ''}.census.enc`
  const originalCipher = await readFile(join(installation, 'data', cipherName))
  const offer = await recovery({ action: 'capture_preview', account })
  assert.deepEqual(offer.preview.dependencies, [comparison])
  const captured = await recovery({ action: 'capture', preview: offer.preview, receipt: offer.receipt })
  await recovery({ action: 'capture', preview: offer.preview, receipt: offer.receipt }, 422)
  const bundle = captured.bundle
  const bundleBytes = await readFile(join(directory, 'synthetic-recovery/bundles', `${bundle}.jwe`))
  const parts = bundleBytes.toString().split('.')
  const key = await readFile(join(directory, 'synthetic-recovery/separately-held-fake-recovery-key'))
  const decipher = createDecipheriv('aes-256-gcm', key, Buffer.from(parts[2], 'base64url'))
  decipher.setAAD(Buffer.from(parts[0])); decipher.setAuthTag(Buffer.from(parts[4], 'base64url'))
  const plain = Buffer.concat([decipher.update(Buffer.from(parts[3], 'base64url')), decipher.final()])
  key.fill(0)
  const decoded = JSON.parse(plain.toString()); plain.fill(0)
  assert.deepEqual(decoded.manifest, offer.preview)
  assert.deepEqual(Buffer.from(decoded.ciphertext), originalCipher)
  assert.equal(decoded.key.length, 32)
  assert.equal(Object.hasOwn(decoded, 'password'), false)
  decoded.key.fill(0); if (decoded.identity) decoded.identity.fill(0)
  checks.push('Exact manifest, cross-account/stale/replayed approval rejection, independent Node AES-GCM authentication and exact captured ciphertext')
  await command('backlog', 'advance', { run })
  invoice = await command('invoices', 'open')
  const laterSelection = invoice.report.rows.find(r => r.send_available).id
  invoicePlan = await command('invoices', 'preview', { selection: laterSelection, target })
  invoice = await command('invoices', 'send', { selection: laterSelection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action })
  unsubscribePlan = await command('unsubscribe', 'preview', { input: unsubscribeInput(rows[1].id) })
  unsub = await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[1].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation })
  const afterEffects = await snapshot(account), external = await effectBytes(account)
  assert.ok(afterEffects.histories[1].effects > before.histories[1].effects)
  assert.ok(afterEffects.histories[2].invoice_effects > before.histories[2].invoice_effects)
  assert.ok(afterEffects.histories[3].unsubscribe_effects > before.histories[3].unsubscribe_effects)
  const ledger = join(directory, 'ledger/addresses.yaml')
  await writeFile(ledger, (await readFile(ledger, 'utf8')) + '\n# synthetic owner edit after capture\n')
  const ledgerBytes = await readFile(ledger)
  const lockInode = (await stat(join(directory, 'config/lifecycle', `${account}.lock`))).ino
  await stop()
  const lost = join(directory, 'lost-installation-preserved'); await mkdir(lost)
  for (const part of ['config', 'data', 'fake-credentials']) await rename(join(directory, part), join(lost, part))
  installation = join(directory, 'fresh-installation')
  await mkdir(installation)
  await start()
  const other = await newAccount(true), unrelatedBefore = await snapshot(other)
  const restoreOffer = await recovery({ action: 'restore_preview', bundle })
  await recovery({ action: 'restore', preview: restoreOffer.preview, receipt: offer.receipt }, 422)
  await stop()
  const freshBase = installation
  for (const phase of ['restore_intent', 'restore_key', 'restore_identity', 'restore_authenticated', 'restore_data', 'restore_metadata', 'restore_committed']) {
    if (phase !== 'restore_intent') {
      await rename(freshBase, `${freshBase}-preserved-${phase}`)
      await mkdir(freshBase)
    }
    await start(phase)
    const currentOffer = await recovery({ action: 'restore_preview', bundle })
    const pending = recovery({ action: 'restore', preview: currentOffer.preview, receipt: currentOffer.receipt }).catch(() => null)
    await reached(phase)
    const oldToken = token
    await stop('SIGKILL'); await pending
    await start()
    assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
    if (phase !== 'restore_committed') {
      await post('open', { account }, 422)
      assert.ok((await recovery({ action: 'status' })).pending.includes(account))
      await recovery({ action: 'resume', account })
    }
    const restored = await snapshot(account)
    assert.equal(restored.hashes[0], before.hashes[0], 'Exact captured rules, pins, decisions and all local histories')
    for (const kind of ['1', '2', '3']) assert.equal(restored.hashes[kind], afterEffects.hashes[kind], 'External reality after capture survives')
    assert.equal(restored.identity.stable, before.identity.stable)
    assert.equal(restored.identity.placeholder, before.identity.placeholder)
    assert.notEqual(restored.identity.approval, before.identity.approval)
    assert.deepEqual(await readFile(join(installation, 'data', cipherName)), originalCipher)
    assert.equal(await stat(join(installation, 'fake-credentials', sourceMetadata.password_ref)).then(() => true, () => false), false)
    assert.deepEqual(await readFile(ledger), ledgerBytes)
    assert.deepEqual(await effectBytes(account), external)
    await command('invoices', 'send', { selection: laterSelection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action }, 422)
    await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[1].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action }, 422)
    for (const action of ['advance', 'resume', 'undo_preview']) await command('backlog', action, { run }, 422)
    await post('proof/download', { account, receipt: proof.receipt, confirmed: true }, 422)
    await recovery({ action: 'restore_preview', bundle }, 422)
    if (phase === 'restore_intent') assert.deepEqual(await snapshot(other), unrelatedBefore)
    assert.deepEqual(await effectBytes(account), external, 'Denied operations cause no send, unsubscribe, move or compensation')
    const status = await recovery({ action: 'status' })
    assert.ok(status.restored.find(r => r.account === account && r.authenticated && r.reconciliation_required))
    checks.push(`Actual SIGKILL/replacement at ${phase}; exact journal/ciphertext; later external effects retained; old approvals and all replay/undo blocked`)
    if (phase !== 'restore_committed') await stop()
  }
  assert.equal((await stat(join(lost, 'config/lifecycle', `${account}.lock`))).ino, lockInode)
  const separateKey = join(directory, 'synthetic-recovery/separately-held-fake-recovery-key')
  const preservedKey = `${separateKey}-preserved-during-loss-test`
  await rename(separateKey, preservedKey)
  try {
    await recovery({ action: 'capture_preview', account }, 422)
    assert.equal(await stat(separateKey).then(() => true, () => false), false)
    assert.deepEqual(await readFile(join(directory, 'synthetic-recovery/bundles', `${bundle}.jwe`)), bundleBytes)
  } finally { await rename(preservedKey, separateKey) }
  checks.push('Missing separate fake recovery key denies capture without generating a replacement or changing the captured bundle')
  const retained = { pid: server.pid, binary, binary_sha256, directory, installation, account, bundle, base, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/recovery-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/recovery-runtime-results.json'), JSON.stringify({ checks, directory, installation, bundle, primary_ciphertext_sha256: sha(originalCipher), captured_local_hashes: before.hashes, post_snapshot_external_hashes: afterEffects.hashes, external_ciphertext_hashes: external, captured_history: { backlog: before.histories[0].backlog.map(r => ({ id: r.id, status: r.status })), invoice_sends: before.histories[0].invoice_sends.map(r => ({ id: r.id, status: r.status })), unsubscribe: before.histories[0].unsubscribe.map(r => ({ id: r.id, status: r.status })), winddown_decisions: before.histories[0].winddown.length, exports: before.histories[0].exports.length }, ledger_sha256: sha(ledgerBytes), lost_source_lock_inode: lockInode }, null, 2) + '\n')
  server.unref()
  console.log(`PASS ${checks.length} synthetic recovery checks. Exact captured local bytes, post-capture external effects and all seven actual crash boundaries verified.`)
} catch (error) { if (server) await stop(); throw error }
