import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { openSync } from 'node:fs'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified retained fake before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const binary_sha256 = sha(await readFile(binary))
const test = 'server::accounts::managesieve_transactions_server_fixture'
const worker = 'acquisition::sieve_transaction::tests::transaction_fixture_worker'
const BLOCK = { kind: 'block' }, INCLUDE = { kind: 'include', script: 'mailroom' }
let server, directory, token, account
const checks = [], crashes = []
const pause = ms => new Promise(r => setTimeout(r, ms))
async function start(fresh = false) {
  if (fresh) directory = await mkdtemp(join(root, '.audit/managesieve-transactions-fixture-'))
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_TRANSACTIONS_FIXTURE: directory }, stdio: ['ignore', 'ignore', openSync(join(directory, 'fixture-stderr.log'), 'a')], detached: true })
  for (let i = 0; i < 400; i++) {
    assert.equal(server.exitCode, null, 'Fixture exited during startup')
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; account = (await observe()).account; return }
    await pause(20)
  }
  throw Error('Transaction fixture startup timed out')
}
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
}
async function stop(signal = 'SIGTERM') { identity(); assert.equal(sha(await readFile(binary)), binary_sha256); const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function observe(action = '') {
  execFileSync(binary, ['--exact', worker], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_TRANSACTIONS_FIXTURE: directory, MAILROOM_SIEVE_WORKER_ACTION: action }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'transaction-observations.json'), 'utf8'))
}
async function post(path, body, expected = 200, extraHeaders = {}) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...extraHeaders }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `Unexpected status for ${path}`); assert.equal(response.headers.get('cache-control'), 'no-store'); return response.json()
}
const command = (action, fields = {}, expected = 200) => post('sieve-transactions', { account, command: { action, ...fields } }, expected)
const place = placement => command('record_placement', { placement })
async function ready(action = 'preview', fields = {}) { const p = await command(action, fields); assert.equal(p.outcome, 'ready', `${action} must be ready: ${p.refusal?.reason}`); return p }
async function terminal(expected = 'complete') {
  const deadline = Date.now() + 70000
  while (Date.now() < deadline) {
    const response = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }); const s = await response.json()
    if (s.job?.progress.state !== 'running') { assert.equal(s.job?.progress.state, expected, 'Unexpected transaction worker outcome'); return }
    await pause(10)
  }
  throw Error('Transaction job did not finish within the 60-second attempt cap')
}
function confirmation(preview) {
  const comp = preview.transaction.compensates
  if (comp) return ['compensation_confirm', { operation: comp, receipt: preview.receipt, confirmed: true }]
  return [preview.transaction.composition?.purpose === 'removal' ? 'removal_confirm' : 'confirm', { receipt: preview.receipt, confirmed: true }]
}
async function confirm(preview, expected = 'complete') { await command(...confirmation(preview)); await terminal(expected) }
function effects(snapshot) { return snapshot.commands.filter(c => ['PUTSCRIPT', 'SETACTIVE'].includes(c)).join(',') }
async function boundary(name) { await writeFile(join(directory, 'transaction-boundary'), name) }
async function reached(name) {
  for (let i = 0; i < 1200; i++) { if (await readFile(join(directory, `reached-${name}`)).then(() => true, () => false)) return; await pause(10) }
  throw Error(`Crash boundary not reached: ${name}`)
}
async function crash(name, placement, phase = 'deploy') {
  await start(true)
  await place(placement)
  const initial = await observe()
  let preview = await ready()
  if (phase === 'compensation') { await confirm(preview); preview = await ready('compensation_preview', { operation: preview.transaction.id }) }
  if (phase === 'removal') { await confirm(preview); preview = await ready('removal_preview') }
  await boundary(name)
  await command(...confirmation(preview))
  await reached(name)
  const oldToken = token
  await stop('SIGKILL')
  await writeFile(join(directory, 'transaction-boundary'), '')
  const beforeRestart = await observe()
  await start()
  assert.notEqual(token, oldToken)
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  const reopened = await command('status')
  const last = reopened.journal.history.at(-1)
  assert.ok(['uncertain', 'stopped'].includes(last.status), 'Restart cannot complete or resume an unfinished operation')
  assert.equal((await observe()).provider_image, beforeRestart.provider_image, 'Restart preserves independent provider state')
  assert.equal(beforeRestart.provider_foreign, initial.provider_foreign, 'Existing rules stay byte-identical at every crash boundary')
  await command('reconcile', { operation: last.id })
  const first = await observe()
  await command('reconcile', { operation: last.id })
  await command('status')
  const second = await observe()
  assert.equal(effects(second), effects(beforeRestart), 'Reconciliation never resends an effect')
  assert.equal(first.revision, second.revision, 'Repeated reconciliation and status do not persist again')
  assert.equal(JSON.stringify(first.transactions), JSON.stringify(second.transactions), 'Repeated reconciliation converges without changing journal semantics')
  assert.equal(second.unrelated, initial.unrelated, 'Crash/reopen/reconcile preserves every unrelated aggregate')
  assert.deepEqual(second.external_unrelated, initial.external_unrelated, 'Crash/reopen/reconcile preserves independent moves, sends and unsubscribe effects')
  assert.equal(second.provider_foreign, initial.provider_foreign)
  crashes.push({ boundary: name, placement: placement.kind, phase, effects_before_reconcile: effects(beforeRestart), effects_after_reconcile: effects(second), status_after_reconcile: second.transactions.at(-1).status, initial_provider: initial.provider_image, reopened_provider: beforeRestart.provider_image })
  await stop()
}
try {
  await start(true)
  assert.equal((await fetch(`${base}/api/accounts/sieve-transactions`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.example' }, { 'Sec-Fetch-Site': 'cross-site' }]) await post('sieve-transactions', {}, 403, headers)
  for (const field of ['endpoint', 'script', 'name', 'ownership', 'permit', 'source', 'trust', 'baseline', 'foreign', 'host']) {
    await post('sieve-transactions', { account, [field]: 'caller-claim', command: { action: 'preview' } }, 422)
    await command('preview', { [field]: 'caller-claim' }, 422)
  }
  await command('record_placement', { placement: { kind: 'block', extra: true } }, 422)
  checks.push('Protected HTTP and unknown-field rejection without caller trust, script or foreign input')
  const initial = await observe()
  assert.equal(initial.placement, null)
  const unchosen = await command('preview')
  assert.deepEqual([unchosen.outcome, unchosen.refusal.reason], ['refused', 'placement_unchosen'])
  for (const invalid of [{ kind: 'include', script: 'Open-Xchange' }, { kind: 'include', script: 'bad"name' }, { kind: 'include', script: '' }]) await command('record_placement', { placement: invalid }, 422)
  assert.equal(effects(await observe()), ''); assert.equal((await observe()).placement, null)
  checks.push('No default placement: composed deploy refuses until an explicit valid choice is recorded')
  await place(BLOCK)
  const p = await ready()
  assert.deepEqual(p.effects, ['upload'])
  assert.equal(p.precedence.foreign_rules[0].name, 'Travel')
  assert.deepEqual(p.precedence.counts, { mailroom: 2, existing_wins: 1, conflicting: 1, undetermined: 0, copies: { copied: 0, beside_existing: 0, undetermined: 0, guard_violations: 0 } })
  assert.deepEqual(p.precedence.copies, []); assert.equal(p.transaction.composition.semantics, 'mailroom-foreign-composition-v2')
  assert.deepEqual(p.precedence.decisions.map(d => [d.rule, d.existing, d.mailroom]), [['Travel', { kind: 'folder', name: 'Reisen' }, { kind: 'inbox' }]])
  const composed = Buffer.from(p.transaction.after.bodies.find(b => b.name === 'Open-Xchange').bytes).toString('utf8')
  assert.ok(composed.indexOf('# mailroom:block-begin v1') > composed.lastIndexOf('Rulename: Travel'), 'Mailroom block follows every existing rule')
  await post('sieve-transactions', { account: 'other-fixture', command: { action: 'confirm', receipt: p.receipt, confirmed: true } }, 422)
  const exportProof = await post('proof/preview', { account })
  await command('confirm', { receipt: exportProof.receipt, confirmed: true }); await terminal('failed')
  await command('removal_confirm', { receipt: p.receipt, confirmed: true }); await terminal('failed')
  await command('confirm', { receipt: `${p.receipt.slice(0, -1)}${p.receipt.endsWith('0') ? '1' : '0'}`, confirmed: true }); await terminal('failed')
  assert.equal(effects(await observe()), '')
  await confirm(p)
  const deployed = await observe()
  assert.equal(deployed.schema, 23); assert.equal(deployed.provider_target, p.transaction.after.bodies.find(b => b.name === p.transaction.target).sha256)
  assert.equal(deployed.provider_foreign, initial.provider_foreign); assert.equal(deployed.active, 'Open-Xchange')
  assert.deepEqual(deployed.placement, BLOCK)
  assert.deepEqual(deployed.external_unrelated, initial.external_unrelated)
  assert.ok((await command('status')).journal.history[0].steps.every(s => s.acknowledgment && s.outcome === 'observed'))
  await confirm(p, 'failed'); assert.equal(effects(await observe()), effects(deployed))
  await command('preview', {}, 422)
  checks.push('Block placement composes filing after existing rules with exact foreign bytes, reported precedence, purpose/alteration/replay denial and no-op re-deploy refusal')
  const removal = await ready('removal_preview'); assert.deepEqual(removal.effects, ['upload'])
  await confirm(removal)
  const removed = await observe()
  assert.equal(removed.provider_target, initial.provider_target, 'Removal restores the exact existing bytes')
  assert.equal((await command('removal_preview')).refusal.reason, 'nothing_to_remove')
  await command('compensation_preview', { operation: p.transaction.id }, 422)
  checks.push('Removal touches only Mailroom segments and compensation refuses after a later change')
  await stop()
  await start(true)
  await place(INCLUDE)
  const inc = await ready(); assert.deepEqual(inc.effects, ['upload_owned', 'upload'])
  await confirm(inc)
  const included = await observe()
  assert.ok(included.names.includes('mailroom')); assert.ok(included.provider_owned)
  assert.equal(included.provider_foreign, initial.provider_foreign); assert.equal(included.active, 'Open-Xchange')
  await place(BLOCK)
  const switched = await ready(); assert.deepEqual(switched.effects, ['upload'])
  await confirm(switched)
  const afterSwitch = await observe()
  assert.equal(afterSwitch.provider_foreign, initial.provider_foreign); assert.equal(afterSwitch.provider_owned, included.provider_owned)
  const back = await ready('compensation_preview', { operation: switched.transaction.id }); await confirm(back)
  assert.equal((await observe()).provider_target, included.provider_target)
  checks.push('Include placement writes the owned script first, then one include line; switching placement and compensation keep existing bytes identical')
  await stop()
  for (const placement of [BLOCK, INCLUDE]) {
    await start(true); await stop()
    await observe('business_copy'); await writeFile(join(directory, 'workspace-authority'), 'explicit-copy-first')
    await start()
    const original = await observe()
    await place(placement)
    const c = await ready()
    const copied = c.precedence.copies.filter(x => x.status === 'copied')
    assert.deepEqual(copied.map(x => [x.rule, x.recipient, x.filed_by]), [['forward.approved', 'archive@example.net', 'existing_rule']], 'The Travel rule files the message with stop and the copy still runs')
    assert.deepEqual(c.precedence.counts.copies, { copied: 1, beside_existing: 1, undetermined: 0, guard_violations: 0 })
    assert.ok(c.precedence.decisions.some(d => d.rule === 'Travel' && d.copies === 1 && d.existing.name === 'Reisen'))
    const host = Buffer.from(c.transaction.after.bodies.find(b => b.name === 'Open-Xchange').bytes).toString('utf8')
    const begin = host.indexOf('# mailroom:copy-begin v1'), end = host.indexOf('# mailroom:copy-end v1')
    assert.ok(begin > 0 && begin < host.indexOf('Rulename: Travel') && end < host.indexOf('Rulename: Travel'), 'Copy segment precedes the first existing rule')
    const segment = host.slice(begin, end)
    assert.ok(segment.includes('redirect :copy "archive@example.net";'))
    for (const filing of ['fileinto', 'keep', 'addflag', 'stop', 'discard']) assert.ok(!segment.includes(filing), `copy segment holds no ${filing}`)
    const owned = c.transaction.after.bodies.find(b => b.name === 'mailroom')
    if (placement.kind === 'include') assert.ok(!Buffer.from(owned.bytes).toString('utf8').includes('redirect'), 'Owned script holds filing only')
    await confirm(c)
    const deployed = await observe()
    assert.equal(deployed.provider_target, c.transaction.after.bodies.find(b => b.name === 'Open-Xchange').sha256); assert.equal(deployed.provider_foreign, original.provider_foreign)
    const removal = await ready('removal_preview'); await confirm(removal)
    const removed = await observe()
    assert.equal(removed.provider_target, original.provider_target, 'Removal restores the exact original host bytes'); assert.equal(removed.active, 'Open-Xchange')
    await stop()
  }
  checks.push('With an approved accounting copy, both placements put the copy segment before the existing Travel rule that files with stop; the copy is reported beside existing filing and removal restores exact bytes')
  await start(true)
  await place(INCLUDE)
  const stale = await ready()
  await observe('foreign_edit')
  const edited = await observe()
  await confirm(stale, 'failed')
  assert.equal(effects(await observe()), ''); assert.equal((await observe()).provider_image, edited.provider_image)
  const refreshed = await ready(); assert.equal(refreshed.precedence.foreign_rules.length, 2)
  checks.push('A foreign edit between preview and write refuses the stale preimage without effects; a fresh review sees the new rule')
  await stop()
  for (const [mode, reason] of [['foreign_unparseable', 'unparseable'], ['foreign_unsafe', 'unsafe_rule'], ['foreign_stop', 'mailroom_unreachable'], ['marker_damaged', 'marker_damaged'], ['inactive_host', 'host_inactive'], ['owned_collision', 'owned_name_foreign']]) {
    await start(true)
    await place(INCLUDE)
    await observe(mode)
    const before = await observe()
    const refused = await command('preview')
    assert.deepEqual([refused.outcome, refused.refusal.reason], ['refused', reason], mode)
    if (reason === 'unsafe_rule') assert.equal(refused.refusal.rule, 'Keep going')
    const after = await observe()
    assert.equal(effects(after), ''); assert.equal(after.provider_image, before.provider_image)
    await stop()
  }
  checks.push('Unparseable, unsafe, unconditionally stopping, damaged-marker, inactive-host and colliding-name foreign content block with a report and no writes')
  await start(true)
  await place(BLOCK)
  const ledgerPreview = await ready()
  const ledgerPath = join(directory, 'ledger/addresses.yaml')
  await writeFile(ledgerPath, (await readFile(ledgerPath, 'utf8')) + '\n# invented external authority change\n')
  await confirm(ledgerPreview, 'failed'); assert.equal(effects(await observe()), '')
  checks.push('External authoritative ledger changes invalidate approval without effects')
  await stop()
  for (const external of ['external_target', 'external_active']) {
    await start(true)
    await place(BLOCK)
    const compensationConflict = await ready(); await confirm(compensationConflict)
    await observe(external)
    await command('compensation_preview', { operation: compensationConflict.transaction.id }, 422)
    await stop()
  }
  checks.push('Conditional compensation refuses external host edits and changed active selection')
  for (const external of ['external_target', 'external_active', 'external_epoch']) {
    await start(true)
    await place(BLOCK)
    const interrupted = await ready(); await boundary('provider_upload_effect')
    await command('confirm', { receipt: interrupted.receipt, confirmed: true }); await reached('provider_upload_effect'); await stop('SIGKILL')
    await observe(external); const prior = await observe()
    await writeFile(join(directory, 'transaction-boundary'), ''); await start()
    await command('reconcile', { operation: interrupted.transaction.id }, external === 'external_epoch' ? 422 : 200)
    assert.equal((await command('status')).journal.history[0].status, 'conflict')
    const first = await observe()
    await command('reconcile', { operation: interrupted.transaction.id }, external === 'external_epoch' ? 422 : 200)
    const second = await observe()
    assert.equal(effects(second), effects(prior)); assert.equal(second.provider_image, prior.provider_image); assert.equal(first.revision, second.revision)
    await stop()
  }
  checks.push('Recovery blocks third-state host/active edits and changed installed ownership epoch without repeating effects')
  await start(true)
  await place(BLOCK)
  const raced = await ready(); await boundary('upload_intent')
  await command('confirm', { receipt: raced.receipt, confirmed: true }); await reached('upload_intent')
  await observe('foreign_edit')
  await writeFile(join(directory, 'continue-upload_intent'), 'continue'); await terminal()
  const raceResult = await observe()
  assert.equal(raceResult.provider_target, raced.transaction.after.bodies.find(b => b.name === 'Open-Xchange').sha256)
  assert.equal(raceResult.provider_foreign, initial.provider_foreign, 'The foreign rule added after the final read was lost')
  checks.push('A foreign edit after the final read is overwritten, exposing the unavoidable non-CAS race')
  await stop()
  for (const name of ['transaction_consumed', 'upload_intent', 'provider_upload_effect', 'upload_acknowledged', 'upload_observed']) await crash(name, BLOCK)
  for (const name of ['owned_upload_intent', 'provider_upload_effect', 'owned_upload_acknowledged', 'owned_upload_observed', 'upload_intent', 'upload_acknowledged']) await crash(name, INCLUDE)
  for (const name of ['compensation_upload_intent', 'provider_upload_effect', 'compensation_upload_acknowledged', 'compensation_upload_observed']) await crash(name, BLOCK, 'compensation')
  for (const name of ['upload_intent', 'provider_upload_effect', 'upload_acknowledged']) await crash(name, INCLUDE, 'removal')
  checks.push('Actual SIGKILL/replacement around intents, provider effects, acknowledgments and between owned/host uploads, including compensation and removal')
  await start(true)
  await place(INCLUDE)
  await boundary('owned_upload_observed')
  const cancelPreview = await ready()
  await command('confirm', { receipt: cancelPreview.receipt, confirmed: true }); await reached('owned_upload_observed')
  await post('scan', { account }, 422)
  await command('confirm', { receipt: cancelPreview.receipt, confirmed: true }, 422)
  await command('cancel'); await writeFile(join(directory, 'continue-owned_upload_observed'), 'continue'); await terminal('cancelled')
  const canceled = await observe(); assert.equal(effects(canceled), 'PUTSCRIPT'); assert.equal(canceled.provider_target, initial.provider_target)
  await command('reconcile', { operation: cancelPreview.transaction.id })
  assert.equal((await command('status')).journal.history[0].status, 'fresh_review')
  const resumed = await ready(); assert.deepEqual(resumed.effects, ['upload']); await confirm(resumed)
  assert.equal((await observe()).provider_foreign, initial.provider_foreign)
  checks.push('Stop preserves the completed owned upload, rejects competing scan and resumes only through a fresh exact review')
  await stop()
  await start(true); await stop()
  await observe('business_copy'); await writeFile(join(directory, 'workspace-authority'), 'explicit-copy-first')
  await start()
  identity()
  const metadata = { base, pid: server.pid, binary, binary_sha256, directory, account, test, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/managesieve-transactions-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/managesieve-transactions-runtime.json'), JSON.stringify({ ...metadata, checks, crashes, synthetic_only: true }, null, 2) + '\n')
  server.unref()
  console.log(`PASS: ${checks.length} composed transaction HTTP groups and ${crashes.length} actual SIGKILL/replacement boundaries against durable local TLS state`)
} catch (error) { if (server) await stop(); throw error }
