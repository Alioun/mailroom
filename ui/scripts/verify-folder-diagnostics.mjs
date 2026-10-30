import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const baseline = process.argv.includes('--baseline')
const phase = baseline ? 'baseline' : 'current'
const audit = `${root}.audit/folder-diagnostics/${phase}/`
await mkdir(`${audit}source`, { recursive: true })
const preceding = path => execFileSync('/usr/bin/git', ['-c', 'core.fsmonitor=false', 'show', `a126c536d621839e901fb31e3273e86a57f16c87:${path}`], { cwd: root, encoding: 'utf8' })
const rust = await readFile(`${root}src/census/folder_provisioning.rs`, 'utf8')
const failures = [...rust.match(/pub enum Failure \{([\s\S]*?)\}/)[1].matchAll(/^\s*([A-Z][A-Za-z0-9_]*),/gm)].map(([, value]) => value.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase())
assert.equal(failures.length, 16)
const name = { decoded: 'Privat/Beispiel', wire: 'Privat/Beispiel' }
const inventory = { capabilities: ['IMAP4rev1'], namespaces: { personal: [{ prefix: '', delimiter: '/' }], other: [], shared: [] }, mailboxes: [] }
const operation = { id: 'display-operation', artifact: 'a'.repeat(64), proof_binding: 'b'.repeat(64), receipt: 'c'.repeat(64), before: inventory, plan: { logical_leaves: [name.decoded], entries: [{ name, role: 'leaf', existing: null }] }, status: 'uncertain', steps: [{ name, outcome: 'uncertain', acknowledgment: false, failure: null, before: inventory, observed: null }] }
const options = { scope: 'inbox', protect_flagged: true, selected: null }
const folderReview = { operation, receipt: operation.receipt, proof: { gate: 'ready', semantics: 'display-only', pins: [], warnings: [], runtime_cross_check: 'Constructed display fixture only', recipient_evidence: 'unknown', capabilities: [], context_binding: 'd'.repeat(64) }, limitations: 'Constructed display fixture only' }
const labelReview = { operation, receipt: operation.receipt, proof: { saved_proof: 'e'.repeat(64), options, missing: [{ name: name.decoded, messages: 1, blocked: null }] }, limitations: 'Constructed display fixture only' }
const view = history => ({ installed: true, recovery_hold: false, journal: { history }, limitations: 'Constructed display fixture only' })
const cases = failures.map(code => {
  const record = structuredClone(operation)
  record.steps[0].failure = code
  return { name: code, code, view: view([record]) }
})
cases.push({ name: 'no-failure', view: view([operation]) })
cases.push({ name: 'empty-history', view: view([]) })
cases.push({ name: 'restored-hold', view: { ...view([operation]), recovery_hold: true } })
cases.push({ name: 'exact-preview', view: view([]), preview: folderReview, labelPreview: labelReview })
const intent = structuredClone(operation)
intent.status = 'running'
intent.steps[0].outcome = 'intent'
cases.push({ name: 'intent-before-send', view: view([intent]) })
const observed = structuredClone(operation)
observed.status = 'complete'
observed.steps[0].outcome = 'observed'
observed.steps[0].acknowledgment = true
observed.steps[0].observed = { ...inventory, mailboxes: [{ name, delimiter: '/', attributes: [] }] }
cases.push({ name: 'observed-creation', view: view([observed]) })
const apiPath = baseline ? `${audit}source/folder-provisioning-api.ts` : `${root}ui/src/folder-provisioning-api.ts`
if (baseline) await writeFile(apiPath, preceding('ui/src/folder-provisioning-api.ts').replaceAll("from './api'", `from '${root}ui/src/api'`).replace("'zod'", JSON.stringify(import.meta.resolve('zod'))))
for (const panel of ['FolderProvisioning', 'GmailLabels']) {
  const source = baseline ? preceding(`ui/src/${panel}.svelte`) : await readFile(`${root}ui/src/${panel}.svelte`, 'utf8')
  for (const [index, item] of cases.entries()) {
    let component = source
    if (panel === 'FolderProvisioning') {
      const initial = 'let view = $state<FolderView | null>(null)'
      assert.ok(component.includes(initial))
      component = component.replace(initial, () => `let view = $state<FolderView | null>(${JSON.stringify(item.view)})`)
      if (item.preview) component = component.replace('let preview = $state<FolderPreview | null>(null)', () => `let preview = $state<FolderPreview | null>(${JSON.stringify(item.preview)})`)
    } else if (item.labelPreview) component = component.replace('let preview = $state<LabelPreview | null>(null)', () => `let preview = $state<LabelPreview | null>(${JSON.stringify(item.labelPreview)})`)
    component = component.replaceAll("from './", `from '${root}ui/src/`).replace(`from '${root}ui/src/folder-provisioning-api'`, `from '${apiPath}'`).replace("'zod'", JSON.stringify(import.meta.resolve('zod')))
    await writeFile(`${audit}source/${panel}${index}.svelte`, component)
  }
}
await writeFile(`${audit}source/entry.mjs`, `${cases.flatMap((_, i) => ['FolderProvisioning', 'GmailLabels'].map(panel => `export { default as ${panel}${i} } from './${panel}${i}.svelte'`)).join('; ')}; export * from '${apiPath}'; ${baseline ? '' : `export * from '${root}ui/src/folder-diagnostics';`} export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}source/entry.mjs`, outDir: `${audit}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'diagnostics.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}built/diagnostics.mjs`))
let reply
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.ok(['/api/accounts/folders', '/api/accounts/backlog/gmail'].includes(path))
  requests++
  return Response.json(reply)
}
const valid = []
for (const item of cases) {
  reply = item.view
  assert.deepEqual(await ui.folderStatus('display-account'), reply)
  assert.deepEqual(ui.labelJournal.parse(reply.journal), reply.journal)
  valid.push(item.name)
}
for (const code of [...failures, null]) {
  reply = structuredClone(folderReview)
  reply.operation.steps[0].failure = code
  assert.deepEqual(await ui.folderPreview('display-account'), reply)
  reply = structuredClone(labelReview)
  reply.operation.steps[0].failure = code
  assert.deepEqual(await ui.labelPreviewFor('display-account', options), reply)
}
const malformed = []
for (const code of ['future_failure', 'Tls', '', 7, { code: 'tls' }]) {
  for (const target of ['status', 'folder-preview', 'label-journal', 'label-preview']) {
    const value = structuredClone(target === 'status' || target === 'label-journal' ? view([operation]) : target === 'folder-preview' ? folderReview : labelReview)
    if ('journal' in value) value.journal.history[0].steps[0].failure = code
    else value.operation.steps[0].failure = code
    reply = value
    let accepted = false
    try {
      if (target === 'status') await ui.folderStatus('display-account')
      else if (target === 'folder-preview') await ui.folderPreview('display-account')
      else if (target === 'label-preview') await ui.labelPreviewFor('display-account', options)
      else ui.labelJournal.parse(value.journal)
      accepted = true
    } catch {}
    if (!baseline) assert.equal(accepted, false, `${target}-${JSON.stringify(code)}`)
    malformed.push({ target, code, accepted })
  }
}
if (!baseline) assert.deepEqual(ui.folderFailure.options, failures)
const controls = body => [...body.matchAll(/<button\b[^>]*>[\s\S]*?<\/button>|<input\b[^>]*>/g)].map(([full]) => full.replace(/svelte-[\w-]+/g, 'svelte-scope'))
const renderings = []
for (const [index, item] of cases.entries()) {
  for (const lang of ['en', 'de']) {
    for (const panel of ['FolderProvisioning', 'GmailLabels']) {
      const props = { account: 'display-account', lang, options, offer: true, history: item.view.journal.history, limitations: 'Constructed display fixture only', disabled: item.view.recovery_hold, onchange: async () => {}, onreload: async () => {} }
      const { body } = ui.render(ui[`${panel}${index}`], { props })
      const clean = body.replace(/<!--[\s\S]*?-->/g, '')
      const text = clean.replace(/<[^>]*>/g, '')
      if (panel === 'FolderProvisioning') assert.ok(!text.includes(name.decoded), 'Folder names stay hidden before private review')
      if (item.name === 'intent-before-send') {
        const pending = lang === 'en' ? 'Intent persisted' : 'Absicht gespeichert'
        if (baseline && panel === 'GmailLabels') assert.ok(text.includes(lang === 'en' ? 'sent, no answer yet' : 'gesendet, noch keine Antwort'))
        else assert.ok(text.includes(pending), 'A saved intent does not establish that a command was sent')
      }
      if (item.name === 'observed-creation' && panel === 'GmailLabels') assert.ok(text.includes(lang === 'en' ? 'read back' : 'geprüft'))
      if (baseline && item.code && panel === 'FolderProvisioning') assert.ok(text.includes(item.code))
      if (!baseline && item.code) {
        assert.ok(text.includes(ui.folderFailureText({ failure: item.code, lang })))
        if (item.code.includes('_')) assert.ok(!text.includes(item.code))
      }
      await writeFile(`${audit}${panel}-${item.name}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
      renderings.push({ name: item.name, panel, lang, controls: controls(clean) })
    }
  }
}
if (baseline) assert.ok(malformed.some(row => row.accepted))
else {
  const before = JSON.parse(await readFile(`${audit}../baseline/results.json`, 'utf8'))
  assert.deepEqual(renderings.map(row => row.controls), before.renderings.map(row => row.controls))
}
await writeFile(`${audit}results.json`, JSON.stringify({ failures, valid, requests, malformed, renderings, limits: 'Constructed histories/previews, compiled component rendering and actual client parsers. No mounted, TLS, native or provider acceptance.' }, null, 2) + '\n')
console.log(`${phase}: ${renderings.length} bilingual folder/label renderings, ${requests} requests and ${malformed.length} malformed diagnostic checks passed.`)
