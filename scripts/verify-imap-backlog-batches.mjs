import assert from 'node:assert/strict'
import { readFile, writeFile, rename } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root, pause } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = []
const wire = async () => (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse)
async function record(name) {
  checks.push({ name, directory: f.directory, passed: true })
  await writeFile(join(root, '.audit/imap-backlog-workflow-batch-results.json'), JSON.stringify({ checks }, null, 2))
}
async function unchangedReconcile(id) {
  const before = await f.observe()
  const commands = await wire()
  await f.command('reconcile', { run: id })
  await f.command('reconcile', { run: id })
  const after = await f.observe()
  assert.equal(after.primary, before.primary)
  assert.deepEqual(after.folders, before.folders)
  assert.deepEqual(await wire(), commands)
}
async function replaceAtBatch() {
  await f.reached('move_between_batches')
  await f.stop('SIGKILL')
  await writeFile(join(f.directory, 'move-boundary'), '')
  const before = await f.observe()
  await f.start()
  const status = await f.command('status')
  assert.equal(status.journal.runs.at(-1).status, 'stopped')
  assert.equal(status.journal.runs.at(-1).stop_requested, true)
  assert.deepEqual((await f.observe()).folders, before.folders)
}
try {
  for (const reverseStop of ['SIGKILL', 'Cancel']) {
    await f.start(true, true, { large: true }); await f.savedWorkspace()
    const before = await f.observe()
    const preview = await f.command('preview', { options })
    assert.equal(preview.operation.plan.filing.changes.length, 501)
    if (reverseStop === 'SIGKILL') await writeFile(join(f.directory, 'move-boundary'), 'move_between_batches')
    await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
    if (reverseStop === 'SIGKILL') {
      await replaceAtBatch()
      const saved = await f.command('status')
      assert.deepEqual(saved.journal.runs.at(-1).batches.map(e => e.attempted), [true, false])
      assert.equal(saved.journal.runs.at(-1).undo_receipt, null)
      await unchangedReconcile(preview.operation.id)
      await f.command('resume', { run: preview.operation.id })
    }
    assert.equal((await f.terminal()).progress.state, 'complete')
    assert.equal((await wire()).length, 2)
    await f.observe('later_flag')
    const undo = await f.command('undo_preview', { run: preview.operation.id })
    assert.equal(undo.reverse.length, 2)
    if (reverseStop === 'SIGKILL') await rename(join(f.directory, 'reached-move_between_batches'), join(f.directory, 'forward-reached-move_between_batches'))
    await writeFile(join(f.directory, 'move-boundary'), 'move_between_batches')
    await f.command('undo', { run: preview.operation.id, receipt: undo.receipt, confirmed: true })
    if (reverseStop === 'Cancel') {
      await f.reached('move_between_batches')
      const cancel = f.command('cancel')
      await pause(100)
      await writeFile(join(f.directory, 'continue-move_between_batches'), 'continue')
      await cancel
      await f.stop('SIGKILL')
      await writeFile(join(f.directory, 'move-boundary'), '')
      await f.start()
    } else await replaceAtBatch()
    const stopped = (await f.command('status')).journal.runs.at(-1)
    assert.equal(stopped.status, 'stopped')
    assert.equal(stopped.undo_receipt, undo.receipt)
    assert.deepEqual(stopped.reverse.map(r => r.evidence.attempted), [true, false])
    assert.equal((await wire()).length, 3)
    await unchangedReconcile(stopped.id)
    await f.command('resume_undo', { run: stopped.id })
    assert.equal((await f.terminal()).progress.state, 'complete')
    const after = await f.observe()
    assert.equal(after.journal.runs.at(-1).status, 'undone')
    assert.equal(after.journal.runs.at(-1).undo_receipt, undo.receipt)
    assert.equal(after.folders.INBOX.uids.length, 502)
    assert.ok(after.folders.INBOX.messages['2000'].flags.includes('$later-client'))
    assert.deepEqual(after.folders.INBOX.messages['99'], before.folders.INBOX.messages['99'])
    assert.deepEqual(after.folders['Personal/Studio'], before.folders['Personal/Studio'])
    assert.equal(after.unrelated, before.unrelated)
    assert.equal(after.independent, before.independent)
    const commands = await wire()
    assert.equal(commands.length, 4)
    assert.deepEqual(commands.map(c => c.command.split(' ')[2].split(',').length), [500, 1, 500, 1])
    await record(`501-forward-and-confirmed-reverse-${reverseStop}-between-batches-replacement-safe-remainder`)
    await f.close()
  }
  console.log(`Passed ${checks.length} actual 501-message forward and reverse batch replacement groups`)
} finally { await f.close() }
