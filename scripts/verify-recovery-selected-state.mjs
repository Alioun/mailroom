import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile, rename, mkdir, stat, readdir, unlink } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { bundlePath, decodeBundle, isBundleName } from './recovery-bundle.mjs'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified fake server before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const binary_sha256 = createHash('sha256').update(await readFile(binary)).digest('hex')
const directory = await mkdtemp(join(root, '.audit/recovery-selected-fixture-'))
let installation = directory
let server, token, sample
const checks = []
const sha = b => createHash('sha256').update(b).digest('hex')
async function start(boundary = '') {
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_RECOVERY_INSTALLATION: installation, MAILROOM_RECOVERY_LEDGER: installation === directory ? join(directory, 'ledger') : join(installation, 'ledger') }
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
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(server.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
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
function authenticate(bytes, secret) {
  return decodeBundle(bytes, secret)
}
async function unrelated() {
  execFileSync(binary, ['--exact', 'acquisition::accounts::recovery::selected::tests::selected_unrelated_fixture_worker'], { cwd: root, env: { ...process.env, MAILROOM_SELECTED_UNRELATED_INSTALLATION: installation }, stdio: 'ignore' })
  return readFile(join(installation, 'unrelated-fixture-id'), 'utf8')
}
try {
  await start()
  sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const account = await newAccount(), comparison = await newAccount(true), unselected = await unrelated()
  const selected = [account, comparison].sort()
  const ledger = join(directory, 'ledger/addresses.yaml')
  const ledgerBytes = Buffer.from('version: 1\nenabled: false\nentries:\n  - address: captured+fixture@example.test\n    service: Synthetic recovery bookkeeping\n    folder: Purchases\n    first_seen: supplied synthetic historical value\n    status: active\n    note: Synthetic private recovery note\n    association:\n      sender_domains: [example.org]\n      approved_evidence: Invented fixture approval\n    owner_extension: {nested: [preserve, exactly]}\nowner_bookkeeping:\n  retained: [synthetic, unknown, fields]\n# exact synthetic owner formatting\n')
  await writeFile(ledger, ledgerBytes)
  const command = (path, action, extra = {}, code = 200) => post(path, { account, command: { action, ...extra } }, code)
  const proof = await post('proof/preview', { account })
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  let invoice = await command('invoices', 'open')
  const selection = invoice.report.rows.find(r => r.send_available).id, target = 'accounting'
  let invoicePlan = await command('invoices', 'preview', { selection, target })
  await command('invoices', 'send', { selection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action })
  let unsub = await command('unsubscribe', 'open', { as_of: sample.as_of })
  const rows = unsub.report.rows.filter(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test')
  assert.ok(rows.length >= 2)
  const unsubscribeInput = id => ({ as_of: sample.as_of, selections: [{ subscription: id, method: 'one_click' }], folders: { personal: 'Muted', business: 'Quiet' } })
  let unsubscribePlan = await command('unsubscribe', 'preview', { input: unsubscribeInput(rows[0].id) })
  await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[0].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation })
  for (const [source, peer] of [[account, comparison], [comparison, account]]) {
    await post('confirm', { account: peer, address: peer === comparison ? 'owner@example.test' : 'team@home.example.test', confirmed: true })
    const wind = (action, extra = {}) => post('winddown', { account: source, as_of: sample.as_of, command: { action, ...extra } })
    const winddown = await wind('open')
    const change = { kind: 'settings', settings: { winding_down: true, comparisons: { [peer]: [peer === comparison ? 'owner@example.test' : 'team@home.example.test'] } } }
    const decision = await wind('preview', { version: winddown.report.version, change })
    await wind('confirm', { version: winddown.report.version, change, receipt: decision.receipt, confirmed: true })
  }
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const backlogPlan = await command('backlog', 'preview', { options })
  const backlog = await command('backlog', 'start', { options, receipt: backlogPlan.receipt, confirmed: true })
  const run = backlog.runs.at(-1).id
  const offer = (include_ledger = true) => recovery({ action: 'selected_capture_preview', accounts: selected, include_ledger })
  const captureOffer = value => recovery({ action: 'selected_capture', preview: value.preview, receipt: value.receipt })
  for (const accounts of [[], [account, account], ['unknown-account']]) await recovery({ action: 'selected_capture_preview', accounts, include_ledger: false }, 422)
  let stale = await offer()
  let ws = await post('workspace/open', { account: comparison })
  await post('workspace/save', { account: comparison, revision: ws.revision, input: ws.workspace.input })
  await recovery({ action: 'selected_capture', preview: stale.preview, receipt: stale.receipt }, 422)
  stale = await offer()
  await recovery({ action: 'selected_capture', preview: { ...stale.preview, accounts: stale.preview.accounts.slice(0, 1) }, receipt: stale.receipt }, 422)
  await writeFile(ledger, Buffer.concat([ledgerBytes, Buffer.from('# newer owner version\n')]))
  await recovery({ action: 'selected_capture', preview: stale.preview, receipt: stale.receipt }, 422)
  await writeFile(ledger, ledgerBytes)
  const legacy = await recovery({ action: 'capture_preview', account })
  const legacyBundle = (await recovery({ action: 'capture', preview: legacy.preview, receipt: legacy.receipt })).bundle
  const excludedOffer = await offer(false), excludedBundle = (await captureOffer(excludedOffer)).bundle
  assert.equal(excludedOffer.preview.ledger.kind, 'excluded')
  const exact = await offer(), captured = await captureOffer(exact), bundle = captured.bundle
  await recovery({ action: 'selected_capture', preview: exact.preview, receipt: exact.receipt }, 422)
  assert.deepEqual(exact.preview.accounts.map(a => a.account), selected)
  assert.deepEqual(exact.preview.excluded_accounts, [unselected])
  for (const a of exact.preview.accounts) assert.deepEqual(a.dependencies, [selected.find(id => id !== a.account)])
  const bundleFile = bundlePath(join(directory, 'synthetic-recovery/bundles'), bundle)
  assert.ok(bundleFile.endsWith('.bundle'), 'New captures are version-3 streamed bundles')
  const bundleBytes = await readFile(bundleFile)
  const keyPath = join(directory, 'synthetic-recovery/separately-held-fake-recovery-key'), secret = await readFile(keyPath)
  const decoded = authenticate(bundleBytes, secret)
  assert.deepEqual(decoded.manifest, exact.preview)
  assert.ok(Buffer.from(decoded.ledger).equals(ledgerBytes), 'Exact authoritative ledger bytes')
  const before = {}, ciphertext = {}, cipherNames = {}, metadata = {}
  for (const id of selected) {
    before[id] = await snapshot(id)
    metadata[id] = JSON.parse(await readFile(join(installation, 'config', `${id}.json`)))
    cipherNames[id] = `${id}${metadata[id].generation ? `.g${metadata[id].generation}` : ''}.census.enc`
    ciphertext[id] = await readFile(join(installation, 'data', cipherNames[id]))
    const c = decoded.accounts.find(a => a.manifest.account === id)
    assert.deepEqual(Buffer.from(c.ciphertext), ciphertext[id]); c.key.fill(0); if (c.identity) c.identity.fill(0)
    assert.equal(Object.hasOwn(c, 'password'), false)
  }
  const unselectedBefore = await snapshot(unselected)
  checks.push('Explicit selected set, mutual comparison dependencies, exclusion inventory, legacy capture, ledger inclusion/exclusion, stale second-account/set/ledger/replayed receipts and independent AES-GCM resource authentication')
  const unknown = join(directory, 'synthetic-recovery/bundles/.selected-unknown.stage-owner')
  await writeFile(unknown, 'unknown owner file retained')
  const captureCrashes = []
  for (const phase of ['selected_capture_staged', 'selected_capture_published', 'selected_capture_before_ack']) {
    await stop(); await start(phase)
    const current = await offer()
    const pending = captureOffer(current).then(() => 'acknowledged', () => 'interrupted')
    await reached(phase)
    const oldToken = token
    await stop('SIGKILL'); assert.equal(await pending, 'interrupted'); await start()
    assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
    for (const id of selected) assert.deepEqual(await readFile(join(installation, 'data', cipherNames[id])), ciphertext[id])
    assert.deepEqual(await snapshot(unselected), unselectedBefore)
    assert.ok((await readFile(ledger)).equals(ledgerBytes), 'Source ledger bytes preserved'); assert.deepEqual(await readFile(bundleFile), bundleBytes); assert.ok((await readFile(keyPath)).equals(secret), 'Separate key bytes preserved')
    assert.equal(await readFile(unknown, 'utf8'), 'unknown owner file retained')
    for (const name of await readdir(join(directory, 'synthetic-recovery/bundles'))) if (isBundleName(name) || name.startsWith('.selected-') && name !== '.selected-unknown.stage-owner') {
      const c = authenticate(await readFile(join(directory, 'synthetic-recovery/bundles', name)), secret)
      if (c.accounts) for (const a of c.accounts) { a.key.fill(0); if (a.identity) a.identity.fill(0) }
      else { c.key.fill(0); if (c.identity) c.identity.fill(0) }
    }
    captureCrashes.push({ phase, signal: 'SIGKILL', source_ciphertexts_equal: true, ledger_equal: true, previous_bundle_equal: true, separate_key_equal: true, unknown_file_preserved: true })
  }
  secret.fill(0)
  checks.push('Actual capture SIGKILL/replacement at encrypted staging, durable publication and before acknowledgment; no incomplete published bundle, source/ledger/key/previous bundle/unknown files preserved')
  await command('backlog', 'advance', { run })
  invoice = await command('invoices', 'open')
  const laterSelection = invoice.report.rows.find(r => r.send_available).id
  invoicePlan = await command('invoices', 'preview', { selection: laterSelection, target })
  await command('invoices', 'send', { selection: laterSelection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action })
  unsubscribePlan = await command('unsubscribe', 'preview', { input: unsubscribeInput(rows[1].id) })
  await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[1].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action, mail_confirmation_action: unsubscribePlan.local_mail_confirmation })
  const afterEffects = await snapshot(account), external = await effectBytes(account)
  for (const [kind,field] of [['1','effects'],['2','invoice_effects'],['3','unsubscribe_effects']]) assert.ok(afterEffects.histories[kind][field] > before[account].histories[kind][field])
  const sourceInodes = {}
  for (const id of selected) sourceInodes[id] = (await stat(join(directory, 'config/lifecycle', `${id}.lock`))).ino
  await stop()
  const lost = join(directory, 'lost-installation-preserved'); await mkdir(lost)
  for (const part of ['config', 'data', 'fake-credentials', 'ledger']) await rename(join(directory, part), join(lost, part))
  installation = join(directory, 'fresh-installation'); await mkdir(installation)
  await start()
  const other = await unrelated(), unrelatedBefore = await snapshot(other)
  const targetLedger = join(installation, 'ledger/addresses.yaml'), targetOriginal = await readFile(targetLedger)
  await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: true }, 422)
  let restoreOffer = await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: false })
  await recovery({ action: 'selected_restore', preview: restoreOffer.preview, receipt: exact.receipt }, 422)
  await recovery({ action: 'selected_restore', preview: { ...restoreOffer.preview, ledger_target: { kind: 'absent' } }, receipt: restoreOffer.receipt }, 422)
  assert.ok((await readFile(targetLedger)).equals(targetOriginal), 'Target ledger bytes preserved')
  await unlink(targetLedger)
  restoreOffer = await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: true })
  await writeFile(targetLedger, Buffer.concat([ledgerBytes, Buffer.from('# current target owner edit\n')]), { mode: 0o600 })
  await recovery({ action: 'selected_restore', preview: restoreOffer.preview, receipt: restoreOffer.receipt }, 422)
  for (const id of selected) assert.equal(await stat(join(installation, 'config', `${id}.json`)).then(() => true, () => false), false)
  await unlink(targetLedger)
  const excludedPreview = await recovery({ action: 'selected_restore_preview', bundle: excludedBundle, restore_ledger: false })
  assert.equal(excludedPreview.preview.ledger_target.kind, 'preserve_current')
  await recovery({ action: 'selected_restore_preview', bundle: excludedBundle, restore_ledger: true }, 422)
  const legacyPreview = await recovery({ action: 'selected_restore_preview', bundle: legacyBundle, restore_ledger: false })
  assert.equal(legacyPreview.preview.manifest.accounts.length, 1)
  const preservedTarget = Buffer.concat([ledgerBytes, Buffer.from('# excluded restore keeps current owner version\n')])
  await writeFile(targetLedger, preservedTarget, { mode: 0o600 })
  const excludedRestore = await recovery({ action: 'selected_restore_preview', bundle: excludedBundle, restore_ledger: false })
  await recovery({ action: 'selected_restore', preview: excludedRestore.preview, receipt: excludedRestore.receipt })
  assert.ok((await readFile(targetLedger)).equals(preservedTarget), 'Excluded target ledger bytes preserved')
  for (const id of selected) assert.deepEqual(await readFile(join(installation, 'data', cipherNames[id])), ciphertext[id])
  checks.push('Actual ledger-excluded restoration preserves differing current owner bytes; included ledger cannot be synthesized from an excluded capture; legacy bundle authenticated by selected path')
  await stop()
  await rename(installation, `${installation}-excluded-restore-preserved`); await mkdir(installation)
  await start()
  const collisionPreview = await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: false })
  await post('synthetic-gmail', {}); await terminal()
  await recovery({ action: 'selected_restore', preview: collisionPreview.preview, receipt: collisionPreview.receipt }, 422)
  for (const id of selected) assert.equal(await stat(join(installation, 'config', `${id}.json`)).then(() => true, () => false), false)
  assert.equal(await stat(join(installation, 'config/recovery-selected.restore')).then(() => true, () => false), false)
  checks.push('Actual current-account collision after preview rejects complete selected restoration before any member publication or transaction marker')
  await stop()
  await rename(installation, `${installation}-collision-preserved`); await mkdir(installation)
  await start()
  await unrelated(); await unlink(targetLedger)
  await stop()
  const freshBase = installation, restoreCrashes = []
  const phases = ['selected_restore_intent','selected_restore_ledger','selected_restore_credentials','selected_restore_data','selected_restore_metadata','selected_restore_first_account','selected_restore_second_credentials','selected_restore_second_data','selected_restore_second_metadata','selected_restore_committed']
  for (const phase of phases) {
    if (phase !== phases[0]) { await rename(freshBase, `${freshBase}-preserved-${phase}`); await mkdir(freshBase) }
    await start(phase)
    if (phase !== phases[0]) { await unrelated(); await unlink(targetLedger) }
    const phaseOther = await readFile(join(installation, 'unrelated-fixture-id'), 'utf8'), phaseOtherBefore = await snapshot(phaseOther)
    const current = await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: true })
    const pending = recovery({ action: 'selected_restore', preview: current.preview, receipt: current.receipt }).catch(() => null)
    await reached(phase)
    const lockInodes = {}
    for (const id of selected) lockInodes[id] = (await stat(join(installation, 'config/lifecycle', `${id}.lock`))).ino
    const oldToken = token
    await stop('SIGKILL'); await pending; await start()
    assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
    if (phase !== 'selected_restore_committed') {
      for (const id of selected) await post('open', { account: id }, 422)
      assert.deepEqual((await recovery({ action: 'status' })).selected_pending, selected)
      if (phase === 'selected_restore_first_account') {
        const owner = Buffer.concat([ledgerBytes, Buffer.from('# owner edit during interrupted restore\n')])
        await writeFile(targetLedger, owner)
        await recovery({ action: 'selected_resume' }, 422)
        assert.ok((await readFile(targetLedger)).equals(owner), 'Current owner edit preserved')
        for (const id of selected) await post('open', { account: id }, 422)
        await rename(targetLedger, `${targetLedger}-owner-interruption-preserved`)
      }
      await recovery({ action: 'selected_resume' })
    }
    for (const id of selected) {
      const restored = await snapshot(id)
      assert.equal(restored.hashes[0], before[id].hashes[0])
      assert.deepEqual(restored.histories[0], before[id].histories[0])
      assert.equal(restored.identity.stable, before[id].identity.stable)
      assert.equal(restored.identity.placeholder, before[id].identity.placeholder)
      assert.notEqual(restored.identity.approval, before[id].identity.approval)
      assert.deepEqual(await readFile(join(installation, 'data', cipherNames[id])), ciphertext[id])
      assert.equal(await stat(join(installation, 'fake-credentials', metadata[id].password_ref)).then(() => true, () => false), false)
      assert.equal((await stat(join(installation, 'config/lifecycle', `${id}.lock`))).ino, lockInodes[id])
    }
    assert.ok((await readFile(targetLedger)).equals(ledgerBytes), 'Restored ledger bytes exact')
    assert.deepEqual(await effectBytes(account), external)
    const observed = await snapshot(account)
    for (const kind of ['1','2','3']) assert.equal(observed.hashes[kind], afterEffects.hashes[kind])
    await command('invoices', 'send', { selection: laterSelection, target, receipt: invoicePlan.receipt, confirmation_action: invoicePlan.plan.confirmation_action }, 422)
    await command('unsubscribe', 'confirm', { input: unsubscribeInput(rows[1].id), receipt: unsubscribePlan.receipt, confirmation_action: unsubscribePlan.confirmation_action }, 422)
    for (const action of ['advance','resume','undo_preview']) await command('backlog', action, { run }, 422)
    await post('proof/download', { account, receipt: proof.receipt, confirmed: true }, 422)
    await recovery({ action: 'selected_restore_preview', bundle, restore_ledger: true }, 422)
    assert.deepEqual(await snapshot(phaseOther), phaseOtherBefore)
    assert.deepEqual(await effectBytes(account), external)
    restoreCrashes.push({ phase, signal: 'SIGKILL', exact_ciphertexts: selected.map(id => ({ account: id, sha256: sha(ciphertext[id]), authenticated_history_sha256: before[id].hashes[0] })), ledger_sha256: sha(ledgerBytes), all_restored_readonly: true, external_unchanged: true })
    checks.push(`Actual multi-account SIGKILL/replacement at ${phase}, all-account hold, exact ciphertext/history/ledger, fresh authority, stable identities and external bytes retained`)
    if (phase !== phases.at(-1)) await stop()
  }
  const comparisonSource = await readFile(join(installation, 'unrelated-fixture-id'), 'utf8')
  const winddown = await post('winddown', { account: comparisonSource, as_of: sample.as_of, command: { action: 'open' } })
  assert.ok(winddown.available.every(a => !selected.includes(a.account)), 'Restored accounts cannot supply current comparison observations')
  assert.ok(winddown.report.sources.every(a => !selected.includes(a.account)))
  await post('winddown', { account: comparisonSource, as_of: sample.as_of, command: { action: 'preview', version: winddown.report.version, change: { kind: 'settings', settings: { winding_down: true, comparisons: { [account]: ['team@home.example.test'] } } } } }, 422)
  checks.push('Restored comparison accounts remain unavailable to wind-down and cannot promote captured observations into a current source')
  for (const id of selected) assert.equal((await stat(join(lost, 'config/lifecycle', `${id}.lock`))).ino, sourceInodes[id])
  const retained = { pid: server.pid, binary, binary_sha256, directory, installation, account, accounts: selected, bundle, base, ledger: join(installation, 'ledger'), branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/recovery-selected-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/recovery-selected-runtime-results.json'), JSON.stringify({ checks, directory, installation, bundle, captureCrashes, restoreCrashes, captured: selected.map(id => ({ account: id, ciphertext_sha256: sha(ciphertext[id]), local_state_sha256: before[id].hashes[0], histories: before[id].histories[0] })), post_capture_external_hashes: afterEffects.hashes, external_ciphertext_hashes: external, ledger_sha256: sha(ledgerBytes), sourceInodes }, null, 2) + '\n')
  server.unref()
  console.log(`PASS ${checks.length} selected-state checks, three actual capture SIGKILLs and ten actual multi-account restore SIGKILLs. Exact resources and independent later effects verified.`)
} catch (error) { if (server) await stop(); throw error }
