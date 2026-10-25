import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
try {
  await f.start(true)
  await f.savedWorkspace()
  await f.observe('scope_mailboxes')
  const inbox = await f.command('preview', { options: { ...options, scope: 'inbox' } })
  assert.deepEqual(inbox.operation.plan.filing.changes.map(c => [c.source.folder, c.source.uid]), [['INBOX', 4]])
  const unprotected = await f.command('preview', { options: { ...options, scope: 'inbox', protect_flagged: false } })
  assert.deepEqual(unprotected.operation.plan.filing.changes.map(c => c.source.uid), [4, 8])
  const unfiled = await f.command('preview', { options })
  assert.deepEqual(unfiled.operation.plan.filing.changes.map(c => [c.source.folder, c.source.uid]), [['Archive evidenced', 4], ['INBOX', 4]])
  assert.ok(!unfiled.operation.plan.filing.diagnostics.some(d => d.startsWith('Unfiled scope unsupported')))
  const advanced = await f.command('preview', { options: { ...options, scope: 'every_folder' } })
  assert.deepEqual(advanced.operation.plan.filing.changes.map(c => [c.source.folder, c.source.uid]), [['Archive evidenced', 4], ['INBOX', 4], ['Other observed', 4]])
  assert.equal(advanced.operation.plan.filing.changes.some(c => c.source.folder === 'Mixed observed'), false)
  const before = await f.observe()
  await f.command('confirm', { options, receipt: unfiled.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const after = await f.observe()
  assert.deepEqual(after.folders.INBOX.uids, [8, 19, 99])
  assert.deepEqual(after.folders['Archive evidenced'].uids, [])
  for (const name of ['Sent observed', 'Drafts observed', 'Spam observed', 'Trash observed', 'Mixed observed', 'Other observed']) {
    assert.deepEqual(after.folders[name], before.folders[name])
  }
  for (const uid of [8, 19, 99]) assert.deepEqual(after.folders.INBOX.messages[uid], before.folders.INBOX.messages[uid])
  assert.equal(after.unrelated, before.unrelated)
  assert.equal(after.independent, before.independent)
  assert.deepEqual(after.independent_effects.slice(1), before.independent_effects.slice(1))
  const wire = (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse)
  assert.deepEqual(wire.map(c => [c.source, c.command]), [['Archive evidenced', 'UID MOVE 4 "Personal/Studio"'], ['INBOX', 'UID MOVE 4 "Personal/Studio"']])
  await writeFile(join(root, '.audit/imap-backlog-workflow-scope-results.json'), JSON.stringify({ directory: f.directory, inbox: 1, inbox_unprotected: 2, inbox_unfiled: 2, advanced: 3, mixed_exclusion_wins: true, protected_and_excluded_unchanged: true, exact_sparse_commands: true }, null, 2))
  console.log('Passed observed Archive role, scope, mixed exclusions and protected exact sparse MOVE')
} finally { await f.close() }
