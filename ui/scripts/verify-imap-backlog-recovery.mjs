import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { chromium } from 'playwright'
import { fixture, root, base } from '../../scripts/imap-backlog-fixture.mjs'

const f = await fixture()
const browser = await chromium.launch({ headless: true })
const artifacts = join(root, '.audit/imap-backlog-workflow-recovery-browser', new Date().toISOString().replace(/[:.]/g, '-'))
await mkdir(artifacts, { recursive: true })
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = [], errors = [], external = []
let page
async function sort() {
  const p = await f.command('preview', { options })
  await f.command('confirm', { options, receipt: p.receipt, confirmed: true }); await f.terminal()
  return (await f.command('status')).journal.runs.at(-1)
}
async function undo(run) {
  const p = await f.command('undo_preview', { run: run.id })
  await f.command('undo', { run: run.id, receipt: p.receipt, confirmed: true }); await f.terminal()
}
async function response(action, button) {
  const [result] = await Promise.all([
    page.waitForResponse(r => r.url().endsWith('/api/accounts/recovery') && r.request().postDataJSON().action === action),
    button.click(),
  ])
  assert.equal(result.status(), 200)
  return result.json()
}
try {
  for (const multiple of [false, true]) {
    await f.start(true, true, { ledger: true }); await f.savedWorkspace(true)
    await undo(await sort()); const run = await sort()
    const accounts = [f.account]
    if (multiple) { await f.post('synthetic-gmail', {}); accounts.push((await f.terminal()).account.id) }
    const capturedState = await f.observe()
    assert.ok(Object.values(capturedState.populated).every(Boolean))
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } })
    page.on('pageerror', e => errors.push(e.message))
    page.on('request', r => { if (!r.url().startsWith(base)) external.push(r.url()) })
    await page.addInitScript(() => { window.protocolRecoveryCsp = []; document.addEventListener('securitypolicyviolation', e => window.protocolRecoveryCsp.push(e.violatedDirective)) })
    await page.goto(base)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    const region = page.locator('.accounts .recovery')
    await region.locator(':scope > summary').click()
    for (const account of accounts) {
      const box = region.locator(`fieldset input[value="${account}"]`)
      assert.equal(await box.isChecked(), false); await box.check()
    }
    assert.equal(await region.locator('.ledger-inclusion').isChecked(), false)
    if (multiple) await region.locator('.ledger-inclusion').check()
    const offer = await response('selected_capture_preview', region.getByRole('button', { name: 'Preview exact capture', exact: true }))
    assert.ok(offer.preview.accounts.every(a => a.state_schema === 23))
    assert.equal(await region.locator('.recovery-preview').getByRole('checkbox').isChecked(), false)
    assert.equal(await region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }).isEnabled(), false)
    await region.locator('.recovery-preview').getByRole('checkbox').check()
    const captured = await response('selected_capture', region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }))
    await region.getByRole('status').waitFor()
    await f.observe('later_flag'); await undo(run)
    const later = await f.observe()
    const wire = await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')
    await f.stop('SIGKILL')
    const installation = join(f.directory, multiple ? 'built-selected-restored' : 'built-single-restored')
    await mkdir(installation); f.restoreInto(installation); await f.start()
    await page.reload(); await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
    await region.locator(':scope > summary').click()
    await region.locator('select').selectOption(captured.bundle)
    assert.equal(await region.locator('.ledger-restoration').isChecked(), false)
    if (multiple) await region.locator('.ledger-restoration').check()
    const restoredOffer = await response('selected_restore_preview', region.getByRole('button', { name: 'Authentifizieren und Wiederherstellung prüfen', exact: true }))
    assert.equal(restoredOffer.authenticated, true)
    assert.deepEqual(restoredOffer.preview.manifest.accounts.map(a => a.account).sort(), accounts.sort())
    const approval = region.locator('.recovery-preview').getByRole('checkbox')
    assert.equal(await approval.isChecked(), false)
    await page.setViewportSize({ width: 390, height: 844 })
    await page.screenshot({ path: join(artifacts, multiple ? 'german-selected-exact-preview.png' : 'german-single-exact-preview.png'), fullPage: true })
    await approval.check()
    await response('selected_restore', region.getByRole('button', { name: 'Diese erfassten Konten lokal wiederherstellen', exact: true }))
    const restored = await f.observe()
    assert.equal(restored.ciphertext, capturedState.ciphertext)
    assert.equal(restored.primary, capturedState.primary)
    assert.deepEqual(restored.journal, capturedState.journal)
    assert.deepEqual(restored.folders, later.folders)
    assert.deepEqual(restored.independent_effects, later.independent_effects)
    await page.locator('.accounts > .census-actions select').selectOption(f.account)
    await page.getByRole('button', { name: 'Gespeicherte Erfassung öffnen', exact: true }).click()
    const panel = page.locator('.protocol-backlog')
    await panel.getByText('Wiederhergestelltes Konto bleibt gesperrt. Quelle und Freigabe werden nicht wiederhergestellt.', { exact: true }).waitFor()
    assert.equal(await panel.getByRole('button', { name: 'Genaue Protokolländerungen anzeigen', exact: true }).isEnabled(), false)
    assert.equal(await panel.getByRole('button', { name: 'Nur Beobachtungen abgleichen', exact: true }).first().isEnabled(), false)
    assert.equal(await panel.getByRole('checkbox', { name: 'Ich bestätige diese genauen UID-MOVE-Änderungen', exact: true }).count(), 0)
    await page.screenshot({ path: join(artifacts, multiple ? 'german-selected-held.png' : 'german-single-held.png'), fullPage: true })
    await panel.screenshot({ path: join(artifacts, multiple ? 'german-selected-held-detail.png' : 'german-single-held-detail.png') })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
    assert.deepEqual(await page.evaluate(() => [localStorage.length, sessionStorage.length, document.cookie]), [0, 0, ''])
    assert.deepEqual(await page.evaluate(() => window.protocolRecoveryCsp), [])
    assert.equal(await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8'), wire)
    assert.equal((await f.command('status')).installed, false)
    checks.push({ name: multiple ? 'built-populated-selected-recovery' : 'built-populated-one-account-recovery', directory: f.directory, installation, accounts, ciphertext_equal: true, primary_equal: true, independent_effects_equal: true, restored_hold: true })
    await page.close(); page = undefined; await f.close()
  }
  assert.deepEqual(errors, []); assert.deepEqual(external, [])
  await writeFile(join(root, '.audit/imap-backlog-workflow-recovery-browser-results.json'), JSON.stringify({ checks, errors, external, artifacts }, null, 2))
  console.log(`Passed ${checks.length} populated built recovery groups`)
} catch (error) {
  if (page) { await page.screenshot({ path: join(artifacts, `failure-${Date.now()}.png`), fullPage: true }); await writeFile(join(artifacts, `failure-${Date.now()}.txt`), await page.locator('body').innerText()) }
  throw error
} finally { await browser.close(); await f.close() }
