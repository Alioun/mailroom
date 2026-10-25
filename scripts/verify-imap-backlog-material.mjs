import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = []
const wires = async () => readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8').catch(() => '')
async function record(name) {
  checks.push({ name, directory: f.directory, passed: true })
  await writeFile(join(root, '.audit/imap-backlog-workflow-material-results.json'), JSON.stringify({ checks }, null, 2))
}
async function edit(kind) {
  const workspace = await f.post('workspace/open', { account: f.account })
  const input = workspace.workspace.input
  if (kind === 'baseline') input.baseline = 'previous'
  if (kind === 'as_of') input.as_of -= 86400
  if (kind === 'pins') {
    const observed = await f.observe('provider_only')
    input.pins = [{ account: f.account, message_id_hash: observed.folders.INBOX.messages['4'].hash, expected: { kind: 'folder', name: 'Personal/Studio' } }]
  }
  if (kind === 'documents') input.documents.override_yaml += '\n# changed immutable input\n'
  await f.post('workspace/save', { account: f.account, revision: workspace.revision, input })
}
try {
  for (const kind of ['baseline', 'as_of', 'pins', 'documents', 'options', 'workspace_authority']) {
    await f.start(true); await f.savedWorkspace()
    const preview = await f.command('preview', { options })
    if (kind === 'workspace_authority') {
      await f.stop('SIGKILL')
      await writeFile(join(f.directory, 'workspace-authority'), 'replacement-workspace-evidence-v2')
      await f.start()
    } else if (kind !== 'options') await edit(kind)
    const before = await f.observe()
    await f.command('confirm', { options: kind === 'options' ? { ...options, protect_flagged: false } : options, receipt: preview.receipt, confirmed: true })
    assert.equal((await f.terminal()).progress.state, 'failed')
    const after = await f.observe()
    assert.equal(after.primary, before.primary)
    assert.deepEqual(after.folders, before.folders)
    assert.equal(after.journal.runs.length, 0)
    assert.equal(await wires(), '')
    await record(`stale-${kind}-approval-rejected-before-consumption-or-MOVE`)
    await f.close()
  }
  for (const kind of ['baseline', 'as_of', 'pins', 'documents', 'workspace_authority']) {
    await f.start(true); await f.savedWorkspace()
    const preview = await f.command('preview', { options })
    await writeFile(join(f.directory, 'move-boundary'), 'move_consumed')
    await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
    await f.reached('move_consumed'); await f.stop('SIGKILL')
    await writeFile(join(f.directory, 'move-boundary'), '')
    if (kind === 'workspace_authority') await writeFile(join(f.directory, 'workspace-authority'), 'replacement-workspace-evidence-v2')
    await f.start()
    if (kind !== 'workspace_authority') await edit(kind)
    const before = await f.observe()
    await f.command('resume', { run: preview.operation.id })
    assert.equal((await f.terminal()).progress.state, 'failed')
    const after = await f.observe()
    assert.equal(after.journal.runs.at(-1).status, 'conflict')
    assert.equal(after.journal.runs.at(-1).id, preview.operation.id)
    assert.deepEqual(after.journal.runs.at(-1).plan, before.journal.runs.at(-1).plan)
    assert.deepEqual(after.journal.runs.at(-1).batches, before.journal.runs.at(-1).batches)
    assert.deepEqual(after.folders, before.folders)
    assert.equal(after.unrelated, before.unrelated)
    assert.equal(await wires(), '')
    await record(`replacement-${kind}-invalidates-safe-remainder-with-history-retained`)
    await f.close()
  }
  console.log(`Passed ${checks.length} material approval and continuation groups`)
} finally { await f.close() }
