import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { readFile, writeFile, mkdir, rm } from 'node:fs/promises'
import { once } from 'node:events'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { chromium } from 'playwright'
const root = fileURLToPath(new URL('../../', import.meta.url)).replace(/\/$/, '')
const metadataPath = join(root, '.audit/imap-provisioning-retained.json')
let metadata = JSON.parse(await readFile(metadataPath, 'utf8'))
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(metadata.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${metadata.binary} --exact ${metadata.test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(metadata.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(metadata.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
}
identity()
assert.equal(createHash('sha256').update(await readFile(metadata.binary)).digest('hex'), metadata.binary_sha256)
const artifacts = join(root, '.audit/imap-provisioning-browser')
await mkdir(artifacts, { recursive: true })
const checks = [], errors = [], remote = []
const pause = ms => new Promise(resolve => setTimeout(resolve, ms))
const observer = 'acquisition::folder_provisioning::tests::folder_fixture_worker'
async function observe(providerOnly = false) {
  execFileSync(metadata.binary, ['--exact', observer], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: metadata.directory, MAILROOM_FOLDERS_ACTION: providerOnly ? 'provider_only' : '' }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(metadata.directory, providerOnly ? 'folder-provider-observations.json' : 'folder-observations.json'), 'utf8'))
}
const creates = snapshot => snapshot.commands.filter(c => c === 'CREATE').length
function preserved(value, initial) {
  assert.equal(value.unrelated, initial.unrelated)
  assert.equal(value.external_unrelated, initial.external_unrelated)
  assert.deepEqual(value.independent_effects, initial.independent_effects)
  assert.ok(Object.values(initial.populated).every(Boolean))
}
async function reached(name) {
  for (let i = 0; i < 1000; i++) {
    if (await readFile(join(metadata.directory, `reached-${name}`)).then(() => true, () => false)) return
    await pause(10)
  }
  throw Error('Deterministic synthetic UI boundary was not reached')
}
let ownedServer
async function restart(removeAuthority = false) {
  identity()
  assert.equal(createHash('sha256').update(await readFile(metadata.binary)).digest('hex'), metadata.binary_sha256)
  if (ownedServer) { const done = once(ownedServer, 'exit'); ownedServer.kill('SIGTERM'); await done; ownedServer = undefined }
  else {
    process.kill(metadata.pid, 'SIGTERM')
    let stopped = false
    for (let i = 0; i < 250; i++) {
      try { process.kill(metadata.pid, 0) }
      catch (error) { if (error.code !== 'ESRCH') throw error; stopped = true; break }
      await pause(20)
    }
    assert.equal(stopped, true, 'Identity-verified retained process must stop before replacement')
  }
  if (removeAuthority) await writeFile(join(metadata.directory, 'workspace-authority'), '')
  const child = spawn(metadata.binary, ['--exact', metadata.test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: metadata.directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 500; i++) {
    assert.equal(child.exitCode, null)
    if (await fetch(`${metadata.base}/api/session`).then(r => r.ok, () => false)) {
      metadata = { ...metadata, pid: child.pid }
      identity()
      await writeFile(metadataPath, JSON.stringify(metadata, null, 2) + '\n')
      ownedServer = child
      child.unref()
      return
    }
    await pause(20)
  }
  throw Error('Synthetic UI restart timed out')
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 950 }, acceptDownloads: true })
  page.setDefaultTimeout(30000)
  page.on('pageerror', () => errors.push('Page error'))
  page.on('request', request => { if (!request.url().startsWith(metadata.base)) remote.push('Non-local request') })
  await page.addInitScript(() => { window.folderCsp = []; document.addEventListener('securitypolicyviolation', e => window.folderCsp.push(e.violatedDirective)) })
  const panel = page.locator('.folder-provisioning')
  const finalStatus = () => page.waitForResponse(r => r.url().endsWith('/api/accounts/folders') && r.request().postDataJSON().command.action === 'status')
  const rules = page.locator('.accounts .rules-proof')
  async function open() {
    await page.goto(metadata.base)
    await page.getByRole('button', { name: 'Load saved accounts', exact: true }).click()
    await page.locator('.accounts > .census-actions select').selectOption(metadata.account)
  }
  async function loadRules() {
    await page.getByRole('button', { name: 'Open saved census', exact: true }).click()
    await rules.getByText('Saved workspace loaded.', { exact: true }).waitFor()
  }
  async function proveDownload(german = false) {
    const response = page.waitForResponse(r => r.url().endsWith('/api/accounts/proof/preview'))
    await rules.getByRole('button', { name: german ? 'Gespeicherte Erfassung prüfen' : 'Prove saved census', exact: true }).click()
    const proof = await (await response).json()
    assert.equal(proof.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')
    const approval = rules.getByRole('checkbox', { name: german ? 'Ich bestätige diese Prüfung für einen lokalen Sieve-Download' : 'I confirm this proof for a local Sieve download', exact: true })
    assert.equal(await approval.isChecked(), false)
    await approval.check()
    const download = page.waitForEvent('download')
    await rules.getByRole('button', { name: german ? 'Bestätigte Sieve-Datei herunterladen' : 'Download confirmed Sieve', exact: true }).click()
    assert.equal(await readFile(await (await download).path(), 'utf8'), proof.proof.artifact.executable)
    return proof.proof.artifact
  }
  async function review(german = false) {
    const response = page.waitForResponse(r => r.url().endsWith('/api/accounts/folders') && r.request().postDataJSON().command.action === 'preview')
    await panel.getByRole('button', { name: german ? 'Exakte Ordner prüfen' : 'Review exact folders', exact: true }).click()
    const preview = await (await response).json()
    assert.equal(await panel.getByRole('checkbox').isChecked(), false)
    assert.ok(preview.operation.plan.entries.length > 2)
    assert.ok(preview.operation.plan.entries.some(e => e.role === 'ancestor' && !e.existing))
    assert.ok(preview.operation.plan.entries.filter(e => e.role === 'leaf').length > 1)
    await panel.getByRole('button', { name: german ? 'Private Ordnernamen anzeigen' : 'Show private folder names', exact: true }).click()
    await panel.getByText(german ? 'Explizite synthetische Empfängerbelege installiert. Dieser Testbeleg schafft kein Anbietervertrauen.' : 'Explicit synthetic recipient evidence installed. This fixture fact grants no provider trust.', { exact: true }).waitFor()
    return preview
  }
  async function changeRoots(german, suffix) {
    await rules.getByRole('button', { name: german ? 'Gespeicherte Regeln und Fixierungen laden' : 'Reload saved rules and pins', exact: true }).click()
    await rules.getByText(german ? 'Gespeicherter Arbeitsstand geladen.' : 'Saved workspace loaded.', { exact: true }).waitFor()
    const details = rules.getByText(german ? 'Regeldokumente' : 'Rule documents', { exact: true })
    const overrides = rules.getByLabel(german ? 'Anpassung als YAML' : 'Override YAML', { exact: true })
    if (!await overrides.isVisible()) await details.click()
    const original = await overrides.inputValue()
    await overrides.fill(original.replace(/^  personal_root:.*$/m, `  personal_root: ${JSON.stringify(`Grüße ${suffix}`)}`).replace(/^  business_root:.*$/m, `  business_root: ${JSON.stringify(`Büro ${suffix}`)}`))
    await rules.getByRole('button', { name: german ? 'Regeln und Fixierungen verschlüsselt speichern' : 'Save rules and pins encrypted', exact: true }).click()
    await rules.getByText(german ? 'Regeln und Fixierungen verschlüsselt gespeichert.' : 'Rules and pins saved encrypted.', { exact: true }).waitFor()
  }
  await rm(join(metadata.directory, 'folder-boundary'), { force: true })
  const run = String(Date.now())
  await open(); await loadRules()
  await changeRoots(false, `English ${run}`)
  const artifact = await proveDownload()
  await panel.getByRole('button', { name: 'Open folder history', exact: true }).click()
  const english = await review()
  assert.equal(english.operation.artifact, artifact.identity)
  assert.deepEqual(english.operation.plan.logical_leaves, artifact.folders)
  const englishInitial = await observe()
  const confirm = panel.getByRole('button', { name: 'Create these synthetic folders', exact: true })
  assert.equal(await confirm.isEnabled(), false)
  await panel.screenshot({ path: join(artifacts, 'english-unchecked-create-review.png') })
  await panel.getByRole('checkbox').check()
  await panel.screenshot({ path: join(artifacts, 'english-exact-create-review.png') })
  const englishFinished = finalStatus()
  await confirm.click()
  await englishFinished
  await panel.locator('article').last().getByRole('heading', { name: 'Visible postconditions observed', exact: true }).waitFor()
  const englishDone = await observe()
  preserved(englishDone, englishInitial)
  assert.equal(creates(englishDone) - creates(englishInitial), english.operation.plan.entries.filter(e => !e.existing).length)
  assert.deepEqual(englishDone.journal.history.at(-1).steps.map(s => s.name), english.operation.plan.entries.filter(e => !e.existing).map(e => e.name))
  assert.ok(englishDone.journal.history.at(-1).steps.every(s => s.acknowledgment && s.outcome === 'observed' && s.observed))
  await panel.screenshot({ path: join(artifacts, 'english-create-completed.png') })
  checks.push('Built English controls prove saved rules, confirm exact download bytes, review matching artifact destinations with unchecked approval and perform ordered real TLS CREATE with separate intent/ack/LIST evidence')
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await changeRoots(true, `Neu ${run}`)
  const germanArtifact = await proveDownload(true)
  const german = await review(true)
  assert.equal(german.operation.artifact, germanArtifact.identity)
  await panel.screenshot({ path: join(artifacts, 'german-unchecked-create-review.png') })
  await panel.getByRole('checkbox').check()
  await panel.screenshot({ path: join(artifacts, 'german-exact-create-review.png') })
  const germanInitial = await observe()
  const germanFinished = finalStatus()
  await panel.getByRole('button', { name: 'Diese synthetischen Ordner anlegen', exact: true }).click()
  await germanFinished
  await panel.locator('article').last().getByRole('heading', { name: 'Sichtbare Nachbedingungen beobachtet', exact: true }).waitFor()
  const germanDone = await observe(); preserved(germanDone, germanInitial)
  assert.deepEqual(germanDone.journal.history.at(-1).steps.map(s => s.name), german.operation.plan.entries.filter(e => !e.existing).map(e => e.name))
  assert.ok(germanDone.journal.history.at(-1).steps.every(s => s.acknowledgment && s.outcome === 'observed' && s.observed))
  assert.equal(creates(germanDone) - creates(germanInitial), german.operation.plan.entries.filter(e => !e.existing).length)
  await panel.screenshot({ path: join(artifacts, 'german-create-completed.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  checks.push('Built narrow German controls save new invented roots, prove/download matching bytes and complete genuine multi-folder CREATE without layout overflow')
  await changeRoots(true, `Angehalten ${run}`)
  await proveDownload(true)
  const cancelled = await review(true)
  const cancelInitial = await observe()
  for (const name of ['reached-folder_observed', 'continue-folder_observed']) await rm(join(metadata.directory, name), { force: true })
  await writeFile(join(metadata.directory, 'folder-boundary'), 'folder_observed')
  await panel.getByRole('checkbox').check()
  await panel.getByRole('button', { name: 'Diese synthetischen Ordner anlegen', exact: true }).click()
  await reached('folder_observed')
  assert.equal(creates(await observe(true)), creates(cancelInitial) + 1)
  const progress = JSON.parse(await readFile(join(metadata.directory, 'folder-progress.json'), 'utf8'))
  assert.equal(progress.unrelated, cancelInitial.unrelated)
  const cancelFinished = finalStatus()
  await panel.getByRole('button', { name: 'Weitere Ordnerbefehle stoppen', exact: true }).click()
  await writeFile(join(metadata.directory, 'continue-folder_observed'), 'continue')
  await cancelFinished
  await panel.locator('article').last().getByRole('heading', { name: 'Gestoppt', exact: true }).waitFor()
  const stopped = await observe(); preserved(stopped, cancelInitial)
  assert.equal(creates(stopped), creates(cancelInitial) + 1)
  assert.equal(stopped.journal.history.at(-1).steps[0].outcome, 'observed')
  await panel.screenshot({ path: join(artifacts, 'german-cancelled-after-create.png') })
  await restart(); await open()
  await panel.getByRole('button', { name: 'Open folder history', exact: true }).click()
  await panel.locator('article').last().getByRole('heading', { name: 'Stopped', exact: true }).waitFor()
  const reopened = await observe(); preserved(reopened, cancelInitial)
  assert.equal(reopened.topology, stopped.topology); assert.equal(creates(reopened), creates(stopped))
  await panel.locator('article').last().getByRole('button', { name: 'Reconcile folders without creating', exact: true }).click()
  await panel.locator('article').last().getByRole('heading', { name: 'Fresh exact review required', exact: true }).waitFor()
  const reconciled = await observe(); preserved(reconciled, cancelInitial)
  assert.equal(reconciled.topology, stopped.topology); assert.equal(creates(reconciled), creates(stopped))
  const renewed = await review()
  assert.equal(await panel.getByRole('checkbox').isChecked(), false)
  assert.notEqual(renewed.receipt, cancelled.receipt)
  checks.push('Actual built UI Cancel after first observed CREATE stops before next effect; process replacement retains folders and all unrelated state; explicit UI reconciliation issues no CREATE and remaining work requires fresh unchecked approval')
  await restart(true); await open()
  await panel.getByRole('button', { name: 'Open folder history', exact: true }).click()
  await panel.getByRole('button', { name: 'Review exact folders', exact: true }).click()
  await panel.getByText('The saved artifact emits no filing destinations. No CREATE is authorized by this review.', { exact: true }).waitFor()
  const approveEmpty = panel.getByRole('checkbox')
  assert.equal(await approveEmpty.isChecked(), false)
  const record = panel.getByRole('button', { name: 'Record the empty folder plan', exact: true })
  assert.equal(await record.isEnabled(), false)
  await panel.getByRole('button', { name: 'Show private folder names', exact: true }).click()
  await approveEmpty.check()
  await panel.screenshot({ path: join(artifacts, 'english-empty-artifact-review.png') })
  const beforeEmpty = await observe()
  const emptyFinished = finalStatus()
  await record.click()
  await emptyFinished
  await panel.locator('article').last().getByRole('heading', { name: 'Visible postconditions observed', exact: true }).waitFor()
  assert.equal(creates(await observe()), creates(beforeEmpty))
  await page.getByRole('radio', { name: 'Deutsch', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await panel.getByRole('button', { name: 'Exakte Ordner prüfen', exact: true }).click()
  await panel.getByText('Das gespeicherte Artefakt enthält keine Ablageziele. Diese Prüfung erlaubt keinen CREATE-Befehl.', { exact: true }).waitFor()
  assert.equal(await panel.getByRole('checkbox').isChecked(), false)
  await panel.screenshot({ path: join(artifacts, 'german-empty-artifact-review.png') })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  checks.push('Removing installation restores conservative empty-plan behavior in English and narrow German; stored artifacts never reinstall synthetic authority and empty confirmation adds zero CREATE')
  assert.equal(await page.evaluate(() => localStorage.length + sessionStorage.length), 0)
  assert.deepEqual(await page.evaluate(() => window.folderCsp), [])
  assert.deepEqual(errors, []); assert.deepEqual(remote, [])
  await page.reload()
  assert.equal(await panel.locator('article').count(), 0)
  checks.push('CSP, no browser persistence, no page errors, local-only requests and no cached private history after reload pass with repository Playwright Chromium')
  await writeFile(join(root, '.audit/imap-provisioning-browser.json'), JSON.stringify({ checks, browser: 'Repository Playwright Chromium, headless', limitations: 'Explicit synthetic installed evidence only. Actual providers, native confirmation, mapping and delivery remain unverified.' }, null, 2) + '\n')
  console.log(`PASS ${checks.length} built-browser groups including saved-workspace multi-folder CREATE, deterministic cancellation, process replacement and conservative uninstalled behavior.`)
} finally { await browser.close() }
