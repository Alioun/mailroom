import assert from 'node:assert/strict'
const base = 'http://127.0.0.1:4173'
const session = await fetch(`${base}/api/session`).then(r => r.json())
async function post(path, body, expected = 200) {
  const r = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': session.token }, body: JSON.stringify(body) })
  assert.equal(r.status, expected, `${path}: ${r.status}`)
  return r.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': session.token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account }
    await new Promise(r => setTimeout(r, 20))
  }
  throw new Error('Synthetic account did not finish')
}
const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': session.token } }).then(r => r.json())
let priorReceipt
for (const gmail of [false, true]) {
  await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
  const account = (await terminal()).id
  await post('scan', { account }); await terminal()
  const state = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: state.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
  const command = (action, extra = {}, expected = 200) => post('backlog', { account, command: { action, ...extra } }, expected)
  const before = await command('open')
  let preview = await command('preview', { options })
  const staleReceipt = preview.receipt
  await post('scan', { account }); await terminal()
  await command('start', { options, receipt: staleReceipt, confirmed: true }, 422)
  preview = await command('preview', { options })
  if (priorReceipt) await command('start', { options, receipt: priorReceipt, confirmed: true }, 422)
  assert.equal(preview.execution_available, true)
  assert.ok(preview.plan.changes.length > 0)
  assert.equal(preview.plan.source.includes('fixture recipient evidence only'), true)
  assert.equal(preview.plan.excluded.flagged, 32)
  const count = preview.plan.changes.length
  const first = preview.plan.changes[0]
  const ordinary = await post('proof/preview', { account })
  assert.equal(ordinary.proof.artifact.context.recipient_evidence, 'unknown')
  assert.equal(ordinary.proof.counts.filed, 0)
  await command('start', { options, receipt: ordinary.receipt, confirmed: true }, 422)
  await command('start', { options, receipt: preview.receipt, confirmed: false }, 422)
  await command('start', { options, receipt: preview.receipt, confirmed: true, synthetic: true }, 422)
  await command('preview', { options, context: sample.context }, 422)
  const empty = await command('preview', { options: { ...options, selected: [] } })
  assert.equal(empty.plan.changes.length, 0)
  await command('start', { options: { ...options, selected: [] }, receipt: preview.receipt, confirmed: true }, 422)
  const concurrent = await Promise.all([0, 1].map(async () => {
    const response = await fetch(`${base}/api/accounts/backlog`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': session.token }, body: JSON.stringify({ account, command: { action: 'start', options, receipt: preview.receipt, confirmed: true } }) })
    return { status: response.status, value: await response.json() }
  }))
  assert.deepEqual(concurrent.map(r => r.status).sort(), [200, 422])
  const started = concurrent.find(r => r.status === 200).value
  const id = started.runs.at(-1).id
  priorReceipt = id
  assert.equal(started.runs.at(-1).status, 'prepared')
  await command('start', { options, receipt: preview.receipt, confirmed: true }, 422)
  await command('stop', { run: id })
  const held = await command('advance', { run: id })
  assert.equal(held.runs.at(-1).outcomes.filter(o => o.state === 'applied').length, 0)
  await command('resume', { run: id })
  let after = await command('advance', { run: id })
  assert.equal(after.runs.at(-1).status, 'completed')
  assert.equal(after.runs.at(-1).outcomes.filter(o => o.state === 'applied').length, count)
  const stable = after.mailbox
  after = await command('advance', { run: id })
  assert.deepEqual(after.mailbox, stable)
  if (gmail) {
    const original = before.mailbox.folders[first.source.folder].messages[first.source.uid]
    const actual = after.mailbox.folders[first.source.folder].messages[first.source.uid]
    for (const label of original.gmail_labels.filter(l => !l.startsWith('\\'))) assert.ok(actual.gmail_labels.includes(label))
    assert.deepEqual(actual.flags, original.flags)
    assert.ok(actual.gmail_labels.includes(first.destination))
    assert.ok(!actual.gmail_labels.includes('\\Inbox'))
  } else {
    assert.ok(!after.mailbox.folders[first.source.folder].messages[first.source.uid])
    const observation = after.runs.at(-1).outcomes[0].observation
    const actual = after.mailbox.folders[observation.destination.folder].messages[observation.destination.uid]
    assert.deepEqual(actual.flags, first.flags)
    assert.ok(observation.commands.some(c => c.startsWith('UID MOVE')))
  }
  const undo = await command('undo_preview', { run: id })
  assert.equal(undo.eligible.length, count)
  await command('undo', { run: id, receipt: undo.receipt, confirmed: false }, 422)
  await command('undo', { run: id, receipt: preview.receipt, confirmed: true }, 422)
  after = await command('undo', { run: id, receipt: undo.receipt, confirmed: true })
  assert.equal(after.runs.at(-1).undo.filter(u => u.state === 'restored').length, count)
  after = await command('resume_undo', { run: id })
  const originals = Object.values(before.mailbox.folders).flatMap(f => Object.values(f.messages)).sort((a,b) => a.headers.message_id_hash.localeCompare(b.headers.message_id_hash))
  const restored = Object.values(after.mailbox.folders).flatMap(f => Object.values(f.messages)).sort((a,b) => a.headers.message_id_hash.localeCompare(b.headers.message_id_hash))
  assert.equal(restored.length, originals.length)
  for (let i = 0; i < originals.length; i++) {
    assert.equal(restored[i].headers.message_id_hash, originals[i].headers.message_id_hash)
    assert.deepEqual(restored[i].flags, originals[i].flags)
    assert.deepEqual(restored[i].gmail_labels, originals[i].gmail_labels)
  }
  assert.equal(after.mailbox.folders.INBOX.messages['3'].flags.includes('\\Flagged'), true)
  console.log(`PASS ${gmail ? 'Gmail' : 'IMAP'} exact ${count} synthetic effects, selection/receipt/replay/injection denials, independent mailbox observation, flags and conditional undo`)
}
