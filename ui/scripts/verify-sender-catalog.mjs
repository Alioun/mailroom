import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { createHash } from 'node:crypto'
import { compile } from 'svelte/compiler'
import { render } from 'svelte/server'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = resolve(root, process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/sender-catalog')
const output = resolve(audit, 'render')
await mkdir(output, { recursive: true })
const log = await readFile(process.argv[2] ?? resolve(audit, 'rust-tests.log'), 'utf8')
function fixture(marker) {
  const line = log.split('\n').find(line => line.includes(marker))
  assert.ok(line, `Missing ${marker} from actual Rust verification`)
  return JSON.parse(line.slice(line.indexOf(marker) + marker.length))
}
const reports = fixture('MAILROOM_CATALOG_DISPLAY_FIXTURE ')
function imports(code) {
  return code.replace(/(['"])(zod|svelte(?:\/[^'"]+)?)\1/g, (_match, _quote, name) => JSON.stringify(import.meta.resolve(name)))
    .replace(/from '(.\/[^']+)'/g, (_match, name) => `from '${name}.mjs'`)
}
for (const name of ['api', 'acquisition-evidence', 'proposal-diagnostics', 'winddown-api']) {
  const source = await readFile(resolve(root, `ui/src/${name}.ts`), 'utf8')
  const emitted = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext }, fileName: `${name}.ts` })
  await writeFile(resolve(output, `${name}.mjs`), imports(emitted.outputText))
}
const api = await import(pathToFileURL(resolve(output, 'winddown-api.mjs')))
for (const key of ['reviewed', 'unreviewed', 'stale', 'historical']) reports[key] = api.reportSchema.parse(reports[key])
function sender(report) {
  const row = report.rows.find(row => row.sender === 'alerts@example.com')
  assert.ok(row, 'Actual Rust report contains the synthetic catalog sender')
  return row
}
assert.equal(sender(reports.reviewed).group, 'service')
assert.equal(sender(reports.unreviewed).group, 'unresolved')
assert.equal(sender(reports.stale).group, 'unresolved')
assert.equal(sender(reports.historical).group, 'unresolved')
assert.equal(sender(reports.historical).settings_link, null)
const legacy = structuredClone(reports.historical)
delete legacy.sender_catalog
assert.equal(api.reportSchema.parse(legacy).sender_catalog, undefined)
const unlicensed = structuredClone(reports.reviewed)
unlicensed.sender_catalog.license = 'CC-BY-SA-4.0'
assert.equal(api.reportSchema.safeParse(unlicensed).success, false)
const compiled = compile(await readFile(resolve(root, 'ui/src/SenderCatalog.svelte'), 'utf8'), { filename: 'SenderCatalog.svelte', generate: 'server', css: 'external' })
assert.equal(compiled.warnings.length, 0)
await writeFile(resolve(output, 'SenderCatalog.mjs'), imports(compiled.js.code))
const { default: Component } = await import(pathToFileURL(resolve(output, 'SenderCatalog.mjs')))
const renders = []
async function screen(name, lang, catalog) {
  const { body } = render(Component, { props: { lang, catalog } })
  assert.ok(!body.includes('href='), 'Catalog inspection never fetches or launches links')
  await writeFile(resolve(output, `${name}-${lang}.html`), `<!doctype html><html lang="${lang}"><meta charset="utf-8"><title>Mailroom synthetic sender catalog</title><style>${compiled.css?.code ?? ''}</style><body>${body}</body></html>`)
  renders.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
for (const lang of ['en', 'de']) {
  const empty = await screen('empty', lang, reports.historical.sender_catalog)
  assert.ok(empty.includes(lang === 'de' ? 'Die erste Absenderliste ist unvollständig.' : 'The first sender list is incomplete.'))
  assert.ok(empty.includes(lang === 'de' ? 'Finanzen' : 'Money'))
  assert.ok(empty.includes('CC0-1.0'))
  const reviewed = await screen('reviewed', lang, reports.reviewed.sender_catalog)
  assert.ok(reviewed.includes(lang === 'de' ? 'Ausdrückliche Prüfungen als Transaktionsdienst' : 'Explicit transactional-service reviews'))
  const old = await screen('stale', lang, reports.stale.sender_catalog)
  assert.ok(old.includes(lang === 'de' ? 'Zuletzt vor mindestens zwei Jahren geprüfte Zeilen: 1' : 'Rows last checked at least two years ago: 1'))
  assert.ok(old.includes(lang === 'de' ? 'Veraltete oder zukünftige Prüfungen lassen diese Zuordnung ungeklärt.' : 'Stale or future reviews leave that classification unresolved.'))
  const complete = structuredClone(reports.reviewed.sender_catalog)
  complete.rows = 120
  complete.categories = Object.fromEntries(api.senderCategory.options.map(name => [name, 20]))
  complete.missing_categories = []
  complete.missing_countries = []
  complete.seed_complete = true
  const full = await screen('count-complete', lang, complete)
  assert.ok(full.includes(lang === 'de' ? 'Die Installation bestätigt die dokumentierten Prüfungen nicht unabhängig.' : 'Installation does not independently prove the recorded reviews.'))
}
await writeFile(resolve(audit, 'render-results.json'), JSON.stringify({ renderings: renders, checks: ['actual report payloads parsed', 'service classification requires explicit review', 'historical removed links absent', 'legacy reports still parse', 'licensed package confusion refused', 'English and German empty/reviewed/stale/count-complete states', 'no catalog link launch'], limits: 'Server rendering verifies text and markup. Browser layout, interaction, installed acceptance and genuine catalog curation remain open.' }, null, 2) + '\n')
console.log(`Passed report contract checks and ${renders.length} bilingual catalog server renderings.`)
