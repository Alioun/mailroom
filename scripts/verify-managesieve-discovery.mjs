import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { request } from 'node:http'

const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the verified retained fake before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const binary_sha256 = sha(await readFile(binary))
const directory = await mkdtemp(join(root, '.audit/managesieve-fixture-'))
const test = 'server::accounts::managesieve_server_fixture'
let server, token
const checks = []
const pause = ms => new Promise(r => setTimeout(r, ms))
async function start() {
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 300; i++) {
    assert.equal(server.exitCode, null)
    const response = await fetch(`${base}/api/session`).catch(() => null)
    if (response?.ok) { token = (await response.json()).token; return }
    await pause(20)
  }
  throw Error('Local Sieve fixture startup timed out')
}
function identity() {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
}
async function stop(signal = 'SIGTERM') {
  identity(); assert.equal(sha(await readFile(binary)), binary_sha256)
  const done = once(server, 'exit'); server.kill(signal); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const response = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(response.status, expected, `Unexpected status for ${path}`)
  assert.equal(response.headers.get('cache-control'), 'no-store')
  return response.json()
}
const command = (account, action, expected = 200) => post('managesieve', { account, command: { action } }, expected)
async function terminal(expected = 'complete') {
  for (let i = 0; i < 1000; i++) {
    const response = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } })
    const s = await response.json()
    if (s.job?.progress.state !== 'running') { assert.equal(s.job?.progress.state, expected); return s.job }
    await pause(10)
  }
  throw Error('Local Sieve job did not finish')
}
async function discover(account, mode, expected = 'complete') {
  await writeFile(join(directory, 'sieve-mode'), mode)
  await command(account, 'start'); await terminal(expected)
  return command(account, 'open')
}
function worker(account, name) {
  execFileSync(binary, ['--exact', `acquisition::sieve_discovery::tests::${name}`], { cwd: root, env: { ...process.env, MAILROOM_SIEVE_FIXTURE: directory, MAILROOM_SIEVE_ACCOUNT: account }, stdio: 'ignore' })
}
async function observe(account) { worker(account, 'sieve_independent_reopen_worker'); return JSON.parse(await readFile(join(directory, 'sieve-reopen.json'), 'utf8')) }
async function wireCount() { return (await readFile(join(directory, 'sieve-wire.log'), 'utf8').catch(() => '')).split('\n').filter(c => c === 'GETSCRIPT').length }
async function waitingRead(account) {
  await writeFile(join(directory, 'sieve-mode'), 'stall_read')
  const count = await wireCount(); await command(account, 'start')
  for (let i = 0; i < 100; i++) { if (await wireCount() > count) return; await pause(5) }
  throw Error('Expected actual GETSCRIPT not reached')
}
await start()
try {
  assert.equal((await fetch(`${base}/api/accounts/managesieve`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403)
  for (const headers of [{ Origin: 'https://foreign.example' }, { 'Sec-Fetch-Site': 'cross-site' }]) assert.equal((await fetch(`${base}/api/accounts/managesieve`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token, ...headers }, body: '{}' })).status, 403)
  const badHost = await new Promise((resolve, reject) => { const req = request(`${base}/api/session`, { headers: { Host: 'foreign.example' } }, res => { res.resume(); resolve(res.statusCode) }); req.on('error', reject); req.end() })
  assert.equal(badHost, 403)
  checks.push('Token, Host, Origin, fetch-site and no-store protections')
  await post('synthetic', {}); const account = (await terminal()).account.id
  worker(account, 'sieve_seed_worker')
  for (const field of ['endpoint', 'root', 'credential', 'profile', 'source', 'capabilities', 'baseline', 'trust', 'provider', 'permit']) {
    await post('managesieve', { account, command: { action: 'start' }, [field]: 'caller-claim' }, 422)
    await post('managesieve', { account, command: { action: 'start', [field]: 'caller-claim' } }, 422)
  }
  checks.push('Unknown fields and caller trust/endpoint/authority claims reject')
  const before = await observe(account)
  const good = await discover(account, 'normal')
  assert.equal(good.current, true); assert.equal(good.facts.latest.outcome, 'complete')
  assert.equal(good.facts.last_successful.names.length, 2)
  const baseline = good.facts.last_successful
  assert.equal(baseline.bodies.length, 1)
  assert.equal(sha(Buffer.from(baseline.bodies[0].bytes)), baseline.bodies[0].sha256)
  assert.equal((await observe(account)).unrelated_sha256, before.unrelated_sha256)
  assert.equal((await observe(account)).schema, 21)
  checks.push('Actual STARTTLS/PLAIN/authenticated CAPABILITY/list/get and independent encrypted reopen preserve populated aggregates')
  for (const mode of ['auth_denied', 'missing', 'partial_list', 'unstable', 'unstable_names']) {
    const failed = await discover(account, mode, 'failed')
    assert.equal(failed.current, false); assert.ok(JSON.stringify(failed.facts.last_successful) === JSON.stringify(baseline))
    assert.equal((await observe(account)).unrelated_sha256, before.unrelated_sha256)
  }
  checks.push('Failed and unstable refreshes retain exact prior baseline and unrelated aggregates')
  await waitingRead(account)
  await command('wrong-account', 'cancel', 422)
  await post('scan', { account }, 422)
  await command(account, 'cancel'); await terminal('cancelled')
  assert.equal((await command(account, 'open')).facts.latest.outcome, 'cancelled')
  checks.push('Responsive actual-read cancellation, cross-account denial and competing census rejection')
  await waitingRead(account)
  const oldToken = token; await stop('SIGKILL'); await start()
  assert.notEqual(token, oldToken)
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  const interrupted = await command(account, 'open')
  assert.equal(interrupted.facts.latest.outcome, 'interrupted'); assert.equal(interrupted.current, false)
  assert.ok(JSON.stringify(interrupted.facts.last_successful) === JSON.stringify(baseline))
  assert.equal((await observe(account)).unrelated_sha256, before.unrelated_sha256)
  checks.push('SIGKILL/replacement marks unfinished attempt interrupted, preserves bytes and replaces launch token')
  const changed = await discover(account, 'changed')
  assert.deepEqual(changed.facts.last_successful.names, [{ name: 'new', active: false }]); assert.deepEqual(changed.facts.last_successful.bodies, [])
  assert.equal(changed.facts.last_successful.capabilities.SIEVE, 'fileinto')
  const empty = await discover(account, 'empty')
  assert.deepEqual(empty.facts.last_successful.names, []); assert.equal(empty.current, true)
  checks.push('Successful replacement removes disappeared names/extensions and distinguishes complete zero scripts')
  await waitingRead(account)
  await post('lifecycle', { action: 'replace_password', account, password: 'invented-replacement' })
  const afterLifecycle = await command(account, 'open')
  assert.equal(afterLifecycle.facts.latest.outcome, 'cancelled'); assert.equal(afterLifecycle.current, false)
  assert.ok(JSON.stringify(afterLifecycle.facts.last_successful) === JSON.stringify(empty.facts.last_successful))
  assert.equal((await observe(account)).unrelated_sha256, before.unrelated_sha256)
  checks.push('Local password lifecycle cancels and joins discovery, preserves baseline/history and invalidates current authority')
  await discover(account, 'normal')
  const wire = (await readFile(join(directory, 'sieve-wire.log'), 'utf8')).trim().split('\n')
  assert.ok(wire.every(c => ['AUTHENTICATE [REDACTED]', 'CAPABILITY', 'LISTSCRIPTS', 'GETSCRIPT', 'LOGOUT'].includes(c)))
  assert.ok(wire.every(c => !c.includes('invented') && !c.includes('keep;')))
  checks.push('Retained fake command log contains only permitted command names and redacted authentication')
  identity()
  const metadata = { base, pid: server.pid, binary, binary_sha256, directory, account, test, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/managesieve-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/managesieve-runtime.json'), JSON.stringify({ ...metadata, checks, synthetic_only: true }, null, 2) + '\n')
  server.unref()
  console.log(`PASS: ${checks.length} ManageSieve protected HTTP/runtime groups with actual loopback TLS, encrypted independent reopen and SIGKILL replacement`)
} catch (error) { if (server) await stop(); throw error }
