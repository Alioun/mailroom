import assert from 'node:assert/strict'

const base = 'http://127.0.0.1:4173'
const post = (path, body, headers = {}) => fetch(base + path, {
  method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body),
})
const sample = await (await fetch(base + '/api/sample')).json()
const response = await post('/api/preview', sample)
assert.equal(response.status, 200)
assert.ok(!response.headers.get('content-security-policy').includes('unsafe-'))
assert.equal(response.headers.get('access-control-allow-origin'), null)
const result = await response.json()
assert.deepEqual(result.counts, { messages: 6, filed: 4, inbox: 2, copies: 4, copied_messages: 4 })
assert.equal(result.decisions[0].destination.name, 'Studio')
assert.equal(result.decisions[1].destination.name, 'Reading')
assert.equal(result.decisions[3].destination.kind, 'inbox')
const availability = await (await fetch(base + '/api/availability')).json()
let denied = 0
for (const action of availability.filter(a => a.operation !== 'preview')) {
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
