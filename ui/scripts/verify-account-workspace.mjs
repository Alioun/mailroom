import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import ts from 'typescript'

const root = fileURLToPath(new URL('../../', import.meta.url))
const audit = `${root}${process.env.MAILROOM_VERIFY_AUDIT ?? '.audit/account-workspace'}/`
await mkdir(`${audit}render`, { recursive: true })
const builds = (await readFile(process.argv[2] ?? `${audit}test-build.jsonl`, 'utf8')).trim().split('\n').map(JSON.parse)
assert.deepEqual(builds.at(-1), { reason: 'build-finished', success: true })
const built = builds.findLast(item => item.reason === 'compiler-artifact' && item.target.name === 'ui_bridge' && item.profile.test && item.executable)
assert.ok(built)
const artifact = { path: built.executable, sha256: createHash('sha256').update(await readFile(built.executable)).digest('hex') }
function imports(code) {
  return code.replace(/(['"])(zod|svelte(?:\/[^'"]+)?|@ark-ui\/svelte\/[^'"]+)\1/g, (_match, _quote, name) => JSON.stringify(import.meta.resolve(name)))
    .replace(/from (['"])(\.\/[^'"]+)\1/g, (_match, _quote, name) => `from '${name.endsWith('.svelte') ? name.slice(0, -7) : name}.mjs'`)
}
for (const name of await readdir(`${root}ui/src`)) {
  if (!name.endsWith('.ts') || name === 'main.ts') continue
  const source = await readFile(`${root}ui/src/${name}`, 'utf8')
  const result = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext }, fileName: name })
  await writeFile(`${audit}render/${name.slice(0, -3)}.mjs`, imports(result.outputText))
}

