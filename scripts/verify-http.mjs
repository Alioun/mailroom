import assert from 'node:assert/strict'

const base = 'http://127.0.0.1:4173'
const session = await (await fetch(base + '/api/session')).json()
const local = (path, options = {}) => fetch(base + path, { ...options, headers: { ...options.headers, 'X-Mailroom-Token': session.token } })
const post = (path, body, headers = {}) => local(path, {
  method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body),
})
const sample = await (await local('/api/sample')).json()
const response = await post('/api/preview', sample)
assert.equal(response.status, 200)
assert.ok(!response.headers.get('content-security-policy').includes('unsafe-'))
assert.equal(response.headers.get('access-control-allow-origin'), null)
const result = await response.json()
assert.deepEqual(result.counts, { messages: 6, filed: 4, inbox: 2, copies: 4, copied_messages: 4 })
assert.equal(result.decisions[0].destination.name, 'Studio')
assert.equal(result.decisions[1].destination.name, 'Reading')
assert.equal(result.decisions[3].destination.kind, 'inbox')
const availability = await (await local('/api/availability')).json()
let denied = 0
for (const action of availability.filter(a => a.effect !== 'synthetic_read')) {
  assert.equal(action.allowed, false)
  assert.equal((await post('/api/operations', { operation: action.operation })).status, 403)
  denied++
}
assert.equal(denied, 10)
assert.equal((await post('/api/operations', { operation: 'send_message', synthetic: true, allowed: true })).status, 422)
assert.equal((await post('/api/preview', sample, { Origin: 'https://foreign.example' })).status, 403)
assert.equal((await post('/api/preview', sample, { 'Sec-Fetch-Site': 'cross-site' })).status, 403)
assert.equal((await post('/api/preview', sample, { 'Content-Type': 'text/plain' })).status, 415)
assert.equal((await post('/api/preview', 'x'.repeat(262145))).status, 413)
console.log('PASS: real HTTP preview, exact counts, 10 denied account operations, authorization injection, Origin, fetch-site, content type, body limit and CSP')

assert.equal((await fetch(base + '/api/census/demo')).status, 403)
assert.equal((await fetch(base + '/api/census/demo', { headers: { 'X-Mailroom-Token': 'invalid' } })).status, 403)
assert.equal((await post('/api/census/demo/start', { mode: 'normal', source: 'imap://example.test' })).status, 422)
let census = await (await post('/api/census/demo/start', { mode: 'normal' })).json()
assert.equal(census.source, 'synthetic')
assert.equal(census.state, 'running')
assert.equal((await post('/api/census/demo/start', { mode: 'normal' })).status, 422)
const readCensus = async () => (await local('/api/census/demo')).json()
async function until(done) {
  for (let attempt = 0; attempt < 60; attempt++) {
    census = await readCensus()
    if (done(census)) return
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error('Synthetic census did not reach the expected state')
}
await until(c => c.completed >= 1)
census = await (await post('/api/census/demo/cancel', {})).json()
assert.equal(census.state, 'cancelled')
assert.ok(census.completed < census.total)
const saved = census.completed
await new Promise(resolve => setTimeout(resolve, 600))
assert.equal((await readCensus()).completed, saved)
await post('/api/census/demo/start', { mode: 'normal' })
await until(c => c.state === 'complete')
assert.equal(census.completed, 8)
assert.equal(census.messages[0].subject, 'Grüße und Rechnung 1')
assert.ok(census.messages.every(m => m.dkim === 'unknown'))
assert.ok(census.addresses.every(a => !a.confirmed))
census = await (await post('/api/census/demo/confirm', { address: 'owner@example.test', confirmed: true })).json()
assert.ok(census.addresses.some(a => a.address === 'owner@example.test' && a.confirmed))
assert.ok(census.summary.includes('alias:'))
for (const sensitive of ['owner@example.test', 'news@studio.example', 'Grüße', 'https://', 'token=']) {
  assert.ok(!census.summary.includes(sensitive))
}
await post('/api/census/demo/start', { mode: 'error' })
await until(c => c.state === 'failed')
assert.equal(census.completed, 2)
await post('/api/census/demo/start', { mode: 'normal' })
await until(c => c.state === 'complete')
assert.equal(census.completed, 8)
console.log('PASS: launch token, synthetic scan progress, cancellation, encrypted resume, failure recovery, address confirmation and summary privacy')
