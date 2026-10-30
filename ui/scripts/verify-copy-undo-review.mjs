import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const auditName = process.argv[2] ?? 'imap-copy-review-ui'
assert.ok(['imap-copy-review-ui', 'imap-copy-continuation-dispatch', 'imap-copy-continuation-controls', 'imap-forward-preview', 'imap-copy-saved-workflow', 'imap-empty-preview', 'imap-copy-multi-batch'].includes(auditName))
const audit = `${root}.audit/${auditName}/`
const inputAudit = auditName === 'imap-copy-continuation-controls' ? `${root}.audit/imap-copy-continuation-dispatch/` : audit
await mkdir(`${audit}render`, { recursive: true })
const log = await readFile(`${inputAudit}compensation-transport.log`, 'utf8')
const fixtures = [...log.matchAll(/MAILROOM_COPY_UNDO_UI_FIXTURE (.+)/g)].map(match => JSON.parse(match[1]))
assert.equal(fixtures.length, 5, 'Parser-backed workflows must emit all five exact reviews')
for (const name of ['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics']) {
  const source = await readFile(`${root}ui/src/${name}.ts`, 'utf8')
  const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText
  const imports = compiled.replace(/from (['"])zod\1/g, `from '${import.meta.resolve('zod')}'`).replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, local) => `from '${local}.mjs'`)
  await writeFile(`${audit}render/${name}.mjs`, imports)
}
const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const requests = []
const responses = []
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return new Response(JSON.stringify({ token: 'invented-private-pipe-token'.padEnd(64, '0') }))
  assert.equal(path, '/api/accounts/backlog/protocol')
  requests.push(JSON.parse(options.body))
  assert.ok(responses.length)
  return new Response(JSON.stringify(responses.shift()), { headers: { 'content-type': 'application/json' } })
}
let rejections = 0
function reject(fixture, mutate, target = fixture.target) {
  const raw = structuredClone(fixture.preview)
  mutate(raw)
  assert.throws(() => api.decodeCopyUndoReview(fixture.account, target, raw))
  rejections += 1
}
function visitModseq(value, replace) {
  if (!value || typeof value !== 'object') return
  for (const [key, child] of Object.entries(value)) {
    if (key === 'modseq' || key === 'highest_modseq') value[key] = replace
    else visitModseq(child, replace)
  }
}
const reviews = []
for (const fixture of fixtures) {
  assert.deepEqual(fixture.candidates, [fixture.target])
  const review = api.decodeCopyUndoReview(fixture.account, fixture.target, fixture.preview)
  reviews.push(review)
  assert.ok(api.copyUndoRows(review).length)
  responses.push(fixture.preview)
  assert.deepEqual(await api.previewProtocolCopyUndo(fixture.account, fixture.target), review)
  assert.deepEqual(requests.pop(), { account: fixture.account, command: { action: 'copy_undo_preview', target: fixture.target } })
  responses.push({})
  await api.confirmProtocolCopyUndo(review)
  assert.deepEqual(requests.pop(), { account: fixture.account, command: { action: 'copy_undo_confirm', target: fixture.target, receipt: fixture.preview.review.receipt, confirmed: true } })
  reject(fixture, raw => { raw.review.account = 'different-account' })
  reject(fixture, raw => { raw.review.run = 'f'.repeat(64) })
  for (const field of ['restore', 'remove_copy', 'clear_deleted', 'conflicts']) {
    reject(fixture, raw => { raw.review.actions[field] += 1 })
    reject(fixture, raw => { raw.review.actions[field] = -1 })
  }
  reject(fixture, raw => { raw.review.receipt = 'not-a-receipt' })
  reject(fixture, () => {}, { ...fixture.target, evidence: {} })
  for (const invalid of ['0', '01', '18446744073709551616', 9007199254740992, null]) reject(fixture, raw => visitModseq(raw, invalid))
  const large = structuredClone(fixture.preview)
  visitModseq(large, '9007199254740993')
  assert.ok(JSON.stringify(api.decodeCopyUndoReview(fixture.account, fixture.target, large)).includes('9007199254740993'))
  if (review.kind === 'after_clear') {
    reject(fixture, raw => { raw.review.forward += 1 })
    reject(fixture, raw => { raw.review.clear += 1 })
    reject(fixture, raw => { raw.review.evidence.after_clear = [] })
    reject(fixture, raw => { raw.review.evidence.after_clear.push(raw.review.evidence.after_clear[0]) })
    reject(fixture, () => {}, { kind: 'initial', run: fixture.target.run })
  } else reject(fixture, () => {}, { kind: 'after_clear', run: fixture.target.run, forward: 0, clear: 0 })
}
assert.deepEqual(reviews.map(review => review.kind).sort(), ['after_clear', 'after_clear', 'initial', 'initial', 'initial'])
assert.ok(reviews.some(review => review.review.actions.restore > 0))
assert.ok(reviews.some(review => review.review.actions.remove_copy > 0))
assert.ok(reviews.some(review => review.review.actions.clear_deleted > 0))
const routerLog = await readFile(`${inputAudit}compensation-router.log`, 'utf8')
const routerMatch = routerLog.match(/MAILROOM_COMPENSATION_ROUTER_FIXTURE (.+)/)
assert.ok(routerMatch)
const router = JSON.parse(routerMatch[1])
async function decodeView(raw) {
  responses.push(raw)
  const view = await api.openProtocolBacklog(router.account)
  assert.deepEqual(requests.pop(), { account: router.account, command: { action: 'status' } })
  return view
}
const views = []
for (const fixture of router.fixtures) {
  const view = await decodeView(fixture.view)
  assert.equal(view.copy_workflow.installed, false)
  assert.ok(Number.isInteger(view.copy_workflow.revision))
  views.push({ stage: fixture.stage, view })
}
let continuationRequests = 0
for (const { view } of views) {
  for (const run of view.journal.runs) {
    assert.equal(api.isCopyProtocolRun(run), true)
    for (const work of ['resume', 'resume_undo', 'reconcile']) {
      responses.push({})
      await api.continueProtocolRun(router.account, run, work)
      assert.deepEqual(requests.pop(), { account: router.account, command: { action: 'copy_continue', run: run.id, work } })
      continuationRequests += 1
    }
    for (const work of ['undo', 'resume ', null, 4, {}, []]) {
      await assert.rejects(api.continueProtocolRun(router.account, run, work))
      assert.equal(requests.length, 0, 'Invalid work must not reach fetch')
      rejections += 1
    }
    const wrongRun = structuredClone(run)
    wrongRun.id = 'not-a-copy-run-identity'
    await assert.rejects(api.continueProtocolRun(router.account, wrongRun, 'resume'))
    assert.equal(requests.length, 0, 'Invalid COPY identity must not reach fetch')
    rejections += 1
  }
}
for (const invalid of [true, null, 'false']) {
  const raw = structuredClone(router.fixtures[0].view)
  raw.copy_workflow.installed = invalid
  await assert.rejects(decodeView(raw))
  requests.pop()
  rejections += 1
}
const legacy = structuredClone(router.fixtures[0].view)
delete legacy.copy_workflow
assert.deepEqual((await decodeView(legacy)).copy_workflow, { installed: false, revision: 0, targets: [] })
let component = await readFile(`${root}ui/src/ProtocolBacklog.svelte`, 'utf8')
const props = "let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()"
assert.equal(component.split(props).length, 2)
component = component.replace(props, "let { account, lang, workspaceRevision = 0, fixture }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number; fixture: ProtocolView } = $props()")
const initial = 'let view = $state<ProtocolView | null>(null)'
assert.equal(component.split(initial).length, 2)
component = component.replace(initial, 'let view = $state<ProtocolView | null>(fixture)')
component = component.replace(/from '(\.\/[^']+)'/g, (_match, name) => `from '${root}ui/src/${name.slice(2)}'`)
await writeFile(`${audit}render/ProtocolBacklog.svelte`, component)
await writeFile(`${audit}render/entry.mjs`, `import '${root}ui/src/style.css'; export { default as ProtocolBacklog } from './ProtocolBacklog.svelte'; export { default as Review } from '${root}ui/src/CopyUndoReview.svelte'; export { default as Controls } from '${root}ui/src/CopyUndo.svelte'; export { render } from 'svelte/server'`)
const bundled = await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}render/entry.mjs`, outDir: `${audit}ssr`, emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'copy.mjs' } } } })
const styles = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css')).map(item => item.source.toString()).join('\n')
const rendered = await import(pathToFileURL(`${audit}ssr/copy.mjs`))
const renderings = []
async function save(name, lang, body) {
  assert.ok(!body.includes('invented-private-pipe-token'))
  await writeFile(`${audit}${name}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
  renderings.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
}
for (const lang of ['en', 'de']) {
  for (const [index, review] of reviews.entries()) {
    for (const disabled of [false, true]) {
      const { body } = rendered.render(rendered.Review, { props: { review, lang, disabled, onconfirm: async () => {}, oncancel: () => {} } })
      assert.ok(body.includes(review.review.receipt))
      assert.ok(body.includes(review.target.run))
      assert.ok(!/<input[^>]*\schecked(?:\s|=|>)/.test(body), 'Every exact review starts unchecked')
      const buttons = [...body.matchAll(/<button([^>]*)>([\s\S]*?)<\/button>/g)]
      assert.equal(buttons.length, 2)
      assert.ok(buttons[0][1].includes('disabled'), 'Apply waits for an explicit checkbox change')
      assert.ok(!buttons[1][1].includes('disabled'), 'Close review stays available')
      if (disabled) assert.match(body, /<fieldset[^>]*disabled/)
      for (const row of api.copyUndoRows(review)) {
        const snapshot = row.kind === 'restore' ? row.intent : row.kind === 'remove_copy' ? row.plan.deletion.before : row.plan.before
        const folder = row.kind === 'restore' ? snapshot.source.name.decoded : snapshot.identity.name.decoded
        assert.ok(body.includes(folder))
        for (const uid of Object.keys(snapshot.messages)) assert.ok(body.includes(uid))
        if (row.kind === 'restore') assert.ok(body.includes(row.intent.destination.name.decoded))
        if (row.kind === 'remove_copy') assert.ok(body.includes(row.plan.retained.identity.name.decoded))
      }
      if (review.kind === 'after_clear') assert.ok(body.includes(lang === 'en' ? 'This is a separate approval' : 'Dies ist eine separate Freigabe'))
      if (review.review.actions.clear_deleted > 0) assert.ok(body.includes(lang === 'en' ? 'Removing that copy requires another review and approval.' : 'Ihre Entfernung braucht eine neue Prüfung und Freigabe.'))
      await save(`review-${index}-${disabled ? 'held' : 'unchecked'}`, lang, body)
    }
    const controls = rendered.render(rendered.Controls, { props: { account: review.account, targets: [review.target], installed: false, disabled: false, lang, onstart: async () => {} } }).body
    assert.match(controls, /<button[^>]*disabled/)
    await save(`controls-${index}`, lang, controls)
  }
  for (const { stage, view } of views) {
    const { body } = rendered.render(rendered.ProtocolBacklog, { props: { account: router.account, lang, fixture: view } })
    const enabled = [...body.matchAll(/<button([^>]*)>([\s\S]*?)<\/button>/g)].filter(button => !button[1].includes('disabled'))
    assert.ok(!body.includes(lang === 'en' ? 'Preview conditional undo' : 'Bedingtes Rückgängigmachen prüfen'), 'COPY uses its separate review controls')
    for (const run of view.journal.runs) {
      if (!run.closed && !run.undo_receipt && !['complete', 'undone'].includes(run.status)) assert.ok(body.includes(lang === 'en' ? 'Resume confirmed COPY filing' : 'Bestätigte COPY-Ablage fortsetzen'))
      if (!run.closed && run.undo_receipt && run.status !== 'undone') assert.ok(body.includes(lang === 'en' ? 'Resume confirmed COPY Undo actions' : 'Bestätigte COPY-Rückgängig-Aktionen fortsetzen'))
    }
    assert.equal(enabled.length, 1)
    assert.ok(enabled[0][2].includes(lang === 'en' ? 'Reload saved protocol runs' : 'Gespeicherte Protokollabläufe laden'))
    if (view.copy_workflow.targets.length) assert.ok(body.includes('class="copy-undo"'))
    await save(`protocol-${stage}`, lang, body)
  }
}
assert.equal(requests.length, 0)
assert.equal(responses.length, 0)
await writeFile(`${audit}ui-results.json`, `${JSON.stringify({ reviews: reviews.length, routerStates: views.length, continuationRequests, rejections, renderings, limitations: 'API contracts and server rendering. No mounted interaction, layout, keyboard, connected TLS or process-crash acceptance.' }, null, 2)}\n`)
console.log(`COPY Undo review passed ${reviews.length} workflow reviews, ${views.length} Router states, ${continuationRequests} continuation requests, ${rejections} rejection cases and ${renderings.length} bilingual server renderings.`)
