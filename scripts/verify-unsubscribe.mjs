import assert from 'node:assert/strict'
const base = 'http://127.0.0.1:4173'
const { token } = await fetch(`${base}/api/session`).then(r => r.json())
const headers = { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `${path} status`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Fake census timeout')
}
const sample = await fetch(`${base}/api/sample`, { headers }).then(r => r.json())
let previousReceipt
for (const provider of ['synthetic', 'synthetic-gmail']) {
  await post(provider, {}); const account = await terminal()
  await post('scan', { account }); await terminal()
  await post('open', { account })
  const workspace = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  const command = (command, expected) => post('unsubscribe', { account, command }, expected)
  const view = await command({ action: 'open', as_of: sample.as_of })
  assert.equal(view.synthetic, true); assert.equal(view.production_available, false); assert.equal(view.q20_scope, 'separate_subscriptions')
  const row = view.report.rows.find(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test' && r.list_id === '<offers.shop.example>')
  assert.equal(row.count_12_months, 20)
  assert.equal(row.flags_known, true); assert.equal(row.recommended, false, 'Currently Seen recent mail prevents initial selection')
  assert.equal(row.authentication, 'synthetic_provider_pass_both_same_signature')
  assert.ok(row.methods.includes('one_click'))
  const people = view.report.rows.find(r => r.class === 'list_with_people')
  assert.ok(people); assert.equal(people.recommended, false)
  const platform = view.report.rows.find(r => r.class === 'platform')
  assert.ok(platform); assert.equal(platform.recommended, false)
  const relay = view.report.rows.find(r => r.sender === 'news@claimed.example')
  assert.equal(relay.original_header_survives, false); assert.equal(relay.recommended, false)
  assert.equal(relay.relay_alias_initially_selected, false); assert.equal(relay.methods.includes('one_click'), false)
  const input = { as_of: sample.as_of, selections: [{ subscription: row.id, method: 'browser_preparation' }], folders: { personal: 'Muted', business: 'Quiet' } }
  let preview = await command({ action: 'preview', input })
  assert.ok(preview.actions[0].copy_card.uri.includes('synthetic-private-token'))
  const send = () => ({ action: 'confirm', input, receipt: preview.receipt, confirmation_action: preview.confirmation_action, mail_confirmation_action: preview.local_mail_confirmation })
  if (previousReceipt) await command({ ...send(), receipt: previousReceipt }, 422)
  previousReceipt = preview.receipt
  const otherPurpose = await post('proof/preview', { account })
  await command({ ...send(), receipt: otherPurpose.receipt }, 422)
  await command({ ...send(), receipt: 'x'.repeat(64) }, 422)
  await command({ ...send(), confirmation_action: 'different action' }, 422)
  for (const injection of ['provider_trust', 'transport', 'source', 'synthetic']) await command({ action: 'preview', input, [injection]: true }, 422)
  await command({ ...send(), input: { ...input, selections: [{ subscription: row.id, method: 'keep' }] } }, 422)
  const saved = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: saved.revision, input: saved.workspace.input })
  await command(send(), 422)
  preview = await command({ action: 'preview', input })
  let next = await command(send())
  assert.equal(next.history.at(-1).status, 'prepared'); assert.equal(Object.keys(next.received).length, 0)
  assert.equal(JSON.stringify(next.history).includes('synthetic-private-token'), false)
  await command(send(), 422)
  input.selections[0].method = 'local_mail'
  preview = await command({ action: 'preview', input })
  assert.deepEqual(preview.actions[0].copy_card, { uri: null, to: 'leave@shop.example', from: row.recipient, subject: 'unsubscribe', body: 'synthetic-mail-token' })
  await command({ ...send(), mail_confirmation_action: null }, 422)
  next = await command(send()); assert.equal(next.history.at(-1).status, 'mail_handoff')
  input.selections[0].method = 'mute'
  preview = await command({ action: 'preview', input })
  assert.ok(preview.proof.artifact.executable.includes('"list-id" "<offers.shop.example>"'))
  const predicted = preview.proof.decisions.filter(d => d.matched_rule === `user.mute.${row.id}`)
  assert.equal(predicted.length, 20); assert.ok(predicted.every(d => d.mark_read && d.destination.name === 'Personal/Muted'))
  next = await command(send()); assert.equal(next.history.at(-1).status, 'muted'); assert.ok(next.mutes[row.id])
  const reopened = await post('workspace/open', { account }); assert.ok(reopened.workspace.input.documents.override_yaml.includes(`user.mute.${row.id}`))
  input.selections[0].method = 'remove_mute'
  preview = await command({ action: 'preview', input }); assert.equal(preview.proof.decisions.some(d => d.matched_rule === `user.mute.${row.id}`), false)
  next = await command(send()); assert.equal(next.history.at(-1).status, 'mute_removed'); assert.ok(next.history.every(e => e.sender_key.length > 0)); assert.equal(Object.keys(next.mutes).length, 0)
  for (const marker of ['synthetic-private-token', 'synthetic-mail-token']) assert.equal(JSON.stringify(next.history).includes(marker), false)
  console.log(`PASS ${provider} protected HTTP facts, unknown/people/platform/relay safety, exact previews, injection/stale/replay denial, preparation only, extra mail confirmation, stable-ID mute proof/read/removal, token-safe history`)
}

