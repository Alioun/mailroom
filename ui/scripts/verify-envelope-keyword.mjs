import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url)).replace(/\/$/, '')
const metadataPath = join(root, '.audit/envelope-keyword-retained.json')
let m = JSON.parse(await readFile(metadataPath, 'utf8'))
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const pause = ms => new Promise(r => setTimeout(r, ms))
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(m.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${m.binary} --exact ${m.test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(m.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`
n${root}
`))
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(m.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
}
identity(); assert.equal(sha(await readFile(m.binary)), m.binary_sha256)
const artifacts = join(root, '.audit/envelope-keyword-browser')
await mkdir(artifacts, { recursive: true })
const checks = [], errors = [], remote = [], replacements = []
let owned
async function restart(epoch) {
  identity(); assert.equal(sha(await readFile(m.binary)), m.binary_sha256)
  const previous = m.pid
  if (owned) { const done = once(owned, 'exit'); owned.kill('SIGTERM'); await done; owned = undefined }
  else {
    process.kill(m.pid, 'SIGTERM'); let stopped = false
    for (let i = 0; i < 300; i++) { try { process.kill(m.pid, 0) } catch (error) { if (error.code !== 'ESRCH') throw error; stopped = true; break }; await pause(20) }
    assert.equal(stopped, true)
  }
  await writeFile(join(m.directory, 'envelope-authority'), epoch)
  owned = spawn(m.binary, ['--exact', m.test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_ENVELOPE_FIXTURE: m.directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 500; i++) {
    assert.equal(owned.exitCode, null)
    if (await fetch(`${m.base}/api/session`).then(r => r.ok, () => false)) {
      m = { ...m, pid: owned.pid }; identity(); await writeFile(metadataPath, JSON.stringify(m, null, 2) + '\n'); replacements.push({ previous, replacement: m.pid, source: epoch || 'removed' }); owned.unref(); return
    }
    await pause(20)
  }
  throw Error('Browser fixture replacement timeout')
}
async function observe() {
  execFileSync(m.binary, ['--exact', 'acquisition::workspace_authority::envelope_tests::envelope_fixture_observer'], { cwd: root, env: { ...process.env, MAILROOM_ENVELOPE_FIXTURE: m.directory }, stdio: 'pipe', encoding: 'utf8' })
  return JSON.parse(await readFile(join(m.directory, 'envelope-observations.json'), 'utf8'))
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 960 }, acceptDownloads: true })
  page.setDefaultTimeout(30000)
  page.on('pageerror', () => errors.push('Page error'))
  page.on('request', r => { if (!r.url().startsWith(m.base)) remote.push('Non-local request') })
  await page.addInitScript(() => { window.envelopeCsp = []; document.addEventListener('securitypolicyviolation', e => window.envelopeCsp.push(e.violatedDirective)) })
  const rules = page.locator('.accounts .rules-proof')
  async function open(german = false) {
    await page.goto(new URL('/advanced.html', m.base).href)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.locator('.accounts > .census-actions select').selectOption(m.account)
    await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
    await rules.getByText('Saved workspace loaded.', { exact: true }).waitFor()
    if (german) await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  }
  async function saveKeyword(keyword, german = false) {
    await saveOverrides(yaml => yaml.replace(/add_keyword:.*$/m, `add_keyword: ${keyword}`), german)
  }
  async function saveOverrides(transform, german = false) {
    const details = rules.getByText(german ? 'Regeldokumente' : 'Rule documents', { exact: true })
    const overrides = rules.getByLabel(german ? 'Anpassung als YAML' : 'Override YAML', { exact: true })
    if (!await overrides.isVisible()) await details.click()
    const current = await overrides.inputValue(), next = transform(current)
    assert.notEqual(next, current)
    await overrides.fill(next)
    const saved = page.waitForResponse(r => r.url().endsWith('/api/accounts/workspace/save'))
    await rules.getByRole('button', { name: german ? 'Regeln und Fixierungen verschlüsselt speichern' : 'Save rules and pins encrypted', exact: true }).click()
    assert.equal((await saved).status(), 200)
    await rules.getByText(german ? 'Regeln und Fixierungen verschlüsselt gespeichert.' : 'Rules and pins saved encrypted.', { exact: true }).waitFor()
  }
  async function prove(german = false) {
    const response = page.waitForResponse(r => r.url().endsWith('/api/accounts/proof/preview'))
    await rules.getByRole('button', { name: german ? 'Gespeicherte Erfassung prüfen' : 'Prove saved census', exact: true }).click()
    const proof = await (await response).json()
    await rules.locator('.decision').first().waitFor()
    return proof
  }
  async function download(proof, german = false) {
    const approval = rules.getByRole('checkbox', { name: german ? 'Ich bestätige diese Prüfung für einen lokalen Sieve-Download' : 'I confirm this proof for a local Sieve download', exact: true })
    assert.equal(await approval.isChecked(), false)
    const button = rules.getByRole('button', { name: german ? 'Bestätigte Sieve-Datei herunterladen' : 'Download confirmed Sieve', exact: true })
    assert.equal(await button.isEnabled(), false)
    await approval.check(); const pending = page.waitForEvent('download'); await button.click()
    const bytes = await readFile(await (await pending).path(), 'utf8'); assert.equal(bytes, proof.proof.artifact.executable)
    await rules.getByText(german ? 'Lokaler Download vorbereitet und Exportvergleich gespeichert. Anwendung beim Anbieter ist ungeprüft.' : 'Local download prepared and export baseline recorded. Provider application is unverified.', { exact: true }).waitFor()
  }
  await open(); await saveKeyword('$browser-route')
  const english = await prove()
  assert.deepEqual(english.proof.decisions[0].keyword_additions, ['$browser-route'])
  await rules.getByText('Recipient source · synthetic-envelope/v1', { exact: true }).waitFor()
  const decisions = rules.locator('.decision')
  await decisions.nth(0).getByText('user.route · Route match', { exact: false }).waitFor()
  await decisions.nth(1).getByText('user.route · Route does not match', { exact: false }).waitFor()
  await decisions.nth(2).getByText('user.route · Route unknown', { exact: false }).waitFor()
  await decisions.nth(3).getByText('Delivery evidence · Unsupported', { exact: true }).waitFor()
  assert.equal(english.proof.decisions[0].observed_flags.length, 5)
  await decisions.nth(0).screenshot({ path: join(artifacts, 'english-route-match.png') })
  await decisions.nth(2).screenshot({ path: join(artifacts, 'english-unknown-hold.png') })
  await download(english)
  async function held(keyword, german = false) {
    await saveOverrides(yaml => yaml.replace('business.test: business', 'business.test: unknown').replace(/add_keyword:.*$/m, `add_keyword: ${keyword}`), german)
    const proof = await prove(german)
    const [first, ...rest] = proof.proof.decisions
    assert.equal(first.delivery_status, 'known'); assert.equal(first.delivery_tags[0].status, 'held_tagged'); assert.deepEqual(first.keyword_additions, [keyword])
    assert.equal(first.review, true); assert.equal(first.mark_read, false); assert.deepEqual(first.copies, []); assert.equal(first.destination.kind, 'inbox'); assert.equal(first.matched_rule, 'system.role_safety')
    for (const d of rest) { assert.equal(d.review, true); assert.deepEqual(d.keyword_additions, []); assert.deepEqual(d.copies, []); assert.equal(d.mark_read, false); assert.notEqual(d.delivery_tags[0].status, 'held_tagged') }
    assert.equal(rest[0].delivery_tags[0].status, 'no_match')
    const executable = proof.proof.artifact.executable
    assert.ok(executable.indexOf(`addflag "${keyword}"`) < executable.indexOf('#@ system.role_safety'))
    await decisions.nth(0).getByText(german ? 'user.route · Routen-Schlagwort ergänzt; zur Prüfung zurückgehalten' : 'user.route · Route keyword added; held for review', { exact: false }).waitFor()
    await decisions.nth(0).getByText(german ? 'Empfängerprüfung. Posteingang und Lesestatus bleiben unverändert. Keine Kopien. Nur ein geprüftes Routen-Schlagwort kann ergänzt werden.' : 'Recipient review. Inbox and read status stay unchanged. No copies. Only a verified route keyword may be added.', { exact: true }).waitFor()
    await decisions.nth(0).getByText(`${german ? 'Geplante zusätzliche Schlagwörter' : 'Planned keyword additions'} · ${keyword}`, { exact: true }).waitFor()
    await decisions.nth(0).screenshot({ path: join(artifacts, `${german ? 'german' : 'english'}-held-tagged.png`) })
    await download(proof, german)
    await saveOverrides(yaml => yaml.replace('business.test: unknown', 'business.test: business'), german)
  }
  await held('$held-route')
  checks.push('Built English held proof: verified envelope match shows tagged-and-held with the keyword, Inbox, unchanged read status and no copies; other evidence stays untagged; exact held download')
  checks.push('Built English saved YAML, parsed route match/nonmatch/unknown/unsupported, observed flags separate from keyword additions, unchecked fresh approval and exact downloaded bytes')
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await saveKeyword('$de-route', true)
  const german = await prove(true)
  await decisions.nth(0).getByText('user.route · Route trifft zu', { exact: false }).waitFor()
  await decisions.nth(1).getByText('user.route · Route trifft nicht zu', { exact: false }).waitFor()
  await decisions.nth(2).getByText('user.route · Route unbekannt', { exact: false }).waitFor()
  await decisions.nth(3).getByText('Zustellbeleg · Nicht unterstützt', { exact: true }).waitFor()
  const evidenceBounds = await decisions.nth(0).locator('.delivery-evidence').evaluate(element => {
    const bounds = element.getBoundingClientRect()
    const row = element.closest('.decision').getBoundingClientRect()
    return { width: bounds.width, rowWidth: row.width, clipped: element.scrollWidth > element.clientWidth }
  })
  assert.equal(evidenceBounds.width, evidenceBounds.rowWidth)
  assert.equal(evidenceBounds.clipped, false)
  await decisions.nth(0).screenshot({ path: join(artifacts, 'german-route-match.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  await download(german, true)
  await held('$de-held-route', true)
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  checks.push('Built narrow German held proof shows translated tagged-and-held status, unchanged hold text and exact held download without overflow')
  await saveKeyword('$de-route', true)
  checks.push('Built narrow German save/prove/download shows translated delivery and route status with independent planned additions and no overflow')
  const stable = await observe()
  await restart('changed-browser-epoch')
  await open(true)
  assert.equal(await rules.getByRole('checkbox').count(), 0)
  const changed = await prove(true)
  assert.ok(changed.proof.decisions.every(d => d.review && d.keyword_additions.length === 0))
  assert.notEqual(changed.proof.artifact.identity, german.proof.artifact.identity)
  assert.equal(await rules.getByRole('checkbox').isChecked(), false)
  await rules.getByText('Gespeicherter Export ist veraltet oder gehört zu einer anderen Engine. Aktuelle Ordner verwenden.', { exact: true }).waitFor()
  await decisions.nth(0).screenshot({ path: join(artifacts, 'german-changed-source-hold.png') })
  assert.equal((await observe()).complete, stable.complete)
  await restart(''); await open(true); const removed = await prove(true)
  assert.ok(removed.proof.artifact.omissions.some(o => o.rule === 'user.route'))
  assert.ok(removed.proof.decisions.every(d => d.review && d.keyword_additions.length === 0))
  await decisions.nth(0).getByText('user.route · Schlagwort ausgelassen', { exact: false }).waitFor()
  assert.equal(await rules.getByRole('checkbox').isChecked(), false)
  await decisions.nth(0).screenshot({ path: join(artifacts, 'german-source-removed.png') })
  assert.equal((await observe()).complete, stable.complete)
  await restart('delivery-fixture-v1'); await open(true); const restored = await prove(true)
  assert.equal(restored.proof.artifact.identity, german.proof.artifact.identity)
  assert.equal(await rules.getByRole('checkbox').isChecked(), false)
  checks.push('Three identity-verified actual process replacements change/remove/restore installed source; incompatible evidence holds and complete omission are visible; old export unusable, no stale approved UI and complete saved-state digest unchanged')
  assert.deepEqual(await page.evaluate(() => ({ local: localStorage.length, session: sessionStorage.length, csp: window.envelopeCsp })), { local: 0, session: 0, csp: [] })
  assert.deepEqual(await page.evaluate(async () => (await indexedDB.databases()).map(d => d.name)), [])
  assert.deepEqual(errors, []); assert.deepEqual(remote, [])
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  checks.push('Empty local/session/IndexedDB storage, no page errors, external requests or CSP violations, no narrow layout overflow')
  await writeFile(join(root, '.audit/envelope-keyword-browser.json'), JSON.stringify({ checks, replacements, screenshots: artifacts, time: new Date().toISOString(), artifact: restored.proof.artifact.identity, independent_sieve: 'blocked', held_tagging: 'tagged_and_held' }, null, 2) + '\n')
  console.log(JSON.stringify({ checks, replacements, screenshots: artifacts }, null, 2))
} finally { await browser.close() }
