# mailroom

Inspect a synthetic header census and preview deterministic mail rules locally. The Rust core serves a Svelte interface with Ark UI controls. The census supports owner-run read-only mailbox.org and Gmail acquisition, encrypted persistence, cancellation and resume. Independent synthetic modes demonstrate it without a provider or credential store. Local census reports, reviewed summary downloads, encrypted rule workspaces and confirmed local Sieve downloads are available. The complete census and personal workflow remain unfinished. All account writes remain denied.

## Run locally

Install Rust with Cargo and Node.js 22.12 or newer with npm. From this checkout, run:

```sh
./scripts/run-local.sh
```

Open <http://127.0.0.1:4173>. The launcher installs locked UI dependencies, builds the UI and Rust binary, and starts the server. Stop it with Ctrl+C. Use this exact URL because the server rejects other Host headers.

Real account setup belongs to the approved frozen macOS installation described in [Native confirmation and protected Keychain credentials](docs/macos-confirmation.md). Its owner wizard offers hidden terminal entry and exact native consent for adding a local account or replacing its password. A checkout build cannot use that path without the protection requirements. Local creation leaves connectivity unverified. Actual terminal, Keychain, helper and provider acceptance remain owner-run work. The connection walkthrough captures no credentials and uses the saved-account interface. Provider writes remain held.

For fake-only account verification, start the built binary with `./target/debug/mailroom --synthetic-accounts`. This mode uses temporary encrypted files, fake credentials and 100 invented headers. It cannot connect to a provider or the OS backend. The UI labels the mode explicitly.

Expand **Examples with invented mail** below the saved accounts, then click **Run sample preview**. Fourteen invented messages produce ten filings, four Inbox retentions and three independent copies. Edit the YAML, compare current folders with the previous sample configuration, switch to Gmail backlog matching or change a census pin's expected folder. A failing pin blocks the proof. Example addresses and folder roots are generic synthetic settings.

Click **Start synthetic census** to parse eight invented headers. Cancel and resume the scan, confirm an own-address candidate, inspect local header details, and open **Summary preview**. **Simulate scan error** stops after two saved headers. The normal action resumes them. Demo snapshots are encrypted in a temporary sandbox with a key held only in memory. They are not a real account census and cannot resume across server restarts.

## Verify the unit

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
npm --prefix ui run check
npm --prefix ui run build
node scripts/verify-http.mjs
node ui/scripts/verify-census.mjs
node ui/scripts/verify-acquisition.mjs
node ui/scripts/verify-reports.mjs
bash -n scripts/connect-account.sh
shellcheck -e SC2329 scripts/connect-account.sh
```

The HTTP and browser commands require the running server and run sequentially. The acquisition and report browser flows require `--synthetic-accounts`. The HTTP script checks the foundation preview, denied account operations, launch tokens, cancellation, encrypted resume, failure recovery and summary privacy. The browser scripts exercise the built interface and save screenshots under `.audit/census-browser/`, `.audit/acquisition-browser/` and `.audit/reports-browser/`. It requires Playwright Chromium, installed with `npm --prefix ui exec playwright install chromium`.

See [Census implementation and remaining work](docs/census.md) for the data model, encrypted file format and incomplete capabilities.

## Rules and proof

See [Rules and proof](docs/rules-proof.md) for the validated YAML model, emitted-output evaluator, local census interface, cross-check scope and incomplete acceptance criteria. Saved-account rules and pins load and save explicitly. A fresh proof and local confirmation can prepare an exact Sieve download for mailbox.org, with an encrypted export baseline. Gmail backlog has no Sieve-download action. A local export grants no provider authority. The examples contain only synthetic mail and configuration. [Backlog sorting](docs/backlog.md) adds exact previews, encrypted journals, recovery and conditional Undo. Its separately installed loopback IMAP source executes verified-TLS UID MOVE against an independently durable synthetic mailbox. Production execution remains unavailable.

```sh
node ui/scripts/verify-rules.mjs
./scripts/verify-sieve.sh
```

The Sieve check builds an isolated image and runs check-sieve and Pigeonhole against synthetic artifacts. It starts no global service and mounts no credentials or personal directories. Its runtime container has no network access.

See [Native confirmation and protected Keychain credentials](docs/macos-confirmation.md) for the pinned confirmation helper, the single-use confirmation boundary and Keychain protection. The owner runs the setup wizard from a root-owned copy of the approved commit, as that page describes; `--plan` previews it.

## Encrypted recovery development

[Encrypted recovery](docs/recovery-format.md) captures an explicit set of up to eight synthetic accounts and optionally the installed authoritative ledger with a separately held recovery key. It authenticates an absent-account restore and preserves later independent mailbox and transport effects. Restored accounts remain read-only pending fresh provider evidence and reconciliation. Production creation and restoration stay denied until the owner has run the native setup and authorized live validation.

Inspect the [owner-run recovery wizard](scripts/recovery-wizard.sh) with `bash scripts/recovery-wizard.sh --plan`. Planning requests no secret and creates no backup. With port 4173 free, run `CARGO_PROFILE_TEST_OPT_LEVEL=1 node scripts/verify-recovery-selected-state.mjs`, then `node ui/scripts/verify-recovery-selected-state.mjs` against its paired synthetic fixture.

## Account boundary

[Local account lifecycle](docs/account-lifecycle.md) adds synthetic-only password replacement, interrupted-creation cleanup, exact local forgetting and safe key generations. The protected owner CLI adds local account creation and password replacement. Other real lifecycle controls remain held. Actual installation and credential access still require exact approval and owner-run verification.

Read-only adapters use fixed provider TLS endpoints. There are no write-enable flags or caller-supplied source URLs. The central policy denies rule upload and activation, folder creation, moves, flags, forwarding changes, sends and unsubscribe requests. The new proof route reads the saved census under the existing account-operation lock. It assigns no recipient trust or forwarding authority.

The server binds only `127.0.0.1:4173`, validates Host and any Origin header, rejects cross-site fetch metadata, has no CORS allowance, and limits JSON POST bodies to 256 KiB. Every API except the session bootstrap requires the current launch token. Its production CSP permits only same-origin external scripts and styles, without `unsafe-inline` or `unsafe-eval`. Native CLI requests may omit Origin. A local process can bootstrap a session and request owner account reads. It cannot unlock account writes. The token binds browser requests to a server launch. It does not isolate mutually untrusted processes running as the same OS user.

The Ark UI compatibility spike exercised the production-built language toggle under this CSP. Its state changed to German without a recorded CSP violation. Future floating or portaled components need their own compatibility check before adoption.
