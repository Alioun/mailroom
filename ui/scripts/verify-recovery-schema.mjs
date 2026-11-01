import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/agent-rejections'}/`
const baseline = process.argv.includes('--baseline')
const name = baseline ? 'baseline' : 'current'
const output = `${audit}recovery-schema/${name}/`
await mkdir(output, { recursive: true })
const original = execFileSync('/usr/bin/git', ['show', '66eeeb5b21c1bd57dfbfac2dd8f7e1a60683b461:ui/src/recovery-api.ts'], { cwd: root, encoding: 'utf8' })
const source = baseline ? original : await readFile(`${root}ui/src/recovery-api.ts`, 'utf8')
await writeFile(`${output}recovery-api.ts`, source.replace("'zod'", JSON.stringify(import.meta.resolve('zod'))).replace("from './api'", `from '${root}ui/src/api'`))
await writeFile(`${output}entry.mjs`, "export { previewCapture } from './recovery-api.ts'")
await build({ root: `${root}ui`, configFile: false, logLevel: 'error', ssr: { noExternal: true }, build: { ssr: `${output}entry.mjs`, outDir: `${output}built`, emptyOutDir: false, minify: false, rollupOptions: { output: { entryFileNames: 'api.mjs' } } } })
const api = await import(pathToFileURL(`${output}built/api.mjs`))
const actual = JSON.parse(await readFile(`${audit}workspace/recovery-offer.json`, 'utf8'))
const supported = actual.preview.accounts[0].state_schema
assert.ok(supported >= 33)
let response = actual
let requests = 0
globalThis.fetch = async path => {
  if (path === '/api/session') return Response.json({ token: '0'.repeat(64) })
  assert.equal(path, '/api/accounts/recovery')
  requests++
  return Response.json(response)
}
const cases = []
for (const version of [...new Set([1, 31, 32, supported, supported + 1])]) {
  response = structuredClone(actual)
  for (const account of response.preview.accounts) account.state_schema = version
  let accepted = false
  try { await api.previewCapture([actual.preview.accounts[0].account], false); accepted = true } catch {}
  const expected = baseline ? version <= 32 : version <= supported
  assert.equal(accepted, expected, `Schema ${version} must ${expected ? 'pass' : 'be refused'}`)
  cases.push({ version, accepted })
}
await writeFile(`${output}results.json`, JSON.stringify({ backendSchema: supported, requests, cases, source: baseline ? 'Exact preceding recovery client.' : 'Current client and actual protected Router capture preview.', limits: 'Dummy preview only. No capture, restore, credential, mounted or provider acceptance.' }, null, 2) + '\n')
console.log(`${name}: checked ${cases.length} schema contracts against backend schema ${supported}.`)
