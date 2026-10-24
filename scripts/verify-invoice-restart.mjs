import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified fake server before this fixture')
await mkdir(join(root, '.audit'), { recursive: true })
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const directory = await mkdtemp(join(root, '.audit/invoice-restart-'))
let server, token, retained
async function start(boundary = '') {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_INVOICE_BOUNDARY: boundary }, stdio: ['ignore', 'ignore', 'ignore'], detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const r = await fetch(`${base}/api/session`).catch(() => null)
    if (r?.ok) { token = (await r.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw new Error('Fake server timeout')
}
async function stop(signal = 'SIGTERM') { const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function post(path, body, expected = 200) {
  const r = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(r.status, expected, `${path}: ${r.status}`)
  return r.json()
}
const command = (account, action, extra = {}, expected = 200) => post('invoices', { account, command: { action, ...extra } }, expected)
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw new Error('Fake census timeout')
}
try {
  for (const boundary of ['before', 'effect']) {
    await start(boundary)
    await post('synthetic', {})
    const account = await terminal()
    await post('scan', { account }); await terminal()
    const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    const workspace = await post('workspace/open', { account })
    const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business') }
    await post('workspace/save', { account, revision: workspace.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
    const opened = await command(account, 'open')
    const selection = opened.report.rows[0].id, target = 'accounting'
    const preview = await command(account, 'preview', { selection, target })
    await stop(); await start(boundary)
    const reproduced = await command(account, 'preview', { selection, target })
    assert.deepEqual(reproduced, preview, 'Exact source and full HMAC reproduce after process replacement')
    const interrupted = command(account, 'send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }).catch(() => null)
    let reached = false
    for (let i = 0; i < 1000; i++) {
      if (await stat(join(directory, `invoice-reached-${boundary}`)).then(() => true, () => false)) { reached = true; break }
      await new Promise(r => setTimeout(r, 20))
    }
    assert.ok(reached)
    const oldToken = token
    await stop('SIGKILL'); await interrupted
    await start()
    assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
    let view = await command(account, 'open')
    assert.equal(view.sends.at(-1).status, 'uncertain')
    const send = view.sends.at(-1).id
    view = await command(account, 'recover', { send })
    assert.equal(view.sends.at(-1).status, boundary === 'before' ? 'uncertain' : 'completed')
    assert.equal(Object.keys(view.received).length, boundary === 'before' ? 0 : 1)
    if (boundary === 'effect') {
      assert.deepEqual(view.received[send], preview.plan)
      assert.deepEqual(Buffer.from(view.received[send].source.original), Buffer.from(preview.plan.source.original))
      assert.deepEqual(view.received[send].source.evidence.attachments, preview.plan.source.evidence.attachments)
    }
    await command(account, 'send', { selection, target, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action }, 422)
    if (boundary === 'before') {
      const another = view.report.rows.find(r => r.send_available)
      const anotherPreview = await command(account, 'preview', { selection: another.id, target })
      await command(account, 'send', { selection: another.id, target, receipt: anotherPreview.receipt, confirmation_action: anotherPreview.plan.confirmation_action }, 422)
      const options = { scope: 'inbox', protect_flagged: true, selected: null }
      const backlog = await post('backlog', { account, command: { action: 'preview', options } })
      await post('backlog', { account, command: { action: 'start', options, receipt: backlog.receipt, confirmed: true } }, 422)
    }
    const repeated = await command(account, 'recover', { send })
    assert.deepEqual(repeated.received, view.received)
    await stop(); await start()
    const reopened = await command(account, 'open')
    assert.equal(reopened.sends.at(-1).status, view.sends.at(-1).status)
    assert.deepEqual(reopened.received, view.received)
    for (const part of ['data', 'mailboxes/invoice-transport']) {
      for (const file of await readdir(join(directory, part))) {
        const bytes = await readFile(join(directory, part, file))
        for (const marker of ['SYNTHETIC-INVOICE-PRIVATE-BYTES', 'archive@example.net', 'synthetic-approved-sender@example.test', preview.receipt, 'preset_yaml']) assert.equal(bytes.includes(Buffer.from(marker)), false, `${part} encrypted marker`)
      }
    }
    console.log(`PASS actual SIGKILL/replacement ${boundary}, full preview recovery, exact independent fake effects, no blind retry, old session denied, repeated reconciliation, encrypted markers`)
    retained = { pid: server.pid, binary, directory, account, send, base }
    if (boundary === 'before') await stop()
  }
  await writeFile(join(root, '.audit/invoice-retained.json'), JSON.stringify(retained, null, 2))
  server.unref()
  console.log('PASS retained persistent fake invoice process and fixture recorded in .audit/invoice-retained.json')
} catch (error) { if (server) await stop(); throw error }
