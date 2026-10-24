import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url))
const artifacts = join(root, '.audit/durable-browser')
await mkdir(artifacts, { recursive: true })
assert.equal(await fetch('http://127.0.0.1:4173/api/session').then(() => true, () => false), false, 'Stop the fake-only server before this restart fixture')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
const binary = build.trim().split('\n').map(line => JSON.parse(line)).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary, 'Missing compiled test fixture')
const directory = await mkdtemp(join(root, '.audit/durable-restart-'))
let server
let browser
async function start() {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory }, stdio: ['ignore', 'ignore', 'ignore'] })
  const deadline = Date.now() + 10000
  while (Date.now() < deadline) {
    assert.equal(server.exitCode, null, 'Synthetic fixture exited before ready')
    if (await fetch('http://127.0.0.1:4173/api/session').then(r => r.ok, () => false)) return
    await new Promise(resolve => setTimeout(resolve, 50))
  }
  throw new Error('Synthetic restart fixture did not start')
}
async function stop() {
  if (!server) return
  const exited = once(server, 'exit')
  server.kill('SIGTERM')
  await exited
  server = undefined
}
try {
  await start()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, acceptDownloads: true })
  await page.goto('http://127.0.0.1:4173')
  await page.getByRole('button', { name: 'Add synthetic account', exact: true }).click()
  await page.getByText('Account action complete', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Run or resume account census', exact: true }).click()
  await page.getByText('Account action complete', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
  const selected = await page.locator('.accounts > .census-actions select').inputValue()
  const region = page.locator('.accounts .rules-proof')
  const run = region.getByRole('button', { name: 'Prove saved census', exact: true })
  await region.getByText('Rule documents', { exact: true }).click()
  const overrides = region.getByLabel('Override YAML', { exact: true })
  const sensitiveDocument = (await overrides.inputValue()).replace('archive@example.net', 'synthetic-private-restart-target@example.test') + '\n# synthetic-private-restart-document-marker\n'
  await overrides.fill(sensitiveDocument)
  await region.getByRole('button', { name: 'Save rules and pins encrypted', exact: true }).click()
  await region.getByText('Rules and pins saved encrypted.', { exact: true }).waitFor()
  await run.click()
  await region.locator('.decision').first().getByRole('button', { name: 'Pin expected folder', exact: true }).click()
  await region.getByRole('button', { name: 'Save rules and pins encrypted', exact: true }).click()
  await region.getByText('Rules and pins saved encrypted.', { exact: true }).waitFor()
  const downloadProof = page.waitForResponse(r => r.url().endsWith('/api/accounts/proof/preview'))
  await run.click()
  const proof = await (await downloadProof).json()
  await region.getByRole('checkbox', { name: 'I confirm this proof for a local Sieve download', exact: true }).check()
  const event = page.waitForEvent('download')
  await region.getByRole('button', { name: 'Download confirmed Sieve', exact: true }).click()
  const downloaded = await event
  const chunks = []
  for await (const chunk of await downloaded.createReadStream()) chunks.push(chunk)
  assert.deepEqual(Buffer.concat(chunks), Buffer.from(proof.proof.artifact.executable, 'utf8'))
  await region.getByText('Local download prepared and export baseline recorded. Provider application is unverified.', { exact: true }).waitFor()
  const comparison = page.waitForResponse(r => r.url().endsWith('/api/accounts/proof/preview'))
  await region.getByRole('button', { name: 'Compare with last recorded export', exact: true }).click()
  const before = await (await comparison).json()
  const previousToken = await page.evaluate(async () => (await (await fetch('/api/session')).json()).token)
  for (const part of ['config', 'data']) {
    for (const file of await readdir(join(directory, part))) {
      const bytes = await readFile(join(directory, part, file))
      for (const marker of ['synthetic-private-restart-target@example.test', 'synthetic-private-restart-document-marker', 'preset_yaml', 'override_yaml']) assert.equal(bytes.includes(Buffer.from(marker)), false)
    }
  }
  await stop()
  await start()
  const oldSession = await fetch('http://127.0.0.1:4173/api/accounts/status', { headers: { 'X-Mailroom-Token': previousToken } })
  assert.equal(oldSession.status, 403)
  await page.reload()
  await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
  await page.locator('.accounts > .census-actions select').selectOption(selected)
  await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
  await region.getByText('Rule documents', { exact: true }).click()
  assert.equal(await overrides.inputValue(), sensitiveDocument)
  assert.equal(await region.getByLabel('Expected folder', { exact: true }).inputValue(), 'INBOX')
  assert.equal(await region.getByRole('checkbox', { name: 'I confirm this proof for a local Sieve download', exact: true }).count(), 0)
  const regenerated = page.waitForResponse(r => r.url().endsWith('/api/accounts/proof/preview'))
  await run.click()
  const after = await (await regenerated).json()
  assert.equal(after.receipt, before.receipt)
  assert.equal(after.previous.identity, before.previous.identity)
  assert.equal(after.proof.artifact.executable, before.proof.artifact.executable)
  assert.equal(await region.getByRole('checkbox', { name: 'I confirm this proof for a local Sieve download', exact: true }).isChecked(), false)
  await region.locator('.workspace-controls').screenshot({ path: join(artifacts, 'restart-recovered.png') })
  console.log('PASS: actual server process restart with fake file-backed credentials, built UI recovery of exact documents/pins/export baseline, reproducible proof receipt/bytes, old launch-token rejection, fresh unchecked confirmation, and private marker absence from encrypted data and account metadata')
} finally {
  if (browser) await browser.close()
  await stop()
  await rm(directory, { recursive: true, force: true })
}
