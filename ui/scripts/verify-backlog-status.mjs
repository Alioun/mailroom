import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'
import { localModules } from './local-modules.mjs'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/backlog-status'}/`
const baseline = process.argv.includes('--baseline')
const phase = baseline ? 'baseline' : 'current'
await mkdir(`${audit}${phase}/source`, { recursive: true })
const input = await readFile(process.argv.find(arg => arg.endsWith('.log')) ?? `${root}.audit/imap-copy-journal/copy-router.log`, 'utf8')
const match = input.match(/MAILROOM_COPY_ROUTER_FIXTURE (.+)/)
assert.ok(match, 'The actual COPY Router fixture must be present')
const fixture = JSON.parse(match[1])
const simulationStates = ['prepared', 'running', 'stopped', 'completed', 'failed', 'uncertain']
const protocolStates = ['prepared', 'running', 'stopped', 'complete', 'uncertain', 'conflict', 'undo_running', 'undone']
const labels = {
  en: { prepared: 'Prepared', running: 'Running', stopped: 'Stopped', completed: 'Processing complete', failed: 'Failed', uncertain: 'Effects uncertain', complete: 'Processing complete', conflict: 'Conflicts found', undo_running: 'Undo in progress', undone: 'Undo finished' },
  de: { prepared: 'Vorbereitet', running: 'Läuft', stopped: 'Angehalten', completed: 'Verarbeitung abgeschlossen', failed: 'Fehlgeschlagen', uncertain: 'Änderungen ungewiss', complete: 'Verarbeitung abgeschlossen', conflict: 'Konflikte festgestellt', undo_running: 'Rückgängigmachen läuft', undone: 'Rückgängigmachen beendet' },
}
for (const name of localModules(['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics', 'proof-diagnostics'])) {
  const source = await readFile(`${root}ui/src/${name}.ts`, 'utf8')
  const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText
  const imports = compiled.replace(/from (['"])zod\1/g, `from '${import.meta.resolve('zod')}'`).replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, local) => `from '${local}.mjs'`)
  await writeFile(`${audit}${phase}/source/${name}.mjs`, imports)
}
const api = await import(pathToFileURL(`${audit}${phase}/source/api.mjs`))
const responses = []
let contractRequests = 0
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return new Response(JSON.stringify({ token: 'invented-status-fixture'.padEnd(64, '0') }))
  assert.ok(responses.length, 'Only a queued dummy response may be read')
  const next = responses.shift()
  assert.equal(path, next.path)
  assert.equal(JSON.parse(options.body).account, fixture.account)
  contractRequests += 1
  return new Response(JSON.stringify(next.value), { headers: { 'content-type': 'application/json' } })
}
const panels = [
  { name: 'Backlog', type: 'BacklogView', kind: 'simulation', path: '/api/accounts/backlog', open: api.openBacklog },
  { name: 'ProtocolBacklog', type: 'ProtocolView', kind: 'protocol', path: '/api/accounts/backlog/protocol', open: api.openProtocolBacklog },
  { name: 'GmailBacklog', type: 'GmailView', kind: 'gmail', path: '/api/accounts/backlog/gmail', open: api.openGmailBacklog },
]
for (const panel of panels) {
  let source = await readFile(`${root}ui/src/${panel.name}.svelte`, 'utf8')
  const props = "let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()"
  assert.equal(source.split(props).length, 2)
  source = source.replace(props, `let { account, lang, workspaceRevision = 0, fixture }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number; fixture: ${panel.type} } = $props()`)
  const initial = `let view = $state<${panel.type} | null>(null)`
  assert.equal(source.split(initial).length, 2)
  source = source.replace(initial, `let view = $state<${panel.type} | null>(fixture)`)
  source = source.replace(/from '(\.\/[^']+)'/g, (_match, name) => `from '${root}ui/src/${name.slice(2)}'`)
  await writeFile(`${audit}${phase}/source/${panel.name}.svelte`, source)
}
await writeFile(`${audit}${phase}/source/entry.mjs`, `${panels.map(panel => `export { default as ${panel.name} } from './${panel.name}.svelte'`).join('; ')}; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}${phase}/source/entry.mjs`, outDir: `${audit}${phase}/ssr`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'panels.mjs' } } } })
const rendered = await import(pathToFileURL(`${audit}${phase}/ssr/panels.mjs`))
const prior = baseline ? null : JSON.parse(await readFile(`${audit}baseline/results.json`, 'utf8'))
const results = []
let rejected = 0
function viewFor(panel, status) {
  const view = structuredClone(fixture.fixtures[0].view)
  const run = view.journal.runs[0]
  run.status = status
  view.installed = false
  view.recovery_hold = true
  if (panel.kind === 'protocol') return view
  if (panel.kind === 'gmail') {
    run.batches = []
    run.reverse = []
    return { ...view, outcomes: {} }
  }
  return { synthetic: true, runs: [{ id: run.id, plan: run.plan.filing, status, outcomes: run.plan.filing.changes.map(() => ({ state: 'pending' })), undo: run.plan.filing.changes.map(() => ({ state: 'not_requested' })), undo_requested: false, batches: 0, error: null }] }
}
for (const panel of panels) {
  const states = panel.kind === 'simulation' ? simulationStates : protocolStates
  for (const status of states) {
    responses.push({ path: panel.path, value: viewFor(panel, status) })
    const view = await panel.open(fixture.account)
    for (const lang of ['en', 'de']) {
      const { body } = rendered.render(rendered[panel.name], { props: { account: fixture.account, lang, fixture: view } })
      const name = `${panel.kind}-${status}-${lang}`
      const controls = [...body.matchAll(/<(?:button|input|select|option)\b[^>]*>/g)].map(match => match[0].replace(/\sclass="[^"]*"/g, ''))
      const state = body.match(/<p\b[^>]*class="run-state[^\"]*"[^>]*>(.*?)<\/p>/s)?.[1]
      assert.ok(state, 'The actual panel must render the run state')
      if (!baseline) {
        assert.ok(state.includes(labels[lang][status]), `${name} must show its translated label`)
        assert.match(body, /<p\b[^>]*class="[^"]*\brun-explanation\b[^"]*"/)
        assert.deepEqual(controls, prior.results.find(row => row.name === name).controls, `${name} must preserve all controls and disabled states`)
        if (status === 'uncertain') assert.ok(body.includes(lang === 'en' ? 'Do not assume it failed.' : 'Nicht davon ausgehen, dass sie fehlgeschlagen ist.'))
        if (status === 'stopped') assert.ok(body.includes(lang === 'en' ? 'Completed changes stay recorded.' : 'Abgeschlossene Änderungen bleiben gespeichert.'))
        if (status === 'undone' || status === 'complete' || status === 'completed') assert.ok(body.includes(lang === 'en' ? 'Check the recorded results' : 'Gespeicherte Ergebnisse'))
      } else if (panel.kind !== 'simulation') assert.equal(state, status, 'Reproduce the raw protocol or Gmail code')
      assert.ok(!body.includes('invented-status-fixture'))
      await writeFile(`${audit}${phase}/${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
      results.push({ name, controls, sha256: createHash('sha256').update(body).digest('hex') })
    }
  }
  for (const status of ['invented_future_state', '', null]) {
    responses.push({ path: panel.path, value: viewFor(panel, status) })
    await assert.rejects(panel.open(fixture.account))
    rejected += 1
  }
}
assert.equal(responses.length, 0)
await writeFile(`${audit}${phase}/results.json`, JSON.stringify({ contractRequests, rejected, results, limits: 'Schema-validated dummy status variants and server rendering of actual panels. Constructed simulation/Gmail views do not establish executed histories. No mounted, visual, TLS, native or provider acceptance.' }, null, 2) + '\n')
console.log(`${phase}: ${results.length} bilingual parent renderings, ${contractRequests} contract calls, ${rejected} rejected future or malformed states.`)
