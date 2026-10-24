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
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw new Error('Synthetic census timeout')
}
const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': session.token } }).then(r => r.json())
for (const gmail of [false, true]) {
await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
const account = await terminal()
await post('scan', { account }); await terminal()
let saved = await post('workspace/open', { account })
const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test').replace('  accounting: {address: archive@example.net}', '  accounting: {address: archive@example.net}\n  alias: {address: archive@example.net}') }
const input = { documents, pins: [], as_of: sample.as_of, baseline: 'current' }
await post('workspace/save', { account, revision: saved.revision, input })
const command = (action, extra = {}, expected = 200) => post('invoices', { account, command: { action, ...extra } }, expected)
let view = await command('open')
assert.equal(view.report.count, 7)
assert.equal(view.report.eligible_business, 7)
if (!gmail) { assert.deepEqual(view.report.rows[0].predicted_targets, ['accounting']); assert.equal(view.report.rows[0].handoff, 'real_handoff_unknown') }
else assert.ok(view.report.rows.every(r => r.predicted_targets.length === 0))
assert.equal(view.report.rows[0].evidence.attachments[0].media_type, 'application/pdf')
assert.ok(view.report.rows[0].reasons.some(r => r.includes('Bulk')))
assert.ok(view.report.rows[2].reasons.some(r => r.includes('convertible')))
assert.ok(!view.report.rows[2].reasons.some(r => r.includes('unsupported')))
assert.ok(view.report.rows[4].reasons.some(r => r.includes('no verified')))
assert.ok(view.report.rows[5].reasons.some(r => r.includes('unsupported')))
assert.equal(view.report.rows[6].evidence.kind, 'unknown')
const selection = view.report.rows[0].id, target = 'accounting'
let preview = await command('preview', { selection, target })
const oldReceipt = preview.receipt
const fallback = await command('prepare_mail_app', { selection, target })
assert.equal(fallback.preview.native_available, false)
assert.equal(fallback.preview.plan.source.original, preview.plan.source.original)
assert.equal(fallback.status, 'preparation_only')
await command('send', { selection, target, receipt: fallback.preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
await command('send', { selection, target, receipt: preview.receipt, confirmation_action: 'Send something else' }, 422)
await command('send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action, confirmed: true }, 422)
await command('send', { selection: view.report.rows[1].id, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
await command('send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action, sending_address: 'intruder@example.test' }, 422)
await command('send', { selection, target: 'alias', receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
await command('send', { selection, target: 'unknown', receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
await post('scan', { account }); await terminal()
await command('send', { selection, target, receipt: oldReceipt, confirmation_action: preview.plan.confirmation_action }, 422)
view = await command('open')
assert.notEqual(view.report.version, preview.plan.report_version)
preview = await command('preview', { selection, target })
const ordinary = await post('proof/preview', { account })
await command('send', { selection, target, receipt: ordinary.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
await post('synthetic', {})
const wrongAccount = await terminal()
await post('invoices', { account: wrongAccount, command: { action: 'send', selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action } }, 422)
const exact = preview.plan
view = await command('send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action })
assert.equal(view.sends.length, 1)
assert.equal(view.sends[0].status, 'completed')
assert.deepEqual(view.received[view.sends[0].id], exact)
assert.deepEqual(Buffer.from(view.received[view.sends[0].id].source.original), Buffer.from(exact.source.original))
assert.deepEqual(view.received[view.sends[0].id].source.evidence.attachments, exact.source.evidence.attachments)
assert.equal(view.report.count, 6)
assert.ok(view.report.rows.every(r => r.accounting_acceptance === 'unknown'))
await command('send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
view = await command('recover', { send: view.sends[0].id })
assert.equal(Object.keys(view.received).length, 1)
saved = await post('workspace/open', { account })
await post('workspace/save', { account, revision: saved.revision, input: { ...input, documents: sample.documents } })
view = await command('open'); assert.equal(view.report.count, 0)
console.log(`PASS ${gmail ? 'Gmail' : 'IMAP'} invoice HTTP exact MIME/attachment send, named confirmation, altered/replayed/stale/cross-purpose authority denials, seven evidence cases, bulk, body conversion, refresh report/count, role exclusion, native preparation only, acceptance unknown`)
}
