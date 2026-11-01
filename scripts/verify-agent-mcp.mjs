import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, readdir, writeFile, chmod, symlink } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { resolve } from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const { Client, StdioClientTransport } = require('../ui/node_modules/playwright-core/lib/utilsBundle.js')
const [binaryArgument, auditArgument] = process.argv.slice(2)
assert.ok(binaryArgument && auditArgument, 'Pass the compiled executable and an empty verification directory')
const binary = resolve(binaryArgument), audit = resolve(auditArgument), exchange = `${audit}/exchange`
await mkdir(audit, { recursive: true })
for (const part of ['home', 'temp']) await mkdir(`${audit}/${part}`, { recursive: true, mode: 0o700 })
const env = { PATH: '/opt/homebrew/bin:/usr/bin:/bin', HOME: `${audit}/home`, TMPDIR: `${audit}/temp` }
const init = spawnSync(binary, ['agent', 'init', '--folder', exchange], { env, encoding: 'utf8', timeout: 10_000 })
assert.equal(init.status, 0, init.stderr)
assert.equal(init.stdout, 'status=agent_exchange_initialized\n')
assert.ok((await readFile(`${exchange}/skills/mailroom-assistant/SKILL.md`, 'utf8')).includes('name: mailroom-assistant'))
const summary = '# Census summary\n\nInvented export. 12 newsletter messages.\n'
await writeFile(`${exchange}/reviewed/census-summary.md`, summary, { mode: 0o600 })
const summaryDigest = createHash('sha256').update(summary).digest('hex')
const proposal = {
  version: 1,
  summary_sha256: summaryDigest,
  recommendations: [{ title: '<img src=x onerror=alert(1)>', reason: 'The invented summary reports newsletters. Read history is unknown.', suggested_change: 'Review local newsletter filing. Preserve recipient holds.' }],
}
let clientCount = 0
async function connected(run) {
  const client = new Client({ name: 'mailroom-invented-acceptance', version: '1.0.0' })
  const transport = new StdioClientTransport({ command: binary, args: ['agent', 'serve', '--folder', exchange], env, stderr: 'pipe' })
  let stderr = ''
  transport.stderr.on('data', chunk => { stderr += chunk })
  try { await client.connect(transport); clientCount++; await run(client) }
  finally { await client.close() }
  assert.equal(stderr, '')
}
let staged
await connected(async client => {
  const listed = await client.listTools()
  assert.deepEqual(listed.tools.map(tool => tool.name), ['read_reviewed_summary', 'submit_proposal'])
  const read = await client.callTool({ name: 'read_reviewed_summary', arguments: {} })
  const disclosed = JSON.parse(read.content[0].text)
  assert.equal(disclosed.summary, summary)
  assert.equal(disclosed.summary_sha256, summaryDigest)
  const result = await client.callTool({ name: 'submit_proposal', arguments: proposal })
  assert.equal(result.isError, false)
  staged = JSON.parse(result.content[0].text)
  assert.equal(staged.status, 'staged_for_local_review')
  assert.equal(staged.recommendations, 1)
  assert.equal(staged.rule_edits, 0)
  assert.deepEqual(Object.keys(staged).sort(), ['proposal', 'recommendations', 'rule_edits', 'status'])
  assert.ok(/^[0-9a-f]{64}$/.test(staged.proposal))
  const repeated = await client.callTool({ name: 'submit_proposal', arguments: proposal })
  assert.equal(JSON.parse(repeated.content[0].text).proposal, staged.proposal)
  const refused = await client.callTool({ name: 'read_reviewed_summary', arguments: { path: 'DUMMY_PRIVATE_FILE' } })
  assert.equal(refused.isError, true)
  assert.ok(!JSON.stringify(refused).includes('DUMMY_PRIVATE_FILE'))
})
const proposalFile = `${exchange}/proposals/${staged.proposal}.json`
const saved = await readFile(proposalFile)
assert.deepEqual(JSON.parse(saved), proposal)
assert.equal(createHash('sha256').update(saved).digest('hex'), staged.proposal)
assert.equal((await readdir(`${exchange}/proposals`)).length, 1)
await connected(async client => {
  const retry = await client.callTool({ name: 'submit_proposal', arguments: proposal })
  assert.equal(JSON.parse(retry.content[0].text).proposal, staged.proposal)
  await writeFile(`${exchange}/reviewed/census-summary.md`, `${summary}Changed export.\n`)
  const stale = await client.callTool({ name: 'submit_proposal', arguments: proposal })
  assert.equal(stale.isError, true)
  assert.equal(stale.content[0].text, 'invalid_or_changed_proposal')
})
await writeFile(`${exchange}/reviewed/census-summary.md`, summary)
const before = await readFile(proposalFile)
await writeFile(proposalFile, 'interrupted partial file')
await connected(async client => {
  const retry = await client.callTool({ name: 'submit_proposal', arguments: proposal })
  assert.equal(retry.isError, true)
  assert.equal(await readFile(proposalFile, 'utf8'), 'interrupted partial file')
})
await writeFile(proposalFile, before)
await chmod(`${exchange}/reviewed/census-summary.md`, 0o644)
await connected(async client => {
  const read = await client.callTool({ name: 'read_reviewed_summary', arguments: {} })
  assert.equal(read.isError, true)
})
await chmod(`${exchange}/reviewed/census-summary.md`, 0o600)
const packaged = await readFile(resolve(import.meta.dirname, '../agent/AGENTS.md'))
const earlier = await readFile(resolve(import.meta.dirname, '../agent/superseded/021643c/AGENTS.md'))
await writeFile(`${exchange}/AGENTS.md`, earlier)
const updated = spawnSync(binary, ['agent', 'init', '--folder', exchange], { env, encoding: 'utf8', timeout: 10_000 })
assert.equal(updated.status, 0, updated.stderr)
assert.deepEqual(await readFile(`${exchange}/AGENTS.md`), packaged)
await writeFile(`${exchange}/AGENTS.md`, `${earlier}Owner note.\n`)
const kept = spawnSync(binary, ['agent', 'init', '--folder', exchange], { env, encoding: 'utf8', timeout: 10_000 })
assert.equal(kept.status, 65)
assert.equal(kept.stderr, 'status=agent_exchange_unavailable\n')
assert.equal(await readFile(`${exchange}/AGENTS.md`, 'utf8'), `${earlier}Owner note.\n`)
await writeFile(`${exchange}/AGENTS.md`, packaged)
await symlink(exchange, `${audit}/linked-exchange`)
const linked = spawnSync(binary, ['agent', 'serve', '--folder', `${audit}/linked-exchange`], { env, encoding: 'utf8', timeout: 10_000 })
assert.equal(linked.status, 65)
assert.equal(linked.stdout, '')
assert.equal(linked.stderr, 'status=agent_exchange_unavailable\n')
await writeFile(`${audit}/result.json`, JSON.stringify({ status: 'passed', sdkClients: clientCount, summaryDigest, proposal: staged.proposal, proposalFile, executable: binary, executableSha256: createHash('sha256').update(await readFile(binary)).digest('hex') }, null, 2) + '\n')
console.log(JSON.stringify({ status: 'passed', sdkClients: clientCount, proposalFiles: 1 }))
