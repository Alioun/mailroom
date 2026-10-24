import assert from 'node:assert/strict'
import { request as httpRequest } from 'node:http'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile, readdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
const as_of = 1792886400
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the directly verified fake server first')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const directory = await mkdtemp(join(root, '.audit/winddown-fixture-'))
let server, token
async function start() {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_WINDDOWN_FIXTURE: '1' }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const r = await fetch(`${base}/api/session`).catch(() => null)
    if (r?.ok) { token = (await r.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw Error('Fake wind-down startup timeout')
}
async function stop(signal = 'SIGTERM') { const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function post(path, body, expected = 200) {
  const r = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(r.status, expected, `${path} status ${expected}`)
  assert.equal(r.headers.get('cache-control'), 'no-store')
  return r.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Fake census timeout')
}
const command = (account, command, code = 200, time = as_of) => post('winddown', { account, as_of: time, command }, code)
const open = account => command(account, { action: 'open' })
const review = (account, report, change, code) => command(account, { action: 'preview', version: report.version, change }, code)
const confirm = (account, review, extra = {}, code) => command(account, { action: 'confirm', version: review.report.version, change: review.change, receipt: review.receipt, confirmed: true, ...extra }, code)
const row = (view, sender) => view.report.rows.find(r => r.sender === sender)
async function newAccount(gmail, sample) {
  await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
  const account = await terminal()
  await post('scan', { account }); await terminal()
  const ws = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: ws.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  return account
}
try {
  await start()
  const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const account = await newAccount(true, sample), comparison = await newAccount(false, sample)
  const ledger = join(directory, 'ledger/addresses.yaml')
  let view = await open(account)
  assert.ok(view.report.blockers.includes('comparison_accounts_absent'))
  assert.equal(view.report.settings.winding_down, false)
  const settings = { kind: 'settings', settings: { winding_down: true, comparisons: { [comparison]: ['new@destination.example.test'] } } }
  const selected = await review(account, view.report, settings)
  assert.equal(selected.resulting_report.sources.length, 2)
  assert.equal(row({ report: selected.resulting_report }, 'alex@people.example').status, 'moved')
  await confirm(account, selected)
  await confirm(account, selected, {}, 422)
  view = await open(account)
  for (const status of ['waiting', 'moved', 'quiet', 'bulk', 'unresolved']) assert.ok(view.report.rows.some(r => r.status === status), `Actual report has ${status}`)
  for (const group of ['people', 'service', 'bulk', 'unresolved']) assert.ok(view.report.rows.some(r => r.group === group), `Actual report has ${group}`)
  assert.equal(row(view, 'receipts@platform.example').settings_link, 'https://platform.example/settings')
  assert.equal(row(view, 'waiting@people.example').settings_link, null)
  for (const sender of ['forwarded@people.example', 'ambiguous@people.example', 'missing@people.example']) { assert.equal(row(view, sender).status, 'waiting'); assert.equal(row(view, sender).migration.length, 0) }
  assert.equal(row(view, 'boundary@people.example').recent, 1)
  assert.ok(row(view, 'waiting@people.example').observations.some(o => o.address === 'own.er@gmail.com' && o.field === 'x-original-to'))
  assert.ok(row(view, 'waiting@people.example').observations.some(o => o.address === 'owner@googlemail.com' && o.field === 'to'))
  assert.equal(view.report.sources.find(s => s.account === comparison).spam_count, null)
  assert.ok(view.report.blockers.includes('spam_count_unknown'))
  assert.equal(view.report.verdict, 'insufficient_evidence_or_waiting')
  const waiting = row(view, 'waiting@people.example')
  const change = { kind: 'dismiss', rows: [waiting.id], dismissed: true }
  let decision = await review(account, view.report, change)
  assert.equal(decision.receipt.length, 64)
  assert.equal(row({ report: decision.resulting_report }, waiting.sender).status, 'dismissed')
  await confirm(account, decision, { confirmed: false }, 422)
  await confirm(account, decision, { change: { ...change, dismissed: false } }, 422)
  await confirm(comparison, decision, {}, 422)
  const proof = await post('proof/preview', { account })
  await confirm(account, decision, { receipt: proof.receipt }, 422)
  const sourceReport = await post('report', { account, as_of })
  const censusSummary = await post('summary/review', { account, as_of, version: sourceReport.version, approved: [] })
  await confirm(account, decision, { receipt: censusSummary.receipt }, 422)
  const unsubscribe = await post('unsubscribe', { account, command: { action: 'open', as_of } })
  const subscription = unsubscribe.report.rows.find(r => r.methods.includes('mute'))
  assert.ok(subscription)
  const unsubscribePreview = await post('unsubscribe', { account, command: { action: 'preview', input: { as_of, selections: [{ subscription: subscription.id, method: 'mute' }], folders: { personal: 'Muted', business: 'Quiet' } } } })
  await confirm(account, decision, { receipt: unsubscribePreview.receipt }, 422)
  const local = await command(account, { action: 'export_preview', version: view.report.version, disclosure: 'local_file' })
  assert.equal(local.text, view.terminal)
  await confirm(account, decision, { receipt: local.receipt }, 422)
  const summary = await command(account, { action: 'export_preview', version: view.report.version, disclosure: 'agent_summary' })
  for (const value of [waiting.sender, 'own.er@gmail.com', 'googlemail.com', 'platform.example/settings', 'new@destination.example.test']) assert.equal(summary.text.includes(value), false)
  assert.ok(summary.text.includes('spam_count_unknown'))
  assert.notEqual(summary.receipt, local.receipt)
  await command(account, { action: 'export', version: view.report.version, disclosure: 'local_file', receipt: summary.receipt, confirmed: true }, 422)
  await command(account, { action: 'export', version: view.report.version, disclosure: 'agent_summary', receipt: local.receipt, confirmed: true }, 422)
  const downloaded = await command(account, { action: 'export', version: view.report.version, disclosure: 'local_file', receipt: local.receipt, confirmed: true })
  assert.equal(downloaded.text, local.text)
  await writeFile(join(root, '.audit/winddown-local-preview.txt'), local.text)
  await writeFile(join(root, '.audit/winddown-summary-preview.txt'), summary.text)
  const actualTerminal = execFileSync(process.execPath, ['scripts/winddown-report.mjs', '--account', account, '--as-of', String(as_of)], { cwd: root, encoding: 'utf8' })
  assert.equal(actualTerminal, local.text)
  await writeFile(join(root, '.audit/winddown-terminal.txt'), actualTerminal)
  await writeFile(ledger, (await readFile(ledger, 'utf8')) + '\n# synthetic external source edit\n')
  await confirm(account, decision, {}, 422)
  await command(account, { action: 'export', version: view.report.version, disclosure: 'local_file', receipt: local.receipt, confirmed: true }, 422)
  view = await open(account)
  decision = await review(account, view.report, change)
  await post('scan', { account: comparison }); await terminal()
  await confirm(account, decision, {}, 422)
  view = await open(account)
  decision = await review(account, view.report, change)
  const before = { invoice: await post('invoices', { account, command: { action: 'open' } }), unsubscribe: await post('unsubscribe', { account, command: { action: 'open', as_of } }), workspace: await post('workspace/open', { account }) }
  view = await open(account)
  decision = await review(account, view.report, change)
  await stop('SIGKILL'); await start()
  assert.deepEqual(await review(account, view.report, change), decision, 'Exact unconfirmed review reproduces across process replacement without inventing approval')
  assert.equal(row(await open(account), waiting.sender).status, 'waiting')
  await confirm(account, decision)
  await confirm(account, decision, {}, 422)
  view = await open(account)
  assert.equal(row(view, waiting.sender).status, 'dismissed')
  assert.deepEqual(await open(account), view, 'Repeated refresh is idempotent')
  const oldToken = token
  await stop('SIGKILL'); await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  const reopened = await open(account)
  assert.deepEqual(reopened, view, 'Actual process replacement preserves confirmed decisions, source identities and exact report')
  assert.deepEqual((await post('invoices', { account, command: { action: 'open' } })).sends, before.invoice.sends)
  assert.deepEqual((await post('unsubscribe', { account, command: { action: 'open', as_of } })).history, before.unsubscribe.history)
  assert.deepEqual((await post('workspace/open', { account })).workspace, before.workspace.workspace)
  const undo = await review(account, reopened.report, { ...change, dismissed: false })
  assert.equal(row({ report: undo.resulting_report }, waiting.sender).status, 'waiting')
  await confirm(account, undo)
  assert.equal(row(await open(account), waiting.sender).status, 'waiting')
  const final = await open(account)
  const redismiss = await review(account, final.report, change); await confirm(account, redismiss)
  async function stillDismissed(label) {
    assert.equal(row(await open(account), waiting.sender).status, 'dismissed', `${label} preserves confirmed wind-down decisions`)
  }
  await post('scan', { account }); await terminal(); await stillDismissed('census writer')
  const workspaceAfter = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: workspaceAfter.revision, input: workspaceAfter.workspace.input })
  await stillDismissed('workspace writer')
  const ledgerView = await post('ledger', { account, command: { action: 'open' } })
  const ledgerBatch = { content_version: ledgerView.snapshot.content_version, revision: ledgerView.revision, enabled: false, entries: [{ address: 'winddown@signup.example.test', service: 'WINDDOWN-PRIVATE-SERVICE', folder: 'Purchases', first_seen: 'explicit synthetic owner value', status: 'retired', note: 'WINDDOWN-PRIVATE-NOTE', category: 'orders', invoice: false, association: { sender_domains: ['people.example'], approved_evidence: 'Exact synthetic service association only' } }] }
  const ledgerReview = await post('ledger', { account, command: { action: 'preview', batch: ledgerBatch } })
  await post('ledger', { account, command: { action: 'confirm', batch: ledgerBatch, receipt: ledgerReview.receipt, confirmed: true } })
  await stillDismissed('ledger writer')
  const businessWorkspace = await post('workspace/open', { account })
  const businessDocuments = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test') }
  await post('workspace/save', { account, revision: businessWorkspace.revision, input: { ...businessWorkspace.workspace.input, documents: businessDocuments } })
  const invoiceNow = await post('invoices', { account, command: { action: 'open' } })
  const invoiceRow = invoiceNow.report.rows.find(r => r.send_available)
  assert.ok(invoiceRow)
  const invoiceReview = await post('invoices', { account, command: { action: 'preview', selection: invoiceRow.id, target: 'accounting' } })
  await post('invoices', { account, command: { action: 'send', selection: invoiceRow.id, target: 'accounting', receipt: invoiceReview.receipt, confirmation_action: invoiceReview.plan.confirmation_action } })
  await stillDismissed('synthetic invoice writer')
  const subscriptionsNow = await post('unsubscribe', { account, command: { action: 'open', as_of } })
  const muteRow = subscriptionsNow.report.rows.find(r => r.methods.includes('mute'))
  assert.ok(muteRow)
  const muteInput = { as_of, selections: [{ subscription: muteRow.id, method: 'mute' }], folders: { personal: 'Muted', business: 'Quiet' } }
  const muteReview = await post('unsubscribe', { account, command: { action: 'preview', input: muteInput } })
  await post('unsubscribe', { account, command: { action: 'confirm', input: muteInput, receipt: muteReview.receipt, confirmation_action: muteReview.confirmation_action, mail_confirmation_action: muteReview.local_mail_confirmation } })
  await stillDismissed('synthetic subscription writer')
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const backlogPreview = await post('backlog', { account, command: { action: 'preview', options } })
  const started = await post('backlog', { account, command: { action: 'start', options, receipt: backlogPreview.receipt, confirmed: true } })
  await post('backlog', { account, command: { action: 'stop', run: started.runs.at(-1).id } })
  await stillDismissed('backlog writer without any batch effects')
  const privateView = await open(account)
  assert.ok(row(privateView, waiting.sender).service_associations.includes('WINDDOWN-PRIVATE-SERVICE'))
  const privateSummary = await command(account, { action: 'export_preview', version: privateView.report.version, disclosure: 'agent_summary' })
  for (const value of ['WINDDOWN-PRIVATE-SERVICE', 'WINDDOWN-PRIVATE-NOTE', 'winddown@signup.example.test']) assert.equal(privateSummary.text.includes(value), false)
  const protections = [
    [{ Origin: 'https://outside.invalid' }, 403], [{ 'Sec-Fetch-Site': 'cross-site' }, 403],
  ]
  for (const [headers, code] of protections) assert.equal((await fetch(`${base}/api/accounts/winddown`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...headers }, body: JSON.stringify({ account, as_of, command: { action: 'open' } }) })).status, code)
  const wrongHost = await new Promise((resolve, reject) => {
    const req = httpRequest(`${base}/api/accounts/winddown`, { method: 'POST', headers: { Host: 'outside.invalid', 'Content-Type': 'application/json', 'X-Mailroom-Token': token } }, response => { response.resume(); resolve(response.statusCode) })
    req.on('error', reject); req.end(JSON.stringify({ account, as_of, command: { action: 'open' } }))
  })
  assert.equal(wrongHost, 403)
  assert.equal((await fetch(`${base}/api/accounts/winddown`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, as_of, command: { action: 'open' } }) })).status, 403)
  await post('winddown', { account, as_of, command: { action: 'open' }, trust_provider: true }, 422)
  for (const file of await readdir(join(directory, 'data'))) {
    const bytes = await readFile(join(directory, 'data', file))
    for (const marker of [waiting.sender, 'own.er@gmail.com', decision.receipt, 'winding_down', 'preset_yaml']) assert.equal(bytes.includes(Buffer.from(marker)), false, 'Encrypted local state')
  }
  const retained = { pid: server.pid, binary, binary_sha256: createHash('sha256').update(await readFile(binary)).digest('hex'), directory, account, comparison, base, as_of, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), tested_head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/winddown-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  server.unref()
  console.log('PASS protected HTTP report, exact statuses and variants, direct/forwarded/ambiguous/unknown comparison, calendar boundary, missing Spam, full receipt isolation/staleness/replay, exact file/terminal/summary, SIGKILL/replacement and independent state preservation. Retained fake metadata .audit/winddown-retained.json')
} catch (error) { if (server) await stop(); throw error }
