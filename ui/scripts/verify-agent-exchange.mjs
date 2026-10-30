import assert from 'node:assert/strict'
import { readFile, mkdir, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const [fixtureArgument, auditArgument] = process.argv.slice(2)
assert.ok(fixtureArgument && auditArgument, 'Pass the SDK result.json and an output directory')
const fixture = JSON.parse(await readFile(resolve(fixtureArgument), 'utf8'))
const document = await readFile(fixture.proposalFile, 'utf8')
const summary = await readFile(resolve(fixture.proposalFile, '../../reviewed/census-summary.md'), 'utf8')
const audit = resolve(auditArgument), root = resolve(import.meta.dirname, '..')
await mkdir(audit, { recursive: true })
await writeFile(`${audit}/entry.mjs`, `export { readAgentProposal } from '${root}/src/agent-exchange.ts'; export { default as Review } from '${root}/src/AgentProposalReview.svelte'; export { default as Import } from '${root}/src/AgentProposalImport.svelte'; export { render } from 'svelte/server'`)
await build({ root, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}/entry.mjs`, outDir: `${audit}/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'agent.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}/built/agent.mjs`))
const proposal = await ui.readAgentProposal(document, summary)
assert.equal(proposal.summary_sha256, fixture.summaryDigest)
let refusals = 0
for (const alter of [
  p => { p.version = 2 },
  p => { p.summary_sha256 = '0'.repeat(64) },
  p => { p.account = 'invented-authority' },
  p => { p.recommendations = [] },
  p => { p.recommendations = Array(21).fill(p.recommendations[0]) },
  p => { p.recommendations[0].title = ' ' },
  p => { p.recommendations[0].title = 'x'.repeat(121) },
  p => { p.recommendations[0].reason = 'x\u0000y' },
  p => { p.recommendations[0].reason = '\ud800' },
  p => { p.recommendations[0].suggested_change = 'x'.repeat(2001) },
  p => { p.recommendations[0].execute = true },
]) {
  const invalid = structuredClone(proposal)
  alter(invalid)
  await assert.rejects(() => ui.readAgentProposal(JSON.stringify(invalid), summary))
  refusals++
}
await assert.rejects(() => ui.readAgentProposal(document, `${summary}\n`)); refusals++
await assert.rejects(() => ui.readAgentProposal('x'.repeat(131_073), summary)); refusals++
await assert.rejects(() => ui.readAgentProposal('{', summary)); refusals++
const unicode = structuredClone(proposal)
unicode.recommendations[0].title = '😀'.repeat(120)
await ui.readAgentProposal(JSON.stringify(unicode), summary)
let renderings = 0
for (const lang of ['en', 'de']) {
  const reviewed = ui.render(ui.Review, { props: { proposal, lang } }).body
  assert.match(reviewed, /&lt;img src=x onerror=alert\(1\)(?:>|&gt;)/u)
  assert.ok(!reviewed.includes('<img'))
  assert.ok(!reviewed.includes('<button'))
  assert.ok(reviewed.includes(proposal.summary_sha256))
  assert.ok(reviewed.includes(lang === 'en' ? 'saves no rules' : 'speichert keine Regeln'))
  const empty = ui.render(ui.Import, { props: { summary, lang } }).body
  assert.ok(empty.includes('type="file"'))
  assert.ok(!empty.includes(proposal.recommendations[0].reason))
  await writeFile(`${audit}/review-${lang}.html`, reviewed)
  await writeFile(`${audit}/import-${lang}.html`, empty)
  renderings += 2
}
await writeFile(`${audit}/result.json`, JSON.stringify({ status: 'passed', refusals, renderings, sdkProposal: fixture.proposal, mountedBrowser: 'not_run' }, null, 2) + '\n')
console.log(JSON.stringify({ status: 'passed', refusals, renderings }))
