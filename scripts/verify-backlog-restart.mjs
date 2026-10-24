import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the fake-only server before this fixture')
await mkdir(join(root, '.audit'), { recursive: true })
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(line => JSON.parse(line)).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const directory = await mkdtemp(join(root, '.audit/backlog-restart-'))
let server, token, lastAccount, lastRun
async function start(boundary = '') {
  server = spawn(binary, ['--exact', 'server::accounts::durable_browser_server_fixture', '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory, MAILROOM_BACKLOG_BOUNDARY: boundary }, stdio: ['ignore', 'ignore', 'ignore'], detached: true })
  for (let i = 0; i < 200; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await new Promise(r => setTimeout(r, 50))
  }
  throw new Error('Synthetic fixture did not start')
}
async function stop(signal = 'SIGTERM') {
  if (!server) return
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `${path} unexpected status`)
  return response.json()
}
const command = (account, action, extra = {}, expected = 200) => post('backlog', { account, command: { action, ...extra } }, expected)
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job.progress.state !== 'running') { assert.equal(s.job.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw new Error('Synthetic census timeout')
}
async function reached(boundary) {
  for (let i = 0; i < 1000; i++) {
    if (await stat(join(directory, `reached-${boundary}`)).then(() => true, () => false)) return
    await new Promise(r => setTimeout(r, 20))
  }
  throw new Error(`Boundary ${boundary} was not reached`)
}
function canonical(mailbox) {
  return Object.values(mailbox.folders).flatMap(f => Object.values(f.messages)).map(m => ({ hash: m.headers.message_id_hash, flags: m.flags, labels: m.gmail_labels })).sort((a,b) => a.hash.localeCompare(b.hash))
}
try {
  for (const gmail of [false, true]) {
    for (const boundary of ['before', 'effect', 'complete']) {
      await start(boundary)
      await post(gmail ? 'synthetic-gmail' : 'synthetic', {})
      const account = await terminal(); lastAccount = account
      await post('scan', { account }); await terminal()
      const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
      const saved = await post('workspace/open', { account })
      await post('workspace/save', { account, revision: saved.revision, input: { documents: sample.documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
      const original = await command(account, 'open')
      const options = { scope: 'inbox_unfiled', protect_flagged: true, selected: null }
      const preview = await command(account, 'preview', { options })
      await stop(); await start(boundary)
      const reproduced = await command(account, 'preview', { options })
      assert.equal(reproduced.receipt, preview.receipt, 'Unchanged exact plan receipt survives process replacement')
      const prepared = await command(account, 'start', { options, receipt: preview.receipt, confirmed: true })
      const id = prepared.runs.at(-1).id; lastRun = id
      await rm(join(directory, `reached-${boundary}`), { force: true })
      const interrupted = command(account, 'advance', { run: id }).catch(() => null)
      await reached(boundary)
      const oldToken = token
      await stop('SIGKILL'); await interrupted
      await start()
      assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
      const recovered = await command(account, 'open')
      assert.equal(recovered.runs.at(-1).status, boundary === 'complete' ? 'completed' : 'uncertain')
      if (boundary !== 'complete') await command(account, 'resume', { run: id })
      let after = await command(account, 'advance', { run: id })
      assert.equal(after.runs.at(-1).status, 'completed')
      assert.equal(after.runs.at(-1).outcomes.filter(o => o.state === 'applied').length, preview.plan.changes.length)
      assert.equal(canonical(after.mailbox).length, canonical(original.mailbox).length)
      const stable = after.mailbox
      after = await command(account, 'advance', { run: id }); assert.deepEqual(after.mailbox, stable)
      if (boundary === 'effect') {
        await stop(); await start('undo-effect')
        const up = await command(account, 'undo_preview', { run: id })
        await rm(join(directory, 'reached-undo-effect'), { force: true })
        const interruptedUndo = command(account, 'undo', { run: id, receipt: up.receipt, confirmed: true }).catch(() => null)
        await reached('undo-effect'); await stop('SIGKILL'); await interruptedUndo
        await start()
        after = await command(account, 'resume_undo', { run: id })
      } else {
        const up = await command(account, 'undo_preview', { run: id })
        after = await command(account, 'undo', { run: id, receipt: up.receipt, confirmed: true })
      }
      assert.equal(after.runs.at(-1).undo.filter(u => u.state === 'restored').length, preview.plan.changes.length)
      assert.deepEqual(canonical(after.mailbox), canonical(original.mailbox))
      const repeated = await command(account, 'resume_undo', { run: id }); assert.deepEqual(repeated.mailbox, after.mailbox)
      for (const part of ['data', 'mailboxes']) {
        for (const file of await readdir(join(directory, part))) {
          const bytes = await readFile(join(directory, part, file))
          for (const marker of [id, 'Synthetic account header', 'team@home.example.test', preview.plan.changes[0].destination, 'preset_yaml']) assert.equal(bytes.includes(Buffer.from(marker)), false)
        }
      }
      console.log(`PASS actual SIGKILL/replacement ${gmail ? 'Gmail' : 'IMAP'} ${boundary}, exact recovery, repeated resume, conditional undo${boundary === 'effect' ? ' across undo-effect crash' : ''}, ciphertext markers and rotated launch session`)
      await stop()
    }
  }
  if (process.env.MAILROOM_BACKLOG_RETAIN === '1') {
    await start()
    await writeFile(join(root, '.audit/backlog-retained.json'), JSON.stringify({ directory, pid: server.pid, binary, account: lastAccount, run: lastRun }, null, 2))
    console.log(`RETAINED fake-only PID ${server.pid}, fixture ${directory}`)
    server.unref(); server = undefined
  }
} finally {
  await stop()
  if (process.env.MAILROOM_BACKLOG_RETAIN !== '1') await rm(directory, { recursive: true, force: true })
}
