import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const root = fileURLToPath(new URL('../../', import.meta.url))
const artifacts = `${root}.audit/personal-prototype`
await mkdir(artifacts, { recursive: true })
const builds = (await readFile(`${artifacts}/bridge-build.jsonl`, 'utf8')).trim().split('\n').map(line => JSON.parse(line))
const artifact = builds.findLast(record => record.reason === 'compiler-artifact' && record.target.name === 'ui_bridge' && record.executable)
assert.ok(artifact?.executable?.startsWith(`${root}target/`), 'Use the exact locally compiled test artifact')
const bridge = spawn(artifact.executable, ['--exact', 'synthetic_ui_bridge', '--ignored', '--nocapture'], { cwd: root, env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin' }, detached: true, stdio: ['pipe', 'pipe', 'pipe'] })
const pending = new Map()
let sequence = 0
let diagnostics = ''
let bridgeFailure = null
bridge.stderr.on('data', bytes => { diagnostics += bytes.toString() })
const closed = new Promise(resolve => {
  bridge.on('error', error => {
    bridgeFailure = error
    for (const request of pending.values()) request.reject(new Error('Synthetic bridge could not start'))
    pending.clear()
    resolve(-1)
  })
  bridge.on('close', code => {
    bridgeFailure = new Error(`Synthetic bridge exited with code ${code}`)
    for (const request of pending.values()) request.reject(bridgeFailure)
    pending.clear()
    resolve(code)
  })
})
createInterface({ input: bridge.stdout }).on('line', line => {
  const start = line.indexOf('MAILROOM_UI_RESPONSE ')
  if (start < 0) return
  const response = JSON.parse(line.slice(start + 'MAILROOM_UI_RESPONSE '.length))
  const request = pending.get(response.id)
  assert.ok(request, 'Each bridge response belongs to one request')
  pending.delete(response.id)
  request.resolve({ ...response, body: Buffer.from(response.body, 'base64') })
})
async function request(path, { method = 'GET', headers = {}, body = '' } = {}) {
  if (bridgeFailure) throw bridgeFailure
  const id = ++sequence
  const reply = new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
  bridge.stdin.write(`${JSON.stringify({ id, method, path, headers: { host: '127.0.0.1:4173', ...headers }, body })}\n`)
  return await reply
}
let browser
let timedOut = false
let phase = 'HTTP checks'
async function verifySavedWorkflows(headers, sample) {
  async function post(action, body, expected = 200) {
    const response = await request(`/api/accounts/${action}`, { method: 'POST', headers, body: JSON.stringify(body) })
    assert.equal(response.status, expected, `${action} returns its expected status`)
    assert.equal(response.headers['cache-control'], 'no-store')
    return JSON.parse(response.body)
  }
  async function complete() {
    const deadline = Date.now() + 15_000
    while (Date.now() < deadline) {
      const response = await request('/api/accounts/status', { headers })
      assert.equal(response.status, 200)
      const status = JSON.parse(response.body)
      if (status.job.progress.state !== 'running') {
        assert.equal(status.job.progress.state, 'complete', 'The synthetic account action completes')
        return status.job.account.id
      }
      await new Promise(resolve => setTimeout(resolve, 20))
    }
    throw new Error('Synthetic account action exceeded its deadline')
  }
  async function create() {
    await post('synthetic', {})
    const account = await complete()
    await post('scan', { account })
    assert.ok(await complete() === account, 'The census completes for the requested account')
    const workspace = await post('workspace/open', { account })
    await post('workspace/save', { account, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
    return account
  }
  const account = await create()
  const other = await create()
  const accounts = await post('list', {})
  assert.equal(accounts.length, 2)
  const view = await post('open', { account })
  assert.equal(view.census.acquisition_state, 'complete')
  assert.ok(view.addresses.every(address => !address.confirmed), 'Observation does not establish ownership')
  await post('confirm', { account, address: 'owner@example.test', confirmed: true })
  const report = await post('report', { account, as_of: sample.as_of })
  assert.equal(report.complete, true)
  assert.equal(report.received.messages, 99)
  assert.equal(report.sent, 1)
  const reviewInput = { account, as_of: sample.as_of, version: report.version, approved: [] }
  const summary = await post('summary/review', reviewInput)
  for (const marker of ['owner@example.test', 'shop.example', 'synthetic-private-token', 'Synthetic account header']) {
    assert.ok(!summary.summary.includes(marker), 'The undisclosed census summary omits private fixture fields')
  }
  const exported = await post('summary/export', { ...reviewInput, receipt: summary.receipt, confirmed: true })
  assert.ok(exported.summary === summary.summary, 'Only the exact reviewed summary is exported')
  await post('summary/export', { ...reviewInput, receipt: summary.receipt, confirmed: false }, 422)
  await post('summary/export', { ...reviewInput, account: other, receipt: summary.receipt, confirmed: true }, 422)
  const proof = await post('proof/preview', { account })
  assert.equal(proof.proof.counts.messages, 99)
  assert.equal(proof.proof.counts.review, 99)
  assert.equal(proof.proof.counts.copies, 0)
  assert.equal(proof.proof.artifact.context.recipient_evidence, 'unknown')
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  await post('proof/download', { account, receipt: summary.receipt, confirmed: true }, 422)
  const ledger = await post('ledger', { account, command: { action: 'open' } })
  assert.equal(ledger.snapshot.document.enabled, false)
  const entry = { address: 'owner@example.test', service: 'PIPE-PRIVATE-SERVICE', folder: 'Services', first_seen: 'owner supplied', status: 'active', note: 'PIPE-PRIVATE-NOTE', category: 'orders', invoice: false }
  const batch = { content_version: ledger.snapshot.content_version, revision: ledger.revision, entries: [entry], enabled: true }
  const ledgerReview = await post('ledger', { account, command: { action: 'preview', batch } })
  const ledgerConfirmation = { action: 'confirm', batch, receipt: ledgerReview.receipt, confirmed: true }
  await post('ledger', { account: other, command: ledgerConfirmation }, 422)
  await post('ledger', { account, command: { ...ledgerConfirmation, receipt: proof.receipt } }, 422)
  await post('ledger', { account, command: { ...ledgerConfirmation, batch: { ...batch, entries: [{ ...entry, note: 'altered' }] } } }, 422)
  const saved = await post('ledger', { account, command: ledgerConfirmation })
  assert.equal(saved.snapshot.document.entries.length, 1)
  const reopened = await post('ledger', { account: other, command: { action: 'open' } })
  assert.ok(JSON.stringify(reopened.snapshot.document.entries[0]) === JSON.stringify(saved.snapshot.document.entries[0]), 'Both accounts observe the authoritative ledger entry')
  await post('ledger', { account, command: ledgerConfirmation }, 422)
  await post('proof/download', { account, receipt: proof.receipt, confirmed: true }, 422)
  const refreshed = await post('proof/preview', { account })
  assert.equal(refreshed.proof.counts.review, 99)
  assert.equal(refreshed.proof.counts.copies, 0)
  const winddown = command => post('winddown', { account, as_of: sample.as_of, command })
  let wind = await winddown({ action: 'open' })
  assert.equal(wind.report.settings.winding_down, false)
  assert.ok(wind.report.blockers.includes('comparison_accounts_absent'))
  const change = { kind: 'settings', settings: { winding_down: true, comparisons: {} } }
  const decision = await winddown({ action: 'preview', version: wind.report.version, change })
  await post('winddown', { account, as_of: sample.as_of, command: { action: 'confirm', version: decision.report.version, change, receipt: decision.receipt, confirmed: true } })
  wind = await winddown({ action: 'open' })
  assert.equal(wind.report.settings.winding_down, true)
  assert.equal(wind.report.verdict, 'insufficient_evidence_or_waiting')
  const local = await winddown({ action: 'export_preview', version: wind.report.version, disclosure: 'local_file' })
  const shared = await winddown({ action: 'export_preview', version: wind.report.version, disclosure: 'agent_summary' })
  assert.ok(local.text === wind.terminal, 'The local wind-down file matches its terminal rendering')
  for (const marker of ['owner@example.test', entry.service, entry.note]) assert.ok(!shared.text.includes(marker), 'Wind-down disclosure omits private fields')
  await post('winddown', { account, as_of: sample.as_of, command: { action: 'export', version: wind.report.version, disclosure: 'local_file', receipt: shared.receipt, confirmed: true } }, 422)
  const download = await winddown({ action: 'export', version: wind.report.version, disclosure: 'local_file', receipt: local.receipt, confirmed: true })
  assert.ok(download.text === local.text, 'Wind-down export matches its exact approved preview')
  const after = await post('summary/review', reviewInput)
  for (const marker of [entry.address, entry.service, entry.note]) assert.ok(!after.summary.includes(marker), 'Ledger bookkeeping stays out of census disclosure')
  console.log('PASS: saved census, ownership, redacted exact export, conservative proof, shared authoritative ledger, stale and wrong-purpose receipts, wind-down bookkeeping and local disclosure through the real router')
  const subscriptions = id => post('unsubscribe', { account: id, command: { action: 'open', as_of: sample.as_of } })
  const unsub = await subscriptions(account)
  const siblings = unsub.report.rows.filter(row => row.sender === 'news@shop.example')
  assert.ok(siblings.length >= 3, 'The fixture exposes separate receiving-address and list subscriptions')
  const chosen = siblings.find(row => row.recipient === 'team@home.example.test' && row.list_id === '<offers.shop.example>')
  assert.ok(chosen?.methods.includes('one_click'), 'Only the installed synthetic adapter offers its fake authenticated one-click action')
  const subscriptionInput = { as_of: sample.as_of, selections: [{ subscription: chosen.id, method: 'one_click' }], folders: { personal: 'Muted', business: 'Quiet' } }
  const subscriptionPreview = () => post('unsubscribe', { account, command: { action: 'preview', input: subscriptionInput } })
  const subscriptionConfirm = (preview, extra = {}, expected = 200) => post('unsubscribe', { account, command: { action: 'confirm', input: subscriptionInput, receipt: preview.receipt, confirmation_action: preview.confirmation_action, mail_confirmation_action: preview.local_mail_confirmation, ...extra } }, expected)
  let preview = await subscriptionPreview()
  await subscriptionConfirm(preview, { receipt: local.receipt }, 422)
  preview = await subscriptionPreview()
  await subscriptionConfirm(preview, { input: { ...subscriptionInput, selections: [...subscriptionInput.selections, { subscription: siblings.find(row => row.id !== chosen.id).id, method: 'keep' }] } }, 422)
  preview = await subscriptionPreview()
  const affected = await subscriptionConfirm(preview)
  assert.equal(affected.synthetic, true)
  assert.equal(affected.history.length, 1)
  assert.ok(affected.history[0].subscription === chosen.id, 'The recorded effect belongs only to the approved subscription')
  assert.equal(affected.history[0].status, 'http_observed')
  assert.equal(affected.history[0].host, 'shop.example')
  assert.equal(affected.history[0].http_status, 202)
  assert.equal(Object.keys(affected.received).length, 1)
  assert.ok(!JSON.stringify(affected.history).includes('synthetic-private-token'), 'The history omits the full unsubscribe URI and token')
  await subscriptionConfirm(preview, {}, 422)
  const untouched = await subscriptions(other)
  assert.equal(untouched.history.length, 0)
  assert.equal(Object.keys(untouched.received).length, 0)
  for (const sibling of siblings.filter(row => row.id !== chosen.id)) assert.ok(!affected.history.some(effect => effect.subscription === sibling.id), 'Sibling subscriptions remain untouched')
  const invoices = await post('invoices', { account, command: { action: 'open' } })
  assert.ok(invoices.report.rows.every(row => !row.send_available), 'Unclassified or mixed recipients cannot authorize an invoice send')
  await post('invoices', { account, command: { action: 'preview', selection: 'unapproved', target: 'accounting', mode: 'smtp' } }, 403)
  await post('invoices', { account, command: { action: 'preview', selection: 'unapproved', target: 'accounting', mode: 'mail_app' } }, 422)
  console.log('PASS: separate subscription approvals, fake-only one-click effect, sibling and account isolation, wrong-purpose and altered-batch rejection, replay denial, host-only history and SMTP-only production denial')
}
const deadline = setTimeout(() => {
  timedOut = true
  if (bridge.pid) { try { process.kill(-bridge.pid, 'SIGTERM') } catch {} }
  void browser?.close()
}, 120_000)
try {
  const session = await request('/api/session')
  assert.equal(session.status, 200)
  const token = JSON.parse(session.body).token
  const jsonHeaders = { 'x-mailroom-token': token, 'content-type': 'application/json' }
  assert.equal((await request('/api/accounts/status')).status, 403)
  const status = await request('/api/accounts/status', { headers: jsonHeaders })
  assert.equal(JSON.parse(status.body).synthetic, true)
  const accounts = await request('/api/accounts/list', { method: 'POST', headers: jsonHeaders, body: '{}' })
  assert.deepEqual(JSON.parse(accounts.body), [])
  const denied = await request('/api/operations', { method: 'POST', headers: jsonHeaders, body: JSON.stringify({ operation: 'send_message' }) })
  assert.equal(denied.status, 403)
  const origin = await request('/api/accounts/status', { headers: { ...jsonHeaders, origin: 'https://outside.example' } })
  assert.equal(origin.status, 403)
  const sample = await request('/api/sample', { headers: jsonHeaders })
  assert.ok(JSON.parse(sample.body).messages.length > 0)
  const home = await request('/')
  assert.equal(home.status, 200)
  assert.match(home.body.toString(), /\/assets\/index-/)
  assert.match(home.headers['content-security-policy'], /script-src 'self'/)
  console.log('PASS: real synthetic router, protected requests, empty account list, production send denial, foreign-origin rejection and built assets over private pipes')
  if (process.argv.includes('--workflows-only')) {
    phase = 'saved account workflows'
    await verifySavedWorkflows(jsonHeaders, JSON.parse(sample.body))
  } else if (!process.argv.includes('--http-only')) {
    phase = 'browser launch'
    try {
      browser = await chromium.launch({ headless: true, chromiumSandbox: true, env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin' }, timeout: 15_000 })
    } catch (error) {
      await writeFile(`${artifacts}/browser-launch-error.log`, error instanceof Error ? error.message : 'Browser launch failed')
      throw new Error('Browser launch failed. Details are in the credential-free browser-launch-error.log.')
    }
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
    phase = 'browser checks'
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    let releaseList
    const listGate = new Promise(resolve => { releaseList = resolve })
    let holdList = true
    let failList = false
    await page.route('**/*', async route => {
      const incoming = route.request()
      const url = new URL(incoming.url())
      assert.equal(url.origin, 'http://127.0.0.1:4173', 'The fixture never requests an external endpoint')
      if (url.pathname === '/api/accounts/list') {
        if (holdList) await listGate
        if (failList) {
          failList = false
          return await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'Invented response failure' }) })
        }
      }
      const response = await request(url.pathname + url.search, { method: incoming.method(), headers: await incoming.allHeaders(), body: incoming.postData() ?? '' })
      await route.fulfill({ status: response.status, headers: response.headers, body: response.body })
    })
    await page.goto('http://127.0.0.1:4173', { waitUntil: 'domcontentloaded' })
    await page.getByText('Loading saved accounts…', { exact: true }).waitFor()
    await page.keyboard.press('Tab')
    assert.equal(await page.getByRole('link', { name: 'Skip to content', exact: true }).evaluate(link => link === document.activeElement), true)
    holdList = false
    releaseList()
    await page.getByText('No saved accounts yet.', { exact: true }).waitFor()
    await page.screenshot({ path: `${artifacts}/empty-desktop.png`, fullPage: true })
    failList = true
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.getByRole('button', { name: 'Retry loading accounts', exact: true }).click()
    await page.getByText('No saved accounts yet.', { exact: true }).waitFor()
    await page.getByRole('button', { name: 'Add synthetic account', exact: true }).click()
    await page.locator('.accounts [aria-live=polite]').getByText('Account action complete', { exact: true }).waitFor()
    const account = await page.locator('.accounts > .census-actions select').inputValue()
    assert.ok(account)
    await page.reload({ waitUntil: 'domcontentloaded' })
    await page.getByRole('button', { name: 'Open saved census', exact: true }).waitFor()
    assert.equal(await page.locator('.accounts > .census-actions select').inputValue(), account)
    await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
    await page.setViewportSize({ width: 390, height: 844 })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    await page.screenshot({ path: `${artifacts}/saved-german-narrow.png`, fullPage: true })
    assert.deepEqual(errors, [])
    assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage) })), { local: [], session: [] })
    console.log('PASS: built loading, empty, retry, synthetic creation, automatic saved-account restoration, keyboard skip link, German narrow layout and empty browser storage')
  }
} finally {
  await browser?.close()
  bridge.stdin.end()
  const code = await closed
  clearTimeout(deadline)
  await writeFile(`${artifacts}/ui-bridge-runtime.log`, diagnostics)
  if (timedOut) throw new Error(`Private-pipe verification exceeded its deadline during ${phase}`)
  assert.equal(code, 0, 'The synthetic bridge exits cleanly')
}
