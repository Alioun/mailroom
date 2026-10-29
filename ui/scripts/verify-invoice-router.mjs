import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/invoice-language'}/`
const builds = (await readFile(`${audit}test-build.jsonl`, 'utf8')).trim().split('\n').map(JSON.parse)
assert.deepEqual(builds.at(-1), { reason: 'build-finished', success: true })
const built = builds.findLast(item => item.reason === 'compiler-artifact' && item.target.name === 'ui_bridge' && item.profile.test && item.executable)
assert.ok(built)
const digest = bytes => createHash('sha256').update(bytes).digest('hex')
const artifact = { path: built.executable, sha256: digest(await readFile(built.executable)) }
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
  failure = new Error('Synthetic invoice workflow exceeded its deadline')
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
  assert.equal((await request('/api/accounts/invoices', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status, 403)
  await post('invoices', {}, 403, { origin: 'https://outside.example' })
  const sampleResponse = await request('/api/sample', { headers })
  assert.equal(sampleResponse.status, 200)
  const sample = await sampleResponse.json()
  const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test') }
  async function complete() {
    const end = Date.now() + 10_000
    while (Date.now() < end) {
      const { job } = await api.readAccountStatus()
      assert.ok(job)
      if (job.progress.state === 'complete') return job.account.id
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
  const initial = await api.openInvoices(owner)
  assert.equal(initial.report.semantics, 'invoice-report-evidence-v2')
  assert.ok(initial.report.warnings.some(warning => warning.kind === 'handoff_unknown'))
  assert.ok(initial.report.rows.every(row => row.accounting_acceptance === 'unknown'))
  const untouched = await api.openInvoices(other)
  const row = initial.report.rows.find(row => row.send_available)
  assert.ok(row)
  const target = initial.targets[0]
  assert.ok(target)
  const preview = await api.previewInvoice(owner, row.id, target)
  await assert.rejects(() => api.sendInvoice(other, preview))
  const workspace = await post('workspace/open', { account: owner })
  await post('workspace/save', { account: owner, revision: workspace.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
  await assert.rejects(() => api.sendInvoice(owner, preview))
  const current = await api.openInvoices(owner)
  assert.equal(current.sends.length, 0)
  const exact = await api.previewInvoice(owner, row.id, target)
  const sent = await api.sendInvoice(owner, exact)
  assert.equal(sent.sends.length, 1)
  assert.equal(sent.sends[0].status, 'completed')
  assert.ok(sent.report.rows.every(row => row.accounting_acceptance === 'unknown'))
  await assert.rejects(() => api.sendInvoice(owner, exact))
  assert.equal((await api.openInvoices(owner)).sends.length, 1)
  assert.deepEqual(await api.openInvoices(other), untouched)
  await post('invoices', { account: owner, command: { action: 'preview', selection: row.id, target, mode: 'smtp' } }, 403)
  const checks = ['session and Origin protections', 'real Router report to typed frontend', 'unknown accounting acceptance retained', 'cross-account send refused', 'changed state invalidates preview', 'one exact synthetic send', 'replay refused without duplicate effect', 'sibling account untouched', 'production SMTP denied']
  await writeFile(`${audit}router-results.json`, JSON.stringify({ syntheticOnly: true, bridgeSha256: artifact.sha256, requests: sequence, checks, limits: 'Private pipes do not establish TCP, browser, native or real-provider acceptance.' }, null, 2) + '\n')
  console.log('Invoice router workflows passed', checks.length)
} finally {
  clearTimeout(deadline)
  bridge.stdin.end()
  const exit = await closed
  await writeFile(`${audit}router-bridge.log`, diagnostics)
  assert.equal(exit, 0, 'Synthetic router bridge exits after stdin closes')
  assert.equal(digest(await readFile(artifact.path)), artifact.sha256)
}
