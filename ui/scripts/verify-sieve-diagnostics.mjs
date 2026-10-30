import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const baseline = process.argv.includes('--baseline')
const phase = baseline ? 'baseline' : 'current'
const audit = `${root}.audit/sieve-diagnostics/${phase}/`
await mkdir(`${audit}source`, { recursive: true })
const preceding = path => execFileSync('/usr/bin/git', ['-c', 'core.fsmonitor=false', 'show', `5e3998c441320a4039292a0faf61cbd4be996f43:${path}`], { cwd: root, encoding: 'utf8' })
const enumValues = name => [...rust.match(new RegExp(`pub enum ${name} \\{([\\s\\S]*?)\\}`))[1].matchAll(/^\s*([A-Z][A-Za-z0-9_]*),/gm)].map(([, value]) => value.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase())
const rust = await readFile(`${root}src/census/sieve.rs`, 'utf8')
const failures = enumValues('Failure')
const outcomes = enumValues('Outcome')
assert.equal(failures.length, 15)
assert.equal(outcomes.length, 5)
const compiled = JSON.parse(await readFile(`${root}.audit/transaction-capacity/compiled-fixtures/5.json`, 'utf8'))
const { context: transportContext, ...original } = compiled.journal.history[0]
assert.equal(transportContext.adapter, 'managesieve-read-v1')
assert.equal(original.steps.length, 1)
const baseTransaction = { installed: true, recovery_hold: false, placement: { kind: 'block' }, limitations: 'Constructed display diagnostics only', journal: { history: [] } }
const context = { account: 'diagnostic-account', provider: 'mailbox_org', profile: 'fixture', source_epoch: 'fixture', tls_identity: 'fixture', authority: 'fixture', adapter: 'managesieve-read-v1' }
const attempt = { id: 'diagnostic-attempt', started_at: 1793260800, finished_at: 1793260801, context, outcome: 'failed', failure: null, capabilities: null, names: null, bodies: [] }
const baseDiscovery = { installed: true, current: false, recovery_hold: false, facts: { version: 1, latest: null, last_successful: null }, limits: { script_bytes: 1048576, attempt_bytes: 4194304, names: 256 }, limitations: 'Constructed display diagnostics only' }
const cases = []
for (const code of failures) {
  const transaction = structuredClone(original)
  transaction.status = 'uncertain'
  transaction.validation = { Err: code }
  transaction.space = { Err: code }
  transaction.steps[0].failure = code
  transaction.steps[0].outcome = 'uncertain'
  cases.push({ name: `transaction-${code}`, panel: 'SieveTransactions', code, view: { ...baseTransaction, journal: { history: [transaction] } } })
  cases.push({ name: `discovery-${code}`, panel: 'ManageSieve', code, outcome: 'failed', view: { ...baseDiscovery, facts: { ...baseDiscovery.facts, latest: { ...attempt, failure: code } } } })
}
for (const outcome of outcomes) cases.push({ name: `outcome-${outcome}`, panel: 'ManageSieve', outcome, view: { ...baseDiscovery, facts: { ...baseDiscovery.facts, latest: { ...attempt, outcome } } } })
for (const state of ['pending', 'success']) {
  const transaction = structuredClone(original)
  transaction.validation = state === 'pending' ? null : { Ok: null }
  transaction.space = state === 'pending' ? null : { Ok: null }
  transaction.steps[0].failure = null
  cases.push({ name: `transaction-${state}`, panel: 'SieveTransactions', view: { ...baseTransaction, journal: { history: [transaction] } } })
}
cases.push({ name: 'no-discovery', panel: 'ManageSieve', view: baseDiscovery })
const sources = {}
for (const panel of ['SieveTransactions', 'ManageSieve']) sources[panel] = baseline ? preceding(`ui/src/${panel}.svelte`) : await readFile(`${root}ui/src/${panel}.svelte`, 'utf8')
const apis = {}
for (const name of ['sieve-transactions-api', 'managesieve-api']) {
  apis[name] = baseline ? `${audit}source/${name}.ts` : `${root}ui/src/${name}.ts`
  if (baseline) await writeFile(apis[name], preceding(`ui/src/${name}.ts`).replace("from './api'", `from '${root}ui/src/api'`).replace("'zod'", JSON.stringify(import.meta.resolve('zod'))))
}
for (const [index, item] of cases.entries()) {
  const kind = item.panel === 'ManageSieve' ? 'SieveView' : 'TransactionView'
  const initializer = `let view = $state<${kind} | null>(null)`
  assert.ok(sources[item.panel].includes(initializer))
  let component = sources[item.panel].replace(initializer, () => `let view = $state<${kind} | null>(${JSON.stringify(item.view).replaceAll('<', '\\u003c')})`)
  component = component.replace("'@ark-ui/svelte/radio-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/radio-group'))).replaceAll("from './", `from '${root}ui/src/`)
  for (const [name, api] of Object.entries(apis)) component = component.replace(`from '${root}ui/src/${name}'`, `from '${api}'`)
  await writeFile(`${audit}source/Case${index}.svelte`, component)
}
const extra = baseline ? '' : `export * from '${root}ui/src/sieve-diagnostics';`
await writeFile(`${audit}source/entry.mjs`, `${cases.map((_, i) => `export { default as Case${i} } from './Case${i}.svelte'`).join('; ')}; export * from '${apis['sieve-transactions-api']}'; export * from '${apis['managesieve-api']}'; ${extra} export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}source/entry.mjs`, outDir: `${audit}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'diagnostics.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}built/diagnostics.mjs`))
let reply
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.ok(['/api/accounts/sieve-transactions', '/api/accounts/managesieve'].includes(path))
  requests++
  return Response.json(reply)
}
for (const item of cases) {
  reply = item.view
  const parsed = item.panel === 'ManageSieve' ? await ui.openSieve('diagnostic-account') : await ui.transactionStatus('diagnostic-account')
  assert.deepEqual(parsed, reply)
}
const malformed = []
for (const code of ['future_failure', 'Tls', '', 7, { code: 'tls' }]) {
  for (const field of ['validation', 'space', 'step', 'attempt']) {
    const item = structuredClone(cases[field === 'attempt' ? 1 : 0])
    if (field === 'attempt') item.view.facts.latest.failure = code
    else if (field === 'step') item.view.journal.history[0].steps[0].failure = code
    else item.view.journal.history[0][field] = { Err: code }
    malformed.push({ ...item, name: `${field}-${JSON.stringify(code)}` })
  }
}
for (const field of ['validation', 'space']) {
  for (const value of [{ Ok: null, Err: 'tls' }, {}, { Err: 'tls', extra: 'unexpected' }]) {
    const item = structuredClone(cases[0])
    item.view.journal.history[0][field] = value
    malformed.push({ ...item, name: `${field}-ambiguous-${JSON.stringify(value)}` })
  }
}
const unknownOutcome = structuredClone(cases[1])
unknownOutcome.view.facts.latest.outcome = 'future_outcome'
malformed.push({ ...unknownOutcome, name: 'unknown-discovery-outcome' })
const parsing = []
for (const item of malformed) {
  reply = item.view
  let accepted = false
  try { if (item.panel === 'ManageSieve') await ui.openSieve('diagnostic-account'); else await ui.transactionStatus('diagnostic-account'); accepted = true } catch {}
  if (!baseline) assert.equal(accepted, false, item.name)
  parsing.push({ name: item.name, accepted })
}
if (!baseline) {
  assert.deepEqual(ui.sieveFailure.options, failures)
  assert.deepEqual(ui.sieveDiscoveryOutcome.options, outcomes)
}
const renderings = []
const controls = body => [...body.matchAll(/<button\b[^>]*>[\s\S]*?<\/button>|<input\b[^>]*>/g)].map(([full]) => full.replace(/svelte-[\w-]+/g, 'svelte-scope'))
for (const [index, item] of cases.entries()) {
  for (const lang of ['en', 'de']) {
    const { body } = ui.render(ui[`Case${index}`], { props: { account: 'diagnostic-account', lang } })
    const clean = body.replace(/<!--[\s\S]*?-->/g, '')
    const text = clean.replace(/<[^>]*>/g, '')
    if (baseline && lang === 'de' && item.code) assert.ok(text.includes(item.code))
    if (!baseline && item.code) {
      const message = ui.sieveFailureText({ failure: item.code, lang })
      const expected = item.panel === 'SieveTransactions' ? 3 : 1
      assert.equal(text.split(message).length - 1, expected)
      if (item.code.includes('_')) assert.ok(!text.includes(item.code))
    }
    if (!baseline && item.outcome) assert.ok(text.includes(ui.sieveDiscoveryOutcomeText({ outcome: item.outcome, lang })))
    await writeFile(`${audit}${item.name}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
    renderings.push({ name: item.name, lang, controls: controls(clean) })
  }
}
if (baseline) assert.ok(parsing.some(row => row.accepted), 'Preceding client accepts unknown or ambiguous failure responses')
else {
  const before = JSON.parse(await readFile(`${audit}../baseline/results.json`, 'utf8'))
  assert.deepEqual(renderings.map(row => row.controls), before.renderings.map(row => row.controls))
}
const results = { failures, outcomes, validCases: cases.length, requests, parsing, renderings, limits: 'Compiled dummy base with constructed display variants. Real component server rendering and client parsing. No mounted, visual, connected, native or provider acceptance.' }
await writeFile(`${audit}results.json`, JSON.stringify(results, null, 2) + '\n')
console.log(`${phase}: ${renderings.length} bilingual diagnostic renderings, ${requests} parser requests and ${parsing.length} malformed-response checks passed.`)
