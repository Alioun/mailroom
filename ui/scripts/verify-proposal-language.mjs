import assert from 'node:assert/strict'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { compile } from 'svelte/compiler'
import { render } from 'svelte/server'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = resolve(root, '.audit/proposal-language')
const output = resolve(audit, 'render')
await mkdir(output, { recursive: true })
const log = await readFile(process.argv[2] ?? resolve(audit, 'proposal-tests.log'), 'utf8')
const marker = 'MAILROOM_PROPOSAL_DISPLAY_FIXTURE '
const record = log.split('\n').find(line => line.includes(marker))
assert.ok(record, 'Run the Rust proposal tests with --nocapture first')
const fixture = JSON.parse(record.slice(record.indexOf(marker) + marker.length))

function imports(code) {
  return code.replace(/(['"])(zod|svelte(?:\/[^'"]+)?)\1/g, (_match, _quote, name) => JSON.stringify(import.meta.resolve(name)))
    .replace(/from '(.\/[^']+)'/g, (_match, name) => `from '${name}.mjs'`)
}
for (const name of ['api', 'acquisition-evidence', 'proposal-diagnostics']) {
  const source = await readFile(resolve(root, `ui/src/${name}.ts`), 'utf8')
  const emitted = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext }, fileName: `${name}.ts` })
  await writeFile(resolve(output, `${name}.mjs`), imports(emitted.outputText))
}
const api = await import(pathToFileURL(resolve(output, 'api.mjs')))
const diagnostics = await import(pathToFileURL(resolve(output, 'proposal-diagnostics.mjs')))
const requests = []
let response
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/proposals')
  requests.push(JSON.parse(options.body))
  return Response.json(response)
}
async function generation(value) {
  response = value
  return api.openProposals(value.account)
}
const supported = await generation(fixture.generation)
const unresolved = await generation(fixture.unresolved)
response = fixture.accept
const accepted = await api.reviewProposals(supported.account, fixture.accept.selection, 'accept')
response = fixture.reject
const rejected = await api.reviewProposals(supported.account, fixture.reject.selection, 'reject')
assert.deepEqual(requests.at(-1).command.selection, fixture.reject.selection)
assert.equal(accepted.receipt, fixture.accept.receipt)
assert.equal(rejected.receipt, fixture.reject.receipt)
const unknownReason = structuredClone(fixture.unresolved)
unknownReason.unresolved[0].reason = 'future_unknown_reason'
await assert.rejects(() => generation(unknownReason), 'Unknown diagnostic codes must not become untranslated interface text')
for (const [matching, total] of [[0, 0], [1, 2], [3, 2], [1.5, 2]]) {
  const invalid = structuredClone(fixture.generation)
  invalid.candidates[0].rationale = { kind: 'subject_majority', matching, total }
  await assert.rejects(() => generation(invalid), 'Invalid majority evidence must fail at the frontend boundary')
}
for (const reason of diagnostics.proposalReason.options) {
  assert.notEqual(diagnostics.proposalReasonText(reason, 'en'), reason)
  assert.notEqual(diagnostics.proposalReasonText(reason, 'de'), reason)
  assert.notEqual(diagnostics.proposalReasonText(reason, 'de'), diagnostics.proposalReasonText(reason, 'en'))
}

