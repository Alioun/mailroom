import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const artifacts = fileURLToPath(new URL('../../.audit/acquisition-browser/', import.meta.url))
await mkdir(artifacts, { recursive: true })
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  await page.addInitScript(() => {
    window.acquisitionCspViolations = []
    document.addEventListener('securitypolicyviolation', event => window.acquisitionCspViolations.push(event.violatedDirective))
  })
  await page.goto('http://127.0.0.1:4173')
  await page.getByText('Synthetic account verification.', { exact: false }).waitFor()
  assert.equal(await page.locator('input[type=password]').count(), 0)
  await page.getByRole('button', { name: 'Add synthetic account', exact: true }).click()
  await page.getByText('Account action complete', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Run or resume account census', exact: true }).click()
  await page.getByText('Account action running', { exact: true }).waitFor()
  await page.waitForFunction(() => [...document.querySelectorAll('.accounts p')].some(p => /(?:25|50|75) headers saved/.test(p.textContent)))
  await page.getByRole('button', { name: 'Cancel account census', exact: true }).click()
  await page.getByText('Account census cancelled', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Run or resume account census', exact: true }).click()
  await page.getByText('Account action complete', { exact: true }).waitFor()
  await page.getByText('100 headers saved · 1 folders', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
  await page.locator('.account-addresses').getByRole('checkbox', { name: 'Confirm own address owner@example.test', exact: true }).check()
  await page.waitForFunction(() => !document.querySelector('.account-addresses input')?.disabled)
  await page.getByText('Saved local headers', { exact: true }).click()
  await page.getByText('Synthetic account header 100 · received · UID 100', { exact: true }).waitFor()
  await page.screenshot({ path: `${artifacts}/complete.png`, fullPage: true })
  await page.reload()
  await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
  await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
  assert.equal(await page.locator('.account-addresses').getByRole('checkbox', { name: 'Confirm own address owner@example.test', exact: true }).isChecked(), true)
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('heading', { name: 'Konto verbinden', exact: true }).waitFor()
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
  await page.screenshot({ path: `${artifacts}/german-narrow.png`, fullPage: true })
  assert.deepEqual(errors, [])
  assert.deepEqual(await page.evaluate(() => window.acquisitionCspViolations), [])
  assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage) })), { local: [], session: [] })
  console.log('PASS: synthetic account connect, scan, cancel, encrypted resume, saved headers, own-address confirmation, reload, German narrow layout, CSP and empty browser storage')
} finally { await browser.close() }
