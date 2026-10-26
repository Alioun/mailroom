import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir, mkdtemp, open, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { request as httpRequest } from 'node:http'
export const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
export const base = 'http://127.0.0.1:4173'
export const pause = ms => new Promise(r => setTimeout(r, ms))
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const test = 'server::accounts::gmail_backlog_server_fixture'
const observer = 'acquisition::gmail_backlog::gmail_workflow_observer'
export async function fixture() {
  assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Identity-stop the retained fixture before any rebuild')
  let build
  try {
    build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_TEST_OPT_LEVEL: '1' }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
  } catch (error) {
    const log = join(root, `.audit/gmail-label-backlog-build-failed-${Date.now()}`)
    await writeFile(`${log}.jsonl`, error.stdout ?? '')
    await writeFile(`${log}.log`, error.stderr ?? '')
    throw Error(`Cargo fixture build failed. Full diagnostics are in ${log}.jsonl and ${log}.log`)
  }
  const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
  assert.ok(binary)
  const binary_sha256 = sha(await readFile(binary))
  let server, directory, token, account, installation
  const metadata = () => ({ binary, binary_sha256, pid: server?.pid, cwd: root, test, directory, account })
  const api = {
    get directory() { return directory }, get installation() { return installation ?? directory },
    restoreInto(path) { installation = path }, get account() { return account }, get binary() { return binary },
    async start(fresh = false, installed = true, profile = {}) {
      if (fresh) {
        installation = undefined
        directory = await mkdtemp(join(root, '.audit/gmail-label-backlog-fixture-'))
        if (profile.large) await writeFile(join(directory, 'gmail-large'), '501 exact eligible messages')
        if (profile.ledger) {
          await mkdir(join(directory, 'ledger'), { mode: 0o700 })
          await writeFile(join(directory, 'ledger/addresses.yaml'), 'version: 1\nenabled: false\nentries:\n  - address: retained+fixture@example.test\n    service: Synthetic retained history\n    folder: Purchases\n    first_seen: supplied synthetic history\n    status: active\n    note: Preserve this synthetic note\nowner_extension: {preserve: [exact, bytes]}\n', { mode: 0o600 })
        }
        if (installed) await writeFile(join(directory, 'workspace-authority'), 'explicit-gmail-workspace-fixture-v1')
      }
      const output = await open(join(directory, `application-${Date.now()}.log`), 'wx')
      server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_GMAIL_FIXTURE: directory, ...(installation ? { MAILROOM_GMAIL_INSTALLATION: installation } : {}) }, stdio: ['ignore', output.fd, output.fd], detached: true })
      await output.close()
      await writeFile(join(directory, `launch-${server.pid}.json`), JSON.stringify(metadata(), null, 2))
      for (let i = 0; i < 800; i++) {
        assert.equal(server.exitCode, null, 'Gmail application fixture exited during startup')
        const response = await fetch(`${base}/api/session`).catch(() => null)
        if (response?.ok) {
          token = (await response.json()).token
          account = (await api.observe('provider_only')).account
          await writeFile(join(directory, 'process-identity.json'), JSON.stringify(metadata(), null, 2))
          await writeFile(join(root, '.audit/gmail-label-backlog-runtime.json'), JSON.stringify(metadata(), null, 2))
          return
        }
        await pause(20)
      }
      throw Error('Gmail application fixture startup timed out')
    },
    async stop(signal = 'SIGTERM') {
      if (server.exitCode !== null || server.signalCode !== null) {
        await writeFile(join(directory, `exited-${server.pid}.json`), JSON.stringify({ ...metadata(), exit_code: server.exitCode, signal: server.signalCode, at: new Date().toISOString() }))
        server = undefined
        return
      }
      assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
      assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
      assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(server.pid), '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
      assert.equal(sha(await readFile(binary)), binary_sha256)
      const done = once(server, 'exit'); server.kill(signal); await done
      await writeFile(join(directory, `stopped-${Date.now()}.json`), JSON.stringify({ ...metadata(), signal, at: new Date().toISOString(), exact_identity_checked: true }))
      server = undefined
    },
    async observe(action = '') {
      const log = join(directory, `observer-${Date.now()}.log`)
      const output = await open(log, 'wx')
      try {
        execFileSync(binary, ['--exact', observer], { cwd: root, env: { ...process.env, MAILROOM_GMAIL_FIXTURE: directory, MAILROOM_GMAIL_ACTION: action, ...(installation ? { MAILROOM_GMAIL_INSTALLATION: installation } : {}) }, stdio: ['ignore', output.fd, output.fd] })
      } catch { throw Error(`Independent observer failed. Diagnostics are in ${log}`) }
      finally { await output.close() }
      return JSON.parse(await readFile(join(directory, 'gmail-observations.json'), 'utf8'))
    },
    async post(path, body, expected = 200, extra = {}) {
      const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...extra }, body: JSON.stringify(body) })
      const value = await response.json()
      assert.equal(response.status, expected, `Unexpected status for ${path}: ${value.error ?? ''}`)
      assert.equal(response.headers.get('cache-control'), 'no-store')
      return value
    },
    async raw(path, body, expected, extra = {}) {
      const value = await new Promise((resolve, reject) => {
        const request = httpRequest(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body), 'X-Mailroom-Token': token, ...extra } }, response => {
          let data = ''; response.setEncoding('utf8'); response.on('data', chunk => { data += chunk });
          response.on('end', () => resolve({ status: response.statusCode, cache: response.headers['cache-control'], body: JSON.parse(data) }))
        }); request.on('error', reject); request.end(body)
      })
      assert.equal(value.status, expected, `Unexpected raw status for ${path} with ${Object.keys(extra).join(', ')}`)
      assert.equal(value.cache, 'no-store')
      return value.body
    },
    command(action, fields = {}, expected = 200) { return api.post('backlog/gmail', { account, command: { action, ...fields } }, expected) },
    async terminal() {
      for (let i = 0; i < 2000; i++) {
        const value = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
        if (value.job?.progress.state !== 'running') return value.job
        await pause(20)
      }
      throw Error('Gmail worker deadline exceeded')
    },
    async reached(name) {
      for (let i = 0; i < 3000; i++) { if (await readFile(join(directory, `reached-${name}`)).then(() => true, () => false)) return; await pause(10) }
      throw Error(`Gmail boundary not reached: ${name}`)
    },
    async boundary(name) { await writeFile(join(directory, 'gmail-boundary'), name) },
    async release(name) { await writeFile(join(directory, `continue-${name}`), 'continue') },
    async mode(value) { await writeFile(join(directory, 'gmail-mode'), value) },
    async savedWorkspace(previous = false) {
      const view = await api.post('workspace/open', { account })
      const input = structuredClone(view.workspace.input)
      input.baseline = 'current'
      await api.post('workspace/save', { account, revision: view.revision, input })
      const proof = await api.post('proof/preview', { account })
      assert.equal(proof.proof.artifact.context.recipient_evidence, 'synthetic_delivered_to')
      assert.equal(proof.download_available, false, 'Gmail rules never download a Sieve script or Gmail filter')
      if (previous) {
        const current = await api.post('workspace/open', { account }); input.baseline = 'previous'
        await api.post('workspace/save', { account, revision: current.revision, input })
      }
      return api.post('proof/preview', { account })
    },
    async close() { if (server) await api.stop() },
  }
  return api
}