const source = await readFile(resolve(root, 'ui/src/Proposals.svelte'), 'utf8')
const renders = []
async function screen(name, lang, initialGeneration, initialReview = null, disabled = false) {
  let component = source
  for (const [declaration, value] of [
    ['let generation = $state<ProposalGeneration | null>(null)', initialGeneration],
    ['let review = $state<ProposalReview | null>(null)', initialReview],
  ]) {
    assert.equal(component.split(declaration).length, 2, 'Inject fixture state only into the known initial declaration')
    component = component.replace(declaration, `${declaration.slice(0, -5)}${JSON.stringify(value)})`)
  }
  const compiled = compile(component, { filename: 'Proposals.svelte', generate: 'server', css: 'external' })
  assert.equal(compiled.warnings.length, 0)
  const destination = resolve(output, `${name}.mjs`)
  await writeFile(destination, imports(compiled.js.code))
  const { default: Component } = await import(pathToFileURL(destination))
  const { body } = render(Component, { props: { account: supported.account, lang, disabled, onChanged: async () => {} } })
  await writeFile(resolve(output, `${name}.html`), `<!doctype html><html lang="${lang}"><meta charset="utf-8"><title>Mailroom synthetic proposal verification</title><style>${compiled.css?.code ?? ''}</style><body>${body}</body></html>`)
  renders.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
for (const lang of ['en', 'de']) {
  const body = await screen(`majority-${lang}`, lang, supported)
  assert.ok(body.includes(diagnostics.proposalRationaleText(supported.candidates[0].rationale, lang)))
  assert.ok(body.includes(diagnostics.proposalStatusText[lang].gate[supported.candidates[0].impact.proof.gate]))
  assert.ok(!body.includes('[object Object]'))
  const missing = await screen(`unresolved-${lang}`, lang, unresolved)
  assert.ok(missing.includes(diagnostics.proposalStatusText[lang].unknownSender))
  for (const observation of unresolved.unresolved) {
    assert.ok(missing.includes(diagnostics.proposalReasonText(observation.reason, lang)))
    assert.ok(!missing.includes(`· ${observation.reason}</p>`))
  }
  const mapped = structuredClone(supported)
  mapped.candidates[0].rationale = { kind: 'mapped_folder' }
  assert.ok((await screen(`folder-${lang}`, lang, await generation(mapped))).includes(diagnostics.proposalRationaleText(mapped.candidates[0].rationale, lang)))
  for (const review of [accepted, rejected]) {
    assert.equal(review.effects, diagnostics.proposalStatusText.en.effects[review.disposition], 'The translated action must track the original bound effects')
    const reviewed = await screen(`${review.disposition}-${lang}`, lang, supported, review)
    assert.ok(reviewed.includes(diagnostics.proposalStatusText[lang].disposition[review.disposition]))
    assert.ok(reviewed.includes(diagnostics.proposalStatusText[lang].effects[review.disposition]))
    assert.ok(reviewed.includes(review.effects), 'The exact original confirmation text stays available')
    assert.match(reviewed, /<button[^>]*disabled[^>]*>[^<]+<\/button>/, 'Saving requires explicit confirmation')
  }
  for (const [matching, total] of [[1, 1], [2000, 3000]]) {
    const counted = structuredClone(supported)
    counted.candidates[0].rationale = { kind: 'subject_majority', matching, total }
    const countedBody = await screen(`count-${total}-${lang}`, lang, await generation(counted))
    assert.ok(countedBody.includes(diagnostics.proposalRationaleText(counted.candidates[0].rationale, lang)))
    if (total === 1) assert.ok(countedBody.includes(lang === 'de' ? '1 von 1 Nachricht zu' : '1 of 1 message,'))
    else assert.ok(countedBody.includes(lang === 'de' ? '2.000 von 3.000' : '2,000 of 3,000'))
  }
}
const untrusted = structuredClone(supported)
untrusted.candidates[0].sender_claim = '<img src=x onerror=alert(1)>'
const escaped = await screen('escaped-de', 'de', untrusted)
assert.ok(!escaped.includes(untrusted.candidates[0].sender_claim))
assert.ok(escaped.includes('&lt;img'))
const empty = await screen('empty-de', 'de', null)
assert.ok(empty.includes('Ablagevorschläge erzeugen oder neu laden'))
assert.ok(!empty.includes('Aufbewahrte Entscheidungen'))
const disabled = await screen('disabled-de', 'de', null, null, true)
assert.ok(disabled.includes('Regeländerungen vor den Vorschlägen speichern oder neu laden.'))
await writeFile(resolve(audit, 'render-results.json'), JSON.stringify({ renders, checks: ['actual Rust payloads parsed', 'unknown reasons refused', 'invalid majority evidence refused', 'complete bilingual reason coverage', 'localized production component renderings', 'exact effects retained', 'explicit confirmation required', 'untrusted values escaped', 'empty and disabled states'], limits: 'Server rendering verifies component text and markup. It does not establish browser layout, interaction, focus, zoom, native, installed or provider acceptance.' }, null, 2) + '\n')
console.log(`Passed API contract checks and ${renders.length} production-component server renderings. Browser acceptance remains open.`)
