import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url)).replace(/\/$/, '')
let metadata = JSON.parse(await readFile(join(root, '.audit/managesieve-transactions-retained.json'), 'utf8'))
const artifacts = join(root, '.audit/managesieve-transactions-browser')
await mkdir(artifacts, { recursive: true })
const checks = [], errors = [], remote = []
const pause = ms => new Promise(r => setTimeout(r, ms))
const browser = await chromium.launch({ headless: true })
let replacement
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(metadata.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${metadata.binary} --exact ${metadata.test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(metadata.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
}
async function replace() {
  identity(); assert.equal(createHash('sha256').update(await readFile(metadata.binary)).digest('hex'), metadata.binary_sha256)
  process.kill(metadata.pid, 'SIGKILL')
  for (let i = 0; i < 300; i++) { try { process.kill(metadata.pid, 0) } catch { break } await pause(10) }
  await writeFile(join(metadata.directory, 'transaction-boundary'), '')
  replacement = spawn(metadata.binary, ['--exact', metadata.test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_TRANSACTIONS_FIXTURE: metadata.directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 300; i++) {
    assert.equal(replacement.exitCode, null)
    if (await fetch(`${metadata.base}/api/session`).then(r => r.ok, () => false)) break
    await pause(20)
  }
  metadata = { ...metadata, pid: replacement.pid }
  identity()
  await writeFile(join(root, '.audit/managesieve-transactions-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  replacement.unref()
}
try {
  identity()
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  page.on('pageerror', () => errors.push('Page error'))
  page.on('request', r => { if (!r.url().startsWith(metadata.base)) remote.push('Non-local request') })
  await page.addInitScript(() => { window.transactionCsp = []; document.addEventListener('securitypolicyviolation', e => window.transactionCsp.push(e.violatedDirective)) })
  async function load() {
    await page.goto(metadata.base)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.locator('.accounts > .census-actions select').selectOption(metadata.account)
  }
  await load()
  const panel = page.locator('.sieve-transactions')
  await panel.getByRole('button', { name: 'Open transaction history', exact: true }).click()
  await panel.getByRole('button', { name: 'Preview exact replacement', exact: true }).waitFor()
  const response = page.waitForResponse(r => r.url().endsWith('/sieve-transactions') && r.request().postDataJSON().command.action === 'preview')
  await panel.getByRole('button', { name: 'Preview exact replacement', exact: true }).click()
  const preview = await (await response).json()
  const approval = panel.getByRole('checkbox', { name: 'I reviewed the exact bytes, active selection and immediate effects.', exact: true })
  await approval.waitFor(); assert.equal(await approval.isChecked(), false)
  const confirm = panel.getByRole('button', { name: 'Confirm exact synthetic transaction', exact: true })
  assert.equal(await confirm.isDisabled(), true)
  assert.match(await panel.innerText(), /CHECKSCRIPT/); assert.match(await panel.innerText(), /conservative|Conservative/)
  await panel.getByRole('button', { name: 'Review exact private bytes', exact: true }).click()
  const reviewed = await panel.locator('pre').allTextContents()
  const target = preview.transaction.before.bodies[0]
  const exact = new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(target.bytes))
  assert.ok(reviewed.some(text => text.includes(exact)))
  assert.ok(reviewed.every(text => text.includes('HEX')))
  assert.equal(await panel.locator('pre img, pre script').count(), 0)
  await panel.getByRole('button', { name: 'Hide private bytes', exact: true }).click()
  await panel.screenshot({ path: join(artifacts, 'english-exact-review-digests.png') })
  checks.push('Built English exact bytes/digests and active selection review, escaped text, conservative proof/warnings and unchecked exact confirmation')
  await approval.check(); await confirm.click()
  await panel.locator('article h4').filter({ hasText: 'Provider state observed' }).first().waitFor()
  assert.match(await panel.locator('article').first().innerText(), /Replace existing script bytes.*Readback observed/s)
  assert.match(await panel.locator('article').first().innerText(), /Activate target.*Readback observed/s)
  const compResponse = page.waitForResponse(r => r.url().endsWith('/sieve-transactions') && r.request().postDataJSON().command.action === 'compensation_preview')
  await panel.locator('article').first().getByRole('button', { name: 'Preview conditional compensation', exact: true }).click()
  const compensation = await (await compResponse).json()
  assert.ok(compensation.effects.includes('disable'))
  assert.equal(await approval.isChecked(), false)
  assert.match(await panel.innerText(), /Empty SETACTIVE disables all scripts/)
  await approval.check(); await confirm.click()
  await panel.locator('article').nth(1).getByRole('heading', { name: /Provider state observed/ }).waitFor()
  checks.push('Built upload and explicit activation show separate acknowledgment/readback, then separately approved compensation explicitly disables to the originally empty selection')
  await writeFile(join(metadata.directory, 'transaction-boundary'), 'upload_observed')
  await panel.getByRole('button', { name: 'Preview exact replacement', exact: true }).click()
  await approval.check(); await confirm.click()
  for (let i = 0; i < 1000; i++) { if (await readFile(join(metadata.directory, 'reached-upload_observed')).then(() => true, () => false)) break; await pause(10) }
  assert.equal(await readFile(join(metadata.directory, 'reached-upload_observed')).then(() => true, () => false), true)
  await replace()
  await page.reload()
  await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
  await page.locator('.accounts > .census-actions select').selectOption(metadata.account)
  await panel.getByRole('button', { name: 'Open transaction history', exact: true }).click()
  await panel.locator('article').nth(2).getByRole('heading', { name: /Uncertain/ }).waitFor()
  await panel.locator('article').nth(2).getByRole('button', { name: 'Reconcile without writes', exact: true }).click()
  await panel.locator('article').nth(2).getByRole('heading', { name: /Fresh exact review required/ }).waitFor()
  await panel.locator('article').nth(2).getByRole('button', { name: 'Preview conditional compensation', exact: true }).click()
  assert.equal(await approval.isChecked(), false); await approval.check(); await confirm.click()
  await panel.locator('article').nth(3).getByRole('heading', { name: /Provider state observed/ }).waitFor()
  checks.push('Actual built-browser SIGKILL/replacement retains uncertainty, reload performs no continuation, reconcile requires fresh review and compensation stays separate')
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await panel.getByRole('heading', { name: 'Sieve-Transaktionen im Testbesitz', exact: true }).waitFor()
  assert.match(await panel.innerText(), /Kompensation macht Weiterleitungen/)
  assert.equal(await panel.locator('pre').count(), 0)
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
  await panel.screenshot({ path: join(artifacts, 'german-narrow-history.png') })
  assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage), csp: window.transactionCsp })), { local: [], session: [], csp: [] })
  assert.deepEqual(errors, []); assert.deepEqual(remote, [])
  checks.push('English/German controls, narrow viewport, empty browser storage, CSP and local-only requests without persisting private script review')
  await writeFile(join(root, '.audit/managesieve-transactions-browser-output.json'), JSON.stringify({ ...metadata, checks, browser: 'repository Playwright Chromium', synthetic_only: true }, null, 2) + '\n')
  console.log(`PASS: ${checks.length} built-browser transaction groups with actual TLS and application process replacement`)
} finally { await browser.close() }
