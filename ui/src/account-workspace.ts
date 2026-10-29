import { openAccount, type AccountView } from './api'

export type SavedAccountState =
  | { kind: 'empty' }
  | { kind: 'loading'; account: string }
  | { kind: 'ready'; account: string; view: AccountView }
  | { kind: 'error'; account: string }

export function accountWorkspace(publish: (state: SavedAccountState) => void) {
  let revision = 0
  let active = true
  return {
    async load(account: string, read: (account: string) => Promise<AccountView> = openAccount) {
      const requested = ++revision
      if (!active) return
      if (!account) { publish({ kind: 'empty' }); return }
      publish({ kind: 'loading', account })
      try {
        const view = await read(account)
        if (active && requested === revision) {
          publish(view.census.account === account ? { kind: 'ready', account, view } : { kind: 'error', account })
        }
      } catch {
        if (active && requested === revision) publish({ kind: 'error', account })
      }
    },
    clear() {
      revision += 1
      if (active) publish({ kind: 'empty' })
    },
    dispose() { active = false; revision += 1 },
  }
}
