import assert from 'node:assert/strict'
const base = 'http://127.0.0.1:4173'
const { token } = await (await fetch(`${base}/api/session`)).json()
const get = path => fetch(base + path, { headers: { 'X-Mailroom-Token': token } })
const post = async (path, body, code = 200) => {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, code, `Unexpected status for ${path}`)
  return response.json()
}
const terminal = async () => {
  const deadline = Date.now() + 30000
  while (Date.now() < deadline) {
    const state = await (await get('/api/accounts/status')).json()
    if (['complete', 'failed', 'cancelled'].includes(state.job?.progress.state)) { assert.equal(state.job.progress.state, 'complete'); return state.job.account.id }
    await new Promise(resolve => setTimeout(resolve, 30))
  }
  throw new Error('Synthetic account deadline exceeded')
}
const sample = await (await get('/api/sample')).json()
await post('synthetic', {})
const account = await terminal()
await post('scan', { account }); await terminal()
let view = await post('workspace/open', { account })
assert.equal(view.workspace, null); assert.deepEqual(view.exports, [])
let input = { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' }
const save = async () => { view = await post('workspace/save', { account, revision: view.revision, input }) }
const preview = () => post('proof/preview', { account })
const download = (receipt, code = 200) => post('proof/download', { account, receipt, confirmed: true }, code)
await save()
let proof = await preview()
assert.equal(proof.proof.counts.review, 99); assert.equal(proof.download_available, true)
assert.equal(proof.proof.artifact.context.recipient_evidence, 'unknown')
assert.deepEqual(proof.proof.artifact.context.capabilities, [])
await post('proof/download', { account, receipt: proof.receipt, confirmed: false }, 422)
await download('0'.repeat(64), 422)
let exported = await download(proof.receipt)
assert.equal(exported.sieve, proof.proof.artifact.executable)
assert.equal(exported.identity, proof.proof.artifact.identity)
assert.equal(exported.workspace.exports.length, 1)
assert.match(exported.workspace.exports[0].provenance, /Provider application is unverified/)
await download(proof.receipt, 422)
view = exported.workspace
const baselineIdentity = exported.identity
input = { ...input, baseline: 'previous' }; await save()
proof = await preview(); assert.equal(proof.previous.identity, baselineIdentity)
const revision = view.revision
await post('workspace/save', { account, revision, input: { ...input, documents: { ...input.documents, preset_yaml: '[' } } }, 422)
assert.equal((await post('workspace/open', { account })).revision, revision)
const old = proof
input = { ...input, documents: { ...input.documents, override_yaml: input.documents.override_yaml + '\n# synthetic durable formatting marker\n' } }
await save()
proof = await preview()
assert.equal(proof.proof.artifact.identity, old.proof.artifact.identity)
assert.notEqual(proof.receipt, old.receipt)
await download(old.receipt, 422)
const beforeConfirmation = proof
await post('confirm', { account, address: 'owner@example.test', confirmed: true })
await download(beforeConfirmation.receipt, 422)
await post('workspace/save', { account, revision: view.revision, input }, 422)
view = await post('workspace/open', { account })
proof = await preview()
const hash = proof.proof.decisions.find(d => d.message_id_hash).message_id_hash
input = { ...input, pins: [{ account, message_id_hash: hash, expected: { kind: 'folder', name: 'Synthetic/Wrong' } }] }
await save()
await download(proof.receipt, 422)
proof = await preview(); assert.equal(proof.proof.gate, 'blocked'); assert.equal(proof.download_available, false)
await download(proof.receipt, 422)
input = { ...input, pins: [{ account, message_id_hash: 'a'.repeat(64), expected: { kind: 'inbox' } }] }; await save()
proof = await preview(); assert.equal(proof.proof.pins[0].status, 'stale'); assert.equal(proof.download_available, true)
exported = await download(proof.receipt); view = exported.workspace
const beforeScan = await preview()
await post('scan', { account }); await terminal()
await download(beforeScan.receipt, 422)
view = await post('workspace/open', { account })
assert.equal(view.exports.length, 2); assert.deepEqual(view.workspace.input, input)
for (const injection of [{ context: sample.context }, { provider: 'gmail' }, { source: 'synthetic' }, { path: '/tmp/forbidden' }, { keys: [] }]) {
  await post('proof/preview', { account, ...injection }, 422)
  await post('workspace/save', { account, revision: view.revision, input, ...injection }, 422)
}
await post('workspace/save', { account, revision: view.revision, input: { ...input, pins: [{ ...input.pins[0], account: 'other-account' }] } }, 422)
await post('synthetic-gmail', {}); const gmail = await terminal()
let gmailView = await post('workspace/open', { account: gmail })
gmailView = await post('workspace/save', { account: gmail, revision: gmailView.revision, input: { ...input, pins: [], baseline: 'current' } })
const gmailProof = await post('proof/preview', { account: gmail })
assert.equal(gmailProof.download_available, false)
await post('proof/download', { account: gmail, receipt: gmailProof.receipt, confirmed: true }, 422)
await post('workspace/save', { account: gmail, revision: gmailView.revision, input: { ...input, pins: [], baseline: 'previous' } })
await post('proof/preview', { account: gmail }, 422)
console.log('PASS: durable HTTP saves, exact confirmed bytes, export history/baseline, replay/tamper and malformed YAML rejection, raw formatting/ownership/pin/scan invalidation, stale save conflict, failing/stale pins, preserved census state, injection rejection and Gmail no-download')
