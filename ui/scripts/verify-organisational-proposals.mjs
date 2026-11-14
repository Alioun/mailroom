import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'

const root = fileURLToPath(new URL('../../', import.meta.url)).replace(/\/$/, '')
const path = join(root, '.audit/organisational-proposals-retained.json')
const metadata = JSON.parse(await readFile(path, 'utf8'))
const artifacts = join(root, '.audit/organisational-proposals-browser')
await mkdir(artifacts, { recursive: true })
async function restart() {
  assert.equal(execFileSync('ps', ['-p', String(metadata.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${metadata.binary} --exact ${metadata.test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(metadata.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  assert.equal(createHash('sha256').update(await readFile(metadata.binary)).digest('hex'), metadata.binary_sha256)
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(metadata.pid), '-iTCP', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  process.kill(metadata.pid, 'SIGTERM')
  for (let i = 0; i < 200; i++) {
    try { process.kill(metadata.pid, 0) } catch { break }
    await new Promise(r => setTimeout(r, 25))
    assert.ok(i < 199, 'Verified fake did not terminate')
  }
  const server = spawn(metadata.binary, ['--exact', metadata.test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: metadata.directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    if (await fetch(`${metadata.base}/api/session`).then(r => r.ok, () => false)) {
      metadata.pid = server.pid; server.unref()
      await writeFile(path, JSON.stringify(metadata, null, 2) + '\n')
      return
    }
    await new Promise(r => setTimeout(r, 50))
  }
  throw Error('Replacement fixture failed to start')
}
const browser = await chromium.launch({ headless: true })
const checks = []
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  const errors = [], remote = []
  page.on('pageerror', e => errors.push(e.message))
  page.on('request', r => { if (!r.url().startsWith(metadata.base)) remote.push('Non-local request') })
  await page.addInitScript(() => { window.proposalCsp = []; document.addEventListener('securitypolicyviolation', e => window.proposalCsp.push(e.violatedDirective)) })
  // The HTTP run left one of its main account's candidates rejected for good; the other account has made no decisions.
  const account = metadata.other
  const region = page.locator('.accounts .rules-proof')
  const panel = region.locator('.filing-proposals')
  async function open() {
    await page.goto(new URL('/advanced.html', metadata.base).href)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.locator('.accounts > .census-actions select').selectOption(account)
    await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
    await region.getByText('Saved workspace loaded.', { exact: true }).waitFor()
    await panel.locator(':scope > summary').click()
    const response = page.waitForResponse(r => r.url().endsWith('/api/accounts/proposals'))
    await panel.getByRole('button', { name: 'Generate or reload filing proposals', exact: true }).click()
    const generation = await (await response).json()
    await panel.locator('.filing-candidate').first().waitFor()
    return generation
  }
  let generation = await open()
  assert.ok(generation.candidates.length >= 2)
  assert.equal(await panel.getByRole('checkbox', { name: /Select proposal/ }).count(), generation.candidates.length)
  for (const box of await panel.locator('.filing-candidate input[type=checkbox]').all()) assert.equal(await box.isChecked(), false)
  checks.push('Actual built local panel loads deterministic candidates with empty initial selection')
  const rejectCandidate = generation.candidates.find(c => !c.rejected)
  const article = panel.locator(`[data-candidate="${rejectCandidate.id}"]`)
  await article.getByRole('checkbox').check()
  await panel.getByRole('button', { name: 'Review exact rejections', exact: true }).click()
  const review = panel.locator('.proposal-review')
  const checkbox = review.getByRole('checkbox', { name: 'I confirm this exact proposal batch', exact: true })
  assert.equal(await checkbox.isChecked(), false)
  assert.equal(await review.getByRole('button', { name: 'Save reviewed local decision', exact: true }).isDisabled(), true)
  await checkbox.check()
  const rejected = page.waitForResponse(r => r.url().endsWith('/api/accounts/proposals') && r.request().postDataJSON().command.action === 'confirm')
  await review.getByRole('button', { name: 'Save reviewed local decision', exact: true }).click()
  assert.equal((await rejected).status(), 200)
  await panel.getByText('Local decision saved. Run fresh proof over the saved rules.', { exact: true }).waitFor()
  await restart()
  generation = await open()
  const retained = panel.locator(`[data-candidate="${rejectCandidate.id}"]`)
  await retained.getByText('Sender proposal rejected. It stays unavailable after refresh.', { exact: true }).waitFor()
  assert.equal(await retained.getByRole('checkbox').isDisabled(), true)
  checks.push('Exact rejection persists through actual server replacement and built UI reload')
  const available = generation.candidates.filter(c => !c.rejected)
  assert.ok(available.length >= 1)
  await panel.locator(`[data-candidate="${available[0].id}"]`).getByRole('checkbox').check()
  const preview = page.waitForResponse(r => r.url().endsWith('/api/accounts/proposals') && r.request().postDataJSON().command.action === 'preview')
  await panel.getByRole('button', { name: 'Review selected local changes', exact: true }).click()
  const reviewed = await (await preview).json()
  assert.equal(reviewed.changes.length, 1)
  await review.getByText('Original rule documents', { exact: true }).waitFor()
  await review.getByText('Resulting rule documents', { exact: true }).waitFor()
  await review.getByText('Pin results', { exact: true }).waitFor()
  await review.getByText('Omissions', { exact: true }).waitFor()
  assert.equal(await checkbox.isChecked(), false)
  await checkbox.check()
  await panel.locator(`[data-candidate="${available[0].id}"]`).getByRole('checkbox').uncheck()
  assert.equal(await review.count(), 0)
  await panel.locator(`[data-candidate="${available[0].id}"]`).getByRole('checkbox').check()
  await panel.getByRole('button', { name: 'Review selected local changes', exact: true }).click()
  assert.equal(await checkbox.isChecked(), false)
  await page.screenshot({ path: join(artifacts, 'english-exact-batch-review.png'), fullPage: true })
  await checkbox.check()
  const accepted = page.waitForResponse(r => r.url().endsWith('/api/accounts/proposals') && r.request().postDataJSON().command.action === 'confirm')
  await review.getByRole('button', { name: 'Save reviewed local decision', exact: true }).click()
  const saved = await (await accepted).json()
  await panel.getByText('Local decision saved. Run fresh proof over the saved rules.', { exact: true }).waitFor()
  assert.equal(saved.decisions.history.filter(d => d.disposition === 'accept').length, generation.decisions.history.filter(d => d.disposition === 'accept').length + 1)
  const documents = await page.evaluate(async account => {
    const token = (await (await fetch('/api/session')).json()).token
    const response = await fetch('/api/accounts/workspace/open', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify({ account }) })
    return (await response.json()).workspace.input.documents
  }, account)
  assert.deepEqual(documents, reviewed.resulting_documents)
  assert.equal(await region.getByRole('checkbox', { name: 'I confirm this proof for a local Sieve download', exact: true }).count(), 0)
  await region.getByRole('button', { name: 'Prove saved census', exact: true }).click()
  await region.getByText('Proof results', { exact: true }).waitFor()
  checks.push('Exact selected-only UI approval saves reviewed bytes, resets confirmation on changed selection, and requires fresh saved proof')
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await panel.getByText('Erweitert · Ablagevorschläge', { exact: true }).waitFor()
  await panel.getByRole('button', { name: 'Ablagevorschläge erzeugen oder neu laden', exact: true }).click()
  await panel.getByText(/Ungeklärte Beobachtungen/).waitFor()
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
  await panel.screenshot({ path: join(artifacts, 'german-narrow-panel.png') })
  assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage), csp: window.proposalCsp })), { local: [], session: [], csp: [] })
  assert.deepEqual(errors, []); assert.deepEqual(remote, [])
  checks.push('English/German, narrow layout, no remote requests, no browser storage and no CSP errors')
  await writeFile(join(root, '.audit/organisational-proposals-browser-output.json'), JSON.stringify({ ...metadata, checks, browser: 'repository Playwright Chromium', synthetic_only: true }, null, 2) + '\n')
  console.log(`PASS: ${checks.length} actual built-browser behavior groups including restart, durable rejection and exact selected-batch acceptance`)
} finally { await browser.close() }
