import assert from 'node:assert/strict'
import { readFile, readdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = []
const wires = async () => (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(JSON.parse)
// Each fault must fail the preview for its own reason. The test build logs the
// MOVE session's error code; the oversized literal fails in the read-only
// session, which logs none, so its reason shows in how soon it fails.
const failures = async () => (await Promise.all((await readdir(f.directory)).filter(n => n.startsWith('application-')).map(n => readFile(join(f.directory, n), 'utf8')))).join('').match(/^backlog\.failure=\w+$/gm) ?? []
const reason = { malformed_greeting: 'Protocol', referral: 'UnknownCode', authentication_failed: 'Authentication', read_only: 'Unsupported', slow_greeting: 'Deadline' }
// The fake trickles its greeting over 15 s, 1.5 times the installed source's
// 10 s greeting deadline, so only that deadline can end the preview sooner.
const slowGreeting = 15_000
async function record(name) {
  checks.push({ name, directory: f.directory, passed: true })
  await writeFile(join(root, '.audit/imap-backlog-workflow-fault-results.json'), JSON.stringify({ checks }, null, 2))
}
try {
  for (const mode of ['malformed_greeting', 'referral', 'authentication_failed', 'read_only', 'oversized_literal', 'slow_greeting']) {
    await f.start(true); await f.savedWorkspace()
    const before = await f.observe()
    await writeFile(join(f.directory, 'move-mode'), mode)
    const started = Date.now()
    await f.command('preview', { options }, 422)
    const elapsed = Date.now() - started
    if (mode === 'oversized_literal') {
      // The fake streams a 64 MiB literal and never hangs up, so only the
      // read-only session's own limit ends this before its 15 s idle deadline.
      assert.deepEqual(await failures(), [])
      assert.ok(elapsed < 10_000, `oversized literal ended after ${elapsed} ms`)
    } else assert.deepEqual(await failures(), [`backlog.failure=${reason[mode]}`])
    if (mode === 'slow_greeting') assert.ok(elapsed < slowGreeting, `slow greeting ended after ${elapsed} ms`)
    const after = await f.observe()
    assert.equal(after.primary, before.primary)
    assert.deepEqual(after.folders, before.folders)
    assert.deepEqual(await wires(), [])
    if (mode === 'authentication_failed') assert.equal(after.commands.length - before.commands.length, 1)
    if (['malformed_greeting', 'referral', 'slow_greeting'].includes(mode)) assert.deepEqual(after.commands, before.commands)
    await record(`${mode}-no-admitted-journal-no-MOVE`)
    await f.close()
  }
  for (const change of ['no_move', 'no_uidplus', 'missing_destination']) {
    await f.start(true); await f.savedWorkspace()
    await f.observe(change)
    await f.command('preview', { options }, 422)
    await f.stop('SIGKILL'); await f.start()
    await f.command('preview', { options }, 422)
    assert.equal((await f.command('status')).journal.runs.length, 0)
    assert.deepEqual(await wires(), [])
    await record(`${change}-unsupported-persists-through-replacement-no-effects`)
    await f.close()
  }
  for (const mode of ['untagged', 'unsolicited', 'missing', 'partial', 'conflicting', 'malformed', 'wrong_validity', 'uidnotsticky', 'lost']) {
    await f.start(true); await f.savedWorkspace(true)
    const preview = await f.command('preview', { options })
    const before = await f.observe()
    await writeFile(join(f.directory, 'move-mode'), mode)
    await f.command('confirm', { options, receipt: preview.receipt, confirmed: true }); await f.terminal()
    const effect = await f.observe()
    const expected = ['untagged', 'unsolicited'].includes(mode) ? 'complete' : ['missing', 'partial', 'lost'].includes(mode) ? 'uncertain' : 'conflict'
    assert.equal(effect.journal.runs.at(-1).status, expected)
    assert.equal(effect.unrelated, before.unrelated)
    assert.equal(effect.independent, before.independent)
    assert.deepEqual(effect.independent_effects.slice(1), before.independent_effects.slice(1))
    assert.equal((await wires()).length, 1)
    assert.equal(effect.journal.runs.at(-1).undo_receipt, null)
    await f.stop('SIGKILL'); await f.start()
    const run = (await f.command('status')).journal.runs.at(-1)
    assert.equal(run.status, expected)
    await f.command('reconcile', { run: run.id })
    const reconciled = await f.observe()
    await f.command('reconcile', { run: run.id })
    const repeated = await f.observe()
    assert.equal(repeated.revision, reconciled.revision)
    assert.deepEqual(repeated.folders, effect.folders)
    assert.equal(repeated.unrelated, before.unrelated)
    await f.command('resume', { run: run.id }); await f.terminal()
    assert.equal((await wires()).length, 1)
    await record(`saved-workspace-${mode}-durable-evidence-no-retry`)
    await f.close()
  }
  for (const change of ['original_validity', 'destination_validity', 'missing_destination', 'epoch']) {
    await f.start(true); await f.savedWorkspace()
    const preview = await f.command('preview', { options })
    await writeFile(join(f.directory, 'move-boundary'), 'move_saved_mapping')
    await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
    await f.reached('move_saved_mapping'); await f.stop('SIGKILL')
    await writeFile(join(f.directory, 'move-boundary'), '')
    const before = await f.observe(change)
    await f.start()
    const run = (await f.command('status')).journal.runs.at(-1)
    await f.command('reconcile', { run: run.id }, 422)
    const conflict = await f.observe()
    assert.equal(conflict.journal.runs.at(-1).status, 'conflict')
    assert.deepEqual(conflict.folders, before.folders)
    const revision = conflict.revision
    await f.command('reconcile', { run: run.id }, 422)
    assert.equal((await f.observe()).revision, revision)
    assert.equal((await wires()).length, 1)
    await record(`mapped-crash-${change}-conflict-after-replacement`)
    await f.close()
  }
  await f.start(true)
  await f.observe('special_destination')
  const workspace = await f.post('workspace/open', { account: f.account })
  const leaf = 'Ü & "quotes" \\backslash'
  const destination = `Personal/${leaf}`
  const input = workspace.workspace.input
  input.documents.preset_yaml = input.documents.preset_yaml.replace('  - id: sender.studio\n', `  - id: sender.studio\n    folder: ${JSON.stringify(leaf)}\n`)
  input.documents.override_yaml = input.documents.override_yaml.replace('folder: Studio', `folder: ${JSON.stringify(leaf)}`)
  await f.post('workspace/save', { account: f.account, revision: workspace.revision, input })
  const proof = await f.post('proof/preview', { account: f.account })
  const exported = await f.post('proof/download', { account: f.account, receipt: proof.receipt, confirmed: true })
  const preview = await f.command('preview', { options })
  assert.equal(preview.operation.plan.filing.artifact.executable, exported.sieve)
  assert.equal(preview.operation.batches[0].intent.destination.name.decoded, destination)
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  assert.equal((await wires())[0].command, 'UID MOVE 4,8,19 "Personal/&ANw- &- \\"quotes\\" \\\\backslash"')
  await f.stop('SIGKILL'); await f.start()
  const run = (await f.command('status')).journal.runs.at(-1)
  const undo = await f.command('undo_preview', { run: run.id })
  await f.command('undo', { run: run.id, receipt: undo.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  assert.deepEqual((await f.observe()).folders[destination].uids, [40])
  await record('saved-artifact-Unicode-ampersand-quote-backslash-exact-wire-and-reverse-after-replacement')
  await f.close()
  console.log(`Passed ${checks.length} saved protocol fault groups`)
} finally { await f.close() }
