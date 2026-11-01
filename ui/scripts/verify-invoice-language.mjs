import assert from 'node:assert/strict'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { compile } from 'svelte/compiler'
import { render } from 'svelte/server'
import ts from 'typescript'
import { localModules } from './local-modules.mjs'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = resolve(root, process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/invoice-language')
const output = resolve(audit, 'render')
await mkdir(output, { recursive: true })
const log = await readFile(resolve(audit, 'invoice-tests.log'), 'utf8')
function fixture(marker) {
  const line = log.split('\n').find(value => value.includes(marker))
  assert.ok(line, `Missing actual Rust fixture ${marker}`)
  return JSON.parse(line.slice(line.indexOf(marker) + marker.length))
}
const reports = fixture('MAILROOM_INVOICE_DISPLAY_FIXTURE ')
const variants = fixture('MAILROOM_INVOICE_DIAGNOSTICS ')
function imports(code) {
  return code.replace(/(['"])(zod|svelte(?:\/[^'"]+)?)\1/g, (_match, _quote, name) => JSON.stringify(import.meta.resolve(name)))
    .replace(/from '(\.\/[^']+)'/g, (_match, name) => `from '${name}.mjs'`)
}
for (const name of localModules(['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics', 'proof-diagnostics'])) {
  const source = await readFile(resolve(root, `ui/src/${name}.ts`), 'utf8')
  const emitted = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } })
  await writeFile(resolve(output, `${name}.mjs`), imports(emitted.outputText))
}
const api = await import(pathToFileURL(resolve(output, 'api.mjs')))
const diagnostics = await import(pathToFileURL(resolve(output, 'invoice-diagnostics.mjs')))
let response
const requests = []
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/invoices')
  requests.push(JSON.parse(options.body))
  return Response.json(response)
}
const view = report => ({ report, sends: [], synthetic: true, targets: ['accounting'], sent_evidence: {} })
async function parsed(value) { response = value; return api.openInvoices('mailbox-fixture') }
const actual = await parsed(view(reports.synthetic))
await parsed(view(reports.real))
await parsed(view(reports.missing))
for (const alter of [
  value => { value.report.rows[0].reasons = [{ kind: 'unknown_future_reason' }] },
  value => { value.report.rows[0].handoff = 'delivered_to_accounting' },
  value => { value.report.rows[0].accounting_acceptance = 'accepted' },
  value => { value.report.rows[0].role = 'guessed_business' },
]) {
  const invalid = structuredClone(actual)
  alter(invalid)
  await assert.rejects(() => parsed(invalid), 'Unsupported evidence must fail at the frontend boundary')
}
for (const variant of variants) {
  const value = diagnostics.invoiceDiagnostic.parse(variant)
  assert.notEqual(diagnostics.invoiceDiagnosticText(value, 'en'), diagnostics.invoiceDiagnosticText(value, 'de'))
}
const source = await readFile(resolve(root, 'ui/src/Invoices.svelte'), 'utf8')
const renders = []
async function screen(name, lang, initial) {
  const declaration = 'let view = $state<InvoiceView | null>(null)'
  assert.equal(source.split(declaration).length, 2)
  const encoded = JSON.stringify(initial).replaceAll('<', '\\u003c')
  const component = source.replace(declaration, () => `let view = $state<InvoiceView | null>(${encoded})`)
  const compiled = compile(component, { filename: 'Invoices.svelte', generate: 'server', css: 'external' })
  assert.equal(compiled.warnings.length, 0)
  const destination = resolve(output, `${name}.mjs`)
  await writeFile(destination, imports(compiled.js.code))
  const { default: Component } = await import(pathToFileURL(destination))
  const { body } = render(Component, { props: { account: 'mailbox-fixture', lang } })
  await writeFile(resolve(output, `${name}.html`), `<!doctype html><html lang="${lang}"><meta charset="utf-8"><title>Mailroom invoice verification</title><style>${compiled.css?.code ?? ''}</style><body>${body}</body></html>`)
  renders.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
  assert.ok(!body.includes('<script>fixture</script>'))
  assert.ok(!body.includes('Qonto'))
  return body
}
for (const lang of ['en', 'de']) {
  await screen(`actual-${lang}`, lang, actual)
  const empty = await screen(`missing-${lang}`, lang, view(reports.missing))
  assert.ok(empty.includes(diagnostics.invoiceDiagnosticText({ kind: 'workspace_required' }, lang)))
  const noCandidates = structuredClone(actual)
  noCandidates.report.rows = []
  noCandidates.report.count = 0
  const noRows = await screen(`no-candidates-${lang}`, lang, noCandidates)
  assert.ok(noRows.includes(lang === 'de' ? 'Dieser Bericht enthält keine Rechnungskandidaten.' : 'No invoice candidates in this report.'))
  assert.ok(!noRows.includes(lang === 'de' ? 'Zuerst ein Regelwerk speichern.' : 'Save a rule workspace first.'))
  const expanded = structuredClone(actual)
  expanded.report.rows = [{ ...actual.report.rows[0], reasons: variants, subject: '<script>fixture</script>', sender: 'fixture@example.test' }]
  expanded.report.warnings.push(variants.find(value => value.kind === 'rule_omitted'))
  const content = await screen(`diagnostics-${lang}`, lang, await parsed(expanded))
  assert.ok(content.includes('&lt;script>fixture&lt;/script>'))
  for (const variant of variants.filter(value => !['filename_hint', 'rule_omitted'].includes(value.kind))) {
    assert.ok(content.includes(diagnostics.invoiceDiagnosticText(variant, lang)))
  }
  for (const handoff of diagnostics.invoiceHandoff.options) {
    const changed = structuredClone(actual)
    changed.report.rows = [{ ...actual.report.rows[0], handoff }]
    const body = await screen(`handoff-${handoff}-${lang}`, lang, await parsed(changed))
    assert.ok(body.includes(diagnostics.invoiceStatusText[lang].handoff[handoff]))
    assert.ok(!body.includes(` · ${handoff}`))
  }
}
await writeFile(resolve(audit, 'render-results.json'), JSON.stringify({ renders, diagnosticVariants: variants.length, checks: ['actual Rust reports parsed', 'unknown diagnostics, roles, handoff and acceptance refused', 'complete bilingual diagnostics and handoff states', 'unsafe values escaped', 'generic accounting uncertainty', 'empty and conservative states'], limits: 'Server renderings establish text and markup, not browser layout, focus, zoom, native or provider acceptance.' }, null, 2) + '\n')
console.log('Invoice renderings passed', renders.length)
