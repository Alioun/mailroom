import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/sender-catalog'}/`
const builds = (await readFile(`${audit}test-build.jsonl`, 'utf8')).trim().split('\n').map(JSON.parse)
assert.deepEqual(builds.at(-1), { reason: 'build-finished', success: true })
const built = builds.findLast(item => item.reason === 'compiler-artifact' && item.target.name === 'ui_bridge' && item.profile.test && item.executable)
assert.ok(built)
const digest = bytes => createHash('sha256').update(bytes).digest('hex')
const artifact = { path: built.executable, sha256: digest(await readFile(built.executable)) }
const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const winddown = await import(pathToFileURL(`${audit}render/winddown-api.mjs`))
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
  failure = new Error('Synthetic catalog workflow exceeded its deadline')
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
  assert.equal((await request('/api/accounts/winddown', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status, 403)
  await post('winddown', {}, 403, { origin: 'https://outside.example' })
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
    return id
  }
  const owner = await account()
  const other = await account()
  const asOf = 1792886400
  const view = await winddown.openWinddown(owner, asOf)
  const catalog = view.report.sender_catalog
  assert.ok(catalog)
  assert.equal(catalog.rows, 0)
  assert.equal(catalog.seed_complete, false)
  assert.equal(catalog.missing_categories.length, 6)
  assert.equal(catalog.service_reviews, 0)
  assert.ok(!view.report.rows.some(row => row.group === 'service'))
  assert.ok(view.terminal.includes('Sender catalog'))
  assert.deepEqual(await winddown.openWinddown(owner, asOf), view, 'Reopening is idempotent')
  const decision = await winddown.reviewWinddown(owner, view.report, { kind: 'settings', settings: { winding_down: true, comparisons: {} } })
  await assert.rejects(() => winddown.confirmWinddown(other, decision))
  await winddown.confirmWinddown(owner, decision)
  await assert.rejects(() => winddown.confirmWinddown(owner, decision))
  await assert.rejects(() => winddown.previewWinddownExport(owner, view.report, 'local_file'))
  const saved = await winddown.openWinddown(owner, asOf)
  const local = await winddown.previewWinddownExport(owner, saved.report, 'local_file')
  const summary = await winddown.previewWinddownExport(owner, saved.report, 'agent_summary')
  assert.equal(local.text, saved.terminal)
  assert.notEqual(local.receipt, summary.receipt)
  await assert.rejects(() => winddown.exportWinddown(owner, saved.report, { ...local, receipt: summary.receipt }))
  assert.deepEqual(await winddown.exportWinddown(owner, saved.report, local), local)
  for (const row of saved.report.rows) {
    if (row.sender) assert.equal(summary.text.includes(row.sender), false)
    if (row.settings_link) assert.equal(summary.text.includes(row.settings_link), false)
  }
  assert.equal((await winddown.openWinddown(other, asOf)).report.settings.winding_down, false)
  await post('winddown', { account: owner, as_of: Number.MAX_SAFE_INTEGER, command: { action: 'open' } }, 422)
  assert.deepEqual(await winddown.openWinddown(owner, asOf), saved, 'Rejected input preserves saved decisions')
  const checks = ['session and Origin protections', 'actual bundled catalog through Router and frontend schema', 'incomplete seed and no invented service classification', 'idempotent refresh', 'cross-account and stale confirmation rejection', 'exact local-file and summary disclosure isolation', 'private sender data absent from summary', 'other account preserved', 'invalid report date rejected without state change']
  await writeFile(`${audit}router-results.json`, JSON.stringify({ syntheticOnly: true, bridgeSha256: artifact.sha256, requests: sequence, checks, limits: 'Private pipes exercise the actual Router and client. They do not establish TCP transport, browser, native, installed or provider acceptance.' }, null, 2) + '\n')
  console.log(`Passed ${checks.length} synthetic catalog router workflow groups.`)
} finally {
  clearTimeout(deadline)
  bridge.stdin.end()
  const exit = await closed
  await writeFile(`${audit}router-bridge.log`, diagnostics)
  assert.equal(exit, 0, 'Synthetic router bridge exits after stdin closes')
  assert.equal(digest(await readFile(artifact.path)), artifact.sha256)
}
