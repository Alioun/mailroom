import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/generated-rule-proof'}/`
await mkdir(`${audit}render/source`, { recursive: true })
const log = await readFile(`${audit}proof.log`, 'utf8')
const reports = ['MAILROOM_GENERATED_RULE_PROOF ', 'MAILROOM_SHADOWED_GENERATED_RULE_PROOF ', 'MAILROOM_OMITTED_GENERATED_RULE_PROOF '].map(marker => {
  const line = log.split('\n').find(line => line.startsWith(marker))
  assert.ok(line, `The actual proof test must emit ${marker}`)
  return JSON.parse(line.slice(marker.length))
})
for (const [index, proof] of reports.entries()) {
  let source = await readFile(`${root}ui/src/RulesProof.svelte`, 'utf8')
  const initial = "let proofState = $state<State>({ kind: 'initial' })"
  assert.equal(source.split(initial).length, 2)
  source = source.replace(initial, () => `let proofState = $state<State>(${JSON.stringify({ kind: 'done', proof }).replaceAll('<', '\\u003c')})`)
  source = source.replace("'@ark-ui/svelte/toggle-group'", JSON.stringify(import.meta.resolve('@ark-ui/svelte/toggle-group')))
  source = source.replaceAll("from './", `from '${root}ui/src/`)
  await writeFile(`${audit}render/source/Proof${index}.svelte`, source)
}
await writeFile(`${audit}render/source/entry.mjs`, `${reports.map((_, i) => `export { default as Proof${i} } from './Proof${i}.svelte'`).join('; ')}; export { evaluate } from '${root}ui/src/api.ts'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}render/source/entry.mjs`, outDir: `${audit}render/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'proofs.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}render/built/proofs.mjs`))
const input = JSON.parse(await readFile(`${root}examples/sample.json`, 'utf8'))
let reply
let calls = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/preview')
  calls++
  return Response.json(reply)
}
for (const proof of reports) {
  reply = proof
  const parsed = await ui.evaluate(input)
  assert.deepEqual(parsed.generated_rules, proof.generated_rules)
  assert.deepEqual(parsed.rules, proof.rules)
  assert.deepEqual(parsed.decisions, proof.decisions)
}
let rejections = 0
for (const alter of [
  value => { delete value.generated_rules },
  value => { value.generated_rules = null },
  value => { value.generated_rules = {} },
  value => { value.generated_rules[0].matched = '2' },
  value => { value.generated_rules[0].monthly = { '2026-10': 'one' } },
  value => { value.generated_rules[0].fully_shadowed = null },
]) {
  reply = structuredClone(reports[0])
  alter(reply)
  await assert.rejects(() => ui.evaluate(input))
  rejections++
}
const renderings = []
for (const [index, proof] of reports.entries()) {
  for (const lang of ['en', 'de']) {
    const { body } = ui.render(ui[`Proof${index}`], { props: { lang } })
    assert.ok(body.includes(lang === 'en' ? 'Generated filing rules' : 'Automatisch erzeugte Ablageregeln'))
    assert.ok(body.includes(lang === 'en' ? 'twelve preceding months' : 'zwölf vorherige Monate'))
    assert.ok(body.includes(lang === 'en' ? 'clear recipient safety' : 'geklärter Empfängersicherheit'))
    assert.deepEqual([...body.matchAll(/data-generated-rule="([^"]+)"/g)].map(match => match[1]), proof.generated_rules.map(row => row.id))
    for (const row of proof.generated_rules) {
      assert.ok(body.includes(`${row.matched} / ${row.candidates}`))
      assert.equal(Object.keys(row.monthly).length, 13)
      assert.equal(Object.values(row.monthly).reduce((sum, count) => sum + count, 0), row.matched)
    }
    if (index === 1) assert.ok(body.includes(lang === 'en' ? 'fully shadowed' : 'vollständig verdeckt'))
    if (index === 2) assert.ok(body.includes(lang === 'en' ? 'an absent row is not a zero-match result' : 'Eine fehlende Zeile bedeutet nicht'))
    const name = `${['generated', 'shadowed', 'omitted'][index]}-${lang}`
    await writeFile(`${audit}render/${name}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><body>${body}</body></html>`)
    renderings.push({ name, sha256: createHash('sha256').update(body).digest('hex') })
  }
}
await writeFile(`${audit}generated-ui-results.json`, JSON.stringify({ calls, rejections, renderings, limits: 'Actual computed proofs, schema contracts and server rendering. No mounted, visual, TLS, native or provider acceptance.' }, null, 2) + '\n')
console.log(`${calls} API contracts, ${rejections} malformed generated-row rejections, ${renderings.length} bilingual proof renderings.`)
