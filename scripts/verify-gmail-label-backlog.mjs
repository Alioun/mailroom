import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fixture, root } from './gmail-backlog-fixture.mjs'

const f = await fixture()
const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
const personal = { ...options, selected: ['Privat/Studio'] }
const only = process.argv.slice(2)
const checks = []
const results = join(root, '.audit/gmail-label-backlog-http-results.json')
async function record(name, extra = {}) {
  checks.push({ name, directory: f.directory, ...extra })
  await writeFile(results, JSON.stringify({ checks }, null, 2))
}
const labels = (o, id) => [...o.all_mail.messages[id].labels].sort()
const flags = (o, id) => [...o.all_mail.messages[id].flags].sort()
const wires = o => o.wires.map(w => w.command)
const run = async () => (await f.command('status')).journal.runs.at(-1)
async function fresh(profile = {}, installed = true) {
  await f.start(true, installed, profile)
  if (installed) await f.savedWorkspace()
}
async function confirm(opts = options) {
  const preview = await f.command('preview', { options: opts })
  assert.deepEqual(await f.command('confirm', { options: opts, receipt: preview.receipt, confirmed: true }), { started: true })
  return preview
}
async function worker(action, fields) {
  assert.deepEqual(await f.command(action, fields), { started: true })
  return (await f.terminal()).progress.state
}
async function undo() {
  const id = (await run()).id
  const preview = await f.command('undo_preview', { run: id })
  assert.equal(await worker('undo', { run: id, receipt: preview.receipt, confirmed: true }), 'complete')
  return preview
}
const wanted = name => only.length === 0 || only.includes(name)

