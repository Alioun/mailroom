// Starts the real release binary in --local-vault mode with a dummy home and drives the vault
// through HTTP and, when Chromium is available, the browser forms. It never adds an account,
// so no provider is contacted. Also renders the vault and connection forms in both languages.
// Usage: node ui/scripts/verify-local-web.mjs <path to target/release/mailroom> [results.json]
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const root = fileURLToPath(new URL('../../', import.meta.url))
const binary = process.argv[2]
assert.ok(binary, 'Pass the release binary')
const resultsPath = process.argv[3]
const MASTER = 'invented master password 3c9a'
const WRONG = 'invented wrong password 77e1'
const ORIGIN = 'http://127.0.0.1:4173'
const checks = []
const check = name => { checks.push(name); console.log(`ok ${name}`) }

const home = await mkdtemp(join(tmpdir(), 'mailroom-web-home-'))
await chmod(home, 0o700)
const linux = process.platform === 'linux'
const webConfig = join(home, '.config/mailroom-web')
const nativeConfig = join(home, '.config/mailroom')
await mkdir(join(nativeConfig, 'lifecycle'), { recursive: true, mode: 0o700 })
await writeFile(join(nativeConfig, 'invented-account.json'), '{"invented":"native account metadata"}')
await writeFile(join(nativeConfig, 'lifecycle/invented-account.json'), '{"invented":"native intent"}')
async function digest(directory) {
  const hash = createHash('sha256')
  for (const name of (await readdir(directory, { recursive: true })).sort()) {
    const path = join(directory, name)
    hash.update(name)
    if ((await stat(path)).isFile()) hash.update(await readFile(path))
  }
  return hash.digest('hex')
}
const nativeBefore = await digest(nativeConfig)

