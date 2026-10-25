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
  await writeFile(join(root, '.audit/imap-backlog-workflow-admission-results.json'), JSON.stringify({ checks }, null, 2))
}
try {
  await f.start(true); await f.savedWorkspace()
  const workspace = await f.post('workspace/open', { account: f.account })
  const input = workspace.workspace.input
  input.documents.override_yaml = input.documents.override_yaml.replace('    unknown@work.example.test: unknown', '    unknown@work.example.test: unknown\n    owner@home.example.test: unknown')
  assert.ok(input.documents.override_yaml.includes('owner@home.example.test: unknown'))
  await f.post('workspace/save', { account: f.account, revision: workspace.revision, input })
  const proof = await f.post('proof/preview', { account: f.account })
  const exported = await f.post('proof/download', { account: f.account, receipt: proof.receipt, confirmed: true })
  const before = await f.observe()
  await f.command('preview', { options }, 422)
  const debug = JSON.parse(await readFile(join(f.directory, 'move-preview-debug.json'), 'utf8'))
  assert.equal(debug.execution.artifact.identity, exported.identity)
  const held = debug.execution.decisions.filter(d => /^INBOX\/101\/(4|8|19)$/.test(d.id))
  assert.equal(held.length, 3)
  assert.ok(held.every(d => d.review && d.destination.kind === 'inbox' && !d.mark_read && d.copies.length === 0 && d.keyword_additions.length === 0))
  const after = await f.observe()
  assert.equal(after.primary, before.primary)
  assert.deepEqual(after.folders, before.folders)
  assert.equal(await wires(), '')
  await record('unknown-recipient-role-holds-actual-observed-proof-with-zero-executable-moves')
  await f.close()

  for (const change of ['cross_account', 'scope_mailboxes', 'original_validity', 'destination_validity']) {
    await f.start(true); await f.savedWorkspace()
    const preview = await f.command('preview', { options })
    let peer
    if (change === 'cross_account') {
      await f.post('synthetic-gmail', {})
      const job = await f.terminal()
      assert.equal(job.progress.state, 'complete')
      peer = job.account.id
    } else await f.observe(change)
    const before = await f.observe()
    if (peer) {
      await f.post('backlog/protocol', { account: peer, command: { action: 'confirm', options, receipt: preview.receipt, confirmed: true } }, 422)
    } else {
      await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
      assert.equal((await f.terminal()).progress.state, 'failed')
    }
    const after = await f.observe()
    assert.equal(after.primary, before.primary)
    assert.deepEqual(after.folders, before.folders)
    assert.equal(after.journal.runs.length, 0)
    assert.equal(await wires(), '')
    await record(`${change}-after-preview-rejects-before-approval-consumption-or-MOVE`)
    await f.close()
  }
  console.log(`Passed ${checks.length} observed role and exact approval admission groups`)
} finally { await f.close() }
