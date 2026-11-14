import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { readFile, writeFile, mkdir, stat, rename } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url))
const retainedPath = join(root, '.audit/recovery-retained.json')
const retained = JSON.parse(await readFile(retainedPath, 'utf8'))
await writeFile(join(root, '.audit/recovery-browser-input.json'), JSON.stringify(retained, null, 2) + '\n')
const artifacts = join(root, '.audit/recovery-browser'); await mkdir(artifacts, { recursive: true })
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
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: retained.directory, MAILROOM_RECOVERY_INSTALLATION: retained.installation }
  delete env.MAILROOM_LIFECYCLE_BOUNDARY
  if (boundary) env.MAILROOM_LIFECYCLE_BOUNDARY = boundary
  child = spawn(retained.binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env, stdio: 'ignore', detached: true })
  retained.pid = child.pid
  for (let i = 0; i < 300; i++) {
    assert.equal(child.exitCode, null)
    const response = await fetch(`${retained.base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; await writeFile(retainedPath, JSON.stringify(retained, null, 2) + '\n'); await writeFile(join(root, '.audit/recovery-browser-runtime.json'), JSON.stringify(retained, null, 2) + '\n'); return }
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
  await page.goto(new URL('/advanced.html', retained.base).href)
  const region = page.locator('.accounts .recovery')
  await region.locator(':scope > summary').click()
  await region.locator('.restored').getByText('Local bundle authenticated', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Add synthetic account', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('.accounts .scan-state')?.textContent === 'Account action complete' && !!document.querySelector('.accounts > .census-actions select')?.value && ![...document.querySelectorAll('.accounts button')].find(b => b.textContent === 'Add synthetic account')?.disabled)
  const account = await page.locator('.accounts > .census-actions select').inputValue()
  const response = action => page.waitForResponse(r => r.url().endsWith('/api/accounts/recovery') && r.request().postDataJSON().action === action)
  await region.locator(`fieldset input[value="${account}"]`).check()
  assert.equal(await region.locator('.ledger-inclusion').isChecked(), false)
  let pending = response('selected_capture_preview')
  await region.getByRole('button', { name: 'Preview exact capture', exact: true }).click()
  const captureOffer = await (await pending).json()
  for (const resource of captureOffer.preview.accounts[0].resources) await region.locator('.recovery-preview').getByText(`${resource.resource} · ${resource.bytes} B · ${resource.sha256}`, { exact: true }).waitFor()
  assert.equal(await region.locator('.recovery-preview').getByRole('checkbox').isChecked(), false)
  assert.equal(await region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }).isEnabled(), false)
  assert.equal(await region.locator('pre').innerText(), captureOffer.confirmation.display)
  await region.locator('.recovery-preview').getByRole('checkbox').check()
  await region.screenshot({ path: join(artifacts, 'english-exact-capture.png') })
  pending = response('selected_capture')
  await region.getByRole('button', { name: 'Create this synthetic encrypted bundle', exact: true }).click()
  const captured = await (await pending).json(); assert.equal(captured.captured, true)
  await region.getByRole('status').waitFor()
  await verifiedStop()
  retained.installation = join(retained.directory, 'browser-fresh-installation'); await mkdir(retained.installation)
  retained.account = account; retained.bundle = captured.bundle
  await rename(join(retained.directory, 'reached-selected_restore_metadata'), join(retained.directory, 'reached-selected_restore_metadata-before-browser-preserved')).catch(error => { if (error.code !== 'ENOENT') throw error })
  await start('selected_restore_metadata')
  await page.reload()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await region.locator(':scope > summary').click()
  await region.locator('select').selectOption(captured.bundle)
  pending = response('selected_restore_preview')
  await region.getByRole('button', { name: 'Authentifizieren und Wiederherstellung prüfen', exact: true }).click()
  const preview = await (await pending).json(); assert.equal(preview.authenticated, true)
  assert.equal(preview.preview.manifest.accounts[0].account, account)
  assert.equal(await region.locator('.recovery-preview').getByRole('checkbox').isChecked(), false)
  assert.equal(await region.getByRole('button', { name: 'Diese erfassten Konten lokal wiederherstellen', exact: true }).isEnabled(), false)
  await region.locator('.recovery-preview').getByRole('checkbox').check()
  await region.screenshot({ path: join(artifacts, 'german-exact-authenticated-restore.png') })
  await region.getByRole('button', { name: 'Diese erfassten Konten lokal wiederherstellen', exact: true }).click()
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(retained.directory, 'reached-selected_restore_metadata')).then(() => true, () => false)) break
    if (i === 999) throw Error('Built UI restore did not reach durable boundary')
    await new Promise(r => setTimeout(r, 20))
  }
  const oldToken = token
  await verifiedStop('SIGKILL'); await start()
  assert.equal((await fetch(`${retained.base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  await page.reload()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await region.locator(':scope > summary').click()
  await region.locator('.selected-pending').getByText(`Lokale Wiederherstellung unterbrochen · ${account}`, { exact: true }).waitFor()
  await region.screenshot({ path: join(artifacts, 'german-interrupted-after-replacement.png') })
  pending = response('selected_resume')
  await region.getByRole('button', { name: 'Authentifizierte lokale Wiederherstellung fortsetzen', exact: true }).click()
  assert.equal((await pending).status(), 200)
  await page.waitForFunction(() => document.querySelectorAll('.accounts .recovery .selected-pending').length === 0 && document.querySelectorAll('.accounts .recovery .restored').length === 1)
  await region.locator('.restored').getByText('Lokale Sicherung authentifiziert', { exact: true }).waitFor()
  assert.ok((await region.locator('.restored').innerText()).includes('Frische Anbieterbelege und Abgleich erforderlich.'))
  await page.setViewportSize({ width: 390, height: 900 })
  await region.screenshot({ path: join(artifacts, 'german-authenticated-readonly.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
  assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage), csp: window.recoveryCsp })), { local: [], session: [], csp: [] })
  assert.equal(await region.locator('input[type="password"]').count(), 0)
  assert.ok(urls.every(url => url.startsWith(retained.base)))
  assert.deepEqual(errors, [])
  retained.account = account; retained.bundle = captured.bundle
  await writeFile(retainedPath, JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/recovery-browser-output.json'), JSON.stringify(retained, null, 2) + '\n')
  child.unref(); child = undefined
  console.log('PASS built exact English capture, German authenticated restore, unchecked single-use controls, actual SIGKILL/replacement and UI recovery, required reconciliation, empty browser storage and narrow layout')
} finally { await browser.close(); if (child) { child.unref(); child = undefined } }
