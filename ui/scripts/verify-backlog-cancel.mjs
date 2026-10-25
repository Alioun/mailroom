import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url))
const base = 'http://127.0.0.1:4173'
const artifacts = join(root, '.audit/backlog-cancel-browser')
await mkdir(artifacts, { recursive: true })
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'The fake fixture port must be free')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(line => JSON.parse(line)).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const directory = await mkdtemp(join(root, '.audit/backlog-cancel-'))
let server, token, browser
const pause = ms => new Promise(resolve => setTimeout(resolve, ms))
async function start(boundary = '', large = false) {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_BACKLOG_BOUNDARY: boundary, MAILROOM_BACKLOG_CANCEL_FIXTURE: large ? '1' : '' }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await pause(50)
  }
  throw Error('Fake fixture startup timeout')
}
async function stop(signal = 'SIGTERM') {
  if (!server) return
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body), signal: AbortSignal.timeout(900000) })
  assert.equal(response.status, expected, `${path} HTTP status`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
const command = (account, action, extra = {}, code = 200) => post('backlog', { account, command: { action, ...extra } }, code)
async function terminal() {
  for (let i = 0; i < 2000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await pause(20)
  }
  throw Error('Synthetic acquisition timeout')
}
async function prepare(gmail = false) {
  await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
  const account = await terminal()
  await post('scan', { account }); await terminal()
  const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const workspace = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  return account
}
async function histories(account) {
  const workspace = await post('workspace/open', { account })
  const census = await post('open', { account })
  assert.ok(census.evidence.age_seconds === null || (Number.isSafeInteger(census.evidence.age_seconds) && census.evidence.age_seconds >= 0))
  delete census.evidence.age_seconds
  const invoices = await post('invoices', { account, command: { action: 'open' } })
  const subscriptions = await post('unsubscribe', { account, command: { action: 'open', as_of: workspace.workspace.input.as_of } })
  return { workspace: workspace.workspace, exports: workspace.exports, census, sends: invoices.sends, history: subscriptions.history, mutes: subscriptions.mutes }
}
async function reached(name) {
  for (let i = 0; i < 2000; i++) {
    if (await stat(join(directory, `reached-${name}`)).then(() => true, () => false)) return
    await pause(20)
  }
  throw Error(`Synthetic boundary ${name} not reached`)
}
function noUndo(run) {
  assert.equal(run.undo_requested, false)
  assert.ok(run.undo.every(u => u.state === 'not_requested'))
}
function counts(run, applied, batches) {
  assert.equal(run.outcomes.filter(o => o.state === 'applied').length, applied)
  assert.equal(run.batches, batches)
  noUndo(run)
}
function messages(mailbox) {
  return Object.values(mailbox.folders).flatMap(f => Object.values(f.messages)).sort((a, b) => a.headers.message_id_hash.localeCompare(b.headers.message_id_hash))
}
try {
  await start()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1280, height: 950 } })
  page.setDefaultTimeout(30000)
  const errors = [], actions = []
  page.on('pageerror', e => errors.push(e.message))
  page.on('request', r => { if (r.url().endsWith('/api/accounts/backlog')) actions.push(r.postDataJSON().command.action) })
  await page.addInitScript(() => { window.cancelCsp = []; document.addEventListener('securitypolicyviolation', e => window.cancelCsp.push(e.violatedDirective)) })
  async function openAccount(id) {
    await page.goto(base)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.locator('.accounts > .census-actions select').selectOption(id)
    await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
    await page.locator('.accounts .backlog').getByRole('button', { name: 'Reload saved backlog runs', exact: true }).waitFor()
  }

  const account = await prepare()
  const originalHistory = await histories(account)
  const original = await command(account, 'open')
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const preview = await command(account, 'preview', { options })
  let view = await command(account, 'start', { options, receipt: preview.receipt, confirmed: true })
  const id = view.runs.at(-1).id
  await openAccount(account)
  const earlyRegion = page.locator('.accounts .backlog')
  let releaseResume, sawResume
  const heldResume = new Promise(resolve => { releaseResume = resolve })
  const resumeSeen = new Promise(resolve => { sawResume = resolve })
  const holdResume = async route => {
    if (route.request().postDataJSON().command.action === 'resume') { sawResume(); await heldResume }
    await route.continue()
  }
  await page.route('**/api/accounts/backlog', holdResume)
  await earlyRegion.getByRole('button', { name: 'Resume saved synthetic run', exact: true }).click()
  await resumeSeen
  await earlyRegion.getByRole('button', { name: 'Cancel sorting', exact: true }).click()
  await earlyRegion.getByText('Cancel requested. Waiting for the current batch checkpoint. An already-started batch may finish.', { exact: true }).waitFor()
  const cancelledResume = page.waitForResponse(r => r.url().endsWith('/api/accounts/backlog') && r.request().postDataJSON().command.action === 'stop')
  releaseResume()
  view = await (await cancelledResume).json()
  assert.equal(view.runs.at(-1).status, 'stopped'); counts(view.runs.at(-1), 0, 0)
  await pause(200)
  assert.equal(actions.filter(a => a === 'advance').length, 0, 'Cancel during a delayed Resume must suppress automatic drive')
  await page.unroute('**/api/accounts/backlog', holdResume)
  console.log('PASS built Cancel during a delayed Resume persists Stop after Resume and starts no batch')
  for (let i = 0; i < 2; i++) view = await command(account, 'stop', { run: id })
  assert.equal(view.runs.at(-1).status, 'stopped'); counts(view.runs.at(-1), 0, 0)
  const oldToken = token
  await stop('SIGKILL'); await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  view = await command(account, 'advance', { run: id })
  assert.equal(view.runs.at(-1).status, 'stopped'); counts(view.runs.at(-1), 0, 0)
  assert.deepEqual(view.mailbox, original.mailbox)
  assert.deepEqual(await histories(account), originalHistory)
  console.log('PASS protected Stop before effects, repeated Cancel, actual SIGKILL/replacement, rotated session, unchanged mailbox and independent histories')

  await command(account, 'resume', { run: id })
  view = await command(account, 'advance', { run: id })
  assert.equal(view.runs.at(-1).status, 'completed')
  const completed = view.mailbox
  view = await command(account, 'stop', { run: id })
  assert.deepEqual(view.mailbox, completed); noUndo(view.runs.at(-1))
  console.log('PASS Cancel after completion retains changes and creates no compensation')
  await openAccount(account)
  const smallRegion = page.locator('.accounts .backlog')
  await smallRegion.getByRole('button', { name: 'Preview conditional undo', exact: true }).click()
  const smallUndo = smallRegion.getByRole('button', { name: 'Undo eligible synthetic changes', exact: true })
  await smallUndo.waitFor()
  assert.equal(await smallUndo.isEnabled(), false)
  const smallCheck = smallRegion.getByRole('checkbox', { name: 'I confirm this conditional undo', exact: true })
  assert.equal(await smallCheck.isChecked(), false)
  const smallUndone = page.waitForResponse(r => r.url().endsWith('/api/accounts/backlog') && r.request().postDataJSON().command.action === 'undo')
  await smallCheck.check(); await smallUndo.click()
  view = await (await smallUndone).json()
  assert.equal(view.runs.at(-1).undo.filter(u => u.state === 'restored').length, preview.plan.changes.length)
  const preserved = mailbox => messages(mailbox).map(m => ({ hash: m.headers.message_id_hash, flags: m.flags, labels: m.gmail_labels }))
  assert.deepEqual(preserved(view.mailbox), preserved(original.mailbox))
  console.log('PASS built separate fresh Undo preview, unchecked confirmation, explicit confirmation and exact small-mailbox restoration')

  await stop(); await start('effect')
  const interruptedAccount = await prepare()
  const interruptedPreview = await command(interruptedAccount, 'preview', { options })
  view = await command(interruptedAccount, 'start', { options, receipt: interruptedPreview.receipt, confirmed: true })
  const interruptedId = view.runs.at(-1).id
  const interrupted = command(interruptedAccount, 'advance', { run: interruptedId }).catch(() => null)
  await reached('effect'); await stop('SIGKILL'); await interrupted
  await start()
  view = await command(interruptedAccount, 'open')
  assert.equal(view.runs.at(-1).status, 'uncertain')
  assert.ok(view.runs.at(-1).outcomes.some(o => o.state === 'intent'))
  const uncertainMailbox = view.mailbox
  const uncertainOutcomes = view.runs.at(-1).outcomes
  view = await command(interruptedAccount, 'stop', { run: interruptedId })
  assert.equal(view.runs.at(-1).status, 'stopped')
  assert.deepEqual(view.runs.at(-1).outcomes, uncertainOutcomes); noUndo(view.runs.at(-1))
  await stop('SIGKILL'); await start()
  view = await command(interruptedAccount, 'advance', { run: interruptedId })
  assert.equal(view.runs.at(-1).status, 'stopped')
  assert.deepEqual(view.mailbox, uncertainMailbox)
  assert.deepEqual(view.runs.at(-1).outcomes, uncertainOutcomes)
  await command(interruptedAccount, 'resume', { run: interruptedId })
  view = await command(interruptedAccount, 'advance', { run: interruptedId })
  assert.equal(view.runs.at(-1).status, 'completed')
  counts(view.runs.at(-1), interruptedPreview.plan.changes.length, 2)
  console.log('PASS effect-before-observation crash preserves uncertain intents through Cancel and replacement, and explicit Resume reconciles without duplicate effects')

  await stop(); await start('before', true)
  const gmail = await prepare(true)
  const initialHistory = await histories(gmail)
  const before = await command(gmail, 'open')
  actions.length = 0
  await openAccount(gmail)
  const region = page.locator('.accounts .backlog')
  await region.getByRole('checkbox', { name: 'Protect flagged messages', exact: true }).uncheck()
  const planned = page.waitForResponse(r => r.url().endsWith('/api/accounts/backlog') && r.request().postDataJSON().command.action === 'preview')
  await region.getByRole('button', { name: 'Preview exact backlog changes', exact: true }).click()
  const plan = await (await planned).json()
  const total = plan.plan.changes.length
  assert.ok(total > 500, 'The actual installed mailbox must require multiple batches')
  assert.equal(messages(before.mailbox).length, 601)
  await region.getByRole('checkbox', { name: 'I confirm these exact synthetic mailbox changes', exact: true }).check()
  await region.getByRole('button', { name: 'Sort synthetic backlog', exact: true }).click()
  await reached('before')
  const cancel = region.getByRole('button', { name: 'Cancel sorting', exact: true })
  assert.equal(await cancel.isEnabled(), true, 'Cancel remains usable during the drive loop')
  assert.equal(await region.getByRole('button', { name: 'Reload saved backlog runs', exact: true }).isEnabled(), false)
  await cancel.click()
  await region.getByText('Cancel requested. Waiting for the current batch checkpoint. An already-started batch may finish.', { exact: true }).waitFor()
  assert.equal(await cancel.isEnabled(), false)
  await region.screenshot({ path: join(artifacts, 'in-flight-cancel.png') })
  const stopped = page.waitForResponse(r => r.url().endsWith('/api/accounts/backlog') && r.request().postDataJSON().command.action === 'stop', { timeout: 900000 })
  await writeFile(join(directory, 'continue-before'), 'release installed fake batch')
  view = await (await stopped).json()
  const run = view.runs.at(-1), runId = run.id
  assert.equal(run.status, 'stopped'); counts(run, 500, 1)
  await region.locator('.run-state').filter({ hasText: `stopped · 500 / ${total} applied` }).waitFor()
  await region.getByText('Sorting stopped. Completed changes are retained. Resume is explicit. Undo requires its own preview and confirmation.', { exact: true }).waitFor()
  await pause(300)
  assert.equal(actions.filter(a => a === 'advance').length, 1)
  assert.equal(actions.filter(a => ['undo', 'undo_preview', 'resume_undo'].includes(a)).length, 0)
  const checkpoint = view.mailbox
  const checkpointUndo = await command(gmail, 'undo_preview', { run: runId })
  for (let i = 0; i < 2; i++) {
    view = await command(gmail, 'stop', { run: runId })
    view = await command(gmail, 'advance', { run: runId })
    assert.equal(view.runs.at(-1).status, 'stopped'); counts(view.runs.at(-1), 500, 1)
    assert.deepEqual(view.mailbox, checkpoint)
  }
  const originals = messages(before.mailbox), retained = messages(view.mailbox)
  assert.equal(retained.length, originals.length)
  for (let i = 0; i < originals.length; i++) {
    assert.deepEqual(retained[i].flags, originals[i].flags)
    for (const label of originals[i].gmail_labels.filter(l => l !== '\\Inbox')) assert.ok(retained[i].gmail_labels.includes(label))
  }
  await region.screenshot({ path: join(artifacts, 'stopped-500.png') })
  assert.deepEqual(await histories(gmail), initialHistory)
  console.log(`PASS actual built Gmail UI Cancel during in-flight batch, 601-message mailbox, ${total} eligible changes, retained 500-effect checkpoint, no later batch or undo, flags, unrelated labels and independent histories`)

  await stop('SIGKILL'); await start()
  await openAccount(gmail)
  await region.locator('.run-state').filter({ hasText: `stopped · 500 / ${total} applied` }).waitFor()
  view = await command(gmail, 'advance', { run: runId })
  assert.deepEqual(view.mailbox, checkpoint); counts(view.runs.at(-1), 500, 1)
  await region.screenshot({ path: join(artifacts, 'replacement-stopped.png') })
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await region.getByText('Sortierung angehalten. Abgeschlossene Änderungen bleiben erhalten. Fortsetzen erfolgt ausdrücklich. Rückgängigmachen benötigt eine eigene Vorschau und Bestätigung.', { exact: true }).waitFor()
  await region.screenshot({ path: join(artifacts, 'german-stopped.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  await page.getByRole('radio', { name: 'English', exact: true }).click()
  const resumed = page.waitForResponse(r => r.url().endsWith('/api/accounts/backlog') && r.request().postDataJSON().command.action === 'advance', { timeout: 900000 })
  await region.getByRole('button', { name: 'Resume saved synthetic run', exact: true }).click()
  view = await (await resumed).json()
  assert.equal(view.runs.at(-1).status, 'completed'); counts(view.runs.at(-1), total, 2)
  await region.locator('.run-state').filter({ hasText: `completed · ${total} / ${total} applied` }).waitFor()
  await region.getByRole('button', { name: 'Preview conditional undo', exact: true }).click()
  const undoButton = region.getByRole('button', { name: 'Undo eligible synthetic changes', exact: true })
  await undoButton.waitFor()
  assert.equal(await undoButton.isEnabled(), false)
  const undoCheck = region.getByRole('checkbox', { name: 'I confirm this conditional undo', exact: true })
  assert.equal(await undoCheck.isChecked(), false)
  await region.screenshot({ path: join(artifacts, 'separate-undo-preview.png') })
  const undoPreview = await command(gmail, 'undo_preview', { run: runId })
  assert.equal(undoPreview.eligible.length, total)
  await command(gmail, 'undo', { run: runId, receipt: undoPreview.receipt, confirmed: false }, 422)
  await command(gmail, 'undo', { run: runId, receipt: checkpointUndo.receipt, confirmed: true }, 422)
  const completedMailbox = view.mailbox
  view = await command(gmail, 'open')
  counts(view.runs.at(-1), total, 2)
  assert.deepEqual(view.mailbox, completedMailbox)
  assert.deepEqual(await histories(gmail), initialHistory)
  assert.equal(await page.evaluate(() => localStorage.length + sessionStorage.length), 0)
  assert.deepEqual(await page.evaluate(() => window.cancelCsp), [])
  assert.deepEqual(errors, [])
  console.log('PASS stopped checkpoint survives actual replacement and built reload, German narrow UI, explicit two-batch Resume, fresh unchecked Undo preview and unconfirmed/stale Undo rejection without compensation, no browser storage or CSP errors')
  await writeFile(join(root, '.audit/backlog-cancel-verification.json'), JSON.stringify({ directory, binary, account: gmail, run: runId, mailboxMessages: 601, eligible: total, stoppedApplied: 500, finalBatches: 2, screenshots: artifacts }, null, 2))
} finally {
  await browser?.close()
  await stop()
}