const accounts = []
for (const provider of ['synthetic', 'synthetic-gmail']) {
  await post(provider, {}); const account = await terminal()
  await post('scan', { account }); await terminal()
  const workspace = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  accounts.push(account)
}
const list = (command, expected) => post('subscription-review', { command }, expected)
let all = await list({ action: 'open', as_of: sample.as_of })
const targets = accounts.flatMap(account => all.accounts.find(a => a.account === account).report.rows.filter(r => r.sender === 'news@shop.example').map(r => ({ account, subscription: r.id, method: 'one_click' })))
assert.equal(targets.length, 6, 'Two accounts each retain three distinct recipient/list subscriptions')
const first = targets[0]
let input = { as_of: sample.as_of, selections: [first], folders: { personal: 'Muted', business: 'Quiet' } }
let plan = await list({ action: 'preview', input })
assert.equal(plan.previews.length, 1); assert.equal(plan.previews[0].actions.length, 1)
assert.equal(plan.previews[0].actions[0].subscription, first.subscription)
const confirmList = (input, plan) => ({ action: 'confirm', input, receipt: plan.receipt, confirmation_action: plan.confirmation_action, mail_confirmation_action: plan.local_mail_confirmation })
for (const sibling of targets.slice(1)) {
  await list(confirmList({ ...input, selections: [first, sibling] }, plan), 422)
  await list(confirmList({ ...input, selections: [sibling] }, plan), 422)
}
await list({ action: 'preview', input: { ...input, selections: [first, first] } }, 422)
for (const field of ['provider_trust', 'transport', 'synthetic']) await list({ action: 'preview', input, [field]: true }, 422)
plan = await list({ action: 'preview', input })
all = await list(confirmList(input, plan))
for (const account of accounts) {
  const saved = all.accounts.find(a => a.account === account)
  assert.deepEqual(saved.history.map(e => e.subscription), account === first.account ? [first.subscription] : [])
  assert.equal(Object.keys(saved.mutes).length, 0)
  const perAccount = await post('unsubscribe', { account, command: { action: 'open', as_of: sample.as_of } })
  assert.equal(Object.keys(perAccount.received).length, account === first.account ? 1 : 0)
}
await list(confirmList(input, plan), 422)
input = { ...input, selections: [targets[1], targets[3]] }
plan = await list({ action: 'preview', input })
assert.equal(plan.previews.length, 2)
assert.equal(plan.previews.flatMap(p => p.actions).length, 2)
all = await list(confirmList(input, plan))
for (const account of accounts) {
  const expected = [first, targets[1], targets[3]].filter(t => t.account === account).map(t => t.subscription).sort()
  assert.deepEqual(all.accounts.find(a => a.account === account).history.map(e => e.subscription).sort(), expected)
}
console.log('PASS Q20 cross-account sender grouping primitives: six separate recipient/list targets, selective single-target POST, sibling substitution/addition and replay denied, explicit two-account list, no hidden history/effects/mutes')

input = { ...input, selections: [{ ...targets[5], method: 'local_mail' }] }
plan = await list({ action: 'preview', input })
assert.ok(plan.local_mail_confirmation)
await list({ ...confirmList(input, plan), mail_confirmation_action: null }, 422)
all = await list({ action: 'open', as_of: sample.as_of })
assert.equal(all.accounts.find(a => a.account === targets[5].account).history.some(e => e.subscription === targets[5].subscription), false)
console.log('PASS global exact mail list additionally rejects missing named mail confirmation before effects')
