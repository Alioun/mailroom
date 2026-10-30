import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/transaction-capacity'}/`
const fixtures = await Promise.all([5, 64].map(async count => JSON.parse(await readFile(`${audit}compiled-fixtures/${count}.json`, 'utf8'))))
assert.deepEqual(fixtures.map(fixture => fixture.count), [5, 64])
const base = { installed: true, recovery_hold: false, placement: { kind: 'block' }, limitations: 'Compiled dummy intent and encrypted reopen fixture. No TLS effects.' }
const baselineDirectory = `${audit}capacity-ui/baseline/`
const currentDirectory = `${audit}capacity-ui/current/`
for (const directory of [baselineDirectory, currentDirectory]) await mkdir(directory, { recursive: true })
const preceding = execFileSync('/usr/bin/git', ['show', 'd7c6d57fbacf5043574cd5e2d15c9b80f5822da2:ui/src/sieve-transactions-api.ts'], { cwd: root, encoding: 'utf8' })
await writeFile(`${baselineDirectory}api.ts`, preceding.replace("'zod'", JSON.stringify(import.meta.resolve('zod'))).replace("from './api'", `from '${root}ui/src/api'`))
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', ssr: { noExternal: true }, build: { ssr: `${baselineDirectory}api.ts`, outDir: `${baselineDirectory}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'api.mjs' } } } })
const old = await import(pathToFileURL(`${baselineDirectory}built/api.mjs`))
let reply
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/sieve-transactions')
  requests++
  return Response.json(reply)
}
const baseline = []
for (const fixture of fixtures) {
  reply = { ...base, journal: fixture.journal }
  await assert.rejects(() => old.transactionStatus('capacity-account'))
  baseline.push({ count: fixture.count, refused: true })
}
const source = await readFile(`${root}ui/src/SieveTransactions.svelte`, 'utf8')
const full = { ...base, journal: fixtures[1].journal }
for (let page = 1; page <= 7; page++) {
  let component = source.replace('let view = $state<TransactionView | null>(null)', () => `let view = $state<TransactionView | null>(${JSON.stringify(full).replaceAll('<', '\\u003c')})`).replace('let historyPage = $state(1)', `let historyPage = $state(${page})`)
  component = component.replace("'@ark-ui/svelte/radio-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/radio-group'))).replaceAll("from './", `from '${root}ui/src/`)
  await writeFile(`${currentDirectory}Page${page}.svelte`, component)
}
await writeFile(`${currentDirectory}entry.mjs`, `${Array.from({ length: 7 }, (_, index) => `export { default as Page${index + 1} } from './Page${index + 1}.svelte'`).join('; ')}; export * from '${root}ui/src/sieve-transactions-api'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${currentDirectory}entry.mjs`, outDir: `${currentDirectory}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'capacity.mjs' } } } })
const ui = await import(pathToFileURL(`${currentDirectory}built/capacity.mjs`))
assert.equal(ui.transactionHistoryLimit, 64)
const accepted = []
for (const fixture of fixtures) {
  reply = { ...base, journal: fixture.journal }
  const parsed = await ui.transactionStatus('capacity-account')
  assert.deepEqual(parsed.journal.history.map(transaction => transaction.id), fixture.journal.history.map(transaction => transaction.id))
  accepted.push(fixture.count)
}
const rejections = []
reply = { ...base, journal: structuredClone(fixtures[1].journal) }
reply.journal.history.push({ ...reply.journal.history[0], id: 'f'.repeat(64) })
await assert.rejects(() => ui.transactionStatus('capacity-account'))
rejections.push('65 operations')
reply = { ...base, journal: structuredClone(fixtures[0].journal) }
reply.journal.history[1].id = reply.journal.history[0].id
await assert.rejects(() => ui.transactionStatus('capacity-account'))
rejections.push('duplicate identities')
const renderings = []
const expected = full.journal.history.map(transaction => transaction.id).reverse()
for (const lang of ['en', 'de']) {
  const observed = []
  for (let page = 1; page <= 7; page++) {
    const { body } = ui.render(ui[`Page${page}`], { props: { account: 'capacity-account', lang } })
    const clean = body.replace(/<!--[\s\S]*?-->/g, '')
    const articles = [...clean.matchAll(/<article\b[^>]*>([\s\S]*?)<\/article>/g)].map(match => match[1])
    const ids = articles.map(article => article.match(/<p>([a-f0-9]{64})<\/p>/)?.[1])
    assert.deepEqual(ids, expected.slice((page - 1) * 10, page * 10))
    assert.ok(clean.includes('64 / 64'))
    assert.ok(clean.includes('4 MiB'))
    const paging = [...clean.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].filter(([, , text]) => /Newer operations|Older operations|Neuere Vorgänge|Ältere Vorgänge/.test(text)).map(([, attributes]) => /\bdisabled(?:\s|=|$)/.test(attributes))
    assert.deepEqual(paging, [page === 1, page === 7])
    for (const article of articles) {
      const controls = [...article.matchAll(/<button\b([^>]*)>/g)].map(([, attributes]) => /\bdisabled(?:\s|=|$)/.test(attributes))
      assert.deepEqual(controls, [false, true], 'Full history keeps reconciliation available and refuses new compensation')
    }
    observed.push(...ids)
    await writeFile(`${currentDirectory}page-${page}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
    renderings.push({ lang, page, operations: ids.length })
  }
  assert.deepEqual(observed, expected)
  assert.equal(new Set(observed).size, 64)
}
await writeFile(`${audit}capacity-ui/results.json`, JSON.stringify({ baseline, accepted, rejections, requests, renderings, limits: 'Actual compiled persistence fixtures and client parsing. Component initial-state server rendering only. No mounted paging events, TLS, provider or execution acceptance.' }, null, 2) + '\n')
console.log(`Capacity contracts and ${renderings.length} bilingual paged renderings passed.`)
