import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const artifacts = fileURLToPath(new URL('../../.audit/census-browser/', import.meta.url))
await mkdir(artifacts, { recursive: true })
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  await page.addInitScript(() => {
    window.censusCspViolations = []
    document.addEventListener('securitypolicyviolation', event => window.censusCspViolations.push(event.violatedDirective))
  })
  await page.goto('http://127.0.0.1:4173')
  const start = page.getByRole('button', { name: 'Start synthetic census', exact: true })
  await start.waitFor()
  assert.equal(await page.getByRole('button', { name: 'Connect a real account', exact: true }).isDisabled(), true)
  await start.click()
  await page.waitForFunction(() => Number(document.querySelector('progress')?.value) >= 1)
  assert.ok(Number(await page.getByRole('progressbar').getAttribute('value')) < 8)
  await page.screenshot({ path: `${artifacts}/progress.png`, fullPage: true })
  await page.getByRole('button', { name: 'Cancel census', exact: true }).click()
  await page.getByText('Cancelled. Saved headers retained.', { exact: true }).waitFor()
  const saved = await page.getByRole('progressbar').getAttribute('value')
  await page.waitForTimeout(700)
  assert.equal(await page.getByRole('progressbar').getAttribute('value'), saved)
  await page.getByRole('button', { name: 'Resume synthetic census', exact: true }).click()
  await page.getByText('Synthetic census complete', { exact: true }).waitFor()
  assert.equal(await page.getByRole('progressbar').getAttribute('value'), '8')
  await page.getByRole('checkbox', { name: /Confirm own address owner@example.test/ }).check()
  await page.waitForFunction(() => !document.querySelector('.address-candidates input')?.disabled)
  await page.getByText('Local header details', { exact: true }).click()
  await page.getByRole('heading', { name: 'Grüße und Rechnung 1', exact: true }).waitFor({ state: 'visible' })
  await page.getByText('Summary preview', { exact: true }).click()
  const summary = await page.locator('.census-summary').innerText()
  assert.ok(summary.includes('alias:'))
  for (const privateValue of ['owner@example.test', 'news@studio.example', 'Grüße', 'https://', 'token=']) {
    assert.ok(!summary.includes(privateValue))
  }
  await page.screenshot({ path: `${artifacts}/report.png`, fullPage: true })
  await page.reload()
  await page.getByText('Synthetic census complete', { exact: true }).waitFor()
  assert.equal(await page.getByRole('checkbox', { name: /Confirm own address owner@example.test/ }).isChecked(), true)
  await page.getByRole('button', { name: 'Simulate scan error', exact: true }).click()
  await page.getByText('Synthetic scan stopped', { exact: true }).waitFor()
  assert.equal(await page.getByRole('progressbar').getAttribute('value'), '2')
  await page.screenshot({ path: `${artifacts}/error.png`, fullPage: true })
  await page.getByRole('button', { name: 'Resume synthetic census', exact: true }).click()
  await page.getByText('Synthetic census complete', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Run sample preview', exact: true }).click()
  await page.getByText('4 messages receive copies.', { exact: false }).waitFor()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('heading', { name: 'Kopfzeilen erfassen', exact: true }).waitFor()
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
  await page.screenshot({ path: `${artifacts}/german-narrow.png`, fullPage: true })
  assert.deepEqual(errors, [])
  assert.deepEqual(await page.evaluate(() => window.censusCspViolations), [])
  console.log('PASS: real browser census progress, cancellation, resume, failure recovery, confirmation, private summary, foundation preview, German narrow layout and CSP')
  console.log(`Screenshots saved in ${artifacts}`)
} finally {
  await browser.close()
}
