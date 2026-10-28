import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
const base = 'http://127.0.0.1:4173'
const { token } = await fetch(`${base}/api/session`).then(r => r.json())
const headers = { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }
const checks = []
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `${path} HTTP status`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const state = await fetch(`${base}/api/accounts/status`, { headers }).then(r => r.json())
    if (state.job.progress.state !== 'running') { assert.equal(state.job.progress.state, 'complete'); return state.job.account.id }
    await new Promise(resolve => setTimeout(resolve, 20))
  }
  throw Error('Synthetic census timeout')
}
await post('synthetic', {})
const account = await terminal()
await post('scan', { account }); await terminal()
const sample = await fetch(`${base}/api/sample`, { headers }).then(r => r.json())
let workspace = await post('workspace/open', { account })
const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test').replace('  accounting: {address: archive@example.net}', '  accounting: {address: archive@example.net}\n  alias: {address: other@example.test}') }
await post('workspace/save', { account, revision: workspace.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
const invoice = (action, extra = {}, expected) => post('invoices', { account, command: { action, ...extra } }, expected)
let view = await invoice('open')
const selection = view.report.rows.find(r => r.send_available).id, target = 'accounting'
let preview = await invoice('preview', { selection, target })
const first = preview
preview = await invoice('preview', { selection, target })
assert.notEqual(first.receipt, preview.receipt)
assert.equal(first.confirmation.plan_digest, preview.confirmation.plan_digest)
assert.equal(preview.confirmation.synthetic, true)
assert.equal(preview.confirmation.request_id, preview.receipt)
assert.equal(preview.confirmation.actions.length, 1)
const exact = preview.confirmation.actions[0]
assert.equal(exact.account, account)
assert.equal(exact.purpose, 'invoice_send')
assert.equal(exact.target, preview.plan.target)
assert.equal(exact.sending_identity, preview.plan.sending_address)
assert.equal(exact.content_digest, preview.plan.content_digest)
assert.equal(exact.revision, preview.plan.state_revision)
for (const value of [account, exact.target, exact.sending_identity, exact.content_digest]) assert.ok(preview.confirmation.display.includes(value))
checks.push('unique request identities, stable exact plan binding and server-derived displayed action')
const send = p => ({ selection, target, receipt: p.receipt, confirmation_action: p.plan.confirmation_action })
await invoice('send', { ...send(first), target: 'alias' }, 422)
await invoice('send', send(first), 422)
view = await invoice('open'); assert.equal(view.sends.length, 0)
checks.push('target substitution consumes the request and produces no journal or effect')
workspace = await post('workspace/open', { account })
await post('workspace/save', { account, revision: workspace.revision, input: workspace.workspace.input })
await invoice('send', send(preview), 422)
await invoice('send', send(preview), 422)
for (const mode of ['smtp', 'mail_app']) await invoice('preview', { selection, target, mode }, 403)
await invoice('send', { ...send(preview), mode: 'mail_app' }, 403)
preview = await invoice('preview', { selection, target })
view = await invoice('send', send(preview))
assert.equal(view.sends.length, 1)
assert.equal(view.sends[0].status, 'completed')
assert.deepEqual(view.received[view.sends[0].id], preview.plan)
assert.deepEqual(Buffer.from(view.received[view.sends[0].id].source.original), Buffer.from(preview.plan.source.original))
assert.deepEqual(view.received[view.sends[0].id].source.evidence.attachments, preview.plan.source.evidence.attachments)
await invoice('send', send(preview), 422)
checks.push('stale state and preparation reject; exact fake MIME and attachment effect completes once')
const unsubscribe = (command, expected) => post('unsubscribe', { account, command }, expected)
let subscriptions = await unsubscribe({ action: 'open', as_of: sample.as_of })
const row = subscriptions.report.rows.find(r => r.methods.includes('local_mail'))
const input = { as_of: sample.as_of, selections: [{ subscription: row.id, method: 'local_mail' }], folders: { personal: 'Muted', business: 'Quiet' } }
let plan = await unsubscribe({ action: 'preview', input })
const action = plan.confirmation.actions[0]
assert.equal(action.purpose, 'unsubscribe_local_mail')
assert.equal(action.target, plan.actions[0].copy_card.to)
assert.equal(action.sending_identity, plan.actions[0].copy_card.from)
assert.equal(action.receiving_identity, row.recipient)
assert.equal(action.content_digest, plan.actions[0].payload_digest)
assert.ok(plan.confirmation.display.includes(action.target))
assert.ok(plan.confirmation.display.includes(action.receiving_identity))
const confirm = p => ({ action: 'confirm', input, receipt: p.receipt, confirmation_action: p.confirmation_action, mail_confirmation_action: p.local_mail_confirmation })
await unsubscribe({ ...confirm(plan), receipt: preview.receipt }, 422)
subscriptions = await unsubscribe(confirm(plan))
assert.equal(subscriptions.history.at(-1).status, 'mail_handoff')
assert.equal(subscriptions.received[subscriptions.history.at(-1).id].payload_digest, action.content_digest)
await unsubscribe(confirm(plan), 422)
checks.push('unsubscribe local mail binds receiving and sending identity, exact target and payload, cross-purpose rejection and replay denial')
const listInput = { ...input, selections: [{ account, subscription: row.id, method: 'local_mail' }] }
const list = (command, expected) => post('subscription-review', { command }, expected)
plan = await list({ action: 'preview', input: listInput })
assert.equal(plan.confirmation.actions[0].account, account)
assert.equal(plan.confirmation.actions[0].receiving_identity, row.recipient)
const listConfirm = { action: 'confirm', input: listInput, receipt: plan.receipt, confirmation_action: plan.confirmation_action, mail_confirmation_action: plan.local_mail_confirmation }
await list({ ...listConfirm, mail_confirmation_action: null }, 422)
await list(listConfirm)
await list(listConfirm, 422)
checks.push('shared multi-account boundary preserves exact list and additional local-mail confirmation')
await mkdir(new URL('../.audit/', import.meta.url), { recursive: true })
await writeFile(new URL('../.audit/macos-confirmation-runtime-results.json', import.meta.url), JSON.stringify({ checks, synthetic: true, native_invocation: false, provider_write_authority: false }, null, 2) + '\n')
for (const check of checks) console.log(`PASS ${check}`)
