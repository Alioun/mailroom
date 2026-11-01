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
const audit = resolve(root, process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/combined-subscriptions')
const output = resolve(audit, 'render')
await mkdir(output, { recursive: true })
const log = await readFile(resolve(audit, 'rust-tests.log'), 'utf8')
const marker = 'MAILROOM_COMBINED_REVIEW_FIXTURE '
const record = log.split('\n').find(line => line.includes(marker))
assert.ok(record, 'Run the combined wired tests with --nocapture first')
for (const sentinel of ['synthetic-mail-token', 'synthetic-private-token', 'synthetic-relay-token']) assert.ok(!record.includes(sentinel), 'The UI fixture export must omit unsubscribe tokens')
const fixture = JSON.parse(record.slice(record.indexOf(marker) + marker.length))
function imports(code) {
  return code.replace(/(['"])(zod|svelte(?:\/[^'"]+)?)\1/g, (_match, _quote, name) => JSON.stringify(import.meta.resolve(name)))
    .replace(/from '(.\/[^']+)'/g, (_match, name) => `from '${name}.mjs'`)
}
for (const name of localModules(['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics', 'proof-diagnostics'])) {
  const source = await readFile(resolve(root, `ui/src/${name}.ts`), 'utf8')
  const emitted = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext }, fileName: `${name}.ts` })
  await writeFile(resolve(output, `${name}.mjs`), imports(emitted.outputText))
}
const sharedSource = await readFile(resolve(root, 'ui/src/ProofDiagnostics.svelte'), 'utf8')
const shared = compile(sharedSource, { filename: 'ProofDiagnostics.svelte', generate: 'server', css: 'external' })
assert.equal(shared.warnings.length, 0)
await writeFile(resolve(output, 'ProofDiagnostics.svelte.mjs'), imports(shared.js.code))
const api = await import(pathToFileURL(resolve(output, 'api.mjs')))
const proofText = await import(pathToFileURL(resolve(output, 'proof-diagnostics.mjs')))
let reply = fixture.view
let lastRequest
let calls = 0
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/subscription-review')
  lastRequest = JSON.parse(options.body)
  calls++
  return Response.json(reply)
}
const view = await api.openSubscriptionReview(1792886400)
assert.equal(view.synthetic, false)
assert.equal(view.review_available, true)
assert.equal(view.accounts.length, 2)
const input = { as_of: 1792886400, selections: fixture.preview.previews.flatMap(p => p.actions.map(a => ({ account: a.account, subscription: a.subscription, method: a.method }))), folders: { personal: 'Muted', business: 'Quiet' } }
reply = fixture.preview
const preview = await api.previewSubscriptionReview(input)
assert.equal(preview.confirmation.synthetic, true)
reply = fixture.mute_preview
const mutePreview = await api.previewSubscriptionReview(fixture.mute_input)
assert.ok(mutePreview.previews.every(preview => preview.proof !== null))
reply = structuredClone(fixture.preview)
reply.confirmation.synthetic = false
const nativePreview = await api.previewSubscriptionReview(input)
assert.equal(nativePreview.confirmation.synthetic, false)
reply.confirmation.synthetic = 'false'
await assert.rejects(() => api.previewSubscriptionReview(input))
reply = fixture.done
const done = await api.confirmSubscriptionReview(input, nativePreview)
assert.deepEqual(lastRequest.command.input.selections, input.selections)
assert.equal(done.accounts.every(a => a.history.length === 1 && a.history[0].status === 'handed_off'), true)
reply = structuredClone(fixture.view)
delete reply.review_available
await assert.rejects(() => api.openSubscriptionReview(1792886400), 'Missing server availability cannot grant UI controls')

const source = await readFile(resolve(root, 'ui/src/Unsubscribe.svelte'), 'utf8')
const renders = []
async function screen(name, lang, initialView, initialPlan = null) {
  let component = source
  for (const [declaration, value] of [
    ['let view = $state<SubscriptionReviewView | null>(null)', initialView],
    ['let plan = $state<SubscriptionReviewPreview | null>(null)', initialPlan],
  ]) {
    assert.equal(component.split(declaration).length, 2)
    component = component.replace(declaration, `${declaration.slice(0, -5)}${JSON.stringify(value)})`)
  }
  const compiled = compile(component, { filename: 'Unsubscribe.svelte', generate: 'server', css: 'external' })
  assert.equal(compiled.warnings.length, 0)
  const destination = resolve(output, `${name}.mjs`)
  await writeFile(destination, imports(compiled.js.code))
  const { default: Component } = await import(pathToFileURL(destination))
  const { body } = render(Component, { props: { lang } })
  await writeFile(resolve(output, `${name}.html`), `<!doctype html><html lang="${lang}"><meta charset="utf-8"><title>Mailroom dummy subscription review</title><body>${body}</body></html>`)
  renders.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
for (const lang of ['en', 'de']) {
  const active = await screen(`wired-${lang}`, lang, view, nativePreview)
  assert.ok(active.includes('data-exact-confirmation'))
  assert.ok(active.includes(lang === 'en' ? 'Confirm this exact action list' : 'Genau diese Aktionsliste bestätigen'))
  assert.ok(active.includes(lang === 'en' ? 'Include exact subscription' : 'Genaues Abonnement einbeziehen'))
  for (const account of view.accounts) assert.ok(active.includes(`data-account="${account.account}"`))
  const mute = await screen(`mute-proof-${lang}`, lang, view, mutePreview)
  for (const preview of mutePreview.previews) {
    for (const notice of preview.proof.diagnostics.notices) assert.ok(mute.includes(proofText.proofNoticeText(notice, lang)))
    assert.ok(mute.includes(proofText.proofGateText(preview.proof.gate, lang)))
  }
  assert.ok(!/<input[^>]*\schecked(?:\s|=|>)/.test(mute), 'Mute confirmation starts unchecked')
  if (lang === 'de') {
    const summaries = [...mute.matchAll(/<p\b[^>]*\bdata-proof-summary(?:="[^"]*")?[^>]*>([\s\S]*?)<\/p>/g)]
    assert.equal(summaries.length, mutePreview.previews.length)
    for (const [, text] of summaries) {
      assert.ok(text.includes('abgelegt'))
      assert.ok(text.includes('unabhängige Kopien'))
      assert.ok(!text.includes('filed') && !text.includes('independent copies'))
    }
  }
  const denied = { ...view, review_available: false }
  const blocked = await screen(`denied-${lang}`, lang, denied)
  assert.ok(!blocked.includes(lang === 'en' ? 'Include exact subscription' : 'Genaues Abonnement einbeziehen'))
  assert.ok(!blocked.includes(lang === 'en' ? 'Preview selected subscription actions' : 'Ausgewählte Abonnementaktionen prüfen'))
  const completed = await screen(`handed-off-${lang}`, lang, done)
  assert.ok(completed.includes(lang === 'en' ? 'Opened in Mail, sending unknown' : 'In Mail geöffnet, Versand unbekannt'))
}
await writeFile(resolve(audit, 'ui-results.json'), JSON.stringify({ contractRequests: calls, renderings: renders, limitations: ['Server rendering and API contract checks only. No interactive browser or native Mail launch.'] }, null, 2) + '\n')
console.log(`PASS: ${calls} API contract calls and ${renders.length} bilingual server renderings`)
