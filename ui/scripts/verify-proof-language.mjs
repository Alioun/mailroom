import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = resolve(root, process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/proof-language')
const output = resolve(audit, 'render')
await mkdir(output, { recursive: true })
function record(log, marker) {
  const line = log.split('\n').find(value => value.startsWith(marker))
  assert.ok(line, `Missing Rust report ${marker}`)
  return JSON.parse(line.slice(marker.length))
}
const diagnosticsLog = await readFile(resolve(audit, 'proof.log'), 'utf8')
const reports = record(diagnosticsLog, 'MAILROOM_PROOF_DIAGNOSTICS_FIXTURE ')
const variants = record(diagnosticsLog, 'MAILROOM_PROOF_DIAGNOSTIC_VARIANTS ')
const forwardMarker = 'MAILROOM_FORWARD_PREVIEW_UI_FIXTURE '
const original = record(await readFile(resolve(root, '.audit/imap-copy-multi-batch/forward-preview.log'), 'utf8'), forwardMarker).preview.execution
const current = record(await readFile(resolve(audit, 'forward-preview.log'), 'utf8'), forwardMarker).preview.execution
for (const key of ['warnings', 'runtime_cross_check', 'counts', 'gate']) assert.deepEqual(current[key], original[key], `Existing proof behavior and historical warnings stay unchanged: ${key}`)
let source = await readFile(resolve(root, 'ui/src/RulesProof.svelte'), 'utf8')
const initial = "let proofState = $state<State>({ kind: 'initial' })"
assert.equal(source.split(initial).length, 2)
source = source.replace(initial, () => `let proofState = $state<State>(${JSON.stringify({ kind: 'done', proof: reports.partial }).replaceAll('<', '\\u003c')})`)
source = source.replace("'@ark-ui/svelte/toggle-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/toggle-group')))
source = source.replaceAll("from './", `from '${root}ui/src/`)
await writeFile(resolve(output, 'RulesProof.svelte'), source)
await writeFile(resolve(output, 'entry.mjs'), `export { default as Proof } from './RulesProof.svelte'; export { default as Diagnostics } from '${root}ui/src/ProofDiagnostics.svelte'; export { evaluate } from '${root}ui/src/api.ts'; export * from '${root}ui/src/proof-diagnostics.ts'; export { render } from 'svelte/server'`)
const bundled = await build({ root: resolve(root, 'ui'), configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: resolve(output, 'entry.mjs'), outDir: resolve(output, 'built'), emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'proof.mjs' } } } })
const styles = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css')).map(item => item.source.toString()).join('\n')
const ui = await import(pathToFileURL(resolve(output, 'built/proof.mjs')))
let reply = reports.partial
let calls = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/preview')
  calls++
  return Response.json(reply)
}
const input = JSON.parse(await readFile(resolve(root, 'examples/sample.json'), 'utf8'))
for (const proof of [reports.complete, reports.partial, current]) {
  reply = proof
  const parsed = await ui.evaluate(input)
  assert.deepEqual(parsed.diagnostics, proof.diagnostics)
  assert.equal(parsed.gate, proof.gate)
  assert.equal(parsed.artifact.executable, proof.artifact.executable)
}
let rejections = 0
for (const alter of [
  value => { value.diagnostics.notices.push('future_unknown_notice') },
  value => { value.diagnostics.notices = [''] },
  value => { value.diagnostics.notices = 'partial_census' },
  value => { value.diagnostics.runtime_cross_check = 'verified_provider' },
  value => { value.diagnostics.runtime_cross_check = 'passed' },
  value => { value.diagnostics.extra_authority = true },
  value => { delete value.diagnostics },
]) {
  reply = structuredClone(reports.partial)
  alter(reply)
  await assert.rejects(() => ui.evaluate(input), 'Unsupported diagnostics fail at the frontend boundary')
  rejections++
}
const all = ui.proofDiagnostics.parse(variants)
assert.equal(all.notices.length, 9)
for (const notice of all.notices) assert.notEqual(ui.proofNoticeText(notice, 'en'), ui.proofNoticeText(notice, 'de'))
const renderings = []
async function screen(name, component, props) {
  const { body } = ui.render(component, { props })
  await writeFile(resolve(output, `${name}.html`), `<!doctype html><html lang="${props.lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
  renderings.push({ name, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
for (const lang of ['en', 'de']) {
  const allBody = await screen(`all-notices-${lang}`, ui.Diagnostics, { diagnostics: all, lang })
  assert.equal((allBody.match(/<li[ >]/g) ?? []).length, 9)
  for (const notice of all.notices) assert.ok(allBody.includes(ui.proofNoticeText(notice, lang)))
  assert.ok(allBody.includes(ui.proofRuntimeText('not_run', lang)))
  const proofBody = await screen(`rules-proof-${lang}`, ui.Proof, { lang })
  assert.ok(proofBody.includes(lang === 'de' ? 'Regeln und Prüfung' : 'Rules and proof'))
  for (const notice of reports.partial.diagnostics.notices) assert.ok(proofBody.includes(ui.proofNoticeText(notice, lang)))
  assert.ok(!proofBody.includes('Delivery tags label a matched ingress address'))
  assert.ok(!proofBody.includes('Not run by this proof. Run scripts/verify-sieve.sh'))
  for (const notice of all.notices) {
    await screen(`notice-${notice}-${lang}`, ui.Diagnostics, { diagnostics: { notices: [notice], runtime_cross_check: 'not_run' }, lang })
  }
}
await writeFile(resolve(audit, 'ui-results.json'), JSON.stringify({ calls, rejections, notices: all.notices.length, renderings, limits: ['Server rendering and actual report payloads. No mounted browser, layout or visual acceptance.'] }, null, 2) + '\n')
console.log(`PASS: ${calls} API contracts, ${rejections} diagnostic rejections, ${renderings.length} bilingual server renderings`)
