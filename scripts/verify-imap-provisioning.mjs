import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the identity-verified retained fake before builds')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const binary_sha256 = sha(await readFile(binary))
const test = 'server::accounts::folder_provisioning_server_fixture'
const observer = 'acquisition::folder_provisioning::tests::folder_fixture_worker'
const pause = ms => new Promise(r => setTimeout(r, ms))
let server, directory, token, account
const checks = [], crashes = []
async function start(fresh = false, installed = false) {
  if (fresh) directory = await mkdtemp(join(root, '.audit/imap-provisioning-fixture-'))
  if (fresh && installed) await writeFile(join(directory, 'workspace-authority'), 'saved-workspace-fixture-v1')
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 400; i++) {
    assert.equal(server.exitCode, null, 'Synthetic fixture exited during startup')
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; account = (await observe()).account; return }
    await pause(20)
  }
  throw Error('Synthetic folder fixture startup timed out')
}
function identity(child, expectedTest) {
  assert.equal(execFileSync('ps', ['-p', String(child.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${expectedTest} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(child.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  if (expectedTest === test) assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(child.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
}
async function stop(signal = 'SIGTERM') { identity(server, test); assert.equal(sha(await readFile(binary)), binary_sha256); const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function observe(action = '') {
  execFileSync(binary, ['--exact', observer], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: directory, MAILROOM_FOLDERS_ACTION: action }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'folder-observations.json'), 'utf8'))
}
async function providerSnapshot() {
  execFileSync(binary, ['--exact', observer], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: directory, MAILROOM_FOLDERS_ACTION: 'provider_only' }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'folder-provider-observations.json'), 'utf8'))
}
async function post(path, body, expected = 200, headers = {}) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...headers }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `Unexpected status for ${path}`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
const command = (action, fields = {}, expected = 200) => post('folders', { account, command: { action, ...fields } }, expected)
async function terminal(expected = 'complete') {
  const deadline = Date.now() + 70000
  while (Date.now() < deadline) {
    const value = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (value.job?.progress.state !== 'running') { assert.equal(value.job?.progress.state, expected); return }
    await pause(10)
  }
  throw Error('Synthetic folder operation did not finish')
}
const creates = snapshot => snapshot.commands.filter(c => c === 'CREATE').length
async function reached(name) {
  for (let i = 0; i < 1500; i++) { if (await readFile(join(directory, `reached-${name}`)).then(() => true, () => false)) return; await pause(10) }
  throw Error(`Synthetic crash boundary not reached ${name}`)
}
async function savedWorkspace() {
  const ledger = await post('ledger', { account, command: { action: 'open' } })
  if (!ledger.snapshot.document.entries.length) {
    const batch = { content_version: ledger.snapshot.content_version, revision: ledger.revision, enabled: true, entries: [{ address: 'kept-alias@example.test', service: 'Invented retained service', folder: 'Aufbewahrt', first_seen: 'synthetic fixture date', status: 'active', note: 'Synthetic preservation evidence', category: 'orders', invoice: false, association: { sender_domains: ['sender.example'], approved_evidence: 'Explicit synthetic association' }, owner_field: { preserve: ['one', 2] } }] }
    const review = await post('ledger', { account, command: { action: 'preview', batch } })
    assert.equal(review.proof.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')
    await post('ledger', { account, command: { action: 'confirm', batch, receipt: review.receipt, confirmed: true } })
  }
  const proposals = await post('proposals', { account, command: { action: 'open' } })
  assert.equal(proposals.current.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')
  for (const candidate of proposals.candidates) assert.equal(candidate.impact.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')

  const view = await post('workspace/open', { account })
  const input = structuredClone(view.workspace.input)
  input.baseline = 'current'
  assert.match(input.documents.override_yaml, /^  personal_root:/m)
  input.documents.override_yaml = input.documents.override_yaml
    .replace(/^  personal_root:.*$/m, `  personal_root: ${JSON.stringify('Grüße & "😀*%')}`)
    .replace(/^  business_root:.*$/m, `  business_root: ${JSON.stringify('Arbeit\\Ü')}`)
  await post('workspace/save', { account, revision: view.revision, input })
  const proof = await post('proof/preview', { account })
  assert.equal(proof.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')
  assert.ok(proof.proof.artifact.folders.length > 2)
  const downloaded = await post('proof/download', { account, receipt: proof.receipt, confirmed: true })
  assert.equal(downloaded.identity, proof.proof.artifact.identity)
  assert.equal(downloaded.sieve, proof.proof.artifact.executable)
  const fresh = await post('proof/preview', { account })
  assert.equal(fresh.proof.artifact.identity, downloaded.identity)
  assert.equal(fresh.proof.artifact.executable, downloaded.sieve)
  const plan = await command('preview')
  assert.equal(plan.operation.artifact, fresh.proof.artifact.identity)
  assert.equal(plan.operation.proof_binding, fresh.receipt)
  assert.equal(plan.proof.context_binding.length, 64)
  assert.equal(fresh.revision, (await post('workspace/open', { account })).revision)
  assert.deepEqual(plan.operation.plan.logical_leaves, fresh.proof.artifact.folders)
  assert.ok(plan.operation.plan.entries.some(e => e.role === 'ancestor' && !e.existing))
  assert.ok(plan.operation.plan.entries.filter(e => e.role === 'leaf').length > 1)
  return plan
}
function preserved(current, initial) {
  assert.equal(current.unrelated, initial.unrelated, 'Complete canonical primary digest excludes only revision and provisioning journal')
  assert.equal(current.external_unrelated, initial.external_unrelated, 'Complete independent mailbox digest excludes only revision and intended folder provider state')
  assert.deepEqual(current.independent_effects, initial.independent_effects)
  assert.ok(Object.values(initial.populated).every(Boolean), 'Every unrelated primary history and independent effect was populated before CREATE')
}
async function progress(initial, expectedCreates = 1) {
  const snapshot = JSON.parse(await readFile(join(directory, 'folder-progress.json'), 'utf8'))
  assert.equal(snapshot.unrelated, initial.unrelated, 'Unrelated primary data is exact during actual progress')
  assert.equal(creates(await providerSnapshot()), expectedCreates)
  return snapshot
}
async function crash(name, external = '') {
  await start(true, true)
  const approval = await savedWorkspace()
  const initial = await observe()
  await writeFile(join(directory, 'folder-boundary'), name)
  await command('confirm', { receipt: approval.receipt, confirmed: true }); await reached(name)
  await stop('SIGKILL')
  await writeFile(join(directory, `continue-${name}`), 'continue')
  await writeFile(join(directory, 'folder-boundary'), '')
  await pause(80)
  if (external === 'authority') await writeFile(join(directory, 'workspace-authority'), 'changed-explicit-fixture-epoch')
  else if (external === 'namespace') await writeFile(join(directory, 'folder-mode'), 'dot')
  else if (external) await observe(external)
  const before = await observe()
  await start()
  const reopened = await command('status')
  const last = reopened.journal.history.at(-1)
  assert.ok(['uncertain', 'stopped'].includes(last.status), 'Reopening cannot continue or complete unfinished work')
  assert.equal((await observe()).source, before.source, 'Reopen must preserve independently installed source authority')
  assert.equal((await observe()).topology, before.topology, 'Reopen must preserve durable provider effects')
  await command('reconcile', { operation: last.id }, ['epoch', 'authority', 'namespace'].includes(external) ? 422 : 200)
  const first = await observe()
  await command('reconcile', { operation: last.id }, ['epoch', 'authority'].includes(external) ? 422 : 200)
  await command('status')
  const second = await observe()
  assert.equal(creates(second), creates(before), 'Status and reconciliation never resend CREATE')
  assert.equal(second.revision, first.revision, 'Repeated reconciliation does not churn revisions')
  assert.equal(second.topology, before.topology)
  preserved(second, initial)
  assert.equal(second.journal.history.at(-1).status, external ? 'conflict' : 'fresh_review')
  if (!external) { const renewed = await command('preview'); assert.notEqual(renewed.receipt, approval.receipt); assert.equal(creates(await observe()), creates(before)) }
  crashes.push({ boundary: name, external: external || null, creates: creates(second), status: second.journal.history.at(-1).status, source_preserved: second.source === before.source, topology_preserved: second.topology === before.topology })
  await stop()
}
try {
  await start(true)
  assert.equal((await fetch(`${base}/api/accounts/folders`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.invalid' }, { 'Sec-Fetch-Site': 'cross-site' }]) await post('folders', {}, 403, headers)
  for (const field of ['folders', 'name', 'namespace', 'endpoint', 'source', 'trust', 'permit', 'plan', 'context', 'authority', 'recipient_evidence', 'capabilities']) {
    await post('folders', { account, [field]: 'caller-claim', command: { action: 'preview' } }, 422)
    await command('preview', { [field]: 'caller-claim' }, 422)
  }
  const p = await command('preview')
  const initial = await observe()
  assert.deepEqual(p.operation.plan.entries, [], 'Pinned saved compiler omits all filing actions')
  assert.deepEqual(p.operation.plan.logical_leaves, [])
  await post('folders', { account: 'other-fixture', command: { action: 'confirm', receipt: p.receipt, confirmed: true } }, 422)
  await command('confirm', { receipt: '0'.repeat(64), confirmed: true }); await terminal('failed')
  const proof = await post('proof/preview', { account })
  await command('confirm', { receipt: proof.receipt, confirmed: true }); await terminal('failed')
  await command('confirm', { receipt: p.receipt, confirmed: true }); await terminal()
  await command('confirm', { receipt: p.receipt, confirmed: true }); await terminal('failed')
  const after = await observe()
  assert.equal(after.schema, 19); assert.equal(creates(after), 0); assert.equal(after.topology, initial.topology)
  assert.equal(after.unrelated, initial.unrelated); assert.equal(after.external_unrelated, initial.external_unrelated)
  checks.push('Protected HTTP, hostile extra fields, wrong account, receipt purpose/alteration/replay and exact empty saved-artifact plan pass with zero CREATE')
  await stop()
  await start(true)
  const stale = await command('preview')
  const ledger = join(directory, 'ledger/addresses.yaml')
  await writeFile(ledger, (await readFile(ledger, 'utf8')) + '\n# invented external edit\n')
  await command('confirm', { receipt: stale.receipt, confirmed: true }); await terminal('failed')
  assert.equal(creates(await observe()), 0)
  checks.push('External ledger edit invalidates saved-artifact approval')
  await stop()
  await start(true, true)
  const primary = await savedWorkspace()
  const primaryInitial = await observe()
  await command('confirm', { receipt: primary.receipt, confirmed: true }); await terminal()
  const created = await observe()
  assert.equal(created.journal.history.at(-1).status, 'complete')
  assert.deepEqual(created.journal.history.at(-1).steps.map(s => s.name), primary.operation.plan.entries.filter(e => !e.existing).map(e => e.name))
  assert.ok(created.journal.history.at(-1).steps.every(s => s.acknowledgment && s.outcome === 'observed' && s.observed))
  assert.equal(creates(created), primary.operation.plan.entries.filter(e => !e.existing).length)
  preserved(created, primaryInitial)
  const existing = await command('preview')
  assert.ok(existing.operation.plan.entries.every(e => e.existing))
  await command('confirm', { receipt: existing.receipt, confirmed: true }); await terminal()
  const unchanged = await observe(); assert.equal(creates(unchanged), creates(created)); preserved(unchanged, primaryInitial)
  await command('confirm', { receipt: primary.receipt, confirmed: true }); await terminal('failed')
  preserved(await observe(), primaryInitial)
  checks.push('Saved HTTP workspace proof, exact confirmed download and artifact-derived multi-folder CREATE share identity, executable, context and logical leaves; existing folders avoid CREATE and completed steps retain intent/ack/LIST')
  await stop()
  await start(true, true)
  const cancelled = await savedWorkspace()
  const cancelInitial = await observe()
  await writeFile(join(directory, 'folder-boundary'), 'folder_observed')
  await command('confirm', { receipt: cancelled.receipt, confirmed: true }); await reached('folder_observed')
  await progress(cancelInitial)
  await command('cancel')
  await writeFile(join(directory, 'continue-folder_observed'), 'continue')
  await terminal('cancelled')
  const stopped = await observe(); assert.equal(creates(stopped), 1); preserved(stopped, cancelInitial)
  assert.equal(stopped.journal.history.at(-1).status, 'stopped')
  await stop(); await start()
  const reopenedCancel = await command('status'); assert.equal(reopenedCancel.journal.history.at(-1).status, 'stopped')
  await command('preview', {}, 422)
  await command('reconcile', { operation: cancelled.operation.id })
  const reconciled = await observe(); preserved(reconciled, cancelInitial); assert.equal(creates(reconciled), 1)
  assert.equal(reconciled.journal.history.at(-1).status, 'fresh_review')
  const remaining = await command('preview'); assert.ok(remaining.operation.plan.entries.some(e => e.existing))
  assert.notEqual(remaining.receipt, cancelled.receipt)
  assert.equal(creates(await observe()), 1)
  checks.push('Deterministic HTTP Cancel after first durable CREATE/readback retains completed folders across reopen and observe-only reconciliation, with no continuation or deletion; remaining work requires fresh exact approval')
  await stop()
  await start(true, true)
  const refusedApproval = await savedWorkspace()
  const refusedInitial = await observe()
  await writeFile(join(directory, 'folder-mode'), 'quota')
  await command('confirm', { receipt: refusedApproval.receipt, confirmed: true }); await terminal('failed')
  const refused = await observe(); preserved(refused, refusedInitial)
  assert.equal(refused.journal.history.at(-1).status, 'refused')
  assert.equal(refused.topology, refusedInitial.topology)
  await writeFile(join(directory, 'folder-mode'), '')
  await command('reconcile', { operation: refusedApproval.operation.id })
  const refusedReconciled = await observe(); preserved(refusedReconciled, refusedInitial)
  assert.equal(creates(refusedReconciled), creates(refused))
  assert.equal(refusedReconciled.journal.history.at(-1).status, 'fresh_review')
  checks.push('Genuinely confirmed nonempty saved plan retains every populated primary history and independent effect through provider quota refusal and observe-only reconciliation')
  await stop()
  for (const replacement of ['', 'changed-synthetic-workspace-epoch']) {
    await start(true, true)
    const staleAuthority = await savedWorkspace()
    const beforeAuthorityChange = await observe()
    await stop()
    await writeFile(join(directory, 'workspace-authority'), replacement)
    await start()
    assert.equal((await post('workspace/open', { account })).exports.at(-1).usable, false)
    await command('confirm', { receipt: staleAuthority.receipt, confirmed: true }); await terminal('failed')
    const rejected = await observe(); assert.equal(creates(rejected), 0); preserved(rejected, beforeAuthorityChange)
    if (!replacement) assert.deepEqual((await command('preview')).operation.plan.logical_leaves, [])
    await stop()
  }
  checks.push('Removed or changed server installation invalidates exact approvals and old export usability without CREATE; reopening does not reconstruct authority from saved exports')
  for (const name of ['folder_consumed','folder_first_intent','folder_provider_effect','folder_acknowledged','folder_observed','folder_leaf_intent']) await crash(name)
  for (const external of ['external','epoch','authority','namespace']) await crash('folder_provider_effect', external)
  checks.push('Ten genuinely HTTP-confirmed saved-workspace SIGKILL boundaries preserve independent TLS topology and journal; repeated protected HTTP reconciliation issues no CREATE')
  await start(true, true)
  const raceApproval = await savedWorkspace()
  await writeFile(join(directory, 'folder-boundary'), 'folder_leaf_intent')
  await command('confirm', { receipt: raceApproval.receipt, confirmed: true }); await reached('folder_leaf_intent')
  const beforeRace = await providerSnapshot()
  await observe('delete_parent')
  const deleted = await providerSnapshot()
  assert.notEqual(deleted.topology, beforeRace.topology)
  await writeFile(join(directory, 'continue-folder_leaf_intent'), 'continue')
  await terminal()
  const completed = await observe()
  assert.equal(completed.journal.history.at(-1).status, 'complete')
  assert.equal(creates(completed), completed.journal.history.at(-1).steps.length)
  checks.push('HTTP-confirmed final-read/CREATE race demonstrably recreates a removed parent implicitly; visible state does not prove attribution or compare-and-swap')
  await stop()
  await start(true, true)
  await savedWorkspace()
  const retained = { pid: server.pid, binary, binary_sha256, test, directory, base, account, cwd: root, argv: [binary, '--exact', test, '--nocapture'], scope: 'Explicitly installed synthetic workspace authority; production remains conservative' }
  identity(server, test)
  await writeFile(join(root, '.audit/imap-provisioning-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/imap-provisioning-runtime.json'), JSON.stringify({ checks, crashes, limitation: 'Actual provider and native confirmation remain unverified. Runtime authority was explicitly installed only by the trusted test launcher.' }, null, 2) + '\n')
  server.unref(); server = undefined
  console.log(`PASS ${checks.length} bounded IMAP groups and ${crashes.length} actual HTTP-confirmed saved-workspace SIGKILL boundaries.`)
} finally {
  if (server) await stop()
}
