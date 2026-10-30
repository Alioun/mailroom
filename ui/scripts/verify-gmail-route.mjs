import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
assert.ok(process.argv[2], 'Pass a verification output directory')
const audit = resolve(process.argv[2])
await mkdir(`${audit}/tools`, { recursive: true })
await mkdir(`${audit}/home`, { recursive: true })
const commandLog = `${audit}/unexpected-commands.log`
await writeFile(commandLog, '')
for (const command of ['open', 'osascript', 'op', 'gh', 'security', 'curl', 'cargo', 'npm']) {
  await writeFile(`${audit}/tools/${command}`, `#!/bin/bash\nprintf '%s\\n' '${command}' >> "$DUMMY_COMMAND_LOG"\nexit 99\n`, { mode: 0o700 })
}
const env = { PATH: `${audit}/tools:/usr/bin:/bin`, HOME: `${audit}/home`, DUMMY_COMMAND_LOG: commandLog }
const script = `${root}scripts/prepare-gmail-route.sh`
const syntax = spawnSync('/bin/bash', ['-n', script], { env, encoding: 'utf8', timeout: 10_000 })
assert.equal(syntax.status, 0)
const plan = JSON.parse(await readFile(`${root}resources/gmail-route-plan.json`, 'utf8'))
assert.equal(plan.version, 1)
assert.equal(plan.kind, 'preparation_only')
assert.deepEqual(plan.stages.map(stage => stage.id), ['destination', 'verification', 'forwarding', 'filing', 'observation', 'stop'])
assert.ok(plan.stages[2].en.body.includes('Do not deploy Gmail filters'))
assert.ok(plan.stages[3].en.body.includes('before existing filing rules'))
assert.ok(plan.stages[3].en.body.includes('Unknown or mixed roles keep Inbox and read status unchanged'))
assert.ok(plan.stages[3].en.body.includes('verified ingress tagging can still apply'))
assert.ok(plan.stages[5].en.body.includes('Undo remains a separate confirmed action'))
for (const args of [[], ['--plan']]) {
  const result = spawnSync('/bin/bash', [script, ...args], { env, cwd: `${audit}/home`, input: 'DUMMY_UNTRUSTED_INPUT', encoding: 'utf8', timeout: 10_000 })
  assert.equal(result.status, 0)
  assert.equal(result.stderr, '')
  assert.ok(!result.stdout.includes('DUMMY_UNTRUSTED_INPUT'))
  assert.ok(!result.stdout.includes('Setup complete'))
  assert.ok(result.stdout.includes('status=preparation_only stages=6 provider_changes=0 evidence=unverified'))
  for (const stage of plan.stages) {
    assert.ok(result.stdout.includes(stage.en.title))
    assert.ok(result.stdout.includes(stage.en.body), 'Wizard and UI must describe identical planned stages')
  }
  await writeFile(`${audit}/plan-${args.length}.log`, result.stdout)
}
for (const args of [['--run'], ['--approve'], ['DUMMY_UNTRUSTED_ARGUMENT'], ['--plan', '--run']]) {
  const result = spawnSync('/bin/bash', [script, ...args], { env, encoding: 'utf8', timeout: 10_000 })
  assert.equal(result.status, 64)
  assert.equal(result.stdout, '')
  assert.ok(result.stderr.startsWith('status=refused'))
  assert.ok(!result.stderr.includes('DUMMY_UNTRUSTED_ARGUMENT'))
}
assert.equal(await readFile(commandLog, 'utf8'), '')
assert.deepEqual(await readdir(`${audit}/home`), [])

await writeFile(`${audit}/entry.mjs`, `export { default as GmailRoute } from '${root}ui/src/GmailRoute.svelte'; export { gmailRoutePlan, gmailRouteText } from '${root}ui/src/gmail-route.ts'; export { render } from 'svelte/server'`)
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true }, build: { ssr: `${audit}/entry.mjs`, outDir: `${audit}/built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'route.mjs' } } } })
const ui = await import(pathToFileURL(`${audit}/built/route.mjs`))
assert.deepEqual(ui.gmailRoutePlan, plan)
let networkRequests = 0
globalThis.fetch = () => { networkRequests++; throw new Error('Route preparation must not contact a server') }
for (const lang of ['en', 'de']) {
  const text = ui.gmailRouteText(lang)
  const { body } = ui.render(ui.GmailRoute, { props: { lang } })
  assert.ok(body.includes(plan[lang].title))
  assert.ok(body.includes(plan[lang].boundary))
  assert.ok(body.includes(plan[lang].next))
  assert.equal([...body.matchAll(/<li(?:\s[^>]*)?>/gu)].length, 8)
  assert.ok(!/<(?:input|form|button)\b/u.test(body), 'This guide cannot claim account setup or request private values')
  assert.ok(body.includes('download="gmail-route-preparation.txt"'))
  const encoded = body.match(/href="data:text\/plain;charset=utf-8,([^"]+)"/u)
  assert.ok(encoded)
  assert.equal(decodeURIComponent(encoded[1]), text)
  assert.ok(text.endsWith('status=preparation_only provider_changes=0 evidence=unverified\n'))
  assert.ok(text.includes('$via-gmail'))
  assert.ok(text.includes('Spam'))
  assert.ok(text.includes('Inbox'))
  assert.ok(text.includes('Delivered-To'))
  for (const [index, stage] of plan.stages.entries()) {
    assert.ok(text.includes(`${index + 1}. ${stage[lang].title}\n${stage[lang].body}`))
  }
  await writeFile(`${audit}/route-${lang}.html`, body)
  await writeFile(`${audit}/route-${lang}.txt`, text)
}
assert.equal(networkRequests, 0)
await writeFile(`${audit}/result.json`, JSON.stringify({ status: 'passed', preparationInvocations: 2, refusedModes: 4, renderings: 2, exactDownloads: 2, networkRequests, providerChanges: 0, mountedBrowser: 'not_run' }, null, 2) + '\n')
console.log('status=passed preparation=2 refusals=4 renderings=2 exact_downloads=2 network_requests=0')
