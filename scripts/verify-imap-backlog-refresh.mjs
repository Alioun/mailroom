import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
try {
  await f.start(true, true, { ledger: true }); await f.savedWorkspace(true)
  const preview = await f.command('preview', { options })
  const before = await f.observe()
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const moved = await f.observe()
  assert.equal(moved.unrelated, before.unrelated)
  const wire = await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')
  await f.post('scan', { account: f.account })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const refreshed = await f.observe()
  assert.deepEqual(refreshed.saved_census, Object.fromEntries(Object.entries(refreshed.folders).map(([name, folder]) => [name, folder.uids])))
  assert.equal(refreshed.non_census, moved.non_census)
  assert.deepEqual(refreshed.journal, moved.journal)
  assert.deepEqual(refreshed.exports, moved.exports)
  assert.deepEqual(refreshed.folders, moved.folders)
  assert.equal(refreshed.independent, moved.independent)
  assert.deepEqual(refreshed.independent_effects, moved.independent_effects)
  assert.equal(await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8'), wire)
  await f.stop('SIGKILL'); await f.start()
  const reopened = await f.observe()
  assert.equal(reopened.primary, refreshed.primary)
  assert.deepEqual((await f.command('status')).journal, refreshed.journal)
  await writeFile(join(root, '.audit/imap-backlog-workflow-refresh-results.json'), JSON.stringify({ directory: f.directory, saved_census: refreshed.saved_census, complete_non_census_equal: true, journal_equal: true, exports_equal: true, independent_effects_equal: true, replacement_equal: true }, null, 2))
  console.log('Passed actual read-only TLS refresh and whole-state preservation')
} finally { await f.close() }
