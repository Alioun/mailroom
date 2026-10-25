import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
try {
  await f.start(true)
  await f.savedWorkspace()
  const preview = await f.command('preview', { options })
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const verified = await f.observe()
  const run = verified.journal.runs.at(-1)
  await f.observe('later_flag')
  await f.command('reconcile', { run: run.id })
  await f.command('reconcile', { run: run.id })
  const current = await f.observe()
  assert.equal(current.journal.runs.at(-1).status, 'complete')
  assert.deepEqual(current.journal, verified.journal)
  assert.equal(current.revision, verified.revision)
  const undo = await f.command('undo_preview', { run: run.id })
  assert.equal(undo.conflicts, 0)
  assert.equal(Object.keys(undo.reverse[0].evidence.intent.messages).length, 3)
  await f.command('undo', { run: run.id, receipt: undo.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const after = await f.observe()
  assert.equal(after.journal.runs.at(-1).status, 'undone')
  assert.ok(after.folders.INBOX.messages['104'].flags.includes('$later-client'))
  assert.deepEqual(after.folders['Personal/Studio'].messages['40'], verified.folders['Personal/Studio'].messages['40'])
  assert.equal(after.unrelated, verified.unrelated)
  assert.equal(after.independent, verified.independent)
  const wire = (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')).trim().split('\n')
  assert.equal(wire.length, 2)
  await writeFile(join(root, '.audit/imap-backlog-workflow-reconcile-results.json'), JSON.stringify({ directory: f.directory, verified_evidence_unchanged: true, reconciliation_revision_unchanged: true, fresh_flags_preserved_by_confirmed_undo: true, move_commands: wire.length }, null, 2))
  console.log('Passed immutable verified filing evidence and separately confirmed current-flag Undo after Reconcile')
} finally { await f.close() }
