import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, readFile, readdir, stat, unlink, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified fake server first')
await mkdir(join(root, '.audit'), { recursive: true })
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const directory = await mkdtemp(join(root, '.audit/unsubscribe-restart-'))
let server, token, retained
async function start(boundary = '') {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_UNSUBSCRIBE_BOUNDARY: boundary }, stdio: ['ignore', 'ignore', 'ignore'], detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw Error('Fake fixture start timed out')
}
async function stop(signal = 'SIGTERM') { const done = once(server, 'exit'); server.kill(signal); await done; server = undefined }
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `${path} status`)
  return response.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Fake census timeout')
}
const command = (account, command, expected) => post('unsubscribe', { account, command }, expected)
try {
  for (const [boundary, method] of [['before', 'one_click'], ['effect', 'one_click'], ['effect', 'local_mail']]) {
    await start(boundary)
    await post('synthetic', {}); const account = await terminal()
    await post('scan', { account }); await terminal()
    const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    const workspace = await post('workspace/open', { account })
    await post('workspace/save', { account, revision: workspace.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
    const view = await command(account, { action: 'open', as_of: sample.as_of })
    const row = view.report.rows.find(r => r.methods.includes(method))
    assert.ok(row)
    const input = { as_of: sample.as_of, selections: [{ subscription: row.id, method }], folders: { personal: 'Muted', business: 'Quiet' } }
    const preview = await command(account, { action: 'preview', input })
    await stop(); await start(boundary)
    assert.deepEqual(await command(account, { action: 'preview', input }), preview, 'Full exact preview and HMAC reproduce after process replacement')
    const send = { action: 'confirm', input, receipt: preview.receipt, confirmation_action: preview.confirmation_action, mail_confirmation_action: preview.local_mail_confirmation }
    await unlink(join(directory, `unsubscribe-reached-${boundary}`)).catch(() => {})
    const pending = command(account, send).catch(() => null)
    let reached = false
    for (let i = 0; i < 1000; i++) {
      if (await stat(join(directory, `unsubscribe-reached-${boundary}`)).then(() => true, () => false)) { reached = true; break }
      await new Promise(r => setTimeout(r, 20))
    }
    assert.ok(reached)
    const oldToken = token
    await stop('SIGKILL'); await pending; await start()
    assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
    let reopened = await command(account, { action: 'open', as_of: sample.as_of })
    const entry = reopened.history.at(-1).id
    assert.equal(reopened.history.at(-1).status, 'uncertain')
    reopened = await command(account, { action: 'recover', entry, as_of: sample.as_of })
    const status = boundary === 'before' ? 'uncertain' : method === 'local_mail' ? 'mail_handoff' : 'http_observed'
    assert.equal(reopened.history.at(-1).status, status)
    assert.equal(reopened.history.at(-1).success, 'unknown')
    assert.equal(Object.keys(reopened.received).length, boundary === 'before' ? 0 : 1)
    if (boundary === 'effect') {
      const ack = reopened.received[entry]
      assert.equal(ack.payload_digest, preview.actions[0].payload_digest)
      assert.equal(ack.host, preview.actions[0].host)
      assert.equal(ack.verb, method === 'one_click' ? 'POST' : 'MAIL')
      assert.equal(ack.mail_handoff, method === 'local_mail')
      assert.equal(ack.form, method === 'one_click' ? 'List-Unsubscribe=One-Click' : null)
      assert.deepEqual(ack.headers, method === 'one_click' ? { 'Content-Type': 'application/x-www-form-urlencoded' } : {})
    }
    await command(account, send, 422)
    if (boundary === 'before') {
      const fresh = await command(account, { action: 'preview', input })
      await command(account, { ...send, receipt: fresh.receipt }, 422)
    }
    const again = await command(account, { action: 'recover', entry, as_of: sample.as_of })
    assert.deepEqual(again.received, reopened.received)
    for (const payload of [reopened.history, reopened.received]) {
      const json = JSON.stringify(payload)
      for (const marker of ['synthetic-private-token', 'synthetic-mail-token', 'https://shop.example/remove']) assert.equal(json.includes(marker), false, 'Serialized plaintext journals/acks are token safe')
    }
    await stop(); await start()
    const replaced = await command(account, { action: 'open', as_of: sample.as_of })
    assert.equal(replaced.history.at(-1).status, status)
    assert.deepEqual(replaced.received, reopened.received)
    for (const part of ['data', 'mailboxes/unsubscribe-transport']) {
      for (const file of await readdir(join(directory, part))) {
        const bytes = await readFile(join(directory, part, file))
        for (const marker of ['synthetic-private-token', 'synthetic-mail-token', preview.receipt, 'preset_yaml']) assert.equal(bytes.includes(Buffer.from(marker)), false, 'Encrypted persistence')
      }
    }
    console.log(`PASS actual SIGKILL/replacement ${boundary} ${method}, exact full HMAC reproduction, independent durable effects, token-safe plaintext history/acks, no retry, old session denied, repeated recovery`)
    retained = { pid: server.pid, binary, directory, account, entry, base }
    if (method !== 'local_mail') await stop()
  }
  await writeFile(join(root, '.audit/unsubscribe-retained.json'), JSON.stringify(retained, null, 2))
  server.unref()
  console.log('PASS retained final fake process recorded in .audit/unsubscribe-retained.json')
} catch (error) { if (server) await stop(); throw error }
