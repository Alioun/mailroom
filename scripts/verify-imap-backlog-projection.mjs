import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
try {
  await f.start(true)
  const workspace = await f.post('workspace/open', { account: f.account })
  const input = workspace.workspace.input
  input.documents.override_yaml = input.documents.override_yaml
    .replace('    unknown@work.example.test: unknown', '    unknown@work.example.test: unknown\n    owner@home.example.test: business')
    .replace('mark_read: null', 'mark_read: true')
    .replace('billing@shop.example.org', 'updates@studio.example.com')
    + '\nlayout:\n  language: en\n  personal_root: Fixture personal\n  business_root: Personal\n  new_addresses: New addresses\n  retired_addresses: Retired addresses\n'
  await writeFile(join(f.directory, 'projection-overrides.yaml'), input.documents.override_yaml)
  await f.post('workspace/save', { account: f.account, revision: workspace.revision, input })
  const proof = await f.post('proof/preview', { account: f.account })
  const exported = await f.post('proof/download', { account: f.account, receipt: proof.receipt, confirmed: true })
  const preview = await f.command('preview', { options })
  assert.equal(preview.operation.plan.filing.artifact.identity, exported.identity)
  assert.equal(preview.operation.plan.filing.artifact.executable, exported.sieve)
  const decisions = preview.execution.decisions.filter(d => /^INBOX\/101\/(4|8|19)$/.test(d.id))
  assert.equal(decisions.length, 3)
  const predictedCopies = decisions.reduce((count, d) => count + d.copies.length, 0)
  assert.ok(predictedCopies > 0)
  assert.ok(decisions.every(d => d.mark_read === true))
  assert.ok(preview.operation.plan.filing.changes.every(c => c.operation.kind === 'imap' && c.operation.method === 'uid_move'))
  assert.ok(preview.operation.plan.filing.diagnostics.some(d => /cop|read|keyword/i.test(d)))
  const before = await f.observe()
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  const after = await f.observe()
  assert.equal(after.unrelated, before.unrelated)
  assert.equal(after.independent, before.independent)
  assert.deepEqual(after.independent_effects.slice(1), before.independent_effects.slice(1))
  const wire = (await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse)
  assert.equal(wire.length, 1)
  assert.ok(wire[0].command.startsWith('UID MOVE 4,8,19 '))
  for (const [source, destination] of [[4, 60], [8, 64], [19, 68]]) {
    assert.deepEqual(after.folders['Personal/Studio'].messages[destination], before.folders.INBOX.messages[source])
  }
  await writeFile(join(root, '.audit/imap-backlog-workflow-projection-results.json'), JSON.stringify({ directory: f.directory, artifact: exported.identity, actual_predicted_copies: predictedCopies, actual_predicted_read_changes: decisions.filter(d => d.mark_read).length, emitted_move_commands: wire.length, flags_and_dates_equal: true, unrelated_and_independent_equal: true }, null, 2))
  console.log('Passed actual saved-artifact accounting-copy and read projection separation')
} finally { await f.close() }
