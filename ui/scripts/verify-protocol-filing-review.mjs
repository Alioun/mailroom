import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}.audit/imap-forward-preview/`
await mkdir(`${audit}forward-render`, { recursive: true })
const log = await readFile(`${audit}forward-preview.log`, 'utf8')
const fixtures = [...log.matchAll(/MAILROOM_FORWARD_PREVIEW_UI_FIXTURE (.+)/g)].map(match => JSON.parse(match[1]))
assert.deepEqual(fixtures.map(fixture => fixture.method).sort(), ['copy', 'move'])
for (const name of ['api', 'invoice-diagnostics', 'acquisition-evidence', 'proposal-diagnostics']) {
  const source = await readFile(`${root}ui/src/${name}.ts`, 'utf8')
  const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText
  const imports = compiled.replace(/from (['"])zod\1/g, `from '${import.meta.resolve('zod')}'`).replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, local) => `from '${local}.mjs'`)
  await writeFile(`${audit}forward-render/${name}.mjs`, imports)
}
const api = await import(pathToFileURL(`${audit}forward-render/api.mjs`))
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
const previews = []
for (const fixture of fixtures) {
  const options = fixture.preview.operation.plan.filing.options
  responses.push(fixture.preview)
  const preview = await api.previewProtocolBacklog(fixture.account, options)
  assert.deepEqual(requests.pop(), { account: fixture.account, command: { action: 'preview', options } })
  assert.equal(api.isCopyProtocolRun(preview.operation), fixture.method === 'copy')
  if (fixture.method === 'copy') assert.ok(JSON.stringify(preview).includes('9007199254740993'), 'MODSEQ must retain its exact value beyond JavaScript integer precision')
  responses.push({})
  await api.confirmProtocolBacklog(fixture.account, preview)
  assert.deepEqual(requests.pop(), { account: fixture.account, command: { action: fixture.method === 'copy' ? 'copy_confirm' : 'confirm', options, receipt: preview.receipt, confirmed: true } })
  for (const mutate of [
    raw => { raw.operation.plan.binding.account = 'different-account' },
    raw => { raw.operation.plan.filing.account = 'different-account' },
    raw => { raw.receipt = 'f'.repeat(64) },
    raw => { raw.operation.receipt = 'f'.repeat(64) },
    raw => { raw.operation.plan.saved_proof = 'f'.repeat(64) },
    raw => { raw.saved.receipt = 'f'.repeat(64) },
    raw => { raw.receipt = raw.operation.receipt = 'not-a-receipt' },
    raw => { raw.operation.status = 'running' },
    raw => { raw.operation.stop_requested = true },
    raw => { raw.operation.undo_receipt = 'f'.repeat(64) },
    raw => { raw.operation.batches = [] },
    raw => { raw.operation.plan.filing.changes = [] },
  ]) {
    const raw = structuredClone(preview)
    mutate(raw)
    await assert.rejects(api.confirmProtocolBacklog(fixture.account, raw))
    assert.equal(requests.length, 0, 'Invalid confirmation must fail before fetch')
    rejections += 1
  }
  await assert.rejects(api.confirmProtocolBacklog('different-account', preview))
  assert.equal(requests.length, 0)
  rejections += 1
  previews.push({ ...fixture, preview })
}
await writeFile(`${audit}forward-render/entry.mjs`, `import '${root}ui/src/style.css'; export { default as Review } from '${root}ui/src/ProtocolFilingReview.svelte'; export { render } from 'svelte/server'`)
const bundled = await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}forward-render/entry.mjs`, outDir: `${audit}forward-ssr`, emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'review.mjs' } } } })
const styles = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css')).map(item => item.source.toString()).join('\n')
const rendered = await import(pathToFileURL(`${audit}forward-ssr/review.mjs`))
const renderings = []
for (const { method, preview } of previews) {
  for (const lang of ['en', 'de']) {
    for (const available of [false, true]) {
      for (const disabled of [false, true]) {
        const { body } = rendered.render(rendered.Review, { props: { preview, lang, available, disabled, onconfirm: async () => {} } })
        assert.ok(!/<input[^>]*\schecked(?:\s|=|>)/.test(body), 'Every preview starts unchecked')
        assert.match(body, /<button[^>]*disabled/)
        assert.equal(/<fieldset[^>]*disabled/.test(body), disabled || !available)
        assert.ok(body.includes(preview.receipt))
        assert.ok(body.includes(preview.operation.plan.binding.account))
        assert.ok(body.includes(preview.operation.plan.saved_proof))
        assert.ok(body.includes(preview.operation.plan.execution_proof))
        for (const batch of preview.operation.batches) {
          assert.ok(body.includes(batch.intent.source.name.decoded))
          assert.ok(body.includes(batch.intent.destination.name.decoded))
          assert.ok(body.includes(`UIDVALIDITY ${batch.intent.source.uidvalidity}`))
          assert.ok(body.includes(`UIDVALIDITY ${batch.intent.destination.uidvalidity}`))
          for (const uid of Object.keys(batch.intent.messages)) assert.ok(body.includes(uid))
        }
        if (method === 'copy') {
          assert.match(body, /UID EXPUNGE/)
          assert.match(body, /Deleted/)
          assert.ok(body.includes(lang === 'en' ? 'I confirm these exact copies and source removals' : 'Ich bestätige diese genauen Kopien und Quellentfernungen'))
          assert.equal(body.includes(lang === 'en' ? 'COPY filing is unavailable in this build.' : 'COPY-Ablage ist in dieser Version nicht verfügbar.'), !available)
          assert.ok(!body.includes(lang === 'en' ? 'I confirm these exact UID MOVE changes' : 'Ich bestätige diese genauen UID-MOVE-Änderungen'))
        } else {
          assert.ok(body.includes(lang === 'en' ? 'I confirm these exact UID MOVE changes' : 'Ich bestätige diese genauen UID-MOVE-Änderungen'))
          assert.ok(!body.includes('UID EXPUNGE'))
        }
        assert.ok(!body.includes('invented-private-pipe-token'))
        const name = `forward-${method}-${available ? 'available' : 'held'}-${disabled ? 'busy' : 'idle'}-${lang}`
        await writeFile(`${audit}${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${body}</body></html>`)
        renderings.push({ name, sha256: createHash('sha256').update(body).digest('hex') })
      }
    }
  }
}
assert.equal(requests.length, 0)
assert.equal(responses.length, 0)
await writeFile(`${audit}forward-ui-results.json`, `${JSON.stringify({ previews: previews.length, rejections, renderings, limitations: 'Credential-free saved-rule previews, intercepted request contracts and server renderings. Available COPY component props are fixture-only. No mounted interaction, visual, connected TLS or process-crash acceptance.' }, null, 2)}\n`)
console.log(`Forward filing review passed ${previews.length} saved-rule previews, ${rejections} rejected confirmations and ${renderings.length} bilingual server renderings.`)
