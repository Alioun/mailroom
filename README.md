# mailroom

Inspect a synthetic header census and preview deterministic mail rules locally. The Rust core serves a Svelte interface with Ark UI controls. The census supports owner-run read-only mailbox.org and Gmail acquisition, encrypted persistence, cancellation and resume. Independent synthetic modes demonstrate it without a provider or credential store. Local census reports and reviewed summary downloads are available. The complete census and personal workflow remain unfinished. All account writes remain denied.

## Run locally

Install Rust with Cargo and Node.js 22.12 or newer with npm. From this checkout, run:

```sh
./scripts/run-local.sh
```

Open <http://127.0.0.1:4173>. The launcher installs locked UI dependencies, builds the UI and Rust binary, and starts the server. Stop it with Ctrl+C. Use this exact URL because the server rejects other Host headers.

For an owner-run connection walkthrough, use `./scripts/connect-account.sh`. Connect an account with an existing credential in the hidden UI field. It goes directly to the OS credential backend. Choose the saved account, run or resume its census, and open the local result. No account settings change. No agent runs credentialed steps. The OS adapters and actual provider behavior have not been validated with real accounts.

For fake-only account verification, start the built binary with `./target/debug/mailroom --synthetic-accounts`. This mode uses temporary encrypted files, fake credentials and 100 invented headers. It cannot connect to a provider or the OS backend. The UI labels the mode explicitly.

Click **Run sample preview**. Fourteen invented messages produce ten filings, four Inbox retentions and three independent copies. Edit the YAML, compare current folders with the previous sample configuration, switch to Gmail backlog matching or change a census pin's expected folder. A failing pin blocks the proof. Example addresses and folder roots are generic synthetic settings.

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

See [Rules and proof](docs/rules-proof.md) for the validated YAML model, emitted-output evaluator, local census interface, cross-check scope and incomplete acceptance criteria. The examples contain only synthetic mail and configuration.

```sh
node ui/scripts/verify-rules.mjs
./scripts/verify-sieve.sh
```

The Sieve check builds an isolated image and runs check-sieve and Pigeonhole against synthetic artifacts. It starts no global service and mounts no credentials or personal directories. Its runtime container has no network access.

## Account boundary

Read-only adapters use fixed provider TLS endpoints. There are no write-enable flags or caller-supplied source URLs. The central policy denies rule upload and activation, folder creation, moves, flags, forwarding changes, sends and unsubscribe requests. The new proof route reads the saved census under the existing account-operation lock. It assigns no recipient trust or forwarding authority.

The server binds only `127.0.0.1:4173`, validates Host and any Origin header, rejects cross-site fetch metadata, has no CORS allowance, and limits JSON POST bodies to 256 KiB. Every API except the session bootstrap requires the current launch token. Its production CSP permits only same-origin external scripts and styles, without `unsafe-inline` or `unsafe-eval`. Native CLI requests may omit Origin. A local process can bootstrap a session and request owner account reads. It cannot unlock account writes. The token binds browser requests to a server launch. It does not isolate mutually untrusted processes running as the same OS user.

The Ark UI compatibility spike exercised the production-built language toggle under this CSP. Its state changed to German without a recorded CSP violation. Future floating or portaled components need their own compatibility check before adoption.
