import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './imap-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const checks = []
const recovery = body => f.post('recovery', body)
async function sort() {
  const preview = await f.command('preview', { options })
  await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
  return (await f.command('status')).journal.runs.at(-1)
}
async function undo(run) {
  const preview = await f.command('undo_preview', { run: run.id })
  await f.command('undo', { run: run.id, receipt: preview.receipt, confirmed: true })
  assert.equal((await f.terminal()).progress.state, 'complete')
}
async function create(gmail = false) {
  await f.post(gmail ? 'synthetic-gmail' : 'synthetic', {})
  const job = await f.terminal()
  assert.equal(job.progress.state, 'complete')
  const account = job.account.id
  return account
}
try {
  for (const selected of [false, true]) {
    await f.start(true, true, { ledger: true }); await f.savedWorkspace(true)
    await undo(await sort())
    const run = await sort()
    const peer = selected ? await create(true) : null
    const excluded = selected ? await create() : null
    const before = await f.observe()
    assert.ok(Object.values(before.populated).every(Boolean))
    assert.equal(before.journal.runs.length, 2)
    assert.equal(before.journal.runs[0].status, 'undone')
    assert.equal(before.journal.runs[0].reverse[0].evidence.mapping.kind, 'validated')
    const accounts = selected ? [f.account, peer].sort() : [f.account]
    const offer = await recovery(selected ? { action: 'selected_capture_preview', accounts, include_ledger: true } : { action: 'capture_preview', account: f.account })
    const manifests = selected ? offer.preview.accounts : [offer.preview]
    assert.ok(manifests.every(m => m.state_schema === 16))
    assert.equal(manifests.find(m => m.account === f.account).resources.find(r => r.resource === 'local_encrypted_state').sha256, before.ciphertext)
    if (selected) assert.ok(offer.preview.excluded_accounts.includes(excluded))
    const captured = await recovery({ action: selected ? 'selected_capture' : 'capture', preview: offer.preview, receipt: offer.receipt })
    await f.observe('later_flag')
    await undo(run)
    const later = await f.observe()
    assert.notDeepEqual(later.folders, before.folders)
    const wire = await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8')
    await f.stop('SIGKILL')
    const installation = join(f.directory, `${selected ? 'selected' : 'single'}-restored`)
    await mkdir(installation)
    f.restoreInto(installation); await f.start()
    const survivor = selected ? await create() : null
    const restore = await recovery(selected ? { action: 'selected_restore_preview', bundle: captured.bundle, restore_ledger: true } : { action: 'restore_preview', bundle: captured.bundle })
    assert.equal(restore.authenticated, true)
    await recovery({ action: selected ? 'selected_restore' : 'restore', preview: restore.preview, receipt: restore.receipt })
    const after = await f.observe()
    assert.equal(after.primary, before.primary)
    assert.equal(after.ciphertext, before.ciphertext)
    assert.equal(after.unrelated, before.unrelated)
    assert.deepEqual(after.journal, before.journal)
    assert.deepEqual(after.exports, before.exports)
    assert.deepEqual(after.folders, later.folders)
    assert.equal(after.independent, later.independent)
    assert.deepEqual(after.independent_effects, later.independent_effects)
    assert.equal(after.recovery_hold, true)
    const status = await f.command('status')
    assert.equal(status.installed, false); assert.equal(status.recovery_hold, true)
    for (const action of ['preview', 'confirm', 'reconcile', 'resume', 'undo_preview', 'undo', 'resume_undo', 'cancel']) {
      await f.command(action, { options, run: run.id, receipt: run.receipt, confirmed: true }, 403)
    }
    if (selected) {
      const installed = await f.post('list', {})
      assert.ok(installed.some(a => a.id === survivor))
      await f.post('workspace/open', { account: excluded }, 422)
      const peerStatus = await f.post('backlog/protocol', { account: peer, command: { action: 'status' } })
      assert.equal(peerStatus.recovery_hold, true)
    }
    await f.stop('SIGKILL'); await f.start()
    assert.deepEqual((await f.command('status')).journal, before.journal)
    assert.equal(await readFile(join(f.directory, 'move-wire-exact.jsonl'), 'utf8'), wire)
    checks.push({ name: selected ? 'populated-selected-schema16-restoration' : 'populated-single-schema16-restoration', directory: f.directory, installation, accounts, journal_ids: before.journal.runs.map(r => r.id), ciphertext: after.ciphertext, complete_primary_equal: true, independent_effects_equal: true, held_without_source: true })
    await writeFile(join(root, '.audit/imap-backlog-workflow-recovery-results.json'), JSON.stringify({ checks }, null, 2))
    await f.close()
  }
  console.log(`Passed ${checks.length} populated protocol recovery groups`)
} finally { await f.close() }
