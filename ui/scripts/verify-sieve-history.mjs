import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const baseline = process.argv.includes('--baseline')
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/sieve-history'}/${baseline ? 'baseline' : 'current'}/`
await mkdir(`${audit}source`, { recursive: true })
const v1 = 'mailroom-foreign-composition-v1'
const v2 = 'mailroom-foreign-composition-v2'
const counts = { mailroom: 2, existing_wins: 1, conflicting: 1, undetermined: 0 }
const oldCounts = { ...counts, suppressed_copies: 7 }
const newCounts = { ...counts, copies: { copied: 2, beside_existing: 1, undetermined: 0, guard_violations: 0 } }
const image = { capabilities: {}, names: [], bodies: [] }
const transaction = {
  id: 'display-history', compensates: null, target: 'Open-Xchange', ownership_epoch: 'fixture', artifact: '0'.repeat(64), proof_binding: '0'.repeat(64), receipt: '0'.repeat(64), before: image, after: image, status: 'complete', validation: { Ok: null }, space: { Ok: null }, steps: [],
}
const composition = { purpose: 'deploy', semantics: v1, placement: { kind: 'block' }, owned: null, foreign_sha256: '0'.repeat(64), existing_rules: ['Travel stop'], precedence: oldCounts }
const variants = [
  { name: 'legacy', transaction: { ...transaction, composition } },
  { name: 'current', transaction: { ...transaction, composition: { ...composition, semantics: v2, precedence: newCounts } } },
  { name: 'whole-script', transaction },
  { name: 'legacy-compensation', transaction: { ...transaction, compensates: 'original', composition: { ...composition, purpose: 'compensation' } } },
  { name: 'removal', transaction: { ...transaction, composition: { ...composition, semantics: v2, purpose: 'removal', placement: null, precedence: null } } },
]
const baseView = { installed: true, recovery_hold: false, placement: { kind: 'block' }, journal: { history: [] }, limitations: 'Constructed display fixture only' }
const original = path => execFileSync('/usr/bin/git', ['show', `1222408de044f6d10558e4d3beb70e95bcac3661:${path}`], { cwd: root, encoding: 'utf8' })
const source = baseline ? original('ui/src/SieveTransactions.svelte') : await readFile(`${root}ui/src/SieveTransactions.svelte`, 'utf8')
const api = baseline ? `${audit}source/sieve-transactions-api.ts` : `${root}ui/src/sieve-transactions-api.ts`
if (baseline) await writeFile(api, original('ui/src/sieve-transactions-api.ts').replace("from './api'", `from '${root}ui/src/api'`).replace("'zod'", JSON.stringify(import.meta.resolve('zod'))))
for (const [index, variant] of variants.entries()) {
  const view = { ...baseView, journal: { history: [variant.transaction] } }
  variant.view = view
  let component = source.replace('let view = $state<TransactionView | null>(null)', () => `let view = $state<TransactionView | null>(${JSON.stringify(view).replaceAll('<', '\\u003c')})`)
  component = component.replace("'@ark-ui/svelte/radio-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/radio-group'))).replaceAll("from './", `from '${root}ui/src/`)
  component = component.replace(`${root}ui/src/sieve-transactions-api`, api)
  await writeFile(`${audit}source/History${index}.svelte`, component)
}
await writeFile(`${audit}source/entry.mjs`, `${variants.map((_, i) => `export { default as History${i} } from './History${i}.svelte'`).join('; ')}; export * from '${api}'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}source/entry.mjs`, outDir: `${audit}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'history.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}built/history.mjs`))
let reply
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/sieve-transactions')
  requests++
  return Response.json(reply)
}
for (const variant of variants) {
  reply = variant.view
  assert.deepEqual((await ui.transactionStatus('dummy')).journal.history[0], variant.transaction)
}
const rejected = []
if (!baseline) {
  for (const [name, alter] of [
    ['unknown semantics', value => { value.semantics = 'mailroom-foreign-composition-v3' }],
    ['missing semantics', value => { delete value.semantics }],
    ['legacy counts under current semantics', value => { value.semantics = v2 }],
    ['missing legacy count', value => { delete value.precedence.suppressed_copies }],
    ['mixed count forms', value => { value.precedence.copies = newCounts.copies }],
    ['negative legacy count', value => { value.precedence.suppressed_copies = -1 }],
    ['fractional count', value => { value.precedence.mailroom = 0.5 }],
  ]) {
    reply = structuredClone(variants[0].view)
    alter(reply.journal.history[0].composition)
    await assert.rejects(() => ui.transactionStatus('dummy'), name)
    rejected.push(name)
  }
  for (const [name, alter] of [
    ['current counts under legacy semantics', value => { value.semantics = v1 }],
    ['missing current counts', value => { delete value.precedence.copies }],
    ['unknown count field', value => { value.precedence.copies.invented = 1 }],
    ['missing current field', value => { delete value.precedence.copies.guard_violations }],
  ]) {
    reply = structuredClone(variants[1].view)
    alter(reply.journal.history[0].composition)
    await assert.rejects(() => ui.transactionStatus('dummy'), name)
    rejected.push(name)
  }
  const precedence = { semantics: v2, placement: { kind: 'block' }, host: 'Open-Xchange', foreign_rules: [], decisions: [], copies: [], counts: newCounts, pins: [], gate: 'warnings', warnings: [] }
  const ready = { outcome: 'ready', transaction: variants[1].transaction, receipt: '0'.repeat(64), proof: { removal: v2 }, precedence, effects: [], limitations: 'Constructed contract fixture only' }
  reply = ready
  assert.deepEqual((await ui.transactionPreview('dummy', 'deploy')).precedence, precedence)
  for (const [name, alter] of [
    ['historical live precedence', value => { value.semantics = v1 }],
    ['unknown live precedence', value => { value.semantics = 'future' }],
    ['legacy live counts', value => { value.counts = oldCounts }],
  ]) {
    reply = structuredClone(ready)
    alter(reply.precedence)
    await assert.rejects(() => ui.transactionPreview('dummy', 'deploy'), name)
    rejected.push(name)
  }
}
const results = []
for (const [index, variant] of variants.entries()) {
  for (const lang of ['en', 'de']) {
    const { body } = ui.render(ui[`History${index}`], { props: { account: 'dummy', lang } })
    const article = body.match(/<article\b[^>]*>([\s\S]*?)<\/article>/)?.[1]
    assert.ok(article)
    const heading = article.match(/<h4>(.*?)<\/h4>/)?.[1].replace(/<[^>]*>/g, '')
    const name = `${variant.name}-${lang}`
    await writeFile(`${audit}${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
    const controls = [...body.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].filter(([, , html]) => !/Newer operations|Older operations|Neuere Vorgänge|Ältere Vorgänge/.test(html)).map(([, attributes, html]) => ({ text: html.replace(/<[^>]*>/g, '').trim(), disabled: /\bdisabled(?:\s|=|$)/.test(attributes) }))
    results.push({ name, heading, controls })
  }
}
await writeFile(`${audit}results.json`, JSON.stringify({ requests, rejected, results, fixture: 'Constructed display histories. No protocol, persistence, mounted or provider acceptance.' }, null, 2) + '\n')
if (baseline) {
  assert.match(results[0].heading, /Deploy copies first, filing after existing rules/)
  assert.match(results[1].heading, /Kopien zuerst, Ablage nach vorhandenen Regeln/)
  assert.match(results[4].heading, /Preview composed deploy/)
  assert.match(results[5].heading, /Zusammengesetzten Einsatz prüfen/)
} else {
  assert.match(results[0].heading, /Deploy Mailroom rules after existing rules/)
  assert.match(results[1].heading, /Mailroom-Regeln nach vorhandenen Regeln/)
  assert.match(results[2].heading, /Deploy copies first, filing after existing rules/)
  assert.match(results[3].heading, /Kopien zuerst, Ablage nach vorhandenen Regeln/)
  assert.match(results[4].heading, /Recorded script transaction/)
  assert.match(results[5].heading, /Gespeicherte Skripttransaktion/)
  const before = JSON.parse(await readFile(`${audit}../baseline/results.json`, 'utf8'))
  assert.deepEqual(results.map(row => row.controls), before.results.map(row => row.controls))
}
console.log(`${requests} history/preview contracts, ${rejected.length} rejected payloads and ${results.length} bilingual history renderings passed.`)
