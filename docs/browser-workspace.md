# Browser workspace

The unified interface uses React and Base UI. Every workflow shares one selected account and sidebar navigation:

- **Overview** — mailbox summary, scan progress, and quick next steps. Adding an account starts a read-only scan and opens saved findings automatically. Running another scan keeps previous findings visible.
- **Senders** — searchable, filterable, paginated sender inventory with detail dialogs.
- **Recommendations** — T3 assistant exchange with exact masked summary preview, automatic proposal arrival, and local rule review before saving.
- **Rules & deploy** — four sub-tabs: *Local rules & proof* (saved YAML, deterministic proposals, simulation, searchable message decisions, pins, and downloadable `rules.sieve`), *Folders* (IMAP folder provisioning), *Sieve deploy* (mailbox.org ManageSieve discovery, auto-recommended `Block` vs. `Include` placement, preview, deploy, rollback, and removal), and *Placement check* (observed vs. predicted folders).
- **Backlog** — mailbox.org IMAP `MOVE` and Gmail label backlog sorting with dry-run preview, stop/resume, undo, and run history.
- **Invoices** — missed-invoice detection and confirmed accounting send-on / Mail.app handoff.
- **Subscriptions** — subscription review across accounts, one-click HTTPS / mailto unsubscribe previews, mute rules, and action history.
- **Addresses & wind-down** — three sub-tabs: *Address ledger* (roles, services, leaks), *Gmail forwarding* (step-by-step setup guide), and *Wind-down* (quiet-history readiness report).
- **Settings** — session Safety Mode toggle (Read-only default / Live writes armed), account connection & rotate/forget, own-address confirmation, local Google Takeout `.mbox` header-only import, configurable T3 provider & model, local vault lock & master password change, and passphrase-encrypted recovery backup & restore.

The legacy `/advanced.html` entry has been retired.

## Session safety mode

Every server launch starts in **Read-only** mode (`live_writes: false`). Real provider writes (ManageSieve upload/activate, IMAP folder creation, backlog moves/labels, invoice send-on, and unsubscribe execution) remain blocked until the user explicitly arms **Live writes** in Settings or the top bar, and every mutation still requires its own exact preview and confirmation. Locking the vault or restarting the server automatically disarms live writes.

## T3 assistant exchange

The T3 adapter uses the installed `/opt/homebrew/bin/t3` CLI with the provider and model configured in Settings (defaulting to `codex_home_v2` and `gpt-6.1-sol`, stored in `assistant-settings.json`). It registers an isolated project for each request. The exchange contains only the reviewed summary, packaged instructions, MCP configuration, and returned proposals. It never receives the browser session token, account credentials, raw mail, or private rule documents.

The displayed states distinguish a prepared request, a T3 launch, an actual MCP summary read, and received proposals. Interrupted launches reconcile existing threads before retrying. An uncertain result never starts another thread blindly. A thread link appears only when the installed CLI returns one.

## Verification

`npm --prefix ui run check` runs TypeScript strict checking (`tsc --noEmit`). `npm --prefix ui run build` produces the single React + Base UI production bundle.

`node ui/scripts/verify-workbench.mjs <mailroom-binary> <artifact-directory>` runs the end-to-end journey in Chromium against the real server with local-vault and synthetic accounts. Port 4173 must be free. The test intercepts only the T3 launch action, then uses the real MCP client to submit a proposal. Rust dummy-CLI tests cover launch, retry reconciliation, configurable provider/model, and environment handling.
