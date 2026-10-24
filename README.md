# mailroom

Inspect a synthetic header census and preview deterministic mail rules locally. The Rust core serves a Svelte interface with Ark UI controls. The census supports owner-run read-only mailbox.org and Gmail acquisition, encrypted persistence, cancellation and resume. Independent synthetic modes demonstrate it without a provider or credential store. Full reports and the complete personal workflow remain unfinished. All account writes remain denied.

## Run locally

Install Rust with Cargo and Node.js 22.12 or newer with npm. From this checkout, run:

```sh
./scripts/run-local.sh
```

Open <http://127.0.0.1:4173>. The launcher installs locked UI dependencies, builds the UI and Rust binary, and starts the server. Stop it with Ctrl+C. Use this exact URL because the server rejects other Host headers.

For an owner-run connection walkthrough, use `./scripts/connect-account.sh`. Connect an account with an existing credential in the hidden UI field. It goes directly to the OS credential backend. Choose the saved account, run or resume its census, and open the local result. No account settings change. No agent runs credentialed steps. The OS adapters and actual provider behavior have not been validated with real accounts.

For fake-only account verification, start the built binary with `./target/debug/mailroom --synthetic-accounts`. This mode uses temporary encrypted files, fake credentials and 100 invented headers. It cannot connect to a provider or the OS backend. The UI labels the mode explicitly.

Click **Run sample preview**. The six bundled messages produce four filings, two Inbox retentions, and four independent copy actions. Turn off **Apply sample overrides** and run again to compare the preset. Switch between English and German at the top. Sample subjects, folders and addresses are invented demonstration data, not recommended personal settings.

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
bash -n scripts/connect-account.sh
shellcheck -e SC2329 scripts/connect-account.sh
```

The HTTP and browser commands require the running server and run sequentially. The acquisition browser flow requires `--synthetic-accounts`. The HTTP script checks the foundation preview, denied account operations, launch tokens, cancellation, encrypted resume, failure recovery and summary privacy. The browser scripts exercise the built interface and save screenshots under `.audit/census-browser/` and `.audit/acquisition-browser/`. It requires Playwright Chromium, installed with `npm --prefix ui exec playwright install chromium`.

See [Census implementation and remaining work](docs/census.md) for the data model, encrypted file format and incomplete capabilities.

## Supported rule model

[examples/sample.json](examples/sample.json) is the complete example request. `GET /api/session` bootstraps a per-launch token. Subsequent API calls require it in `X-Mailroom-Token`. Send the fixture to `POST /api/preview` with `Content-Type: application/json`. `GET /api/sample` returns the same fixture. `GET /api/availability` returns the central policy. `POST /api/operations` accepts only an `operation` enum and never starts account work. Dedicated account routes implement owner-run reads. Tokens stay in process and browser memory. The launcher and verification scripts never print them.

The preset has an ID, a nonzero major version, filing rules and copy rules. Overrides pin that ID and major version. They can patch an existing filing rule's destination, conditions or enabled state and add filing rules. Unknown patch IDs, duplicate IDs, empty patches, invalid destinations, unknown JSON fields and unsupported conditions fail the request. Category patches, renamed-ID migration and copy-rule overrides are outside this subset.

Each filing rule has one destination, either `inbox` or a `folder` name. The compiler orders guards, patched and added user rules, bulk rules, From rules and Subject rules. Input order breaks ties within each stage. A guard requires a spam condition, bulk requires a list marker, From requires a sender test, and Subject requires a subject test. Patched guards keep guard precedence. The first match decides the only filing destination. Unmatched messages stay in Inbox.

Conditions within a rule are ANDed. Supported conditions are `from_address`, `from_domain`, `subject_contains`, `list_id_exists`, `one_click` and `spam_flag`. Sender and subject comparisons use ASCII case folding. Domain tests include subdomains. `spam_flag` tests `X-Spam-Flag: yes`. `one_click` tests whether `List-Unsubscribe-Post` contains `List-Unsubscribe=One-Click`.

Every matching copy rule runs before filing, including before guard retention. Counts distinguish copy actions from copied messages and filing destinations. The preview includes stable matched rule IDs and override traces. Generated Sieve uses the same program with `redirect :copy`, one filing action and `stop`, plus final Inbox retention. Folders must already exist. The output never creates folders.

## Limits and account boundary

This is compiled-subset evaluation, not Sieve execution or provider verification. It accepts at most 200 header-only messages and 100 rules per request. Headers must be ASCII, at most 16 KiB per message, with one bare From mailbox. Folded headers are unfolded. Duplicate headers, MIME encoded words, bodies, display names, non-example address domains, envelope tests, regex and arbitrary Sieve are rejected or unsupported. Sender, recipient, condition and copy addresses must use reserved example domains. No message content is stored.

Pinned-message proof, deployment, journaled sorting, undo, unsubscribe, address policy and daily-use readiness are unavailable. Read-only adapters use fixed provider TLS endpoints. There are no write-enable flags or caller-supplied source URLs. The central policy classifies synthetic reads, account reads and account writes. It permits the synthetic preview and synthetic census. It denies rule upload and activation, folder creation, moves, flags, forwarding changes, sends and unsubscribe requests. Dedicated account actions permit connection, folder discovery and header acquisition only.

The server binds only `127.0.0.1:4173`, validates Host and any Origin header, rejects cross-site fetch metadata, has no CORS allowance, and limits JSON POST bodies to 256 KiB. Every API except the session bootstrap requires the current launch token. Its production CSP permits only same-origin external scripts and styles, without `unsafe-inline` or `unsafe-eval`. Native CLI requests may omit Origin. A local process can bootstrap a session and request owner account reads. It cannot unlock account writes. The token binds browser requests to a server launch. It does not isolate mutually untrusted processes running as the same OS user.

The Ark UI compatibility spike exercised the production-built language toggle under this CSP. Its state changed to German without a recorded CSP violation. Future floating or portaled components need their own compatibility check before adoption.
