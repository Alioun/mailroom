import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url)).replace(/\/$/, '')
const base = 'http://127.0.0.1:4173'
assert.equal(await fetch(`${base}/api/session`).then(() => true, () => false), false, 'Stop the identity-verified retained fake before building')
const build = execFileSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 16 * 1024 * 1024 })
const binary = build.trim().split('\n').map(JSON.parse).find(a => a.reason === 'compiler-artifact' && a.profile.test && a.executable)?.executable
assert.ok(binary)
const sha = b => createHash('sha256').update(b).digest('hex')
const binary_sha256 = sha(await readFile(binary))
const directory = await mkdtemp(join(root, '.audit/organisational-proposals-fixture-'))
const test = 'server::accounts::durable_browser_server_fixture'
let server, token
const checks = []
async function start() {
  server = spawn(binary, ['--exact', test, '--nocapture'], { cwd: root, env: { ...process.env, MAILROOM_DURABLE_BROWSER_FIXTURE: directory }, stdio: 'ignore', detached: true })
  for (let i = 0; i < 300; i++) {
    assert.equal(server.exitCode, null)
    const r = await fetch(`${base}/api/session`).catch(() => null)
    if (r?.ok) { token = (await r.json()).token; return }
    await new Promise(r => setTimeout(r, 40))
  }
  throw Error('Synthetic proposal fixture startup timed out')
}
async function stop() {
  assert.equal(execFileSync('ps', ['-p', String(server.pid), '-o', 'command='], { cwd: root, encoding: 'utf8' }).trim(), `${binary} --exact ${test} --nocapture`)
  assert.ok(execFileSync('lsof', ['-a', '-p', String(server.pid), '-d', 'cwd', '-Fn'], { cwd: root, encoding: 'utf8' }).includes(`\nn${root}\n`))
  assert.equal(sha(await readFile(binary)), binary_sha256)
  assert.ok(execFileSync('lsof', ['-nP', '-a', '-p', String(server.pid), '-iTCP', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' }).includes('127.0.0.1:4173'))
  const done = once(server, 'exit'); server.kill('SIGTERM'); await done; server = undefined
}
async function post(path, body, expected = 200) {
  const r = await fetch(`${base}/api/accounts/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify(body) })
  assert.equal(r.status, expected, `Unexpected ${path} ${body.command?.action ?? ''} status`)
  assert.equal(r.headers.get('cache-control'), 'no-store')
  return r.json()
}
async function terminal() {
  for (let i = 0; i < 1000; i++) {
    const s = await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
    if (s.job?.progress.state !== 'running') { assert.equal(s.job?.progress.state, 'complete'); return s.job.account.id }
    await new Promise(r => setTimeout(r, 20))
  }
  throw Error('Synthetic census timed out')
}
const command = (account, command, expected) => post('proposals', { account, command }, expected)
const open = account => command(account, { action: 'open' })
const selection = (g, indices) => ({ account: g.account, generation: g.generation, candidates: indices.map(i => ({ id: g.candidates[i].id, change: g.candidates[i].change })) })
const preview = (account, selection, disposition = 'accept', expected) => command(account, { action: 'preview', selection, disposition }, expected)
const confirm = (account, review, extra = {}, expected) => command(account, { action: 'confirm', selection: review.selection, disposition: review.disposition, receipt: review.receipt, confirmed: true, ...extra }, expected)
async function observe(account) {
  execFileSync(binary, ['--exact', 'rules::proposals::tests::proposal_observation_worker'], { cwd: root, env: { ...process.env, MAILROOM_PROPOSALS_FIXTURE: directory, MAILROOM_PROPOSALS_ACCOUNT: account }, stdio: 'ignore' })
  return JSON.parse(await readFile(join(directory, 'proposal-observation.json'), 'utf8'))
}
await start()
try {
  for (const headers of [{}, { 'X-Mailroom-Token': token, Origin: 'https://foreign.example' }, { 'X-Mailroom-Token': token, 'Sec-Fetch-Site': 'cross-site' }]) {
    const r = await fetch(`${base}/api/accounts/proposals`, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: '{}' })
    assert.equal(r.status, 403)
  }
  checks.push('Protected session, Origin and fetch-site boundaries')
  const sample = await fetch(`${base}/api/sample`, { headers: { 'X-Mailroom-Token': token } }).then(r => r.json())
  const documents = { ...sample.documents, override_yaml: sample.documents.override_yaml.replace('home.example.test: personal', 'home.example.test: business').replace('- address: invoices@work.example.test', '- address: team@home.example.test'), preset_yaml: sample.documents.preset_yaml.replace('value: order}', "value: 'Synthetic account header'}") }
  async function newAccount() {
    await post('synthetic', {}); const id = await terminal()
    await post('scan', { account: id }); await terminal()
    const ws = await post('workspace/open', { account: id })
    await post('workspace/save', { account: id, revision: ws.revision, input: { documents, pins: [], as_of: sample.as_of, baseline: 'current' } })
    return id
  }
  const account = await newAccount(), other = await newAccount()
  await post('open', { account })
  const invoice = await post('invoices', { account, command: { action: 'open' } })
  const invoiceRow = invoice.report.rows.find(r => r.send_available)
  assert.ok(invoiceRow)
  const invoiceReview = await post('invoices', { account, command: { action: 'preview', selection: invoiceRow.id, target: 'accounting' } })
  await post('invoices', { account, command: { action: 'send', selection: invoiceRow.id, target: 'accounting', receipt: invoiceReview.receipt, confirmation_action: invoiceReview.plan.confirmation_action } })
  const subscriptions = await post('unsubscribe', { account, command: { action: 'open', as_of: sample.as_of } })
  const subscription = subscriptions.report.rows.find(r => r.sender === 'news@shop.example' && r.recipient === 'team@home.example.test')
  assert.ok(subscription)
  const muteInput = { as_of: sample.as_of, selections: [{ subscription: subscription.id, method: 'mute' }], folders: { personal: 'Muted', business: 'Quiet' } }
  const muteReview = await post('unsubscribe', { account, command: { action: 'preview', input: muteInput } })
  await post('unsubscribe', { account, command: { action: 'confirm', input: muteInput, receipt: muteReview.receipt, confirmation_action: muteReview.confirmation_action, mail_confirmation_action: muteReview.local_mail_confirmation } })
  const options = { scope: 'inbox', protect_flagged: true, selected: null }
  const backlogReview = await post('backlog', { account, command: { action: 'preview', options } })
  const backlog = await post('backlog', { account, command: { action: 'start', options, receipt: backlogReview.receipt, confirmed: true } })
  await post('backlog', { account, command: { action: 'stop', run: backlog.runs.at(-1).id } })
  const exportProof = await post('proof/preview', { account })
  await post('proof/download', { account, receipt: exportProof.receipt, confirmed: true })
  checks.push('Prepared actual synthetic invoice-send, mute, stopped backlog and export histories before proposal decisions')
  let g = await open(account)
  assert.ok(g.candidates.length >= 2)
  assert.deepEqual(await open(account), g)
  assert.ok(g.unresolved.length > 0)
  assert.ok(g.candidates.every(c => c.uncertainty.includes('unauthenticated')))
  assert.ok(g.candidates.every(c => c.impact.proof.decisions.every(d => d.destination.kind === 'inbox' && d.copies.length === 0)))
  checks.push('Deterministic ordered proposals, unresolved evidence, conservative saved-account proof')
  const s = selection(g, [0])
  const r = await preview(account, s)
  await confirm(other, r, {}, 422)
  await confirm(account, r, { confirmed: false }, 422)
  await confirm(account, r, { receipt: '0'.repeat(64) }, 422)
  const wrong = await post('proof/preview', { account })
  await confirm(account, r, { receipt: wrong.receipt }, 422)
  await confirm(account, r, { disposition: 'reject' }, 422)
  for (const selected of [ { ...s, candidates: [] }, { ...s, candidates: [...s.candidates, ...s.candidates] }, { ...s, account: other }, { ...s, candidates: [{ ...s.candidates[0], id: '0'.repeat(64) }] } ]) await preview(account, selected, 'accept', 422)
  const altered = structuredClone(s); altered.candidates[0].change.rule.mark_read = true
  await preview(account, altered, 'accept', 422)
  await command(account, { action: 'open', capabilities: ['fileinto'] }, 422)
  checks.push('Empty, duplicate, unknown, altered, wrong-account, unconfirmed, tampered and cross-purpose rejections')
  const before = await post('workspace/open', { account })
  const reject = await preview(account, s, 'reject')
  g = await confirm(account, reject)
  assert.equal(g.candidates[0].rejected, true)
  assert.deepEqual((await post('workspace/open', { account })).workspace, before.workspace)
  await confirm(account, reject, {}, 422)
  const oldToken = token
  await stop(); await start()
  assert.equal((await fetch(`${base}/api/accounts/status`, { headers: { 'X-Mailroom-Token': oldToken } })).status, 403)
  g = await open(account)
  assert.equal(g.candidates[0].rejected, true)
  assert.equal((await observe(account)).schema, 12)
  checks.push('Durable rejection survives actual process replacement and encrypted independent reopen; documents unchanged')
  let selected = selection(g, [1])
  let reviewed = await preview(account, selected)
  const unchanged = structuredClone(before.workspace.input)
  unchanged.documents.override_yaml += '\n# exact raw-byte change without executable change\n'
  const ws = await post('workspace/open', { account })
  await post('workspace/save', { account, revision: ws.revision, input: unchanged })
  await confirm(account, reviewed, {}, 422)
  g = await open(account)
  assert.equal(g.current.proof.artifact.identity, r.current.proof.artifact.identity)
  assert.notEqual(g.candidates[0].id, s.candidates[0].id)
  assert.equal(g.candidates[0].prior_rejections, 1)
  assert.equal(g.candidates[0].rejected, false)
  checks.push('Equal-parsed raw-byte edit rejects old approval and keeps prior exact rejection history')
  selected = selection(g, [1]); reviewed = await preview(account, selected)
  await writeFile(join(directory, 'ledger/addresses.yaml'), (await readFile(join(directory, 'ledger/addresses.yaml'), 'utf8')) + '\n# external source-byte edit\n')
  await confirm(account, reviewed, {}, 422)
  g = await open(account)
  selected = selection(g, [1]); reviewed = await preview(account, selected)
  await post('scan', { account }); await terminal()
  await confirm(account, reviewed, {}, 422)
  checks.push('External ledger edits and fresh census acquisition invalidate approval under existing locks')
  g = await open(account)
  const oldProof = await post('proof/preview', { account })
  selected = selection(g, [1]); reviewed = await preview(account, selected)
  const baseline = await post('workspace/open', { account })
  const ledgerBytes = await readFile(join(directory, 'ledger/addresses.yaml'))
  const prior = await observe(account)
  const accepted = await confirm(account, reviewed)
  const after = await post('workspace/open', { account })
  assert.equal(after.workspace.input.documents.preset_yaml, baseline.workspace.input.documents.preset_yaml)
  assert.equal(after.workspace.input.documents.override_yaml, reviewed.resulting_documents.override_yaml)
  assert.deepEqual(after.workspace.input.pins, baseline.workspace.input.pins)
  assert.deepEqual(after.exports, baseline.exports)
  assert.deepEqual(await readFile(join(directory, 'ledger/addresses.yaml')), ledgerBytes)
  const resulting = await observe(account)
  assert.equal(prior.unrelated_sha256, resulting.unrelated_sha256)
  assert.equal(resulting.decision_count, prior.decision_count + 1)
  await confirm(account, reviewed, {}, 422)
  await post('proof/download', { account, receipt: oldProof.receipt, confirmed: true }, 422)
  assert.notEqual((await post('proof/preview', { account })).receipt, oldProof.receipt)
  assert.ok(accepted.decisions.history.some(d => d.disposition === 'accept'))
  checks.push('Selected-only atomic rules/decision save, complete unrelated aggregate preservation, no ledger/export write, replay rejection and fresh proof')
  await stop(); await start()
  assert.equal((await observe(account)).decisions_sha256, resulting.decisions_sha256)
  checks.push('Accepted decision history survives encrypted process replacement')
  const metadata = { base, pid: server.pid, binary, binary_sha256, directory, account, other, test, branch: execFileSync('git', ['branch', '--show-current'], { cwd: root, encoding: 'utf8' }).trim(), head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() }
  await writeFile(join(root, '.audit/organisational-proposals-retained.json'), JSON.stringify(metadata, null, 2) + '\n')
  await writeFile(join(root, '.audit/organisational-proposals-runtime.json'), JSON.stringify({ ...metadata, checks, synthetic_only: true }, null, 2) + '\n')
  server.unref()
  console.log(`PASS: ${checks.length} protected HTTP behavior groups with synthetic inputs, independent encrypted reopen and actual process replacement`)
} catch (error) { if (server) await stop(); throw error }
