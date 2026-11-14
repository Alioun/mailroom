import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { readFile, writeFile, stat, unlink } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url))
const path = join(root, '.audit/account-lifecycle-retained.json')
const retained = JSON.parse(await readFile(path, 'utf8'))
let child
async function verifyAndStop(signal = 'SIGTERM') {
  assert.equal(execFileSync('ps', ['-p', String(retained.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${retained.binary} --exact server::accounts::durable_browser_server_fixture --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(retained.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root.replace(/\/$/, '')}\n`))
  assert.equal(createHash('sha256').update(await readFile(retained.binary)).digest('hex'), retained.binary_sha256)
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(retained.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  if (child) { const exited = once(child, 'exit'); child.kill(signal); await exited; child = undefined }
  else process.kill(retained.pid, signal)
  for (let i = 0; i < 200; i++) {
    if (!await fetch(`${retained.base}/api/session`).then(() => true, () => false)) return
    await new Promise(r => setTimeout(r, 25))
  }
  throw Error('Verified synthetic process did not stop')
}
async function start(boundary) {
  const env = { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: retained.directory }
  delete env.MAILROOM_LIFECYCLE_BOUNDARY
  if (boundary) env.MAILROOM_LIFECYCLE_BOUNDARY = boundary
  child = spawn(retained.binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env, stdio: 'ignore', detached: true })
  retained.pid = child.pid
  await writeFile(path, JSON.stringify(retained, null, 2) + '\n')
  for (let i = 0; i < 200; i++) {
    assert.equal(child.exitCode, null)
    const response = await fetch(`${retained.base}/api/session`).catch(() => null)
    if (response?.ok) return (await response.json()).token
    await new Promise(r => setTimeout(r, 25))
  }
  throw Error('Replacement synthetic process did not start')
}
async function interrupt(phase, action) {
  await verifyAndStop()
  await unlink(join(retained.directory, `reached-${phase}`)).catch(e => { if (e.code !== 'ENOENT') throw e })
  const token = await start(phase)
  const pending = fetch(`${retained.base}/api/accounts/lifecycle`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(action) }).catch(() => null)
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(retained.directory, `reached-${phase}`)).then(() => true, () => false)) break
    assert.equal(child.exitCode, null)
    await new Promise(r => setTimeout(r, 20))
  }
  assert.equal(await stat(join(retained.directory, `reached-${phase}`)).then(() => true, () => false), true)
  await verifyAndStop('SIGKILL'); await pending; await start()
}
const browser = await chromium.launch({ headless: true })
try {
  const account = retained.account
  const before = JSON.parse(await readFile(join(retained.directory, 'config', `${account}.json`), 'utf8'))
  await interrupt('replace_password', { action: 'replace_password', account, password: 'synthetic-ui-recovery-only' })
  const page = await browser.newPage({ viewport: { width: 1280, height: 950 } })
  await page.goto(new URL('/advanced.html', retained.base).href)
  await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
  await page.locator('.accounts > .census-actions select').selectOption(account)
  const region = page.locator('.accounts .lifecycle')
  await region.locator(':scope > summary').click()
  await region.getByRole('button', { name: 'Refresh lifecycle recovery status', exact: true }).click()
  await region.getByText(`${account} · Interrupted password replacement`, { exact: true }).waitFor()
  await region.screenshot({ path: join(root, '.audit/account-lifecycle-browser/english-recovery-pending.png') })
  let response = page.waitForResponse(r => r.url().endsWith('/api/accounts/lifecycle') && r.request().postDataJSON().action === 'recover')
  await region.getByRole('button', { name: `Resume local lifecycle recovery · ${account}`, exact: true }).click()
  assert.equal((await response).status(), 200)
  await region.getByText('Local lifecycle change saved. Account previews were cleared.', { exact: true }).waitFor()
  assert.equal(JSON.parse(await readFile(join(retained.directory, 'config', `${account}.json`), 'utf8')).password_ref, before.password_ref)
  await interrupt('metadata', { action: 'rotate', account, confirmed: true })
  await page.reload()
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.getByRole('button', { name: 'Gespeicherte Konten laden', exact: true }).click()
  await page.locator('.accounts > .census-actions select').selectOption(account)
  await region.locator(':scope > summary').click()
  await region.getByRole('button', { name: 'Wiederherstellungsstatus aktualisieren', exact: true }).click()
  await region.getByText(`${account} · Schlüsselwechsel ausstehend`, { exact: true }).waitFor()
  await region.screenshot({ path: join(root, '.audit/account-lifecycle-browser/german-recovery-pending.png') })
  response = page.waitForResponse(r => r.url().endsWith('/api/accounts/lifecycle') && r.request().postDataJSON().action === 'recover')
  await region.getByRole('button', { name: `Lokale Kontowiederherstellung fortsetzen · ${account}`, exact: true }).click()
  assert.equal((await response).status(), 200)
  await region.getByText('Lokale Änderung gespeichert. Kontovorschauen wurden geleert.', { exact: true }).waitFor()
  const after = JSON.parse(await readFile(join(retained.directory, 'config', `${account}.json`), 'utf8'))
  assert.equal(after.generation, before.generation + 1)
  assert.equal(await stat(join(retained.directory, 'fake-credentials', before.local_secret_ref)).then(() => true, () => false), false)
  child.unref(); child = undefined
  console.log('PASS built English password rollback recovery and German mixed-generation rotation recovery after exact-process SIGKILL and durable credential-backend reconstruction')
} finally { await browser.close(); if (child) { child.unref(); child = undefined } }
