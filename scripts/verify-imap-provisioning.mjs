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
const driver = 'acquisition::folder_provisioning::tests::synthetic_folder_crash_worker'
const pause = ms => new Promise(r => setTimeout(r, ms))
let server, worker, directory, token, account
const checks = [], crashes = []
async function start(fresh = false) {
  if (fresh) directory = await mkdtemp(join(root, '.audit/imap-provisioning-fixture-'))
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
}
async function stop() { identity(server, test); assert.equal(sha(await readFile(binary)), binary_sha256); const done = once(server, 'exit'); server.kill('SIGTERM'); await done; server = undefined }
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
  for (let i = 0; i < 1000; i++) {
    const value = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (value.job?.progress.state !== 'running') { assert.equal(value.job?.progress.state, expected); return }
    await pause(10)
  }
  throw Error('Synthetic folder operation did not finish')
}
const creates = snapshot => snapshot.commands.filter(c => c === 'CREATE').length
async function reached(name) {
  for (let i = 0; i < 1500; i++) { if (await readFile(join(directory, `reached-${name}`)).then(() => true, () => false)) return; if (worker) assert.equal(worker.exitCode, null); await pause(10) }
  throw Error(`Synthetic crash boundary not reached ${name}`)
}
async function drive() {
  worker = spawn(binary, ['--exact', driver, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_FOLDERS_FIXTURE: directory }, stdio: 'ignore' })
  worker.on('error', () => {})
}
async function killWorker() { identity(worker, driver); assert.equal(sha(await readFile(binary)), binary_sha256); const done = once(worker, 'exit'); worker.kill('SIGKILL'); await done; worker = undefined }
async function crash(name, external = '') {
  await start(true)
  await command('preview')
  const initial = await observe()
  await writeFile(join(directory, 'folder-boundary'), name)
  await drive(); await reached(name)
  await killWorker()
  await writeFile(join(directory, `continue-${name}`), 'continue')
  await writeFile(join(directory, 'folder-boundary'), '')
  await pause(80)
  if (external) await observe(external)
  const before = await observe()
  await stop(); await start()
  const reopened = await command('status')
  const last = reopened.journal.history.at(-1)
  assert.ok(['uncertain', 'stopped'].includes(last.status), 'Reopening cannot continue or complete unfinished work')
  assert.equal((await observe()).source, before.source, 'Reopen must preserve independently installed source authority')
  assert.equal((await observe()).topology, before.topology, 'Reopen must preserve durable provider effects')
  await command('reconcile', { operation: last.id }, external === 'epoch' ? 422 : 200)
  const first = await observe()
  await command('reconcile', { operation: last.id }, external === 'epoch' ? 422 : 200)
  await command('status')
  const second = await observe()
  assert.equal(creates(second), creates(before), 'Status and reconciliation never resend CREATE')
  assert.equal(second.revision, first.revision, 'Repeated reconciliation does not churn revisions')
  assert.equal(second.topology, before.topology)
  assert.equal(second.unrelated, initial.unrelated)
  assert.equal(second.external_unrelated, initial.external_unrelated)
  assert.deepEqual(second.independent_effects, initial.independent_effects)
  if (external) assert.equal(second.journal.history.at(-1).status, 'conflict')
  crashes.push({ boundary: name, external: Boolean(external), creates: creates(second), status: second.journal.history.at(-1).status, source_preserved: second.source === before.source, topology_preserved: second.topology === before.topology })
  await stop()
}
try {
  await start(true)
  assert.equal((await fetch(`${base}/api/accounts/folders`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.invalid' }, { 'Sec-Fetch-Site': 'cross-site' }]) await post('folders', {}, 403, headers)
  for (const field of ['folders', 'name', 'namespace', 'endpoint', 'source', 'trust', 'permit', 'plan']) {
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
  assert.equal(after.schema, 12); assert.equal(creates(after), 0); assert.equal(after.topology, initial.topology)
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
  for (const name of ['folder_consumed','folder_first_intent','folder_provider_effect','folder_acknowledged','folder_observed','folder_leaf_intent']) await crash(name)
  for (const external of ['external','epoch']) await crash('folder_provider_effect', external)
  checks.push('Eight actual client SIGKILL boundaries preserve independent TLS topology and journal; repeated protected HTTP reconciliation issues no CREATE')
  await start(true)
  await writeFile(join(directory, 'folder-boundary'), 'folder_leaf_intent')
  await drive(); await reached('folder_leaf_intent')
  const beforeRace = await providerSnapshot()
  await observe('delete_parent')
  const deleted = await providerSnapshot()
  assert.notEqual(deleted.topology, beforeRace.topology, 'External editor removed a parent after the final LIST')
  const done = once(worker, 'exit')
  await writeFile(join(directory, 'continue-folder_leaf_intent'), 'continue')
  const [code] = await done; assert.equal(code, 0); worker = undefined
  const completed = await observe()
  assert.ok(creates(completed) > 0); assert.equal(completed.journal.history.at(-1).status, 'complete')
  assert.equal(creates(completed), completed.journal.history.at(-1).steps.length, 'Descendant CREATE recreated the deleted parent implicitly without another explicit parent command')
  checks.push('Explicit synthetic artifact drives exact German wire CREATE and persisted readback; the final-read/CREATE race demonstrably recreates a deleted parent and visible state cannot prove attribution')
  await stop()
  await start(true)
  const retained = { pid: server.pid, binary, binary_sha256, test, directory, base, account, cwd: root, argv: [binary, '--exact', test, '--nocapture'], scope: 'Synthetic partial implementation; saved compiler emits no destinations' }
  identity(server, test)
  await writeFile(join(root, '.audit/imap-provisioning-retained.json'), JSON.stringify(retained, null, 2) + '\n')
  await writeFile(join(root, '.audit/imap-provisioning-runtime.json'), JSON.stringify({ checks, crashes, limitation: 'Saved-workspace CREATE acceptance is unmet because pinned compiler omits all filing actions. No trust change was made.' }, null, 2) + '\n')
  server.unref(); server = undefined
  console.log(`PASS ${checks.length} bounded IMAP groups and ${crashes.length} actual synthetic client SIGKILL boundaries. Saved-workspace CREATE remains unmet.`)
} finally {
  if (worker) await killWorker()
  if (server) await stop()
}
