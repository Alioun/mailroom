import assert from 'node:assert/strict'
import { readFile, writeFile, stat } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root, pause } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = []
const resultName = process.argv.includes('--tail') ? 'imap-backlog-workflow-safety-tail-results.json' : 'imap-backlog-workflow-safety-results.json'
const wire = async () => (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(JSON.parse)
async function prepared() { await f.start(true); await f.savedWorkspace(); return f.command('preview', { options }) }
async function forward() {
  const preview = await prepared()
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  return (await f.command('status')).journal.runs.at(-1)
}
async function rejected(action, fields) {
  const before = await wire()
  await f.command(action, fields)
  assert.equal((await f.terminal()).progress.state, 'failed')
  assert.deepEqual(await wire(), before)
}
async function record(name, extra = {}) {
  checks.push({ name, directory: f.directory, passed: true, ...extra })
  await writeFile(join(root, '.audit', resultName), JSON.stringify({ checks }, null, 2))
}
let preview
try {
  if (!process.argv.includes('--tail')) {
  await f.start(true, false)
  const unknown = await f.post('proof/preview', { account: f.account })
  assert.equal(unknown.proof.artifact.context.recipient_evidence, 'unknown')
  const empty = await f.command('preview', { options })
  assert.equal(empty.operation.batches.length, 0, 'Held recipients leave no executable batch')
  await f.command('confirm', { options, receipt: empty.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'failed', 'Confirming an empty preview fails before any run')
  assert.equal((await f.observe()).journal.runs.length, 0, 'An empty preview starts no run')
  await f.post('backlog/protocol', { account: f.account, command: { action: 'preview', options, trust: true, endpoint: 'https://invented.example.test' } }, 422)
  assert.equal((await wire()).length, 0)
  await record('MOVE-source-and-headers-do-not-install-workspace-evidence')
  await f.close()

  preview = await prepared()
  for (const headers of [{ 'X-Mailroom-Token': 'invented-invalid-session' }, { Origin: 'https://other.example.test' }, { Host: 'other.example.test' }, { 'Sec-Fetch-Site': 'cross-site' }]) {
    await f.raw('backlog/protocol', JSON.stringify({ account: f.account, command: { action: 'preview', options } }), 403, headers)
  }
  await f.raw('backlog/protocol', '{', 422)
  await f.post('backlog/protocol', { account: f.account, command: { action: 'preview', options, unexpected: true } }, 422)
  await f.post('backlog/protocol', { account: f.account, command: { action: 'preview', options }, padding: 'x'.repeat(400000) }, 413)
  const proof = await f.post('proof/preview', { account: f.account })
  const tampered = `${preview.receipt[0] === '0' ? '1' : '0'}${preview.receipt.slice(1)}`
  for (const [receipt, confirmed] of [[preview.receipt, false], [proof.receipt, true], [tampered, true]]) {
    await rejected('confirm', { options, receipt, confirmed })
    assert.equal((await f.command('status')).journal.runs.length, 0)
  }
  const workspace = await f.post('workspace/open', { account: f.account })
  workspace.workspace.input.documents.override_yaml += '\n# raw material changed\n'
  await f.post('workspace/save', { account: f.account, revision: workspace.revision, input: workspace.workspace.input })
  await rejected('confirm', { options, receipt: preview.receipt, confirmed: true })
  preview = await f.command('preview', { options })
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true }); await f.terminal()
  const run = (await f.command('status')).journal.runs.at(-1)
  await rejected('confirm', { options, receipt: preview.receipt, confirmed: true })
  const undo = await f.command('undo_preview', { run: run.id })
  await rejected('undo', { run: run.id, receipt: undo.receipt, confirmed: false })
  await rejected('undo', { run: run.id, receipt: preview.receipt, confirmed: true })
  await f.observe('later_flag')
  await rejected('undo', { run: run.id, receipt: undo.receipt, confirmed: true })
  const current = await f.command('undo_preview', { run: run.id })
  await f.command('undo', { run: run.id, receipt: current.receipt, confirmed: true }); await f.terminal()
  await rejected('undo', { run: run.id, receipt: current.receipt, confirmed: true })
  assert.equal((await wire()).length, 2)
  await record('unconfirmed-purpose-tamper-stale-material-replay-and-current-flags')
  await f.close()

  const partial = await forward()
  await f.observe('moved_away'); await f.observe('later_flag')
  const eligible = await f.command('undo_preview', { run: partial.id })
  assert.equal(eligible.conflicts, 1)
  assert.equal(Object.keys(eligible.reverse[0].evidence.intent.messages).length, 2)
  await f.command('undo', { run: partial.id, receipt: eligible.receipt, confirmed: true }); await f.terminal()
  const reversed = await f.observe()
  assert.equal(reversed.journal.runs.at(-1).undo_conflicts, 1)
  assert.deepEqual(reversed.folders['Personal/Studio'].uids, [40])
  assert.equal(reversed.folders.INBOX.uids.length, 3)
  assert.ok(Object.values(reversed.folders.INBOX.messages).some(m => m.flags.includes('$later-client')))
  await record('per-message-conditional-Undo-retains-conflicts-and-current-flags')
  await f.close()

  const stale = await forward()
  await f.observe('original_validity')
  const conflict = await f.command('undo_preview', { run: stale.id })
  assert.equal(conflict.conflicts, 3); assert.equal(conflict.reverse.length, 0)
  await rejected('undo', { run: stale.id, receipt: conflict.receipt, confirmed: true })
  await record('changed-original-UIDVALIDITY-is-conflict-with-no-reverse')
  await f.close()

  for (const boundary of ['move_undo_consumed', 'move_before_intent', 'move_attempted', 'move_provider_effect', 'move_parsed_mapping', 'move_saved_mapping', 'move_completion', 'move_readback', 'move_saved_readback']) {
    const run = await forward()
    const undo = await f.command('undo_preview', { run: run.id })
    await writeFile(join(f.directory, 'move-boundary'), boundary)
    await f.command('undo', { run: run.id, receipt: undo.receipt, confirmed: true })
    await f.reached(boundary)
    await f.stop('SIGKILL')
    await writeFile(join(f.directory, 'move-boundary'), '')
    const independent = await f.observe()
    const count = (await wire()).length
    await f.start()
    const reopened = await f.command('status')
    assert.ok(reopened.journal.runs.at(-1).undo_receipt)
    assert.equal((await wire()).length, count)
    await f.command('reconcile', { run: run.id })
    const first = await f.observe()
    await f.command('reconcile', { run: run.id })
    const second = await f.observe()
    assert.equal(second.revision, first.revision)
    assert.deepEqual(second.folders, independent.folders)
    const safe = ['move_undo_consumed', 'move_before_intent'].includes(boundary)
    const mapped = ['move_saved_mapping', 'move_completion', 'move_readback', 'move_saved_readback'].includes(boundary)
    assert.equal(second.journal.runs.at(-1).status, mapped ? 'undone' : safe ? 'stopped' : 'uncertain')
    if (safe) {
      await f.command('resume_undo', { run: run.id }); await f.terminal()
      assert.equal((await f.command('status')).journal.runs.at(-1).status, 'undone')
      assert.equal((await wire()).length, count + 1)
    } else {
      await f.command('resume_undo', { run: run.id }); await f.terminal()
      assert.equal((await wire()).length, count)
    }
    await record('reverse-actual-SIGKILL-observe-only-reconcile-no-duplicate', { boundary, mapped, safe })
    await f.close()
  }
  }

  preview = await prepared()
  const before = await f.observe()
  await writeFile(join(f.directory, 'move-mode'), 'slow')
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  for (let i = 0; i < 1000; i++) {
    if (await readFile(join(f.directory, 'move-effect-reached')).then(() => true, () => false)) break
    await pause(10)
  }
  await f.post('scan', { account: f.account }, 422)
  const ledgerBytes = await readFile(join(f.directory, 'ledger/addresses.yaml'))
  await writeFile(join(f.directory, 'ledger/addresses.yaml'), 'invalid: [')
  await writeFile(join(f.directory, 'move-boundary'), 'move_cancel_before_save')
  const stopping = f.command('cancel')
  await f.reached('move_cancel_before_save')
  await f.command('resume', { run: preview.operation.id }, 422)
  await f.post('scan', { account: f.account }, 422)
  await writeFile(join(f.directory, 'continue-move_cancel_before_save'), 'continue')
  await stopping
  const cancelled = await f.observe()
  assert.equal(cancelled.journal.runs.at(-1).stop_requested, true)
  assert.equal(cancelled.journal.runs.at(-1).status, 'uncertain')
  assert.equal(cancelled.journal.runs.at(-1).undo_receipt, null)
  assert.equal(cancelled.unrelated, before.unrelated)
  assert.equal((await wire()).length, 1)
  await writeFile(join(f.directory, 'invalid-ledger-preserved.yaml'), await readFile(join(f.directory, 'ledger/addresses.yaml')))
  await writeFile(join(f.directory, 'ledger/addresses.yaml'), ledgerBytes)
  await f.stop('SIGKILL'); await f.start()
  const stopped = await f.command('status')
  assert.equal(stopped.journal.runs.at(-1).stop_requested, true)
  assert.equal((await wire()).length, 1)
  await record('live-socket-Cancel-survives-broken-ledger-and-replacement-with-no-Undo')
  await f.close()

  await f.start(true, true, { large: true }); await f.savedWorkspace()
  const rotation = await f.command('preview', { options })
  const preserved = await f.observe()
  await writeFile(join(f.directory, 'move-boundary'), 'move_between_batches')
  await f.command('confirm', { options, receipt: rotation.receipt, confirmed: true })
  await f.reached('move_between_batches')
  const rotating = f.post('lifecycle', { action: 'rotate', account: f.account, confirmed: true })
  await pause(100)
  await writeFile(join(f.directory, 'continue-move_between_batches'), 'continue')
  await rotating
  const rotated = await f.observe()
  assert.equal(rotated.unrelated, preserved.unrelated)
  assert.equal(rotated.independent, preserved.independent)
  assert.deepEqual(rotated.independent_effects.slice(1), preserved.independent_effects.slice(1))
  const retained = rotated.journal.runs.at(-1)
  assert.equal(retained.id, rotation.operation.id)
  assert.equal(retained.stop_requested, true)
  assert.equal(retained.batches[1].attempted, false)
  assert.equal(retained.undo_receipt, null)
  await rejected('resume', { run: retained.id })
  assert.equal((await wire()).length, 1)
  const forgetting = await f.post('lifecycle', { action: 'forget_preview', account: f.account })
  await f.post('lifecycle', { action: 'forget', account: f.account, preview: forgetting, confirmed: true })
  for (const resource of forgetting.resources) assert.equal(await stat(resource.path).then(() => true, () => false), false)
  await record('rotation-joins-and-stops-worker-preserves-history-invalidates-Resume-and-exact-forget')
  console.log(`Passed ${checks.length} protocol safety groups`)
} finally { await f.close() }
