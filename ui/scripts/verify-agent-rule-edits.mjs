import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const [logArgument, auditArgument] = process.argv.slice(2)
assert.ok(logArgument && auditArgument, 'Pass the credential-free Router test log and an output directory')
const log = await readFile(resolve(logArgument), 'utf8')
const marker = 'MAILROOM_AGENT_RULE_EDITS_FIXTURE '
const line = log.split('\n').find(line => line.startsWith(marker))
assert.ok(line, 'The actual Router test supplies the exact review and saved response')
const fixture = JSON.parse(line.slice(marker.length))
const audit = resolve(auditArgument), root = resolve(import.meta.dirname, '..')
await mkdir(audit, { recursive: true })
const source = (await readFile(`${root}/src/AgentRuleEditReview.svelte`, 'utf8'))
  .replace(/from '(\.\/[^']+)'/gu, (_match, name) => `from '${root}/src/${name.slice(2)}'`)
for (const confirmed of [false, true]) {
  const component = source
    .replace('let review = $state<AgentEditReview | null>(null)', `let review = $state<AgentEditReview | null>(${JSON.stringify(fixture.review)})`)
    .replace('let confirmed = $state(false)', `let confirmed = $state(${confirmed})`)
  assert.notEqual(component, source)
  await writeFile(`${audit}/Review${confirmed}.svelte`, component)
}
await writeFile(`${audit}/entry.mjs`, `export { default as Unconfirmed } from './Reviewfalse.svelte'; export { default as Confirmed } from './Reviewtrue.svelte'; export { default as Import } from '${root}/src/AgentProposalImport.svelte'; export { previewAgentEdits, confirmAgentEdits } from '${root}/src/api.ts'; export { readAgentProposal } from '${root}/src/agent-exchange.ts'; export { render } from 'svelte/server'`)
await build({ root, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}/entry.mjs`, outDir: `${audit}/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'agent-rule-edits.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}/built/agent-rule-edits.mjs`))

const proposal = await ui.readAgentProposal(JSON.stringify(fixture.proposal), fixture.summary)
assert.deepEqual(proposal.rule_edits, fixture.proposal.rule_edits)
let refusals = 0
for (const alter of [
  p => { p.rule_edits = {} },
  p => { p.rule_edits = { patches: [] } },
  p => { p.rule_edits.forwards = [] },
  p => { p.rule_edits.patches[0].retain = true },
  p => { p.rule_edits.patches[0].when = [{ kind: 'execute', value: 'x' }] },
  p => { p.rule_edits.patches = Array(21).fill(p.rule_edits.patches[0]) },
  p => { p.rule_edits.additions = [{ id: 'user.x', category: 'credentials', stage: 'subject', when: [] }] },
]) {
  const invalid = structuredClone(fixture.proposal)
  alter(invalid)
  await assert.rejects(() => ui.readAgentProposal(JSON.stringify(invalid), fixture.summary))
  refusals++
}

const requests = []
let refused = false
globalThis.fetch = async (path, options) => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/agent-edits')
  const body = JSON.parse(options.body)
  requests.push(body)
  if (refused) return Response.json({ error: 'Agent proposal repeats a rejected rule edit' }, { status: 422 })
  return Response.json(body.command.action === 'preview' ? fixture.review : fixture.saved)
}
const reviewed = await ui.previewAgentEdits(fixture.account, fixture.binding, fixture.proposal)
assert.deepEqual(requests[0], { account: fixture.account, summary: fixture.binding, proposal: fixture.proposal, command: { action: 'preview' } })
assert.equal(reviewed.accept_receipt, fixture.review.accept_receipt)
const saved = await ui.confirmAgentEdits(fixture.account, fixture.binding, fixture.proposal, 'accept', reviewed.accept_receipt)
assert.deepEqual(requests[1].command, { action: 'confirm', disposition: 'accept', receipt: fixture.review.accept_receipt, confirmed: true })
assert.equal(saved.status, 'local_rules_saved')
refused = true
await assert.rejects(() => ui.previewAgentEdits(fixture.account, fixture.binding, fixture.proposal))

const labels = {
  en: { original: 'Current override document', resulting: 'Resulting override document', current: 'Current proof', after: 'Resulting proof', accept: 'Save these edits to local rules', reject: 'Reject these exact edits', hold: 'never deploys rules' },
  de: { original: 'Aktuelles Anpassungsdokument', resulting: 'Resultierendes Anpassungsdokument', current: 'Aktuelle Prüfung', after: 'Resultierende Prüfung', accept: 'Diese Änderungen in lokalen Regeln speichern', reject: 'Genau diese Änderungen ablehnen', hold: 'aktiviert keine Regeln' },
}
let renderings = 0
for (const lang of ['en', 'de']) {
  const l = labels[lang]
  for (const confirmed of [false, true]) {
    const { body } = ui.render(confirmed ? ui.Confirmed : ui.Unconfirmed, { props: { account: fixture.account, summary: fixture.binding, proposal: fixture.proposal, lang } })
    for (const text of [l.original, l.resulting, l.current, l.after, l.hold]) assert.ok(body.includes(text), `${lang}: ${text}`)
    assert.ok(body.indexOf(l.original) < body.indexOf(l.resulting) && body.indexOf(l.current) < body.indexOf(l.after))
    assert.ok(body.includes('Trips'))
    assert.ok(body.includes('topic.travel'))
    assert.equal(body.match(/class="side-by-side/gu).length, 2)
    for (const label of [l.accept, l.reject]) {
      const button = body.match(new RegExp(`<button[^>]*>${label}</button>`, 'u'))
      assert.ok(button, label)
      assert.equal(button[0].includes('disabled'), !confirmed)
    }
    assert.ok(!body.includes('<img'))
    await writeFile(`${audit}/review-${lang}-${confirmed}.html`, body)
    renderings++
  }
  const empty = ui.render(ui.Import, { props: { account: fixture.account, summary: fixture.summary, binding: fixture.binding, lang } }).body
  assert.ok(empty.includes('type="file"'))
  assert.ok(!empty.includes(l.accept))
  renderings++
}
await writeFile(`${audit}/result.json`, JSON.stringify({ status: 'passed', refusals, renderings, requests: requests.length, mountedBrowser: 'not_run' }, null, 2) + '\n')
console.log(JSON.stringify({ status: 'passed', refusals, renderings, requests: requests.length }))
