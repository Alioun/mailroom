import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const [logArgument, auditArgument] = process.argv.slice(2)
assert.ok(logArgument && auditArgument, 'Pass the credential-free Router test log and an output directory')
const log = await readFile(resolve(logArgument), 'utf8')
const marker = 'MAILROOM_SUMMARY_DISCLOSURE_FIXTURE '
const line = log.split('\n').find(line => line.startsWith(marker))
assert.ok(line, 'The actual Router test supplies the exact preview and download')
const fixture = JSON.parse(line.slice(marker.length))
const audit = resolve(auditArgument), root = resolve(import.meta.dirname, '..')
await mkdir(audit, { recursive: true })
const source = await readFile(`${root}/src/Reports.svelte`, 'utf8')
const imports = source.replace(/from '(\.\/[^']+)'/gu, (_match, name) => `from '${root}/src/${name.slice(2)}'`)
for (const confirmed of [false, true]) {
  const component = imports
    .replace('let report = $state<CensusReport | null>(null)', `let report = $state<CensusReport | null>(${JSON.stringify(fixture.report)})`)
    .replace('let preview = $state<SummaryPreview | null>(null)', `let preview = $state<SummaryPreview | null>(${JSON.stringify(fixture.preview)})`)
    .replace('let approved = $state<string[]>([])', `let approved = $state<string[]>(${JSON.stringify(fixture.export.approved)})`)
    .replace('let confirmed = $state(false)', `let confirmed = $state(${confirmed})`)
  await writeFile(`${audit}/Report${confirmed}.svelte`, component)
}
await writeFile(`${audit}/entry.mjs`, `export { default as Unconfirmed } from './Reportfalse.svelte'; export { default as Confirmed } from './Reporttrue.svelte'; export { exportSummary } from '${root}/src/api.ts'; export { render } from 'svelte/server'`)
await build({ root, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}/entry.mjs`, outDir: `${audit}/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'disclosure.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}/built/disclosure.mjs`))
const requests = []
let refused = false
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/summary/export')
  assert.equal(options.method, 'POST')
  requests.push(JSON.parse(options.body))
  return refused ? Response.json({ error: 'Invalid account input' }, { status: 422 }) : Response.json(fixture.downloaded)
}
const result = await ui.exportSummary(fixture.export.account, fixture.report, fixture.export.approved, fixture.preview.receipt)
assert.deepEqual(requests[0], fixture.export)
assert.deepEqual(result, fixture.downloaded)
refused = true
await assert.rejects(() => ui.exportSummary(fixture.export.account, fixture.report, fixture.export.approved, fixture.preview.receipt))
let renderings = 0
for (const lang of ['en', 'de']) {
  for (const confirmed of [false, true]) {
    const { body } = ui.render(confirmed ? ui.Confirmed : ui.Unconfirmed, { props: { account: fixture.export.account, lang } })
    const label = lang === 'en' ? 'Export after native confirmation' : 'Nach Systembestätigung exportieren'
    assert.ok(body.includes(label))
    assert.ok(body.includes(lang === 'en' ? 'Cancellation or unavailable protection refuses the export' : 'Abbruch oder fehlender Schutz sperren den Export'))
    const button = body.match(new RegExp(`<button[^>]*>${label}</button>`, 'u'))
    assert.ok(button)
    assert.equal(button[0].includes('disabled'), !confirmed)
    assert.ok(body.includes(lang === 'en' ? 'Open agent recommendations' : 'Agentenvorschläge öffnen'))
    await writeFile(`${audit}/report-${lang}-${confirmed}.html`, body)
    renderings++
  }
}
await writeFile(`${audit}/result.json`, JSON.stringify({ status: 'passed', renderings, requests: requests.length, nativeHelperInvoked: false, mountedBrowser: 'not_run' }, null, 2) + '\n')
console.log(JSON.stringify({ status: 'passed', renderings, requests: requests.length }))
