# mailroom

Inspect a header census, preview deterministic mail rules locally, and run the personal Mac workflow for mailbox.org and Gmail. The Rust core serves a code-split React + Base UI workbench (with WCAG 2.1 AA light/dark themes and instant push status updates) at `http://127.0.0.1:4173`. Every server launch starts in **Read-only** mode (`live_writes: false`); live provider writes (ManageSieve deployment, IMAP folder provisioning, backlog sorting, invoice send-on, and unsubscribe execution) stay blocked until explicitly armed for the current session (with a configurable auto-disarm countdown, default 15 minutes), and every mutation still requires an exact preview and confirmation. Locking the vault (manually or via optional inactivity auto-lock) or restarting disarms live writes automatically.

The workbench unifies all ten views (`Overview`, `Senders`, `Recommendations`, `Rules & deploy`, `Backlog`, `Invoices`, `Subscriptions`, `Addresses & wind-down`, `Activity & undo`, and `Settings`), includes a curated CC0 starter sender catalog (`data/*.csv`, v1.1.0), supports local Google Takeout `.mbox` header-only import (with native macOS Finder picker) alongside IMAP census, configurable T3 provider/model presets, and full local-vault lifecycle & passphrase-encrypted recovery with drag-and-drop bundle import.

The [development agent exchange](docs/agent-exchange.md) includes separate folders, a skill, a stdio MCP command and local review of text recommendations. Summary sharing shows the exact masked summary in the browser before launching T3.

## Run locally

Read [Personal-use readiness](docs/personal-readiness.md) for implemented workflows, remaining acceptance gates and the scope of the Mac prototype.

The [Gmail forwarding preparation guide](docs/gmail-forwarding.md) provides the settled route and a terminal plan beside the account workspace's Setup and recovery tools. Live setup remains owner-run after separate approval.

Install Rust with Cargo and Node.js 22.12 or newer with npm. From this checkout, run:

```sh
./scripts/run-local.sh
```

Open <http://127.0.0.1:4173>. The launcher installs locked UI dependencies, builds the UI and Rust binary, and starts the server. Stop it with Ctrl+C. Use this exact URL because the server rejects other Host headers.

The [local web setup](docs/local-web.md) is the browser-only way to use real accounts. `scripts/local-web.sh` builds an exact commit once into a runtime in your home folder and starts it with one command. The browser then creates or unlocks an encrypted vault with a master password and adds accounts. It needs no Keychain, native dialog or terminal wizard, and it leaves the native setup's accounts untouched.

Real accounts can also use the signed local runtime that `scripts/setup.sh` builds from the HEAD commit into `build/local/<commit>`, without `sudo` or any installation, described in [Native confirmation and protected Keychain credentials](docs/macos-confirmation.md). Its owner wizard offers hidden terminal entry and exact native consent for adding a local account or replacing its password. After setup, `scripts/setup.sh run` starts the server from that runtime without a rebuild. `./scripts/run-local.sh` builds without a helper pin or hardened runtime, so it cannot use real accounts. Local creation leaves connectivity unverified. Actual terminal, Keychain, helper and provider acceptance remain owner-run work. The connection walkthrough captures no credentials and uses the saved-account interface. Provider writes remain held.

For fake-only account verification, run `./scripts/run-local.sh --synthetic-accounts`. This mode uses temporary encrypted files, fake credentials and 100 invented headers. It cannot connect to a provider or the OS backend. The UI labels the mode explicitly.

Development and review runners in this workspace share one Cargo target and a stable isolated Cargo home, as recorded in the workspace `AGENTS.md`. Pass that same absolute `CARGO_TARGET_DIR` to child runners that construct their own environment. The launcher uses `cargo run` so it respects the selected target directory. Keep profiles and compiler flags stable when the check permits it. Frozen owner-acceptance stages retain their approved build environments.

Expand **Examples with invented mail** below the saved accounts, then click **Run sample preview**. Fourteen invented messages produce ten filings, four Inbox retentions and three independent copies. Edit the YAML, compare current folders with the previous sample configuration, switch to Gmail backlog matching or change a census pin's expected folder. A failing pin blocks the proof. Example addresses and folder roots are generic synthetic settings.

Click **Start synthetic census** to parse eight invented headers. Cancel and resume the scan, confirm an own-address candidate, inspect local header details, and open **Summary preview**. **Simulate scan error** stops after two saved headers. The normal action resumes them. Demo snapshots are encrypted in a temporary sandbox with a key held only in memory. They are not a real account census and cannot resume across server restarts.

