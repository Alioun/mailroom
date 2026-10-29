import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/proposal-language'}/`
const builds = (await readFile(process.argv[2] ?? `${audit}test-build.jsonl`, 'utf8')).trim().split('\n').map(JSON.parse)
assert.deepEqual(builds.at(-1), { reason: 'build-finished', success: true })
const built = builds.findLast(item => item.reason === 'compiler-artifact' && item.target.name === 'ui_bridge' && item.profile.test && item.executable)
assert.ok(built)
const artifact = { path: built.executable, sha256: createHash('sha256').update(await readFile(built.executable)).digest('hex') }
const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const bridge = spawn(artifact.path, ['--exact', 'synthetic_ui_bridge', '--ignored', '--nocapture'], {
  cwd: root, env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin' }, stdio: ['pipe', 'pipe', 'pipe'],
})
let sequence = 0
let failure
let diagnostics = ''
const pending = new Map()
const closed = new Promise(resolve => bridge.on('close', code => {
  failure = new Error('Synthetic router bridge closed')
  for (const waiter of pending.values()) waiter.reject(failure)
  pending.clear()
  resolve(code)
}))
bridge.on('error', error => {
  failure = error
  for (const waiter of pending.values()) waiter.reject(error)
  pending.clear()
})
bridge.stderr.on('data', bytes => { diagnostics += bytes.toString() })
createInterface({ input: bridge.stdout }).on('line', line => {
  const marker = 'MAILROOM_UI_RESPONSE '
  const offset = line.indexOf(marker)
  if (offset < 0) return
  const record = JSON.parse(line.slice(offset + marker.length))
  const waiter = pending.get(record.id)
  assert.ok(waiter)
  pending.delete(record.id)
  waiter.resolve(new Response(Buffer.from(record.body, 'base64'), { status: record.status, headers: record.headers }))
})
async function request(path, options = {}) {
  if (failure) throw failure
  const id = ++sequence
  const reply = new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
  const headers = Object.fromEntries(new Headers(options.headers).entries())
  bridge.stdin.write(`${JSON.stringify({ id, method: options.method ?? 'GET', path, headers: { host: '127.0.0.1:4173', ...headers }, body: options.body ?? '' })}\n`)
  return reply
}
globalThis.fetch = request
const deadline = setTimeout(() => {
  failure = new Error('Synthetic proposal workflow exceeded its deadline')
  for (const waiter of pending.values()) waiter.reject(failure)
  pending.clear()
  bridge.stdin.end()
}, 45_000)
try {
  const session = await (await request('/api/session')).json()
  const headers = { 'content-type': 'application/json', 'x-mailroom-token': session.token }
  async function post(path, body, expected = 200, extraHeaders = {}) {
    const response = await request(`/api/accounts/${path}`, { method: 'POST', headers: { ...headers, ...extraHeaders }, body: JSON.stringify(body) })
    assert.equal(response.status, expected, `Unexpected ${path} status`)
    assert.equal(response.headers.get('cache-control'), 'no-store')
    return response.json()
  }
  assert.equal((await request('/api/accounts/proposals', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status, 403)
  await post('proposals', {}, 403, { origin: 'https://outside.example' })
  const sample = await (await request('/api/sample', { headers })).json()
  const documents = {
    ...sample.documents,
    override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test'),
    preset_yaml: sample.documents.preset_yaml.replace('value: order}', "value: 'Synthetic account header'}"),
  }
  async function complete() {
    const end = Date.now() + 10_000
    while (Date.now() < end) {
      const { job } = await api.readAccountStatus()
      assert.ok(job)
      if (job.progress.state === 'complete') {
        assert.ok(job.account)
        return job.account.id
      }
      assert.equal(job.progress.state, 'running')
      await new Promise(resolve => setTimeout(resolve, 10))
    }
    throw new Error('Synthetic account job did not complete')
  }
  async function account() {
    await post('synthetic', {})
    const id = await complete()
    await post('scan', { account: id })
    await complete()
    const workspace = await post('workspace/open', { account: id })
    await post('workspace/save', { account: id, revision: workspace.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
    return id
  }
  const owner = await account()
  const other = await account()
  const generated = await api.openProposals(owner)
  assert.ok(generated.candidates.length >= 2)
  assert.ok(generated.candidates.every(candidate => candidate.rationale.kind === 'subject_majority'))
  assert.ok(generated.current.proof.decisions.every(decision => decision.destination.kind === 'inbox' && decision.copies.length === 0))
  function selection(candidate) {
    return { account: owner, generation: generated.generation, candidates: [{ id: candidate.id, change: candidate.change }] }
  }
  const rejected = await api.reviewProposals(owner, selection(generated.candidates[0]), 'reject')
  const retained = await api.confirmProposals(owner, rejected)
  assert.ok(retained.candidates[0].rejected)
  assert.equal(retained.candidates[1].rejected, false)
  const approved = await api.reviewProposals(owner, selection(retained.candidates[1]), 'accept')
  assert.ok(approved.acceptance_available)
  const altered = structuredClone(approved.selection)
  altered.candidates[0].change.rule.mark_read = true
  await assert.rejects(() => api.reviewProposals(owner, altered, 'accept'))
  await assert.rejects(() => api.confirmProposals(other, approved))
  const saved = await api.confirmProposals(owner, approved)
  assert.equal(saved.decisions.history.length, 2)
  assert.equal((await api.openProposals(other)).decisions.history.length, 0)
  await assert.rejects(() => api.confirmProposals(owner, approved))
  const proof = await api.previewWorkspace(owner)
  assert.ok(proof.proof.decisions.every(decision => decision.destination.kind === 'inbox' && decision.copies.length === 0))
  const result = { syntheticOnly: true, bridgeSha256: artifact.sha256, requests: sequence, candidates: generated.candidates.length, decisions: saved.decisions.history.length, checks: ['session and Origin protections', 'actual HTTP router to frontend parser', 'generated counted rationale', 'separate exact rejection and acceptance', 'altered rule refused', 'cross-account confirmation refused', 'stale confirmation refused', 'other account preserved', 'fresh conservative proof'] }
  await writeFile(`${audit}router-results.json`, JSON.stringify(result, null, 2) + '\n')
  console.log(`Passed ${result.checks.length} synthetic router workflow groups over private pipes.`)
} finally {
  clearTimeout(deadline)
  bridge.stdin.end()
  const exit = await closed
  await writeFile(`${audit}router-bridge.log`, diagnostics)
  assert.equal(exit, 0, 'Synthetic router bridge exits after stdin closes')
  assert.equal(createHash('sha256').update(await readFile(artifact.path)).digest('hex'), artifact.sha256)
}