const api = await import(pathToFileURL(`${audit}render/api.mjs`))
const { accountWorkspace } = await import(pathToFileURL(`${audit}render/account-workspace.mjs`))
const bridge = spawn(artifact.path, ['--exact', 'synthetic_ui_bridge', '--ignored', '--nocapture'], {
  cwd: root, env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin' }, stdio: ['pipe', 'pipe', 'pipe'],
})
let sequence = 0
let failed = false
let diagnosticBytes = 0
const pending = new Map()
function rejectPending() {
  failed = true
  for (const item of pending.values()) item.reject(new Error('Synthetic account bridge unavailable'))
  pending.clear()
}
bridge.on('error', rejectPending)
const closed = new Promise(resolve => bridge.on('close', code => { rejectPending(); resolve(code) }))
bridge.stderr.on('data', bytes => { diagnosticBytes += bytes.length })
createInterface({ input: bridge.stdout }).on('line', line => {
  const marker = 'MAILROOM_UI_RESPONSE '
  const offset = line.indexOf(marker)
  if (offset < 0) return
  const record = JSON.parse(line.slice(offset + marker.length))
  const item = pending.get(record.id)
  assert.ok(item)
  pending.delete(record.id)
  item.resolve(new Response(Buffer.from(record.body, 'base64'), { status: record.status, headers: record.headers }))
})
async function request(path, options = {}) {
  if (failed) throw new Error('Synthetic account bridge unavailable')
  const id = ++sequence
  const result = new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
  bridge.stdin.write(`${JSON.stringify({ id, method: options.method ?? 'GET', path, headers: { host: '127.0.0.1:4173', ...Object.fromEntries(new Headers(options.headers).entries()) }, body: options.body ?? '' })}\n`)
  return result
}
let override
let intercepted = 0
globalThis.fetch = async (path, options) => {
  if (path === '/api/accounts/open' && override) { intercepted++; return override(path, options) }
  return request(path, options)
}
const deadline = setTimeout(() => { rejectPending(); bridge.stdin.end() }, 45_000)
const checks = []
const renderings = []
try {
  const session = await (await request('/api/session')).json()
  const headers = { 'content-type': 'application/json', 'x-mailroom-token': session.token }
  async function post(path, body) {
    const response = await request(`/api/accounts/${path}`, { method: 'POST', headers, body: JSON.stringify(body) })
    assert.equal(response.status, 200)
    assert.equal(response.headers.get('cache-control'), 'no-store')
    return response.json()
  }
  async function finish() {
    const until = Date.now() + 5000
    while (Date.now() < until) {
      const status = await api.readAccountStatus()
      if (status.job.progress.state !== 'running') {
        assert.equal(status.job.progress.state, 'complete')
        return status.job.account
      }
      await new Promise(resolve => setTimeout(resolve, 10))
    }
    throw new Error('Synthetic census exceeded its deadline')
  }
  async function create(provider) {
    await post(provider, {})
    const account = await finish()
    await post('scan', { account: account.id })
    await finish()
    return account
  }
  const first = await create('synthetic')
  const second = await create('synthetic-gmail')
  const accounts = await api.listAccounts()
  const firstView = await api.openAccount(first.id)
  const secondView = await api.openAccount(second.id)
  assert.deepEqual(firstView.landing, { kind: 'unavailable', reason: 'no_recorded_deployment' })
  await writeFile(`${audit}first-view.json`, JSON.stringify(firstView) + '\n')
  if (process.argv.includes('--recovery-preview')) {
    const recovery = await import(pathToFileURL(`${audit}render/recovery-api.mjs`))
    const offer = await recovery.previewCapture([first.id], false)
    await writeFile(`${audit}recovery-offer.json`, JSON.stringify(offer) + '\n')
  }
  const status = await api.readAccountStatus()
  const states = []
  const loader = accountWorkspace(state => states.push(state))
  const latest = () => states.at(-1)
  await loader.load(first.id)
  assert.equal(latest().kind, 'ready')
  assert.equal(latest().view.census.account, first.id)
  checks.push('Saved census opens through the real API and router')

  async function delay(account, fail = false) {
    let release
    let seen
    const gate = new Promise(resolve => { release = resolve })
    const started = new Promise(resolve => { seen = resolve })
    override = async (path, options) => {
      const response = await request(path, options)
      if (JSON.parse(options.body).account === account) { seen(); await gate; if (fail) throw new Error('private-response-sentinel') }
      return response
    }
    return { release, started }
  }
  let gate = await delay(first.id)
  let old = loader.load(first.id)
  await gate.started
  await loader.load(second.id)
  gate.release(); await old
  assert.equal(latest().view.census.account, second.id)
  checks.push('Delayed previous-account success cannot replace the selection')

  gate = await delay(first.id, true)
  old = loader.load(first.id)
  await gate.started
  await loader.load(second.id)
  gate.release(); await old
  assert.equal(latest().kind, 'ready')
  assert.equal(latest().view.census.account, second.id)
  checks.push('Delayed previous-account failure cannot replace the selection')

  gate = await delay(first.id)
  old = loader.load(first.id)
  await gate.started
  loader.clear()
  gate.release(); await old
  assert.equal(latest().kind, 'empty')
  checks.push('Clearing the workspace prevents old content from returning')
  const beforeEmpty = sequence
  await loader.load('')
  assert.equal(sequence, beforeEmpty)
  assert.equal(latest().kind, 'empty')
  checks.push('An empty selection clears content without a request')

  override = async () => { throw new Error('private-response-sentinel') }
  await loader.load(first.id)
  assert.deepEqual(latest(), { kind: 'error', account: first.id })
  assert.ok(!JSON.stringify(states).includes('private-response-sentinel'))
  override = undefined
  await loader.load(first.id)
  assert.equal(latest().view.census.account, first.id)
  checks.push('A failed request has a fixed error state and explicit retry succeeds')

  override = async () => Response.json(secondView)
  await loader.load(first.id)
  assert.deepEqual(latest(), { kind: 'error', account: first.id })
  override = undefined
  checks.push('A valid response for a different account is refused')

  const address = firstView.addresses[0].address
  await loader.load(first.id, id => api.confirmAccount(id, address, true))
  assert.equal(latest().view.addresses.find(item => item.address === address).confirmed, true)
  checks.push('Ownership update reloads the same exact account without assigning a role')

  gate = await delay(first.id)
  old = loader.load(first.id)
  await gate.started
  loader.dispose()
  const count = states.length
  const requestsBeforeDispose = intercepted
  gate.release(); await old
  await loader.load(second.id)
  assert.equal(states.length, count)
  assert.equal(intercepted, requestsBeforeDispose)
  override = undefined
  checks.push('A disposed workspace publishes nothing and starts no requests')

  await writeFile(`${audit}workspace-results.json`, JSON.stringify({ artifact, routerRequests: sequence, checks }, null, 2) + '\n')
  let component = await readFile(`${root}ui/src/Accounts.svelte`, 'utf8')
  const declarations = [
    ['let status = $state<AccountStatus | null>(null)', 'status'],
    ['let accounts = $state<Account[]>([])', 'accounts'],
    ["let selected = $state('')", 'selected'],
    ["let savedAccount = $state<SavedAccountState>({ kind: 'empty' })", 'savedAccount'],
    ["let accountLoad = $state<AccountLoad>({ kind: 'loading', revision: 0 })", 'accountLoad'],
  ]
  const props = "let { lang }: { lang: 'en' | 'de' } = $props()"
  assert.equal(component.split(props).length, 2)
  component = component.replace(props, "let { lang, fixture }: { lang: 'en' | 'de'; fixture: { status: AccountStatus | null; accounts: Account[]; selected: string; savedAccount: SavedAccountState; accountLoad: AccountLoad } } = $props()")
  for (const [declaration, key] of declarations) {
    assert.equal(component.split(declaration).length, 2)
    component = component.replace(declaration, `${declaration.slice(0, declaration.indexOf('(') + 1)}fixture.${key})`)
  }
  component = component.replace(/from '(.\/[^']+)'/g, (_match, name) => `from '${root}ui/src/${name.slice(2)}'`)
  await writeFile(`${audit}render/Accounts.svelte`, component)
  await writeFile(`${audit}render/entry.mjs`, `import '${root}ui/src/style.css'; export { default as Accounts } from './Accounts.svelte'; export { render } from 'svelte/server'`)
  const bundled = await build({
    root: `${root}ui`, configFile: false, logLevel: 'error', plugins: [svelte({ configFile: false })], ssr: { noExternal: true },
    build: { ssr: `${audit}render/entry.mjs`, outDir: `${audit}ssr`, emptyOutDir: false, ssrEmitAssets: true, cssCodeSplit: false, minify: false, rollupOptions: { output: { entryFileNames: 'workspace.mjs' } } },
  })
  const assets = (Array.isArray(bundled) ? bundled : [bundled]).flatMap(item => item.output).filter(item => item.type === 'asset' && item.fileName.endsWith('.css'))
  const styles = assets.map(item => item.source.toString()).join('\n')
  assert.ok(styles.includes('.account-tasks'), 'Use the CSS emitted from the actual account workspace source')
  const rendered = await import(pathToFileURL(`${audit}ssr/workspace.mjs`))
  async function screen(name, lang, state) {
    const { body } = rendered.render(rendered.Accounts, { props: { lang, fixture: state } })
    for (const sentinel of ['synthetic-private-token', 'synthetic-mail-token', 'synthetic-relay-token', 'private-response-sentinel']) assert.ok(!body.includes(sentinel))
    const ids = [...body.matchAll(/\sid="([^"]+)"/g)].map(item => item[1])
    assert.equal(new Set(ids).size, ids.length, 'Rendered anchor identities are unique')
    for (const [, target] of body.matchAll(/href="#(account-[^"]+)"/g)) assert.ok(ids.includes(target), 'Every account task link has a rendered target')
    await writeFile(`${audit}${name}-${lang}.html`, `<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Mailroom dummy account workspace</title><style>${styles}</style><body><main class="app-shell">${body}</main></body></html>`)
    renderings.push({ name, lang, sha256: createHash('sha256').update(body).digest('hex') })
    return body
  }
  const base = { status, accounts, selected: first.id, savedAccount: { kind: 'ready', account: first.id, view: firstView }, accountLoad: { kind: 'ready' } }
  for (const lang of ['en', 'de']) {
    const ready = await screen('ready', lang, base)
    assert.ok(ready.includes(lang === 'en' ? 'Account tasks' : 'Kontoaufgaben'))
    const destinations = ['overview', 'rules', 'backlog', 'invoices', 'subscriptions', 'winddown', 'setup'].map(name => ready.indexOf(`id="account-${name}"`))
    assert.ok(destinations.every(index => index >= 0))
    assert.deepEqual(destinations, [...destinations].sort((a, b) => a - b))
    assert.ok(ready.includes(second.login), 'Activity retains the actual job account identity')
    const loading = await screen('opening', lang, { ...base, savedAccount: { kind: 'loading', account: first.id } })
    assert.ok(loading.includes(lang === 'en' ? 'Opening the saved census' : 'Gespeicherte Erfassung wird geöffnet'))
    assert.ok(!loading.includes('href="#account-rules"'))
    const error = await screen('error', lang, { ...base, savedAccount: { kind: 'error', account: first.id } })
    assert.ok(error.includes('role="alert"'))
    assert.ok(error.includes(lang === 'en' ? 'Retry opening census' : 'Erfassung erneut öffnen'))
    const initial = await screen('initial', lang, { status: null, accounts: [], selected: '', savedAccount: { kind: 'empty' }, accountLoad: { kind: 'loading', revision: 0 } })
    assert.ok(initial.includes(lang === 'en' ? 'Loading saved accounts' : 'Gespeicherte Konten werden geladen'))
    const listError = await screen('list-error', lang, { ...base, savedAccount: { kind: 'empty' }, accountLoad: { kind: 'error' } })
    assert.ok(listError.includes(lang === 'en' ? 'Retry loading accounts' : 'Konten erneut laden'))
    const waiting = structuredClone(base)
    waiting.status.job.progress.state = 'running'
    waiting.savedAccount = { kind: 'empty' }
    const pending = await screen('waiting', lang, waiting)
    assert.ok(pending.includes(lang === 'en' ? 'The saved workspace will open when the account activity ends.' : 'Der gespeicherte Arbeitsbereich öffnet sich, sobald die Kontoaktion endet.'))
    assert.ok(!pending.includes('href="#account-rules"'))
    const empty = await screen('empty', lang, { status: { synthetic: false, job: null }, accounts: [], selected: '', savedAccount: { kind: 'empty' }, accountLoad: { kind: 'ready' } })
    assert.ok(empty.includes(lang === 'en' ? 'No saved accounts yet.' : 'Noch keine gespeicherten Konten.'))
    assert.ok(empty.includes(lang === 'en' ? 'approved protected setup' : 'freigegebene geschützte Einrichtung'))
    assert.ok(!empty.includes(lang === 'en' ? 'Add synthetic account' : 'Synthetisches Konto hinzufügen'))
    const stale = await screen('mismatched', lang, { ...base, savedAccount: { kind: 'ready', account: second.id, view: secondView } })
    assert.ok(!stale.includes('id="account-overview"'))
    const long = structuredClone(base)
    long.accounts[0].login = `${'long-address-'.repeat(12)}@example.test`
    long.savedAccount.view.census.acquisition_state = 'cancelled'
    assert.ok((await screen('long-partial', lang, long)).includes(lang === 'en' ? 'Account census cancelled' : 'Kontoerfassung abgebrochen'))
  }
  assert.equal(createHash('sha256').update(await readFile(artifact.path)).digest('hex'), artifact.sha256)
  await writeFile(`${audit}results.json`, JSON.stringify({ artifact, routerRequests: sequence, checks, renderings, diagnosticBytes, limitations: ['No mounted browser, viewport layout, focus traversal or native/provider acceptance. Automatic component selection effects require browser acceptance.'] }, null, 2) + '\n')
  console.log(`PASS: ${checks.length} real API workspace checks and ${renderings.length} bilingual server renderings`)
} finally {
  clearTimeout(deadline)
  bridge.stdin.end()
  assert.equal(await closed, 0, 'The private-pipe fixture exited cleanly')
}
