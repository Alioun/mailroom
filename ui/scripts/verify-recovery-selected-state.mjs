import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { readFile, writeFile, mkdir, stat, rename } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url))
const retainedPath = join(root, '.audit/recovery-selected-retained.json')
const retained = JSON.parse(await readFile(retainedPath, 'utf8'))
await writeFile(join(root, '.audit/recovery-selected-browser-input.json'), JSON.stringify(retained, null, 2) + '\n')
const artifacts = join(root, '.audit/recovery-selected-browser'); await mkdir(artifacts, { recursive: true })
let child, token
async function verifiedStop(signal = 'SIGTERM') {
  assert.equal(execFileSync('ps', ['-p', String(retained.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${retained.binary} --exact server::accounts::durable_browser_server_fixture --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(retained.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root.replace(/\/$/, '')}\n`))
  assert.equal(createHash('sha256').update(await readFile(retained.binary)).digest('hex'), retained.binary_sha256)
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(retained.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  if (child) { const done = once(child, 'exit'); child.kill(signal); await done; child = undefined }
  else process.kill(retained.pid, signal)
  for (let i = 0; i < 200; i++) { if (!await fetch(`${retained.base}/api/session`).then(() => true, () => false)) return; await new Promise(r => setTimeout(r, 25)) }
  throw Error('Verified fake process did not stop')
}
async function start(boundary = '') {
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: retained.directory, MAILROOM_RECOVERY_INSTALLATION: retained.installation, MAILROOM_RECOVERY_LEDGER: retained.ledger }
  delete env.MAILROOM_LIFECYCLE_BOUNDARY
  if (boundary) env.MAILROOM_LIFECYCLE_BOUNDARY = boundary
  child = spawn(retained.binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env, stdio: 'ignore', detached: true })
  retained.pid = child.pid
  for (let i = 0; i < 300; i++) {
    assert.equal(child.exitCode, null)
    const response = await fetch(`${retained.base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; await writeFile(retainedPath, JSON.stringify(retained, null, 2) + '\n'); await writeFile(join(root, '.audit/recovery-selected-browser-runtime.json'), JSON.stringify(retained, null, 2) + '\n'); return }
    await new Promise(r => setTimeout(r, 40))
  }
  throw Error('Replacement synthetic startup timeout')
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 950 } })
  page.setDefaultTimeout(30000)
  const errors = [], urls = []
  page.on('pageerror', e => errors.push(e.message)); page.on('request', r => urls.push(r.url()))
  await page.addInitScript(() => { window.recoveryCsp = []; document.addEventListener('securitypolicyviolation', e => window.recoveryCsp.push(e.violatedDirective)) })
  await page.goto(retained.base)
  await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
  const region = page.locator('.accounts .recovery')
  await region.locator(':scope > summary').click()
  assert.equal(await region.locator('.ledger-inclusion').isChecked(), false)
  assert.equal(await region.locator('.ledger-restoration').isChecked(), false)
  await region.locator('fieldset input').first().waitFor()
  for (const checkbox of await region.locator('fieldset input').all()) assert.equal(await checkbox.isChecked(), false)
  assert.equal(await region.getByRole('button', { name: 'Preview exact capture', exact: true }).isEnabled(), false)
  const chosen = []
  for (const account of retained.accounts) {
    const box = region.locator(`fieldset input[value="${account}"]`)
    await box.check(); chosen.push(await box.inputValue())
  }
  await region.locator('.ledger-inclusion').check()
  const response = action => page.waitForResponse(r => r.url().endsWith('/api/accounts/recovery') && r.request().postDataJSON().action === action)
  let pending = response('selected_capture_preview')
  await region.getByRole('button', { name: 'Preview exact capture', exact: true }).click()
  let captureOffer = await (await pending).json()
  assert.deepEqual(captureOffer.preview.accounts.map(a => a.account).sort(), chosen.sort())
  assert.equal(captureOffer.preview.ledger.kind, 'included')
  for (const a of captureOffer.preview.accounts) for (const resource of a.resources) await region.locator('.recovery-preview').getByText(`${resource.resource} · ${resource.bytes} B · ${resource.sha256}`, { exact: true }).waitFor()
  const exact = region.locator('.recovery-preview')
  assert.equal(await exact.getByRole('checkbox').isChecked(), false)
  assert.equal(await region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }).isEnabled(), false)
  assert.equal(await exact.locator('pre').innerText(), captureOffer.confirmation.display)
  await region.locator('.ledger-inclusion').uncheck()
  await exact.waitFor({ state: 'hidden' })
  await region.locator('.ledger-inclusion').check()
  pending = response('selected_capture_preview')
  await region.getByRole('button', { name: 'Preview exact capture', exact: true }).click()
  captureOffer = await (await pending).json()
  await exact.getByRole('checkbox').check()
  await region.screenshot({ path: join(artifacts, 'english-selected-ledger-capture.png') })
  pending = response('selected_capture')
  await region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }).click()
  const captured = await (await pending).json(); assert.equal(captured.captured, true)
  await region.getByRole('status').waitFor()
  await verifiedStop()
  retained.installation = join(retained.directory, `selected-browser-fresh-installation-${Date.now()}`); await mkdir(retained.installation)
  retained.ledger = join(retained.installation, 'ledger')
  retained.bundle = captured.bundle
  await rename(join(retained.directory, 'reached-selected_restore_first_account'), join(retained.directory, 'reached-selected_restore_first_account-before-browser-preserved')).catch(error => { if (error.code !== 'ENOENT') throw error })
  await start('selected_restore_first_account')
  const { unlink } = await import('node:fs/promises')
  await unlink(join(retained.ledger, 'addresses.yaml'))
  await page.reload()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await region.locator(':scope > summary').click()
  await region.locator('select').selectOption(captured.bundle)
  assert.equal(await region.locator('.ledger-restoration').isChecked(), false)
  await region.locator('.ledger-restoration').check()
  pending = response('selected_restore_preview')
  await region.getByRole('button', { name: 'Authentifizieren und Wiederherstellung prüfen', exact: true }).click()
  const preview = await (await pending).json(); assert.equal(preview.authenticated, true)
  assert.deepEqual(preview.preview.manifest.accounts.map(a => a.account).sort(), chosen.sort())
  assert.equal(preview.preview.ledger_target.kind, 'absent')
  assert.equal(await exact.getByRole('checkbox').isChecked(), false)
  assert.equal(await region.getByRole('button', { name: 'Diese erfassten Konten lokal wiederherstellen', exact: true }).isEnabled(), false)
  await exact.getByRole('checkbox').check()
  await region.screenshot({ path: join(artifacts, 'german-selected-authenticated-restore.png') })
  await region.getByRole('button', { name: 'Diese erfassten Konten lokal wiederherstellen', exact: true }).click()
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(retained.directory, 'reached-selected_restore_first_account')).then(() => true, () => false)) break
    if (i === 999) throw Error('Built selected UI restore did not reach durable boundary')
    await new Promise(r => setTimeout(r, 20))
  }
  const oldToken = token
  await verifiedStop('SIGKILL'); await start()
  assert.equal((await fetch(`${retained.base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  await page.reload()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await region.locator(':scope > summary').click()
  for (const account of chosen) assert.ok((await region.locator('.selected-pending').innerText()).includes(account))
  await region.screenshot({ path: join(artifacts, 'german-selected-interrupted.png') })
  pending = response('selected_resume')
  await region.getByRole('button', { name: 'Authentifizierte lokale Wiederherstellung fortsetzen', exact: true }).click()
  assert.equal((await pending).status(), 200)
  await page.waitForFunction(() => document.querySelectorAll('.accounts .recovery .restored').length === 2 && document.querySelectorAll('.accounts .recovery .selected-pending').length === 0)
  assert.equal(await region.locator('.restored').count(), 2)
  assert.equal(await region.locator('.selected-pending').count(), 0)
  await page.getByRole('button', { name: 'Gespeicherte Konten laden', exact: true }).click()
  const chooser = page.locator('.accounts > .census-actions select')
  assert.equal(await chooser.locator('option').count(), 3)
  for (const account of chosen) await chooser.selectOption(account)
  for (const item of await region.locator('.restored').all()) assert.ok((await item.innerText()).includes('Frische Anbieterbelege und Abgleich erforderlich.'))
  await page.setViewportSize({ width: 390, height: 900 })
  await region.screenshot({ path: join(artifacts, 'german-selected-authenticated-readonly.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
  assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage), csp: window.recoveryCsp })), { local: [], session: [], csp: [] })
  assert.equal(await region.locator('input[type="password"]').count(), 0)
  assert.ok(urls.every(url => url.startsWith(retained.base)))
  assert.deepEqual(errors, [])
  await writeFile(retainedPath, JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/recovery-selected-browser-output.json'), JSON.stringify(retained, null, 2) + '\n')
  child.unref(); child = undefined
  console.log('PASS built English selected capture, optional unchecked ledger/confirmation, preview invalidation, German two-account authenticated restore, actual first-account SIGKILL/restart/UI resume, accurate account list, read-only reconciliation hold and empty browser storage')
} finally { await browser.close(); if (child) { child.unref(); child = undefined } }
