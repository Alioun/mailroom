import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}.audit/imap-copy-journal/`
await mkdir(`${audit}render`, { recursive: true })
const log = await readFile(`${audit}copy-router.log`, 'utf8')
const match = log.match(/MAILROOM_COPY_ROUTER_FIXTURE (.+)/)
assert.ok(match, 'The real protected Router must produce the fixture')
const fixture = JSON.parse(match[1])
assert.equal(fixture.fixtures.length, 8)
for (const name of ['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics']) {
  const source = await readFile(`${root}ui/src/${name}.ts`, 'utf8')
  const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText
  const imports = compiled.replace(/from (['"])zod\1/g, `from '${import.meta.resolve('zod')}'`).replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, local) => `from '${local}.mjs'`)
  await writeFile(`${audit}render/${name}.mjs`, imports)
}
const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const responses = []
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return new Response(JSON.stringify({ token: 'invented-private-pipe-token'.padEnd(64, '0') }))
  assert.equal(path, '/api/accounts/backlog/protocol')
  assert.deepEqual(JSON.parse(options.body), { account: fixture.account, command: { action: 'status' } })
  assert.ok(responses.length)
  return new Response(JSON.stringify(responses.shift()), { headers: { 'content-type': 'application/json' } })
}
async function decode(raw) {
  responses.push(raw)
  return api.openProtocolBacklog(fixture.account)
}
const views = []
for (const entry of fixture.fixtures) {
  const view = await decode(entry.view)
  const raw = entry.view.journal.runs[0].batches[0].evidence
  assert.deepEqual(view.journal.runs[0].batches[0].copy_journal, raw)
  assert.equal(raw.before.messages['4'].modseq, '9007199254740993')
  assert.equal(view.installed, false)
  views.push({ stage: entry.stage, view })
}
let rejections = 0
for (const value of [0, 9007199254740992, '0', '01', '18446744073709551616', '', '-1']) {
  const raw = structuredClone(fixture.fixtures[0].view)
  raw.journal.runs[0].batches[0].evidence.before.messages['4'].modseq = value
  await assert.rejects(decode(raw))
  rejections += 1
}
const maximum = structuredClone(fixture.fixtures[0].view)
maximum.journal.runs[0].batches[0].evidence.before.highest_modseq = '18446744073709551615'
assert.equal((await decode(maximum)).journal.runs[0].batches[0].copy_journal.before.highest_modseq, '18446744073709551615')
const historical = structuredClone(fixture.fixtures[0].view)
historical.journal.runs[0].batches[0] = historical.journal.runs[0].batches[0].evidence.copy
historical.journal.runs[0].plan.filing.semantics = 'imap-move-mapping-readback/v1'
const moveView = await decode(historical)
assert.equal(moveView.journal.runs[0].batches[0].copy_journal, null)
for (const change of ['kind', 'evidence']) {
  const smuggled = structuredClone(historical)
  smuggled.journal.runs[0].batches[0][change] = change === 'kind' ? 'copy_uid_expunge' : {}
  await assert.rejects(decode(smuggled))
  rejections += 1
}
let component = await readFile(`${root}ui/src/ProtocolBacklog.svelte`, 'utf8')
const props = "let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()"
assert.equal(component.split(props).length, 2)
component = component.replace(props, "let { account, lang, workspaceRevision = 0, fixture }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number; fixture: ProtocolView } = $props()")
const initial = 'let view = $state<ProtocolView | null>(null)'
assert.equal(component.split(initial).length, 2)
component = component.replace(initial, 'let view = $state<ProtocolView | null>(fixture)')
component = component.replace(/from '(\.\/[^']+)'/g, (_match, name) => `from '${root}ui/src/${name.slice(2)}'`)
await writeFile(`${audit}render/ProtocolBacklog.svelte`, component)
await writeFile(`${audit}render/entry.mjs`, `import '${root}ui/src/style.css'; export { default as ProtocolBacklog } from './ProtocolBacklog.svelte'; export { render } from 'svelte/server'`)
const bundled = await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}render/entry.mjs`, outDir: `${audit}ssr`, emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'copy.mjs' } } } })
const styles = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css')).map(item => item.source.toString()).join('\n')
const rendered = await import(pathToFileURL(`${audit}ssr/copy.mjs`))
const results = []
for (const { stage, view } of [...views, { stage: 'HistoricalMove', view: moveView }]) {
  for (const lang of ['en', 'de']) {
    const { body } = rendered.render(rendered.ProtocolBacklog, { props: { account: fixture.account, lang, fixture: view } })
    assert.ok(!body.includes('invented-private-pipe-token'))
    const buttons = [...body.matchAll(/<button([^>]*)>/g)]
    assert.ok(buttons.length >= 4)
    assert.equal(buttons.filter(button => !button[1].includes('disabled')).length, 1, 'Only reload is available without an installed source')
    if (stage === 'HistoricalMove') {
      assert.ok(!body.includes('class="copy-phases"'))
      assert.ok(!body.includes(lang === 'en' ? 'COPY history is preserved.' : 'COPY-Verlauf bleibt erhalten.'))
    } else {
      assert.ok(body.includes('class="copy-phases"'))
      assert.ok(body.includes(lang === 'en' ? 'COPY phases' : 'COPY-Phasen'))
      assert.ok(body.includes(lang === 'en' ? 'Execution, reconciliation and Undo are unavailable' : 'Ausführung, Abgleich und Rückgängigmachen sind in dieser Version nicht verfügbar'))
      if (['StoreUncertain', 'Marked', 'ExpungeUncertain', 'Filed', 'Undone'].includes(stage)) assert.ok(body.includes('4 = 9007199254740993'))
      if (['ExpungeUncertain', 'Filed', 'Undone'].includes(stage)) assert.ok(body.includes('data-copy-phase="expunge"'))
      if (stage === 'StoreUncertain') assert.ok(body.includes(lang === 'en' ? 'No acknowledgment recorded' : 'Keine Bestätigung gespeichert'))
    }
    await writeFile(`${audit}${stage}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
    results.push({ stage, lang, sha256: createHash('sha256').update(body).digest('hex') })
  }
}
assert.equal(responses.length, 0)
await writeFile(`${audit}ui-results.json`, `${JSON.stringify({ decodedRouterPhases: views.length, rejections, exactModSeq: '9007199254740993', maximumModSeq: '18446744073709551615', renderings: results, limitations: 'Server rendering with initial-state injection. No mounted browser, layout, action event or process-crash acceptance.' }, null, 2)}\n`)
console.log(`COPY history passed ${views.length} Router contracts, ${rejections} rejection cases and ${results.length} bilingual server renderings.`)
