import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/unmatched-archive'}/`
await mkdir(`${audit}render`, { recursive: true })
const log = await readFile(process.argv[2] ?? `${audit}verified-3.log`, 'utf8')
const fixtures = [...log.matchAll(/MAILROOM_ARCHIVE_FIXTURE (.+)/g)].map(match => JSON.parse(match[1]))
assert.deepEqual(fixtures.map(f => f.provider), ['mailbox_org', 'gmail'])
for (const name of ['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics', 'proof-diagnostics']) {
  const source = await readFile(`${root}ui/src/${name}.ts`, 'utf8')
  const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText
  const imports = compiled.replace(/from (['"])zod\1/g, `from '${import.meta.resolve('zod')}'`).replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, local) => `from '${local}.mjs'`)
  await writeFile(`${audit}render/${name}.mjs`, imports)
}
const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const responses = []
globalThis.fetch = async (_path, options) => {
  if (_path === '/api/session') return new Response(JSON.stringify({ token: 'invented-private-pipe-token'.padEnd(64, '0') }))
  const response = responses.shift()
  assert.ok(response)
  const command = JSON.parse(options.body).command
  assert.equal(command.action, response.action)
  if (command.options) assert.equal(command.options.intent ?? 'sort', response.value.plan.options.intent ?? 'sort')
  return new Response(JSON.stringify(response.value), { headers: { 'content-type': 'application/json' } })
}
let component = await readFile(`${root}ui/src/Backlog.svelte`, 'utf8')
const props = "let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()"
assert.equal(component.split(props).length, 2)
component = component.replace(props, "let { account, lang, workspaceRevision = 0, fixture }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number; fixture: { preview: BacklogPreview | null; view: BacklogView | null } } = $props()")
for (const [declaration, key] of [["let preview = $state<BacklogPreview | null>(null)", 'preview'], ["let view = $state<BacklogView | null>(null)", 'view']]) {
  assert.equal(component.split(declaration).length, 2)
  component = component.replace(declaration, `${declaration.slice(0, declaration.indexOf('(') + 1)}fixture.${key})`)
}
component = component.replace(/from '(.\/[^']+)'/g, (_match, name) => `from '${root}ui/src/${name.slice(2)}'`)
await writeFile(`${audit}render/Backlog.svelte`, component)
await writeFile(`${audit}render/entry.mjs`, `import '${root}ui/src/style.css'; export { default as Backlog } from './Backlog.svelte'; export { default as Offer } from '${root}ui/src/ArchiveOffer.svelte'; export { default as Folders } from '${root}ui/src/FolderProvisioning.svelte'; export { render } from 'svelte/server'`)
const bundled = await build({
  root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true },
  build: { ssr: `${audit}render/entry.mjs`, outDir: `${audit}ssr`, emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'archive.mjs' } } },
})
const styles = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css')).map(item => item.source.toString()).join('\n')
const rendered = await import(pathToFileURL(`${audit}ssr/archive.mjs`))
const results = []
async function screen(name, lang, Component, props) {
  const { body } = rendered.render(Component, { props })
  assert.ok(!body.includes('invented-private-pipe-token'))
  await writeFile(`${audit}${name}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
  results.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
  return body
}
for (const fixture of fixtures) {
  const account = fixture.preview.plan.account
  responses.push({ action: 'preview', value: fixture.preview })
  const preview = await api.previewBacklog(account, fixture.preview.plan.options)
  assert.equal(preview.plan.options.intent, 'archive_unmatched')
  responses.push({ action: 'open', value: fixture.completed })
  const view = await api.openBacklog(account)
  assert.equal(view.runs[0].plan.options.intent, 'archive_unmatched')
  if (fixture.provider === 'gmail') assert.deepEqual(preview.plan.changes[0].operation, { kind: 'gmail', add: [], remove: ['\\Inbox'] })
  responses.push({ action: 'preview', value: fixture.ordinary })
  const ordinary = await api.previewBacklog(account, fixture.ordinary.plan.options)
  assert.equal(ordinary.plan.changes.length, 0)
  for (const lang of ['en', 'de']) {
    const emptyView = { ...view, runs: [] }
    const emptySort = await screen(`${fixture.provider}-empty-sort`, lang, rendered.Backlog, { account, lang, fixture: { preview: ordinary, view: emptyView } })
    assert.ok(emptySort.includes('class="archive-offer"'), 'An empty successful sorting preview still offers separate archive review')
    const blocked = await screen(`${fixture.provider}-blocked-sort`, lang, rendered.Backlog, { account, lang, fixture: { preview: { ...ordinary, plan: { ...ordinary.plan, proof_gate: 'blocked' } }, view: emptyView } })
    assert.ok(!blocked.includes('class="archive-offer"'), 'A blocked preview cannot unlock the archive offer')
    const initial = await screen(`${fixture.provider}-initial`, lang, rendered.Backlog, { account, lang, fixture: { preview: null, view: { ...view, runs: [] } } })
    assert.ok(!initial.includes('class="archive-offer"'))
    const completed = await screen(`${fixture.provider}-completed`, lang, rendered.Backlog, { account, lang, fixture: { preview: null, view } })
    assert.ok(completed.includes('class="archive-offer"'))
    const planned = await screen(`${fixture.provider}-preview`, lang, rendered.Backlog, { account, lang, fixture: { preview, view } })
    assert.ok(planned.includes(lang === 'en' ? 'Archive this reviewed list' : 'Geprüfte Liste archivieren'))
    assert.ok(planned.includes('disabled', planned.indexOf('class="confirmation"')))
    const confirmation = planned.slice(planned.indexOf('class="confirmation"'))
    assert.ok(!confirmation.includes('type="checkbox" checked'))
  }
}
for (const lang of ['en', 'de']) {
  const offer = await screen('unchecked-offer', lang, rendered.Offer, { account: 'dummy', lang, disabled: false, folders: true, onpreview() {} })
  assert.match(offer, /<input type="checkbox"\s*\/?\s*>/)
  assert.match(offer, /<button disabled/)
  assert.ok(!offer.includes('class="folder-provisioning"'))
  const folders = await screen('archive-folder-title', lang, rendered.Folders, { account: 'dummy', lang, intent: 'archive_unmatched' })
  assert.ok(folders.includes(lang === 'en' ? 'Archive folders for old unmatched mail' : 'Archivordner für alte unzugeordnete Nachrichten'))
}
assert.equal(responses.length, 0)
await writeFile(`${audit}ui-results.json`, `${JSON.stringify({ apiChecks: 6, renderings: results, limitations: 'Server rendering with fixture initial-state injection. Mounted actions, layout and browser lifecycle remain unverified.' }, null, 2)}\n`)
console.log(`Archive API checks passed. ${results.length} bilingual server renderings passed.`)
