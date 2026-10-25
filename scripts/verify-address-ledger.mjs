import { createHash } from 'node:crypto'
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified fake server before this fixture')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const fixtureBinaryHash = createHash('sha256').update(await readFile(binary)).digest('hex')
const directory = await mkdtemp(join(root, '.audit/address-ledger-fixture-'))
const ledgerFile = join(directory, 'ledger/addresses.yaml')
let server, token, retained
async function start(boundary = '') {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_LEDGER_BOUNDARY: boundary }, stdio: ['ignore', 'ignore', 'ignore'], detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const r = await fetch(`${base}/api/session`).catch(() => null)
    if (r?.ok) { token = (await r.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw Error('Fake ledger server timeout')
}
async function stop(signal = 'SIGTERM') {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact server::accounts::durable_browser_server_fixture --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root.replace(/\/$/, '')}\n`))
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(server.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  assert.equal(createHash('sha256').update(await readFile(binary)).digest('hex'), fixtureBinaryHash)
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const r = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(r.status, expected, `${path} status`)
  return r.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Synthetic census timeout')
}
const command = (account, command, expected) => post('ledger', { account, command }, expected)
const opened = account => command(account, { action: 'open' })
const batchFor = (view, entries = [], enabled = true) => ({ content_version: view.snapshot.content_version, revision: view.revision, entries, enabled })
const preview = (account, batch, code) => command(account, { action: 'preview', batch }, code)
const confirm = (account, review, extra = {}, code) => command(account, { action: 'confirm', batch: review.batch, receipt: review.receipt, confirmed: true, ...extra }, code)
const entry = address => ({ address, service: 'SYNTHETIC-LEDGER-PRIVATE-SERVICE', folder: 'Purchases', first_seen: 'owner supplied 2001-01-01', status: 'active', note: 'SYNTHETIC-LEDGER-PRIVATE-NOTE', category: 'orders', invoice: false, association: { sender_domains: ['unrelated.example'], approved_evidence: 'Original explicit synthetic service association' }, owner_field: { preserve: ['one', 2] } })
async function newAccount(sample) {
  await post('synthetic', {}); const id = await terminal()
  await post('scan', { account: id }); await terminal()
  const workspace = await post('workspace/open', { account: id })
  await post('workspace/save', { account: id, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  return id
}
try {
  await start()
  const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const account = await newAccount(sample), other = await newAccount(sample)
  const disabledProof = await post('proof/preview', { account })
  await writeFile(ledgerFile, (await readFile(ledgerFile, 'utf8')) + '\n# synthetic external edit while disabled and never manually refreshed\n')
  await post('proof/download', { account, receipt: disabledProof.receipt, confirmed: true }, 422)
  const initialWorkspace = await post('workspace/open', { account })
  initialWorkspace.workspace.input.documents.override_yaml = initialWorkspace.workspace.input.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business')
  const pinSource = await post('proof/preview', { account })
  initialWorkspace.workspace.input.pins = [{ account, message_id_hash: pinSource.proof.decisions.find(d => d.message_id_hash).message_id_hash, expected: { kind: 'inbox' } }]
  await post('workspace/save', { account, revision: initialWorkspace.revision, input: initialWorkspace.workspace.input })
  const subscriptions = await post('unsubscribe', { account, command: { action: 'open', as_of: sample.as_of } })
  const subscription = subscriptions.report.rows.find(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test' && r.list_id === '<offers.shop.example>')
  assert.ok(subscription)
  const muteInput = { as_of: sample.as_of, selections: [{ subscription: subscription.id, method: 'mute' }], folders: { personal: 'Muted', business: 'Quiet' } }
  const muteReview = await post('unsubscribe', { account, command: { action: 'preview', input: muteInput } })
  const muted = await post('unsubscribe', { account, command: { action: 'confirm', input: muteInput, receipt: muteReview.receipt, confirmation_action: muteReview.confirmation_action, mail_confirmation_action: muteReview.local_mail_confirmation } })
  assert.ok(muted.mutes[subscription.id])
  const invoiceReport = await post('invoices', { account, command: { action: 'open' } })
  const invoice = invoiceReport.report.rows.find(r => r.send_available)
  assert.ok(invoice)
  const invoiceReview = await post('invoices', { account, command: { action: 'preview', selection: invoice.id, target: 'accounting' } })
  const sent = await post('invoices', { account, command: { action: 'send', selection: invoice.id, target: 'accounting', receipt: invoiceReview.receipt, confirmation_action: invoiceReview.plan.confirmation_action } })
  assert.equal(sent.sends.at(-1).status, 'completed')
  const approvedProof = await post('proof/preview', { account })
  await post('proof/download', { account, receipt: approvedProof.receipt, confirmed: true })
  let view = await opened(account)
  assert.equal(view.snapshot.document.enabled, false)
  assert.ok(view.proposals.length > 0)
  for (const p of view.proposals) { assert.equal(p.ownership, 'unknown'); assert.equal(p.role, 'unknown'); assert.equal(p.service, null); assert.match(p.first_seen_meaning, /not first receipt/); assert.ok(p.evidence.every(e => e.delivery_authority.startsWith('unknown'))); }
  assert.ok(view.proposals.some(p => p.evidence.some(e => e.recipient_header === 'to')))
  const proposal = view.proposals.find(p => p.address === 'team@home.example.test') ?? view.proposals.find(p => !['invoices@work.example.test', 'old@home.example.test'].includes(p.address))
  assert.ok(proposal)
  const proposed = entry(proposal.address)
  const beforeWorkspace = await post('workspace/open', { account })
  await preview(account, batchFor(view, [proposed, proposed]), 422)
  await preview(account, batchFor(view, [{ ...proposed, folder: 'bad/path' }]), 422)
  await preview(account, batchFor(view, [{ ...proposed, invoice: true }]), 422)
  const reviewed = await preview(account, batchFor(view, [proposed]))
  assert.equal(reviewed.receipt.length, 64)
  assert.ok(reviewed.proof.proof.counts.review > 0)
  assert.equal(reviewed.proof.proof.counts.copies, 0)
  await confirm(other, reviewed, {}, 422)
  await confirm(account, reviewed, { confirmed: false }, 422)
  await confirm(account, reviewed, { batch: { ...reviewed.batch, entries: [{ ...proposed, note: 'tampered' }] } }, 422)
  const wrongPurpose = await post('proof/preview', { account })
  await confirm(account, reviewed, { receipt: wrongPurpose.receipt }, 422)
  const report = await post('report', { account, as_of: sample.as_of })
  const summary = await post('summary/review', { account, as_of: sample.as_of, version: report.version, approved: [] })
  await confirm(account, reviewed, { receipt: summary.receipt }, 422)
  await stop(); await start()
  const reproduced = await preview(account, reviewed.batch)
  assert.deepEqual(reproduced, reviewed, 'Exact review and full HMAC reproduce after process replacement')
  view = await confirm(account, reviewed)
  assert.equal(view.snapshot.document.entries.length, 1)
  assert.equal(view.snapshot.document.entries[0].first_seen, proposed.first_seen)
  assert.deepEqual(view.snapshot.document.entries[0].owner_field, proposed.owner_field)
  assert.ok(view.leaks.length > 0)
  assert.deepEqual(view.snapshot.document.entries[0].association, proposed.association)
  await confirm(account, reviewed, {}, 422)
  const repeat = await opened(account)
  assert.equal(repeat.revision, view.revision)
  const preservedSubscriptions = await post('unsubscribe', { account, command: { action: 'open', as_of: sample.as_of } })
  assert.deepEqual(preservedSubscriptions.mutes, muted.mutes)
  assert.deepEqual(preservedSubscriptions.history, muted.history)
  const preservedInvoices = await post('invoices', { account, command: { action: 'open' } })
  assert.deepEqual(preservedInvoices.sends, sent.sends)
  assert.deepEqual(preservedInvoices.received, sent.received)
  const afterWorkspace = await post('workspace/open', { account })
  assert.deepEqual(afterWorkspace.workspace.input.pins, beforeWorkspace.workspace.input.pins)
  assert.deepEqual(afterWorkspace.exports, beforeWorkspace.exports)
  assert.equal(afterWorkspace.workspace.input.documents.preset_yaml, beforeWorkspace.workspace.input.documents.preset_yaml)
  await post('workspace/save', { account, revision: afterWorkspace.revision, input: { ...afterWorkspace.workspace.input, documents: { ...afterWorkspace.workspace.input.documents, override_yaml: afterWorkspace.workspace.input.documents.override_yaml.replace('folder: Purchases', 'folder: Tampered') } } }, 422)
  const routingOptions = { scope: 'inbox', protect_flagged: true, selected: null }
  const sorting = await post('backlog', { account: other, command: { action: 'preview', options: routingOptions } })
  assert.equal(sorting.execution_available, true, 'Independent unpinned synthetic account can prepare sorting')
  const prepared = await post('backlog', { account: other, command: { action: 'start', options: routingOptions, receipt: sorting.receipt, confirmed: true } })
  const sortingRun = prepared.runs.at(-1).id
  const validLedger = await readFile(ledgerFile)
  await writeFile(ledgerFile, 'version: invalid\n')
  const stopped = await post('backlog', { account: other, command: { action: 'stop', run: sortingRun } })
  assert.equal(stopped.runs.at(-1).status, 'stopped')
  assert.equal(stopped.runs.at(-1).batches, 0)
  assert.equal(stopped.runs.at(-1).undo_requested, false)
  await post('proof/preview', { account }, 422)
  await writeFile(ledgerFile, validLedger)
  const proof = await post('proof/preview', { account })
  const prior = await preview(account, batchFor(view, [{ ...proposed, status: 'retired' }]))
  const external = structuredClone(view.snapshot.document)
  external.owner_root = { preserve: 'ROOT-OWNER-FIELD' }
  external.entries[0].note = 'NEWER-SYNTHETIC-OWNER-EDIT'
  await writeFile(ledgerFile, JSON.stringify(external))
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true }, 422)
  await confirm(account, prior, {}, 422)
  view = await opened(account)
  assert.equal(view.snapshot.document.entries[0].note, external.entries[0].note)
  assert.deepEqual(view.snapshot.document.owner_root, external.owner_root)
  const staleOther = await opened(other)
  const otherReview = await preview(other, batchFor(staleOther, [entry('second-ledger@signup.example.test')]))
  let retire = await preview(account, batchFor(view, [{ ...view.snapshot.document.entries[0], status: 'retired' }]))
  view = await confirm(account, retire)
  await post('backlog', { account: other, command: { action: 'resume', run: sortingRun } }, 422)
  await confirm(other, otherReview, {}, 422)
  assert.equal(view.snapshot.document.entries.length, 1)
  assert.equal(view.snapshot.document.entries[0].status, 'retired')
  const refreshed = await opened(other)
  assert.equal(refreshed.snapshot.document.entries[0].status, 'retired')
  const summaryAfter = await post('summary/review', { account, as_of: sample.as_of, version: report.version, approved: [] })
  for (const marker of [proposed.address, proposed.service, proposed.note, external.entries[0].note, 'ROOT-OWNER-FIELD', 'Original explicit synthetic service association']) assert.equal(JSON.stringify(summaryAfter).includes(marker), false, 'Ledger fields absent from agent summary')
  for (const injection of [{ path: '/tmp/forbidden' }, { provider: 'gmail' }, { adapter: 'production' }, { trusted: true }]) await post('ledger', { account, command: { action: 'open' }, ...injection }, 422)
  assert.equal((await stat(join(directory, 'ledger'))).mode & 0o777, 0o700)
  assert.equal((await stat(ledgerFile)).mode & 0o777, 0o600)
  console.log('PASS protected HTTP exact batch, purpose/account/tamper/replay rejection, proposal provenance, leak signals, preserved fields, external proof invalidation without refresh, cross-account stale content, retirement and private summary')
  await stop(); await start('file')
  view = await opened(account)
  const interrupted = await preview(account, batchFor(view, [entry('interrupted@signup.example.test')]))
  const pending = confirm(account, interrupted).catch(() => null)
  let reached = false
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(directory, 'ledger-reached-file')).then(() => true, () => false)) { reached = true; break }
    await new Promise(r => setTimeout(r, 20))
  }
  assert.ok(reached)
  execFileSync(binary, ['--exact', 'ledger::tests::shared_file_process_probe'], { cwd: root, env: { ...process.env, MAILROOM_LEDGER_LOCK_PROBE: directory }, stdio: 'pipe' })
  const oldToken = token
  await stop('SIGKILL'); await pending
  // A later owner edit supersedes the saved-but-unprojected document before recovery.
  const ownerDoc = structuredClone(interrupted.resulting_document)
  ownerDoc.entries.find(e => e.address === 'interrupted@signup.example.test').note = 'AFTER-CRASH-OWNER-EDIT'
  await writeFile(ledgerFile, JSON.stringify(ownerDoc))
  await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  view = await opened(account)
  assert.equal(view.snapshot.document.entries.length, 2)
  assert.equal(view.snapshot.document.entries.find(e => e.address === 'interrupted@signup.example.test').note, 'AFTER-CRASH-OWNER-EDIT')
  await confirm(account, interrupted, {}, 422)
  const latest = await opened(account)
  assert.equal(latest.revision, view.revision)
  assert.deepEqual(latest.snapshot, view.snapshot)
  await stop(); await start()
  assert.deepEqual((await opened(account)).snapshot, latest.snapshot)
  for (const file of await readdir(join(directory, 'data'))) {
    const bytes = await readFile(join(directory, 'data', file))
    for (const marker of [proposed.service, proposed.note, proposed.address, 'AFTER-CRASH-OWNER-EDIT', reviewed.receipt]) assert.equal(bytes.includes(Buffer.from(marker)), false, 'Derived account state encrypted')
  }
  retained = { pid: server.pid, binary, binary_sha256: fixtureBinaryHash, directory, account, base }
  await writeFile(join(root, '.audit/address-ledger-retained.json'), JSON.stringify(retained, null, 2))
  server.unref()
  console.log('PASS actual file-before-projection SIGKILL/replacement, cross-process lock, newer owner edit recovery, convergent reconciliation, old-session rejection and encrypted projection; fake process retained')
} catch (error) { if (server) await stop(); throw error }
