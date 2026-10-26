import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import vm from 'node:vm'
import { parse } from 'svelte/compiler'
import ts from 'typescript'

const require = createRequire(import.meta.url)
const source = await readFile(new URL('../src/FolderProvisioning.svelte', import.meta.url), 'utf8')
const { instance } = parse(source, { modern: true })
assert.ok(instance)
const script = ts.transpileModule(source.slice(instance.content.start, instance.content.end), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText

function component(states, { rejectStatus = false } = {}) {
  const requests = []
  const timers = new Map()
  const pending = []
  let destroy
  let timerId = 0
  const folderApi = {
    folderCommand: async (account, command) => { requests.push(command.action); return { started: true } },
    folderStatus: async () => { requests.push('folder-status'); return { journal: { history: [] } } },
  }
  const context = vm.createContext({
    exports: {},
    require: name => {
      if (name === 'svelte') return { onDestroy: fn => { destroy = fn } }
      if (name === './folder-provisioning-api') return folderApi
      if (name === './api') return { localFetch: async path => {
        assert.equal(path, '/api/accounts/status')
        requests.push('account-status')
        if (rejectStatus) throw Error('synthetic unavailable status')
        assert.ok(states.length, 'The component requested an unexpected job state')
        const state = states.shift()
        return { json: async () => ({ job: state === null ? null : { progress: { state } } }) }
      } }
      if (name === 'zod') return require('zod')
      throw Error('Unexpected component dependency')
    },
    $props: () => ({ account: 'synthetic-account', lang: 'en' }),
    $state: value => value,
    $derived: value => value,
    setTimeout: (fn, delay) => { assert.equal(delay, 200); timers.set(++timerId, fn); return timerId },
    clearTimeout: id => timers.delete(id),
    pending,
  })
  vm.runInContext(script + `
    const originalPoll = poll;
    poll = () => { const result = originalPoll(); pending.push(result); return result; };
    globalThis.subject = {
      confirm: () => { preview = { receipt: 'synthetic-receipt' }; approved = true; return act('confirm'); },
      state: () => ({ running, error, view }),
    };
  `, context)
  return {
    subject: context.subject,
    requests,
    async settle() { while (pending.length) await pending.shift() },
    async tick() {
      assert.equal(timers.size, 1)
      const [id, fn] = timers.entries().next().value
      timers.delete(id)
      fn()
      await this.settle()
    },
    destroy: () => destroy(),
    timers,
  }
}

for (const terminal of ['complete', 'failed', 'cancelled', null]) {
  const ui = component(['running', terminal])
  await ui.subject.confirm()
  await ui.settle()
  assert.deepEqual(ui.requests, ['confirm', 'account-status'])
  assert.equal(ui.subject.state().running, true)
  await ui.tick()
  assert.deepEqual(ui.requests, ['confirm', 'account-status', 'account-status', 'folder-status'])
  assert.equal(ui.subject.state().running, false)
  assert.equal(ui.subject.state().error, '')
  assert.equal(ui.timers.size, 0)
}
const immediate = component(['complete'])
await immediate.subject.confirm()
await immediate.settle()
assert.deepEqual(immediate.requests, ['confirm', 'account-status', 'folder-status'])
assert.equal(immediate.subject.state().running, false)

const failedRead = component([], { rejectStatus: true })
await failedRead.subject.confirm()
await failedRead.settle()
assert.deepEqual(failedRead.requests, ['confirm', 'account-status'])
assert.equal(failedRead.subject.state().running, false)
assert.notEqual(failedRead.subject.state().error, '')
assert.equal(failedRead.timers.size, 0)

const destroyed = component(['running'])
await destroyed.subject.confirm()
await destroyed.settle()
destroyed.destroy()
assert.equal(destroyed.timers.size, 0)
assert.deepEqual(destroyed.requests, ['confirm', 'account-status'])
console.log('PASS 7 offline component polling cases. Terminal jobs request folder status. Failed reads and unmounts do not. Browser, DOM, HTTP and TLS acceptance remain unverified.')
