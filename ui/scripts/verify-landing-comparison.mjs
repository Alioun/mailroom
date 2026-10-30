import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/landing-comparison'}/`
await mkdir(`${audit}render/source`, { recursive: true })
const marker = 'MAILROOM_LANDING_FIXTURE '
const line = (await readFile(`${audit}rules.log`, 'utf8')).split('\n').find(line => line.startsWith(marker))
assert.ok(line, 'The compiled Rust comparison supplies the received-message fixture')
const report = JSON.parse(line.slice(marker.length)).compared
const source = await readFile(`${root}ui/src/LandingComparison.svelte`, 'utf8')
for (const page of [2, 3]) await writeFile(`${audit}render/source/Page${page}.svelte`, source.replace("from './landing'", `from '${root}ui/src/landing'`).replace('let page = $state(1)', `let page = $state(${page})`))
await writeFile(`${audit}render/source/entry.mjs`, `export { default as Comparison } from '${root}ui/src/LandingComparison.svelte'; export { default as Page2 } from './Page2.svelte'; export { default as Page3 } from './Page3.svelte'; export { landingReport } from '${root}ui/src/landing'; export { openAccount } from '${root}ui/src/api'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}render/source/entry.mjs`, outDir: `${audit}render/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'comparison.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}render/built/comparison.mjs`))
assert.deepEqual(ui.landingReport.parse(report), report)
const rejections = []
for (const [name, alter] of [
  ['unknown semantics', r => { r.semantics = 'future' }],
  ['unknown comparison', r => { r.rows[0].prediction.comparison = 'delivered' }],
  ['unknown origin', r => { r.rows[0].prediction.origin = 'provider_verified' }],
  ['negative counts', r => { r.counts.matches = -1 }],
  ['mismatched counts', r => { r.counts.matches++ }],
  ['missing completeness', r => { delete r.complete }],
  ['false completeness', r => { r.scan_state = 'failed'; r.complete = true }],
  ['invalid time', r => { r.refreshed_at = Number.MAX_SAFE_INTEGER }],
  ['invalid census identity', r => { r.census_sha256 = 'caller-claim' }],
]) {
  const changed = structuredClone(report)
  alter(changed)
  assert.equal(ui.landingReport.safeParse(changed).success, false, name)
  rejections.push(name)
}
assert.equal(ui.landingReport.safeParse({ kind: 'unavailable', reason: 'future' }).success, false)
rejections.push('unknown unavailable reason')
const view = JSON.parse(await readFile(`${audit}workspace/first-view.json`, 'utf8'))
let response = { ...view, landing: { ...report, account: view.census.account } }
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/open')
  requests++
  return Response.json(response)
}
assert.equal((await ui.openAccount(view.census.account)).landing.kind, 'compared')
response = { ...response, landing: { ...response.landing, account: 'another-account' } }
await assert.rejects(() => ui.openAccount(view.census.account))
rejections.push('cross-account comparison')
const renderings = []
async function screen(name, component, report, lang) {
  const { body } = ui.render(component, { props: { report, lang } })
  await writeFile(`${audit}render/${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><body>${body}</body></html>`)
  renderings.push({ name, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
const first = report.rows.find(row => row.prediction.kind === 'folder' && row.prediction.comparison === 'matches')
assert.ok(first)
const long = { ...report, counts: { matches: 103, elsewhere: 0, additional_folders: 0, uncheckable: 0 }, rows: Array.from({ length: 103 }, (_, i) => ({ ...first, id: `row-${i}`, subject: i === 0 ? '<img src=x onerror=alert(1)>' : `message-${i}` })) }
ui.landingReport.parse(long)
for (const lang of ['en', 'de']) {
  const body = await screen(`compared-${lang}`, ui.Comparison, report, lang)
  assert.equal((body.match(/<tbody>.*?<\/tbody>/s)?.[0].match(/<tr>/g) ?? []).length, 4)
  assert.ok(body.includes(lang === 'en' ? 'Existing rule' : 'Vorhandene Regel'))
  const partial = await screen(`historical-partial-${lang}`, ui.Comparison, { ...report, complete: false, scan_state: 'failed', historical: true }, lang)
  assert.ok(partial.includes(lang === 'en' ? 'not a current provider observation' : 'Keine aktuelle Anbieterbeobachtung'))
  assert.ok(partial.includes(lang === 'en' ? 'acquisition is incomplete' : 'Erfassung ist unvollständig'))
  for (const reason of ui.landingReport.options[0].shape.reason.options) {
    const unavailable = await screen(`unavailable-${reason}-${lang}`, ui.Comparison, { kind: 'unavailable', reason }, lang)
    assert.ok(!unavailable.includes('<table>'))
  }
  for (const [page, component, expected] of [[1, ui.Comparison, 50], [2, ui.Page2, 50], [3, ui.Page3, 3]]) {
    const body = await screen(`page-${page}-${lang}`, component, long, lang)
    assert.equal((body.match(/<tbody>.*?<\/tbody>/s)?.[0].match(/<tr>/g) ?? []).length, expected)
    assert.ok(!body.includes('<img src=x'))
    const buttons = [...body.matchAll(/<button([^>]*)>/g)].map(([, attributes]) => /\bdisabled(?:\s|=|$)/.test(attributes))
    assert.deepEqual(buttons, [page === 1, page === 3])
    const start = (page - 1) * 50
    const end = Math.min(page * 50, 103) - 1
    assert.ok(body.includes(`message-${end}`))
    if (page > 1) { assert.ok(body.includes(`message-${start}`)); assert.ok(!body.includes(`message-${start - 1}<`)) }
    if (page < 3) assert.ok(!body.includes(`message-${end + 1}<`))
  }
  const empty = await screen(`empty-${lang}`, ui.Comparison, { ...report, rows: [], counts: { matches: 0, elsewhere: 0, additional_folders: 0, uncheckable: 0 } }, lang)
  assert.ok(empty.includes(lang === 'en' ? 'No received messages' : 'Keine empfangenen Nachrichten'))
}
await writeFile(`${audit}ui-results.json`, JSON.stringify({ source: 'Actual compiled comparison over constructed credential-free persisted deployment, plus explicit display variants.', requests, rejections, renderings, limits: ['Server rendering and client boundary checks only. No mounted events, layout, connected TLS, initial delivery, continued activation or provider acceptance.'] }, null, 2) + '\n')
console.log(`Passed ${rejections.length} malformed-input refusals and ${renderings.length} bilingual renderings.`)
