import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const auditName = process.argv[2] ?? 'imap-copy-account-compensation'
assert.ok(['imap-copy-account-compensation', 'imap-copy-after-clear'].includes(auditName))
const audit = `${root}.audit/${auditName}/`
await mkdir(`${audit}render`, { recursive: true })
const log = await readFile(`${audit}compensation-router.log`, 'utf8')
const match = log.match(/MAILROOM_COMPENSATION_ROUTER_FIXTURE (.+)/)
assert.ok(match, 'The real protected Router must produce the fixture')
const fixture = JSON.parse(match[1])
assert.equal(fixture.fixtures.length, auditName === 'imap-copy-after-clear' ? 16 : 10)
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
  if (entry.stage === 'Compacted') {
    assert.equal(view.journal.summaries[0].applied, 0)
    assert.equal(view.journal.summaries[0].restored, 0)
    assert.equal(view.journal.summaries[0].copies_removed, 2)
  } else {
    assert.deepEqual(view.journal.runs[0].compensation, entry.view.journal.runs[0].compensation)
    assert.equal(view.journal.runs[0].batches[0].copy_journal.before.messages['4'].modseq, '9007199254740993')
    if (entry.stage.startsWith('AfterClear')) {
      const run = view.journal.runs[0]
      assert.equal(run.compensation.restored, 0)
      assert.equal(run.compensation.deleted_cleared, 0)
      if (entry.stage === 'AfterClearCleaned') {
        assert.equal(run.status, 'undone')
        assert.equal(run.compensation.copies_removed, 2)
      } else if (entry.stage === 'AfterClearConflict') {
        assert.equal(run.status, 'conflict')
        assert.equal(run.compensation.copies_removed, 1)
        assert.equal(run.compensation.conflicts, 1)
      } else if (entry.stage.endsWith('Uncertain')) {
        assert.equal(run.status, 'uncertain')
        assert.equal(run.compensation.uncertain, 2)
      } else {
        assert.equal(run.status, 'stopped')
        assert.equal(run.compensation.pending, 2)
      }
    }
    if (entry.stage === 'Cleared') {
      assert.equal(view.journal.runs[0].status, 'stopped')
      assert.equal(view.journal.runs[0].compensation.deleted_cleared, 2)
      assert.equal(view.journal.runs[0].compensation.copies_removed, 0)
    }
  }
  assert.equal(view.installed, false)
  views.push({ stage: entry.stage, view })
}
let rejections = 0
for (const field of ['restored', 'copies_removed', 'deleted_cleared', 'pending', 'uncertain', 'conflicts']) {
  for (const value of [-1, 0.5, '2', null]) {
    const raw = structuredClone(fixture.fixtures[0].view)
    raw.journal.runs[0].compensation[field] = value
    await assert.rejects(decode(raw))
    rejections += 1
  }
}
for (const value of [[], {}, { ...fixture.fixtures[0].view.journal.runs[0].compensation, evidence: {} }]) {
  const raw = structuredClone(fixture.fixtures[0].view)
  raw.journal.runs[0].compensation = value
  await assert.rejects(decode(raw))
  rejections += 1
}
const legacy = structuredClone(fixture.fixtures[0].view)
delete legacy.journal.runs[0].compensation
assert.equal((await decode(legacy)).journal.runs[0].compensation, undefined)
const oldSummary = structuredClone(fixture.fixtures.at(-1).view)
delete oldSummary.journal.summaries[0].copies_removed
assert.equal((await decode(oldSummary)).journal.summaries[0].copies_removed, 0)
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
for (const { stage, view } of views) {
  for (const lang of ['en', 'de']) {
    const { body } = rendered.render(rendered.ProtocolBacklog, { props: { account: fixture.account, lang, fixture: view } })
    assert.ok(!body.includes('invented-private-pipe-token'))
    const buttons = [...body.matchAll(/<button([^>]*)>/g)]
    assert.equal(buttons.filter(button => !button[1].includes('disabled')).length, 1)
    const reload = lang === 'en' ? 'Reload saved protocol runs' : 'Gespeicherte Protokollabläufe laden'
    assert.ok(body.includes(reload), 'Reload remains visible')
    const enabledButtons = [...body.matchAll(/<button([^>]*)>([\s\S]*?)<\/button>/g)].filter(button => !button[1].includes('disabled'))
    assert.ok(enabledButtons[0][2].includes(reload), 'Only reload is enabled')
    if (stage === 'Compacted') {
      assert.ok(body.includes(lang === 'en' ? '2 extra copies removed' : '2 zusätzliche Kopien entfernt'))
      assert.ok(body.includes(lang === 'en' ? '0 restored' : '0 zurückgesetzt'))
    } else {
      assert.ok(body.includes('class="copy-compensation"'))
      assert.ok(body.includes(lang === 'en' ? 'COPY undo history' : 'COPY-Rückgängigverlauf'))
      assert.ok(body.includes(lang === 'en' ? 'Extra copies removed' : 'Entfernte zusätzliche Kopien'))
      if (stage === 'Cleared') {
        assert.ok(body.includes(lang === 'en' ? 'Undo is still incomplete.' : 'Das Rückgängigmachen ist noch nicht abgeschlossen.'))
      }
      assert.ok(body.includes(lang === 'en' ? 'Execution, reconciliation and Undo are unavailable' : 'Ausführung, Abgleich und Rückgängigmachen sind in dieser Version nicht verfügbar'))
    }
    await writeFile(`${audit}${stage}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
    results.push({ stage, lang, sha256: createHash('sha256').update(body).digest('hex') })
  }
}
assert.equal(responses.length, 0)
await writeFile(`${audit}ui-results.json`, `${JSON.stringify({ decodedRouterPhases: views.length, rejections, renderings: results, limitations: 'Server rendering with initial-state injection. No mounted browser, layout, action event or process-crash acceptance.' }, null, 2)}\n`)
console.log(`COPY compensation passed ${views.length} Router contracts, ${rejections} rejection cases and ${results.length} bilingual server renderings.`)
