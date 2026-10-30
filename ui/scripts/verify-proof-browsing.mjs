import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/proof-browsing'}/`
await mkdir(`${audit}source`, { recursive: true })
const log = await readFile(`${root}.audit/generated-rule-proof/proof.log`, 'utf8')
function proof(marker) {
  const line = log.split('\n').find(line => line.startsWith(marker))
  assert.ok(line, marker)
  return JSON.parse(line.slice(marker.length))
}
const actual = proof('MAILROOM_GENERATED_RULE_PROOF ')
const held = proof('MAILROOM_OMITTED_GENERATED_RULE_PROOF ')
const large = structuredClone(actual)
large.decisions = Array.from({ length: 137 }, (_, index) => ({
  ...actual.decisions[0], id: `row-${index}`, subject: index === 0 ? 'Café <img src=x onerror=alert(1)>' : `Message ${index}`,
  from: `sender${index}@example.test`, review: index % 4 === 0,
  destination: index % 4 === 0 ? { kind: 'inbox' } : { kind: 'folder', name: 'Privat/Bestellungen' },
  copies: index % 4 === 1 ? [{ rule: 'forward.test', recipient: 'accounting@example.test' }] : [],
}))
large.diff = large.decisions.map(row => ({ id: row.id, before: ['INBOX'], after: row.destination.kind === 'inbox' ? 'INBOX' : row.destination.name }))
large.counts = { messages: 137, filed: 102, inbox: 35, copies: 34, copied_messages: 34, review: 35 }
const empty = { ...actual, decisions: [], diff: [], counts: { messages: 0, filed: 0, inbox: 0, copies: 0, copied_messages: 0, review: 0 } }
const variants = [
  { name: 'actual-all', proof: actual, filter: 'all', query: '', page: 0 },
  { name: 'actual-review', proof: actual, filter: 'review', query: '', page: 0 },
  { name: 'actual-vendor', proof: actual, filter: 'all', query: 'VENDOR', page: 0 },
  { name: 'unknown-authority', proof: held, filter: 'review', query: '', page: 0 },
  { name: 'large-first', proof: large, filter: 'all', query: '', page: 0 },
  { name: 'large-middle', proof: large, filter: 'all', query: '', page: 1 },
  { name: 'large-last', proof: large, filter: 'all', query: '', page: 2 },
  { name: 'large-review', proof: large, filter: 'review', query: '', page: 2 },
  { name: 'large-copies', proof: large, filter: 'copies', query: '', page: 0 },
  { name: 'unicode-search', proof: large, filter: 'all', query: '  CAFE\u0301  ', page: 0 },
  { name: 'empty-filter', proof: large, filter: 'copies', query: 'no-match', page: 2 },
  { name: 'empty-census', proof: empty, filter: 'all', query: '', page: 0 },
]
const current = await readFile(`${root}ui/src/RulesProof.svelte`, 'utf8')
const baseline = execFileSync('/usr/bin/git', ['show', '702fba26c06655afb6c10acae9b41fe82013839b:ui/src/RulesProof.svelte'], { cwd: root, encoding: 'utf8' })
for (const [index, variant] of variants.entries()) {
  for (const [name, original] of [['Current', current], ['Before', baseline]]) {
    let source = original.replace("let proofState = $state<State>({ kind: 'initial' })", () => `let proofState = $state<State>(${JSON.stringify({ kind: 'done', proof: variant.proof }).replaceAll('<', '\\u003c')})`)
    source = source.replace("let filters = $state(['all'])", () => `let filters = $state(${JSON.stringify([variant.filter])})`)
    source = source.replace("let query = $state('')", () => `let query = $state(${JSON.stringify(variant.query).replaceAll('<', '\\u003c')})`)
    source = source.replace('let pageIndex = $state(0)', `let pageIndex = $state(${variant.page})`)
    source = source.replace("'@ark-ui/svelte/toggle-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/toggle-group'))).replaceAll("from './", `from '${root}ui/src/`)
    await writeFile(`${audit}source/${name}${index}.svelte`, source)
  }
}
await writeFile(`${audit}source/entry.mjs`, `${variants.flatMap((_, i) => ['Current', 'Before'].map(name => `export { default as ${name}${i} } from './${name}${i}.svelte'`)).join('; ')}; export * from '${root}ui/src/proof-results.ts'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}source/entry.mjs`, outDir: `${audit}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'browsing.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}built/browsing.mjs`))
globalThis.fetch = () => { throw new Error('Browsing must not make a network request') }
const checks = []
function check(name, run) { run(); checks.push(name) }
check('The message page limit is 50', () => assert.equal(ui.PROOF_PAGE_SIZE, 50))
const serialized = JSON.stringify(actual)
const ids = result => result.decisions.map(row => row.id)
check('Unknown and mixed recipients form the actual review list', () => assert.deepEqual(ids(ui.proofResults(actual, 'review', '', 0)), ['INBOX/1/8', 'INBOX/1/9']))
check('Case-insensitive sender search finds the actual vendor message', () => assert.deepEqual(ids(ui.proofResults(actual, 'all', 'VENDOR', 0)), ['INBOX/1/6']))
check('Unknown authority retains every actual message in review', () => assert.deepEqual(ids(ui.proofResults(held, 'review', '', 0)), held.decisions.map(row => row.id)))
check('Sender search can reach messages beyond the first page', () => assert.deepEqual(ids(ui.proofResults(large, 'all', 'sender100@', 0)), ['row-100']))
check('Destination search covers the complete constructed list', () => assert.equal(ui.proofResults(large, 'all', 'bestellungen', 0).total, 102))
check('Composed and decomposed accented subjects match', () => assert.deepEqual(ids(ui.proofResults(large, 'all', 'CAFE\u0301', 0)), ['row-0']))
check('Recipient review counts all held messages', () => assert.equal(ui.proofResults(large, 'review', '', 2).total, 35))
check('Copy filtering counts messages independently of filing', () => assert.equal(ui.proofResults(large, 'copies', '', 0).total, 34))
check('Inbox filtering retains the Inbox messages', () => assert.equal(ui.proofResults(large, 'inbox', '', 0).total, 35))
const pages = [0, 1, 2].map(index => ui.proofResults(large, 'all', '', index))
check('Three pages contain the complete 137-message list', () => assert.deepEqual(pages.map(page => page.decisions.length), [50, 50, 37]))
check('Page traversal preserves original order', () => assert.deepEqual(pages.flatMap(ids), large.decisions.map(row => row.id)))
check('Page traversal neither duplicates nor loses a message', () => assert.equal(new Set(pages.flatMap(ids)).size, 137))
check('A stale page clamps to the last result page', () => assert.equal(ui.proofResults(large, 'all', '', 999).page, 2))
check('Previous-page bounds retain the first page', () => assert.equal(ui.proofResults(large, 'all', '', -1).page, 0))
check('A narrowed filter clamps a previously selected page', () => assert.equal(ui.proofResults(large, 'copies', '', 2).page, 0))
const scale = { decisions: Array.from({ length: 50003 }, (_, index) => ({ ...actual.decisions[0], id: `scale-${index}` })), diff: [] }
const scaleLast = ui.proofResults(scale, 'all', '', 1000)
check('The last constructed scale page retains its three messages', () => assert.deepEqual(ids(scaleLast), ['scale-50000', 'scale-50001', 'scale-50002']))
check('The constructed scale list has 1001 pages', () => assert.equal(scaleLast.pages, 1001))
check('Browsing preserves the complete actual proof', () => assert.equal(JSON.stringify(actual), serialized))

function controls(body, lang) {
  const exclude = lang === 'en' ? ['Recipient review only', 'Clear search and filters', 'Previous page', 'Next page', 'Pin expected folder'] : ['Nur Empfängerprüfung', 'Suche und Filter zurücksetzen', 'Vorherige Seite', 'Nächste Seite', 'Erwarteten Ordner fixieren']
  return [...body.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].map(([, attributes, html]) => ({ text: html.replace(/<[^>]*>/g, '').trim(), disabled: /\bdisabled(?:\s|=|$)/.test(attributes) })).filter(control => !exclude.includes(control.text))
}
const renderings = []
for (const [index, variant] of variants.entries()) {
  const result = ui.proofResults(variant.proof, variant.filter, variant.query, variant.page)
  for (const lang of ['en', 'de']) {
    const { body } = ui.render(ui[`Current${index}`], { props: { lang } })
    const before = ui.render(ui[`Before${index}`], { props: { lang } }).body
    assert.deepEqual(controls(body, lang), controls(before, lang), `${variant.name} preserves existing controls`)
    assert.deepEqual([...body.matchAll(/data-proof-message="([^"]+)"/g)].map(match => match[1]), ids(result))
    assert.ok(body.includes(`data-result-count="${result.total}"`))
    assert.ok(body.includes(lang === 'en' ? 'Proof totals and approval gates above still cover the full census.' : 'Prüfzahlen und Freigabesperren oben gelten weiterhin für die gesamte Erfassung.'))
    assert.ok(body.includes(lang === 'en' ? 'Search subject, sender or destination' : 'Betreff, Absender oder Ziel suchen'))
    const diff = body.match(/<details[^>]*class="proof-page-diff[^>]*>([\s\S]*?)<\/details>/)?.[1]
    assert.ok(diff)
    assert.equal((diff.match(/<p>/g) ?? []).length, result.diff.length)
    for (const change of result.diff) assert.ok(diff.includes(`${change.id} ·`))
    const fullCounts = html => html.match(/<dl class="counts[^>]*>([\s\S]*?)<\/dl>/)?.[1]
    assert.equal(fullCounts(body), fullCounts(before))
    const buttons = [...body.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].map(([, attributes, html]) => ({ text: html.replace(/<[^>]*>/g, '').trim(), disabled: /\bdisabled(?:\s|=|$)/.test(attributes) }))
    const previous = buttons.find(button => button.text === (lang === 'en' ? 'Previous page' : 'Vorherige Seite'))
    const next = buttons.find(button => button.text === (lang === 'en' ? 'Next page' : 'Nächste Seite'))
    if (result.pages > 1) {
      assert.equal(previous?.disabled, result.page === 0)
      assert.equal(next?.disabled, result.page + 1 === result.pages)
    } else {
      assert.equal(previous, undefined)
      assert.equal(next, undefined)
    }
    if (variant.name === 'empty-filter') assert.ok(body.includes(lang === 'en' ? 'No messages match these filters.' : 'Keine Nachricht passt zu diesen Filtern.'))
    if (variant.name === 'empty-census') assert.ok(body.includes(lang === 'en' ? 'This proof contains no messages.' : 'Diese Prüfung enthält keine Nachrichten.'))
    if (variant.name === 'unicode-search') {
      assert.match(body, /&lt;img src=x onerror=alert\(1\)(?:&gt;|>)/)
      assert.ok(!body.includes('<img src=x'))
    }
    const name = `${variant.name}-${lang}`
    await writeFile(`${audit}${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
    renderings.push({ name, visible: result.decisions.length, matches: result.total, page: result.page, sha256: createHash('sha256').update(body).digest('hex') })
  }
}
await writeFile(`${audit}results.json`, JSON.stringify({ checks, actualProofFixtures: 2, constructedScaleMessages: 50003, renderings, unchangedExistingControls: true, limits: ['Server rendering and pure result-model execution only. Pagination events, focus, narrow layout and mounted/browser acceptance remain unperformed.', 'The constructed scale case establishes partitioning, not latency, census acquisition or memory acceptance.'] }, null, 2) + '\n')
console.log(`${checks.length} browsing checks, 50003-message partitioning and ${renderings.length} bilingual renderings passed.`)