let output = ''
function serve() {
  const child = spawn(binary, ['--local-vault'], { env: { HOME: home, PATH: '/usr/bin:/bin' }, stdio: ['ignore', 'pipe', 'pipe'] })
  child.stdout.on('data', bytes => { output += bytes })
  child.stderr.on('data', bytes => { output += bytes })
  const exited = new Promise(resolve => child.on('exit', code => resolve(code)))
  return { child, exited }
}
async function started(server) {
  const until = Date.now() + 20_000
  while (Date.now() < until) {
    if (server.child.exitCode !== null) throw new Error(`server exited: ${output}`)
    try { if ((await fetch(`${ORIGIN}/api/session`)).ok && server.child.exitCode === null) return } catch {}
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error('server did not start')
}
async function stop(server) { server.child.kill('SIGTERM'); await server.exited }
async function session() { return (await (await fetch(`${ORIGIN}/api/session`)).json()).token }
async function api(token, path, body) {
  const response = await fetch(`${ORIGIN}${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'x-mailroom-token': token, 'content-type': 'application/json', origin: ORIGIN },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  for (const secret of [MASTER, WRONG]) assert.ok(!text.includes(secret), `secret in ${path}`)
  return { status: response.status, body: text ? JSON.parse(text) : null }
}

const occupied = await fetch(`${ORIGIN}/api/session`).then(() => true, () => false)
assert.ok(!occupied, 'Port 4173 already serves another process; stop it first')
let browser
let server = serve()
try {
  await started(server)
  const second = serve()
  const code = await Promise.race([second.exited, new Promise(resolve => setTimeout(() => resolve('running'), 15_000))])
  assert.notEqual(code, 'running', 'a second server stops')
  assert.notEqual(code, 0)
  assert.match(output, /status=vault_in_use/)
  check('a second server stops at the vault lease before binding')

  let token = await session()
  const status = await api(token, '/api/accounts/status')
  assert.deepEqual([status.body.source, status.body.vault], ['vault', 'absent'])
  assert.equal((await api(token, '/api/accounts/list', {})).status, 423)
  assert.equal((await fetch(`${ORIGIN}/api/vault/create`, { method: 'POST', headers: { 'content-type': 'application/json', origin: 'https://outside.example', 'x-mailroom-token': token }, body: JSON.stringify({ password: MASTER, confirmation: MASTER }) })).status, 403)
  check('the fresh server reports an absent vault, refuses accounts with 423 and other sites with 403')

  let chromium
  try { ({ chromium } = await import('playwright')); browser = await chromium.launch({ headless: true, timeout: 15_000 }) } catch (error) { browser = undefined; console.log(`browser unavailable: ${error.message.split('\n')[0]}`) }
  if (browser) {
    const page = await browser.newPage()
    await page.goto(ORIGIN)
    await page.getByRole('heading', { name: 'Create your Mailroom password' }).waitFor()
    assert.ok(await page.getByText('If you forget the master password, nobody can open the vault').isVisible())
    await page.getByLabel('Master password', { exact: true }).fill(MASTER)
    await page.getByLabel('Repeat master password').fill(`${MASTER}x`)
    assert.ok(await page.getByText('The two passwords differ.').isVisible())
    assert.ok(await page.getByRole('button', { name: 'Create vault' }).isDisabled())
    await page.getByLabel('Repeat master password').fill(MASTER)
    assert.ok(await page.getByRole('button', { name: 'Create vault' }).isDisabled(), 'needs the loss acknowledgement')
    await page.getByLabel('I understand that a forgotten master password cannot be recovered').check()
    await page.getByRole('button', { name: 'Create vault' }).click()
    await page.getByRole('heading', { name: 'Add an account' }).waitFor()
    assert.equal(await page.getByLabel('App password').getAttribute('autocomplete'), 'new-password')
    assert.equal(await page.evaluate(() => localStorage.length + sessionStorage.length), 0)
    assert.ok(!(await page.content()).includes(MASTER))
    check('the browser creates the vault with a repeated password and a loss warning, and stores nothing')

    await stop(server)
    server = serve()
    await started(server)
    await page.reload()
    await page.getByRole('heading', { name: 'Unlock Mailroom' }).waitFor()
    await page.getByLabel('Master password').fill(WRONG)
    await page.getByRole('button', { name: 'Unlock' }).click()
    await page.getByText('That master password did not open the vault.').waitFor()
    assert.equal(await page.getByLabel('Master password').inputValue(), '')
    await page.getByLabel('Master password').fill(MASTER)
    await page.getByRole('button', { name: 'Unlock' }).click()
    await page.getByRole('heading', { name: 'Add an account' }).waitFor()
    assert.ok(await page.getByText('No saved accounts yet.').isVisible())
    await page.getByText('Deutsch', { exact: true }).click()
    await page.getByRole('heading', { name: 'Konto hinzufügen' }).waitFor()
    assert.ok(!(await page.content()).includes(MASTER) && !(await page.content()).includes(WRONG))
    check('after a restart the browser shows unlock, clears a wrong password, then opens the accounts in both languages')
  } else {
    let response = await api(token, '/api/vault/create', { password: MASTER, confirmation: MASTER })
    assert.equal(response.body.vault, 'unlocked')
    await stop(server)
    server = serve()
    await started(server)
    token = await session()
    assert.equal((await api(token, '/api/accounts/status')).body.vault, 'locked')
    response = await api(token, '/api/vault/unlock', { password: WRONG })
    assert.deepEqual([response.status, response.body.code], [422, 'unlock_failed'])
    response = await api(token, '/api/vault/unlock', { password: MASTER })
    assert.equal(response.body.vault, 'unlocked')
    check('without a browser: create, restart, wrong and right unlock over HTTP')
  }
  token = await session()
  assert.deepEqual((await api(token, '/api/accounts/list', {})).body, [])
  const vault = await stat(join(webConfig, 'vault.mrv'))
  assert.equal(vault.mode & 0o777, 0o600)
  assert.equal((await stat(webConfig)).mode & 0o777, 0o700)
  const dataRoot = linux ? join(home, '.local/share/mailroom-web') : join(home, 'Library/Application Support/mailroom-web')
  assert.equal((await stat(dataRoot)).mode & 0o777, 0o700)
  assert.equal(await digest(nativeConfig), nativeBefore)
  const sealed = await readFile(join(webConfig, 'vault.mrv'))
  assert.equal(sealed.subarray(0, 10).toString('latin1'), 'MRVAULT1\x00\x01')
  check('the vault lives in its own private roots and the native account root is unchanged')
} finally {
  await browser?.close()
  if (server.child.exitCode === null) await stop(server)
}
for (const secret of [MASTER, WRONG]) assert.ok(!output.includes(secret), 'secret in server output')
check('no master password reached server output')

const audit = `${root}.audit/local-web/`
await mkdir(`${audit}render`, { recursive: true })
await writeFile(`${audit}render/entry.mjs`, `export { default as Vault } from '${root}ui/src/Vault.svelte'; export { default as ConnectAccount } from '${root}ui/src/ConnectAccount.svelte'; export { render } from 'svelte/server'`)
await build({
  root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true },
  build: { ssr: `${audit}render/entry.mjs`, outDir: `${audit}ssr`, emptyOutDir: true, minify: false, rollupOptions: { output: { entryFileNames: 'forms.mjs' } } },
})
const forms = await import(pathToFileURL(`${audit}ssr/forms.mjs`))
const base = { synthetic: false, source: 'vault', vault: 'unlocked', vault_problem: null, interrupted: [], job: null }
const render = (component, props) => forms.render(component, { props: { onOpened() {}, onStatus() {}, ...props } }).body
for (const [lang, words] of Object.entries({
  en: { create: 'Create vault', repeat: 'Repeat master password', unlock: 'Unlock', damaged: 'The vault file is damaged', unsafe: 'unsafe permissions or links', auth: 'The provider refused the sign-in', correct: 'Save and reconnect', retry: 'Retry connection', clean: 'Clean up interrupted setup', readOnly: 'It never changes, moves or sends mail.' },
  de: { create: 'Tresor anlegen', repeat: 'Hauptpasswort wiederholen', unlock: 'Entsperren', damaged: 'Die Tresordatei ist beschädigt', unsafe: 'unsichere Berechtigungen', auth: 'Der Anbieter hat die Anmeldung abgelehnt', correct: 'Speichern und neu verbinden', retry: 'Verbindung erneut versuchen', clean: 'Unterbrochene Einrichtung aufräumen', readOnly: 'Es ändert, verschiebt oder sendet nie Nachrichten.' },
})) {
  const absent = render(forms.Vault, { lang, status: { ...base, vault: 'absent' } })
  assert.ok(absent.includes(words.create) && absent.includes(words.repeat) && absent.includes('autocomplete="new-password"'))
  assert.ok(render(forms.Vault, { lang, status: { ...base, vault: 'locked' } }).includes('autocomplete="current-password"'))
  assert.ok(render(forms.Vault, { lang, status: { ...base, vault: 'unavailable', vault_problem: 'damaged' } }).includes(words.damaged))
  assert.ok(render(forms.Vault, { lang, status: { ...base, vault: 'unavailable', vault_problem: 'unsafe_file' } }).includes(words.unsafe))
  const account = { id: 'invented-account', provider: 'gmail', login: 'owner@example.test', connection: 'failed' }
  const failed = render(forms.ConnectAccount, { lang, disabled: false, account, status: { ...base, interrupted: ['invented-other'], job: { account, progress: { state: 'failed', saved: 0, folders: 0 }, error: 'authentication' } } })
  for (const text of [words.auth, words.correct, words.retry, words.clean, words.readOnly]) assert.ok(failed.includes(text), `${lang}: ${text}`)
  const observed = render(forms.ConnectAccount, { lang, disabled: false, account: { ...account, connection: 'observed' }, status: base })
  assert.ok(!observed.includes(words.correct))
}
check('the vault and connection forms render every state in English and German')

await rm(home, { recursive: true, force: true })
if (resultsPath) await writeFile(resultsPath, JSON.stringify({ binary, browser: Boolean(browser), checks }, null, 2) + '\n')
console.log(`PASS: ${checks.length} local web checks${browser ? '' : ' (no browser)'}`)