try {
  if (wanted('authority')) {
    await fresh({}, false)
    const proof = await f.post('proof/preview', { account: f.account })
    assert.equal(proof.proof.artifact.context.recipient_evidence, 'unknown')
    await f.command('preview', { options }, 422)
    await f.command('preview', { options, endpoint: '127.0.0.1:993', trust: true }, 422)
    assert.equal((await f.observe()).wires.length, 0)
    await record('gmail-source-alone-grants-no-recipient-authority')
    await f.close()
  }

  if (wanted('forward')) {
    await fresh()
    const before = await f.observe()
    assert.deepEqual(before.capabilities.includes('X-GM-EXT-1'), true)
    await f.post('backlog/protocol', { account: f.account, command: { action: 'preview', options } }, 403)
    for (const headers of [{ 'X-Mailroom-Token': 'invented-invalid-session' }, { Origin: 'https://other.example.test' }, { Host: 'other.example.test' }, { 'Sec-Fetch-Site': 'cross-site' }]) {
      await f.raw('backlog/gmail', JSON.stringify({ account: f.account, command: { action: 'preview', options } }), 403, headers)
    }
    await f.raw('backlog/gmail', '{', 422)
    await f.post('backlog/gmail', { account: 'uninstalled-account', command: { action: 'preview', options } }, 422)
    const preview = await f.command('preview', { options })
    const plan = preview.operation.plan.filing
    assert.deepEqual(plan.changes.map(c => c.source.gmail_id).sort(), ['1001', '1002', '1003', '1005', '1006'])
    assert.deepEqual(plan.excluded, { excluded_role: 1, flagged: 1, outside_scope: 1 })
    assert.equal(plan.unmatched, 1)
    assert.equal(plan.provider, 'gmail')
    assert.equal(preview.operation.plan.saved_proof, (await f.post('proof/preview', { account: f.account })).receipt)
    assert.deepEqual(preview.operation.batches.map(b => [b.intent.destination.decoded, b.intent.context.name.decoded, b.steps.map(s => [s.kind, s.uids])]), [
      ['Geschäftlich/Studio', '[Gmail]/All Mail', [['add_label', [31]], ['remove_inbox', [31]]]],
      ['Privat/Studio', '[Gmail]/All Mail', [['add_label', [4, 8, 30]], ['remove_inbox', [4, 8, 19]]]],
    ])
    const tampered = `${preview.receipt[0] === '0' ? '1' : '0'}${preview.receipt.slice(1)}`
    for (const [receipt, confirmed] of [[preview.receipt, false], [tampered, true]]) {
      assert.equal(await worker('confirm', { options, receipt, confirmed }), 'failed')
      assert.equal((await f.command('status')).journal.runs.length, 0)
    }
    assert.equal(await worker('confirm', { options, receipt: preview.receipt, confirmed: true }), 'complete')
    const after = await f.observe()
    assert.deepEqual(wires(after), [
      'UID STORE 31 +X-GM-LABELS ("Gesch&AOQ-ftlich/Studio")',
      'UID STORE 31 -X-GM-LABELS (\\Inbox)',
      'UID STORE 4,8,30 +X-GM-LABELS ("Privat/Studio")',
      'UID STORE 4,8,19 -X-GM-LABELS (\\Inbox)',
    ])
    for (const [id, want] of [['1001', ['Privat/Studio']], ['1002', ['Andere', 'Privat/Studio']], ['1003', ['Privat/Studio']], ['1004', ['\\Inbox']], ['1005', ['Privat/Studio']], ['1006', ['Geschäftlich/Studio', '\\Important']], ['1007', ['\\Sent']], ['1008', ['\\Inbox']], ['1009', ['Andere']]]) {
      assert.deepEqual(labels(after, id), want, id)
      assert.deepEqual(flags(after, id), flags(before, id), id)
      assert.equal(after.all_mail.messages[id].uid, before.all_mail.messages[id].uid, id)
    }
    assert.deepEqual(Object.keys(after.inbox.messages).sort(), ['1004', '1008'])
    assert.equal(after.all_mail.uidvalidity, before.all_mail.uidvalidity)
    assert.deepEqual(after.mailboxes, before.mailboxes)
    assert.equal(after.unrelated, before.unrelated)
    assert.equal(after.forbidden, null)
    assert.equal(after.schema, 18)
    const status = await f.command('status')
    assert.equal(status.journal.runs[0].status, 'complete')
    assert.ok(Object.values(status.outcomes[status.journal.runs[0].id].forward).flat().every(step => Object.values(step).every(o => o === 'verified')))
    await f.command('reconcile', { run: status.journal.runs[0].id })
    const reconciled = await f.observe()
    await f.command('reconcile', { run: status.journal.runs[0].id })
    assert.equal((await f.observe()).revision, reconciled.revision)
    await f.observe('label:1001:Später')
    await f.observe('flag:1002')
    const undoPreview = await undo()
    assert.equal(undoPreview.conflicts, 0)
    const undone = await f.observe()
    assert.deepEqual(wires(undone).slice(4), [
      'UID STORE 31 +X-GM-LABELS (\\Inbox)',
      'UID STORE 31 -X-GM-LABELS ("Gesch&AOQ-ftlich/Studio")',
      'UID STORE 4,8,19 +X-GM-LABELS (\\Inbox)',
      'UID STORE 4,8,30 -X-GM-LABELS ("Privat/Studio")',
    ])
    for (const [id, want] of [['1001', ['Später', '\\Inbox']], ['1002', ['Andere', '\\Inbox']], ['1003', ['Privat/Studio', '\\Inbox']], ['1005', []], ['1006', ['\\Important', '\\Inbox']]]) assert.deepEqual(labels(undone, id), want, id)
    assert.ok(flags(undone, '1002').includes('\\Flagged'))
    assert.equal((await run()).status, 'undone')
    await f.post('scan', { account: f.account })
    assert.equal((await f.terminal()).progress.state, 'complete')
    const refreshed = await f.observe()
    assert.deepEqual(refreshed.saved_census['1001'], ['Später', '\\Inbox'])
    assert.deepEqual(refreshed.saved_census['1003'], ['Privat/Studio', '\\Inbox'])
    assert.deepEqual(refreshed.journal, undone.journal)
    assert.equal(refreshed.non_census, undone.non_census)
    assert.deepEqual(wires(refreshed), wires(undone))
    await f.stop('SIGKILL'); await f.start()
    assert.deepEqual((await f.command('status')).journal, (await f.observe()).journal)
    assert.deepEqual(wires(await f.observe()), wires(undone))
    await record('saved-workspace-label-forward-reconcile-refresh-undo', { wires: wires(undone) })
    await f.close()
  }

  if (wanted('stale')) {
    await fresh()
    const preview = await f.command('preview', { options })
    await f.observe('flag:1001')
    assert.equal(await worker('confirm', { options, receipt: preview.receipt, confirmed: true }), 'failed')
    const o = await f.observe()
    assert.equal(o.wires.length, 0)
    assert.equal(o.journal.runs?.length ?? 0, 0)
    const changed = await f.command('preview', { options })
    assert.equal(changed.operation.plan.filing.excluded.flagged, 2)
    await record('concurrent-flag-after-preview-rejects-before-effects')
    await f.close()
  }

  const boundaries = [
    ['gmail_attempted_add_label', 'uncertain', 'uncertain', 0, false],
    ['gmail_provider_effect', 'uncertain', 'uncertain', 1, false],
    ['gmail_completion_add_label', 'uncertain', 'stopped', 1, true],
    ['gmail_saved_readback_add_label', 'stopped', 'stopped', 1, true],
    ['gmail_completion_remove_inbox', 'uncertain', 'complete', 2, false],
  ]
  for (const [boundary, reopened, reconciled, effected, resumable] of boundaries) {
    if (!wanted(boundary)) continue
    await fresh()
    await f.boundary(boundary)
    await confirm(personal)
    await f.reached(boundary)
    await f.stop('SIGKILL')
    await f.boundary('')
    await f.start()
    const replaced = await f.observe()
    assert.equal(replaced.wires.length, effected, boundary)
    assert.equal((await run()).status, reopened, boundary)
    assert.equal((await f.observe()).wires.length, effected, `${boundary}: reopen executes nothing`)
    await f.command('reconcile', { run: (await run()).id })
    assert.equal((await run()).status, reconciled, boundary)
    if (resumable) {
      assert.equal(await worker('resume', { run: (await run()).id }), 'complete', boundary)
      assert.equal((await run()).status, 'complete', boundary)
    } else if (reconciled !== 'complete') {
      assert.equal(await worker('resume', { run: (await run()).id }), 'failed', boundary)
    }
    const final = await f.observe()
    const commands = wires(final)
    assert.equal(new Set(commands).size, commands.length, `${boundary}: no label store repeated`)
    if (reconciled === 'uncertain') {
      assert.equal(commands.length, effected, boundary)
      const undoPreview = await f.command('undo_preview', { run: (await run()).id })
      assert.equal(undoPreview.reverse.length, 0, boundary)
      assert.equal(undoPreview.conflicts, 3, boundary)
    } else {
      assert.deepEqual(commands, ['UID STORE 4,8,30 +X-GM-LABELS ("Privat/Studio")', 'UID STORE 4,8,19 -X-GM-LABELS (\\Inbox)'], boundary)
      assert.deepEqual(labels(final, '1001'), ['Privat/Studio'])
    }
    await record(`sigkill-${boundary}`, { reopened, reconciled, commands })
    await f.close()
  }

  if (wanted('undo_boundary')) {
    await fresh()
    await confirm(personal)
    assert.equal((await f.terminal()).progress.state, 'complete')
    const id = (await run()).id
    const preview = await f.command('undo_preview', { run: id })
    await f.boundary('gmail_completion_restore_inbox')
    await f.command('undo', { run: id, receipt: preview.receipt, confirmed: true })
    await f.reached('gmail_completion_restore_inbox')
    await f.stop('SIGKILL')
    await f.boundary('')
    await f.start()
    assert.equal((await run()).status, 'uncertain')
    await f.command('reconcile', { run: id })
    assert.equal((await run()).status, 'stopped')
    assert.equal(await worker('resume_undo', { run: id }), 'complete')
    const o = await f.observe()
    assert.equal((await run()).status, 'undone')
    assert.deepEqual(labels(o, '1001'), ['\\Inbox'])
    assert.deepEqual(labels(o, '1003'), ['Privat/Studio', '\\Inbox'])
    assert.equal(new Set(wires(o)).size, 4)
    await record('sigkill-undo-after-restore-before-readback', { commands: wires(o) })
    await f.close()
  }

  if (wanted('cancel')) {
    await fresh({ large: true })
    const preview = await f.command('preview', { options: personal })
    assert.equal(preview.operation.batches.length, 2)
    assert.equal(Object.keys(preview.operation.batches[0].intent.messages).length, 500)
    await f.boundary('gmail_saved_readback_add_label')
    await f.command('confirm', { options: personal, receipt: preview.receipt, confirmed: true })
    await f.reached('gmail_saved_readback_add_label')
    await f.boundary('gmail_cancel_signalled')
    const cancelling = f.command('cancel')
    await f.reached('gmail_cancel_signalled')
    await f.release('gmail_saved_readback_add_label')
    await f.release('gmail_cancel_signalled')
    await cancelling
    await f.boundary('')
    const stopped = await run()
    assert.equal(stopped.status, 'stopped')
    assert.equal(stopped.stop_requested, true)
    assert.equal(stopped.undo_receipt, null)
    assert.equal(stopped.batches[0].steps[1].attempted, false)
    let o = await f.observe()
    assert.equal(o.wires.length, 1)
    assert.equal(o.wires[0].command.split(' ')[2].split(',').length, 499)
    await f.stop('SIGKILL'); await f.start()
    assert.equal((await run()).status, 'stopped')
    assert.equal((await f.observe()).wires.length, 1)
    assert.equal(await worker('resume', { run: stopped.id }), 'complete')
    o = await f.observe()
    assert.equal(o.wires.length, 4)
    assert.equal(new Set(wires(o)).size, 4)
    assert.equal((await run()).undo_receipt, null)
    await record('live-cancel-501-replacement-and-safe-remainder', { commands: o.wires.map(w => w.command.slice(0, 40)) })
    await f.close()
  }

  if (wanted('labels')) {
    await fresh()
    await f.observe('drop_label:Geschäftlich/Studio')
    const mailboxes = (await f.observe()).mailboxes
    const preview = await f.command('preview', { options })
    assert.equal(preview.operation.plan.filing.excluded.missing_label, 1)
    assert.ok(preview.operation.plan.filing.diagnostics.some(d => d.includes('Geschäftlich/Studio')))
    await f.command('confirm', { options, receipt: preview.receipt, confirmed: true })
    assert.equal((await f.terminal()).progress.state, 'complete')
    const o = await f.observe()
    assert.deepEqual(o.mailboxes, mailboxes)
    assert.ok(wires(o).every(w => !w.includes('Gesch')))
    await f.observe('no_extension')
    await f.command('preview', { options }, 422)
    await record('missing-label-never-created-and-missing-extension-refused')
    await f.close()
  }

  if (wanted('recovery')) {
    const recovery = body => f.post('recovery', body)
    for (const selected of [false, true]) {
      await fresh()
      await confirm(personal)
      assert.equal((await f.terminal()).progress.state, 'complete')
      await undo()
      const forward = await confirm(personal)
      assert.equal((await f.terminal()).progress.state, 'complete')
      let peer = null
      if (selected) {
        await f.post('synthetic-gmail', {})
        const job = await f.terminal()
        assert.equal(job.progress.state, 'complete')
        peer = job.account.id
      }
      const before = await f.observe()
      assert.equal(before.journal.runs.length, 2)
      assert.equal(before.journal.runs[0].status, 'undone')
      const accounts = selected ? [f.account, peer].sort() : [f.account]
      const offer = await recovery(selected ? { action: 'selected_capture_preview', accounts, include_ledger: true } : { action: 'capture_preview', account: f.account })
      const manifests = selected ? offer.preview.accounts : [offer.preview]
      assert.ok(manifests.every(m => m.state_schema === 18))
      assert.equal(manifests.find(m => m.account === f.account).resources.find(r => r.resource === 'local_encrypted_state').sha256, before.ciphertext)
      const captured = await recovery({ action: selected ? 'selected_capture' : 'capture', preview: offer.preview, receipt: offer.receipt })
      await undo()
      const later = await f.observe()
      await f.stop('SIGKILL')
      const installation = join(f.directory, `${selected ? 'selected' : 'single'}-restored`)
      await mkdir(installation)
      f.restoreInto(installation); await f.start()
      const restore = await recovery(selected ? { action: 'selected_restore_preview', bundle: captured.bundle, restore_ledger: true } : { action: 'restore_preview', bundle: captured.bundle })
      assert.equal(restore.authenticated, true)
      await recovery({ action: selected ? 'selected_restore' : 'restore', preview: restore.preview, receipt: restore.receipt })
      const after = await f.observe()
      assert.equal(after.primary, before.primary)
      assert.equal(after.ciphertext, before.ciphertext)
      assert.deepEqual(after.journal, before.journal)
      assert.deepEqual(after.all_mail, later.all_mail)
      assert.equal(after.independent, later.independent)
      assert.equal(after.recovery_hold, true)
      const status = await f.command('status')
      assert.equal(status.installed, false); assert.equal(status.recovery_hold, true)
      for (const action of ['preview', 'confirm', 'reconcile', 'resume', 'undo_preview', 'undo', 'resume_undo', 'cancel']) {
        await f.command(action, { options, run: forward.operation.id, receipt: forward.receipt, confirmed: true }, 403)
      }
      await f.stop('SIGKILL'); await f.start()
      assert.deepEqual((await f.command('status')).journal, before.journal)
      assert.deepEqual(wires(await f.observe()), wires(later))
      await record(`populated-${selected ? 'selected' : 'single'}-schema18-gmail-restoration`, { installation, accounts })
      await f.close()
    }
  }
  console.log(`Passed ${checks.length} Gmail label HTTP/TLS groups`)
} finally { await f.close() }
