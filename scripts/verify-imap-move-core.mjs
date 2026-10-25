import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { mkdir, open, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url))
const audit = join(root, '.audit', 'imap-move-core')
await mkdir(audit, { recursive: true })
const commands = []
const listener = spawnSync('lsof', ['-nP', '-iTCP:4173', '-sTCP:LISTEN'], { cwd: root, encoding: 'utf8' })
assert.equal(listener.status, 1, 'Identity-stop the retained fixture before rebuilding its executable')
assert.equal(listener.stdout, '')

for (const filter of ['backlog::imap_move', 'transport::message_move']) {
  const args = ['test', '--locked', '--lib', filter]
  const log = join(audit, `${filter.replaceAll('::', '-')}-${Date.now()}.log`)
  const output = await open(log, 'wx')
  const result = await new Promise((resolve, reject) => {
    const child = spawn('cargo', args, {
      cwd: root,
      env: { ...process.env, CARGO_PROFILE_TEST_OPT_LEVEL: '1', MAILROOM_MOVE_TEST_EVIDENCE: join(audit, 'fixtures') },
      stdio: ['ignore', output.fd, output.fd],
    })
    child.once('error', reject)
    child.once('exit', (code, signal) => resolve({ code, signal }))
  }).finally(() => output.close())
  commands.push({ command: ['cargo', ...args], ...result, log })
  await writeFile(join(audit, 'results.json'), JSON.stringify({
    scope: 'Direct domain and TLS transport checks. Saved-workspace HTTP and built UI run in separate verification scripts.',
    end_to_end_acceptance: 'not_assessed_by_this_check',
    commands,
  }, null, 2) + '\n')
  assert.equal(result.code, 0, `Core check failed. See ${log}`)
}

console.log('PASS direct domain and TLS transport checks. Saved-workspace acceptance is checked separately.')
