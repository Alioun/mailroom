# mailroom

Preview deterministic mail rules locally with synthetic header fixtures. This first unit has a Rust compiler and HTTP server, a Svelte interface, and Ark UI controls. It does not connect to accounts or authorize real writes.

## Run locally

Install Rust with Cargo and Node.js 22.12 or newer with npm. From this checkout, run:

```sh
./scripts/run-local.sh
```

Open <http://127.0.0.1:4173>. The launcher installs locked UI dependencies, builds the UI and Rust binary, and starts the server. Stop it with Ctrl+C. Use this exact URL because the server rejects other Host headers.

Click **Run sample preview**. The six bundled messages produce four filings, two Inbox retentions, and four independent copy actions. Turn off **Apply sample overrides** and run again to compare the preset. Switch between English and German at the top. Sample subjects, folders and addresses are invented demonstration data, not recommended personal settings.

## Verify the unit

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
npm --prefix ui run check
npm --prefix ui run build
node scripts/verify-http.mjs
```

The last command requires the running server. It verifies a real preview and direct denial of every account operation, foreign origins, invalid content types, excessive bodies and caller-supplied authorization fields.

## Supported rule model

[examples/sample.json](examples/sample.json) is the complete example request. Send it to `POST /api/preview` with `Content-Type: application/json`. `GET /api/sample` returns the same fixture. `GET /api/availability` returns the central policy. `POST /api/operations` accepts only an `operation` enum and denies all account operations.

The preset has an ID, a nonzero major version, filing rules and copy rules. Overrides pin that ID and major version. They can patch an existing filing rule's destination, conditions or enabled state and add filing rules. Unknown patch IDs, duplicate IDs, empty patches, invalid destinations, unknown JSON fields and unsupported conditions fail the request. Category patches, renamed-ID migration and copy-rule overrides are outside this subset.

Each filing rule has one destination, either `inbox` or a `folder` name. The compiler orders guards, patched and added user rules, bulk rules, From rules and Subject rules. Input order breaks ties within each stage. A guard requires a spam condition, bulk requires a list marker, From requires a sender test, and Subject requires a subject test. Patched guards keep guard precedence. The first match decides the only filing destination. Unmatched messages stay in Inbox.

Conditions within a rule are ANDed. Supported conditions are `from_address`, `from_domain`, `subject_contains`, `list_id_exists`, `one_click` and `spam_flag`. Sender and subject comparisons use ASCII case folding. Domain tests include subdomains. `spam_flag` tests `X-Spam-Flag: yes`. `one_click` tests whether `List-Unsubscribe-Post` contains `List-Unsubscribe=One-Click`.

Every matching copy rule runs before filing, including before guard retention. Counts distinguish copy actions from copied messages and filing destinations. The preview includes stable matched rule IDs and override traces. Generated Sieve uses the same program with `redirect :copy`, one filing action and `stop`, plus final Inbox retention. Folders must already exist. The output never creates folders.

## Limits and account boundary

This is compiled-subset evaluation, not Sieve execution or provider verification. It accepts at most 200 header-only messages and 100 rules per request. Headers must be ASCII, at most 16 KiB per message, with one bare From mailbox. Folded headers are unfolded. Duplicate headers, MIME encoded words, bodies, display names, non-example address domains, envelope tests, regex and arbitrary Sieve are rejected or unsupported. Sender, recipient, condition and copy addresses must use reserved example domains. No message content is stored.

Connection, header census, pinned-message proof, deployment, journaled sorting, undo, unsubscribe, address policy and daily-use readiness are unavailable. There are no provider adapters, credentials, write-enable flags or caller-supplied source URLs. The central policy classifies synthetic reads, account reads and account writes. It denies rule upload and activation, folder creation, moves, flags, forwarding changes, sends and unsubscribe requests. Account reads are also unavailable in this unit.

The server binds only `127.0.0.1:4173`, validates Host and any Origin header, rejects cross-site fetch metadata, has no CORS allowance, and limits JSON POST bodies to 256 KiB. Its production CSP permits only same-origin external scripts and styles, without `unsafe-inline` or `unsafe-eval`. Native CLI requests may omit Origin. A local process can run previews but cannot unlock account writes.

The Ark UI compatibility spike exercised the production-built language toggle under this CSP. Its state changed to German without a recorded CSP violation. Future floating or portaled components need their own compatibility check before adoption.