## Verify the unit

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
npm --prefix ui run check
npm --prefix ui run build
scripts/verify-local-web-launcher.sh
node ui/scripts/verify-workbench.mjs "$PWD/target/debug/mailroom" .audit/workbench-browser
bash -n scripts/connect-account.sh
shellcheck -e SC2329 scripts/connect-account.sh
```

`verify-workbench.mjs` runs 18 end-to-end browser checks in Playwright Chromium against the real server with local-vault and synthetic accounts (port 4173 must be free), covering vault lock/unlock, dark & light WCAG 2.1 AA contrast, safety timer countdown, recovery bundle upload, Takeout import, Activity & undo, responsive 320/768/1440px EN+DE layouts, and DOM accessibility across all 10 views. Install Chromium once with `npm --prefix ui exec playwright install chromium`.

See [Census implementation and remaining work](docs/census.md) for the data model, encrypted file format and incomplete capabilities.

## Rules and proof

See [Rules and proof](docs/rules-proof.md) for the validated YAML model, emitted-output evaluator, local census interface, cross-check scope and incomplete acceptance criteria. Saved-account rules and pins load and save explicitly. A fresh proof and local confirmation can prepare an exact Sieve download for mailbox.org, with an encrypted export baseline. Gmail backlog has no Sieve-download action. A local export grants no provider authority. The examples contain only synthetic mail and configuration. [Backlog sorting](docs/backlog.md) adds exact previews, encrypted journals, recovery and conditional Undo. Its separately installed loopback IMAP source executes verified-TLS UID MOVE against an independently durable synthetic mailbox. Production execution remains unavailable.

```sh
./scripts/verify-sieve.sh
```

The Sieve check builds an isolated image and runs check-sieve and Pigeonhole against synthetic artifacts. It starts no global service and mounts no credentials or personal directories. Its runtime container has no network access.

See [Native confirmation and protected Keychain credentials](docs/macos-confirmation.md) for the pinned confirmation helper, the single-use confirmation boundary and Keychain protection. The owner runs the setup wizard from a clean checkout of the approved commit, as that page describes. `scripts/setup.sh --plan` previews it.

## Encrypted recovery development

[Encrypted recovery](docs/recovery-format.md) captures an explicit set of up to eight synthetic accounts and optionally the installed authoritative ledger with a separately held recovery key. It authenticates an absent-account restore and preserves later independent mailbox and transport effects. Restored accounts remain read-only pending fresh provider evidence and reconciliation. Production creation and restoration stay denied until the owner has run the native setup and authorized live validation.

Inspect the [owner-run recovery wizard](scripts/recovery-wizard.sh) with `bash scripts/recovery-wizard.sh --plan`. Planning requests no secret and creates no backup. With port 4173 free, run `CARGO_PROFILE_TEST_OPT_LEVEL=1 node scripts/verify-recovery-selected-state.mjs` against its paired synthetic fixture.

## Account boundary

[Local account lifecycle](docs/account-lifecycle.md) adds synthetic-only password replacement, interrupted-creation cleanup, exact local forgetting and safe key generations. The protected owner CLI adds local account creation and password replacement. Other real lifecycle controls remain held. Actual runtime use and credential access still require exact approval and owner-run verification.

Read-only adapters use fixed provider TLS endpoints. There are no write-enable flags or caller-supplied source URLs. The central policy denies rule upload and activation, folder creation, moves, flags, forwarding changes, sends and unsubscribe requests. The new proof route reads the saved census under the existing account-operation lock. It assigns no recipient trust or forwarding authority.

The server binds only `127.0.0.1:4173`, validates Host and any Origin header, rejects cross-site fetch metadata, has no CORS allowance, and limits JSON POST bodies to 256 KiB. Every API except the session bootstrap requires the current launch token. Its production CSP permits only same-origin external scripts and styles, without `unsafe-inline` or `unsafe-eval`. Native CLI requests may omit Origin. A local process can bootstrap a session and request owner account reads. It cannot unlock account writes. The token binds browser requests to a server launch. It does not isolate mutually untrusted processes running as the same OS user.

All floating and portaled Base UI components (dialogs, selects, tabs, and navigation drawers) are verified under this strict CSP with zero violations in `ui/scripts/verify-workbench.mjs`.
