# mailroom

Local-first Rust core + React (`@base-ui/react`) workbench at `http://127.0.0.1:4173` for inspecting header-only mailbox censuses, simulating deterministic filing rules, and safely managing personal `mailbox.org` and `Gmail` accounts on macOS.

> **Status:** Development prototype. Every server launch defaults to read-only, and daily real-account use requires owner acceptance ([Personal-use readiness](docs/personal-readiness.md)).

## Safety Model

- **Read-only by default (`live_writes: false`):** Provider mutations stay disabled at startup until explicitly armed for the current session (configurable auto-disarm timer, default 15 minutes; locking the vault or restarting disarms immediately).
- **Preview & confirm every write:** ManageSieve publishing, IMAP folder creation, backlog moves/undo, invoice send-on, and unsubscribe execution require exact per-action previews and confirmations.
- **Localhost & CSP boundary:** Binds `127.0.0.1:4173` only, validates exact `Host`, `Origin`, and same-origin `Sec-Fetch-Site` headers, enforces a per-launch token on `/api/*` (256 KiB JSON body limit, no CORS), and serves a strict CSP without `unsafe-inline` or `unsafe-eval`.
- **Encrypted local storage:** Local-vault mode stores credentials and census keys in `~/.config/mailroom-web/vault.mrv` (Argon2id + AES-256-GCM); native macOS mode uses Keychain + a pinned confirmation helper. Passphrase-encrypted recovery bundles support up to 8 accounts plus the private address ledger.

## Workbench

| View | Purpose |
| --- | --- |
| `Overview` | Mailbox activity, monthly volume charts, cross-account health, and scan/import entry points |
| `Senders` | Searchable, sortable sender breakdown with sort-stable bulk selection into Recommendations or Subscriptions |
| `Recommendations` | Review masked census summaries before sharing with T3; inspect and accept or reject rule proposals |
| `Rules & deploy` | Edit YAML presets/overrides, simulate rules with pins, export `rules.sieve`, provision IMAP folders, publish to `mailbox.org`, and compare placement |
| `Backlog` | Preview and run checkpointed backlog moves/labels with stop support and separate confirmed Undo |
| `Invoices` | Inspect business invoice candidates, verify attachments read-only, and send approved originals without duplicates |
| `Subscriptions` | Rank unread bulk senders, apply per-row or batch actions (unsubscribe, link card, local mute), and resolve uncertain outcomes |
| `Addresses & wind-down` | Manage the private service-address ledger (with leak alerts), Gmail forwarding prep, and account wind-down |
| `Activity & undo` | Cross-account action log and open undo windows |
| `Settings` | Safety timers, single-key shortcuts toggle, Takeout `.mbox` import, T3 presets, vault lock/password change, key rotation, removal, and encrypted backup/restore |

Bilingual EN/DE UI with Light/Dark WCAG 2.1 AA themes, bundled CC0 starter sender catalog (`data/*.csv`, v1.1.0), and local Google Takeout `.mbox` header-only import.

## Quick Start

Requires Rust with Cargo and Node.js `>= 22.12` with npm:

```sh
git clone https://github.com/Alioun/mailroom.git && cd mailroom
./scripts/run-local.sh          # builds once & starts local vault at http://127.0.0.1:4173
./scripts/run-local.sh --demo   # synthetic sandbox (100 invented headers, no accounts needed)
```

| Mode | Command | Storage & Scope |
| --- | --- | --- |
| **Local checkout (fastest)** | `./scripts/run-local.sh [--demo\|--open\|--debug]` | Defaults to `--local-vault` (`~/.config/mailroom-web/vault.mrv`); `--demo` uses a temporary sandbox |
| **Pinned web runtime** | `COMMIT=$(git rev-parse HEAD) && /usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin /usr/bin/git --no-replace-objects -C "$PWD" show "$COMMIT:scripts/local-web.sh" \| /usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin /bin/bash -s -- approve "$PWD" "$COMMIT"` | Standalone SHA-256-pinned binary in `~/Library/Application Support/Mailroom Web Runtime/start` ([guide](docs/local-web.md)) |
| **Native macOS runtime** | `scripts/setup.sh` (`--plan` to preview), then `scripts/setup.sh run` | Signed runtime in `build/local/<commit>` with Keychain + pinned confirmation helper ([guide](docs/macos-confirmation.md)) |

Open `http://127.0.0.1:4173` directly (other `Host` headers are rejected). Inspect the terminal recovery wizard anytime with `bash scripts/recovery-wizard.sh --plan`.

## T3 Agent Exchange

- **Browser flow:** `Recommendations` publishes a reviewed masked summary into a server-owned exchange and launches T3; returned proposals appear automatically for local approval.
- **Standalone CLI:** `mailroom agent init --folder <abs-path>` initializes a `0700` exchange directory and `mailroom agent serve --folder <abs-path>` runs stdio MCP exposing `read_reviewed_summary` and `submit_proposal` ([docs/agent-exchange.md](docs/agent-exchange.md)).

## Verification

```sh
# Rust core
cargo fmt --check
cargo check --locked
cargo test --locked

# UI build & Playwright E2E (port 4173 must be free; run `npm --prefix ui exec playwright install chromium` once)
npm --prefix ui run check
npm --prefix ui run build
cargo build --locked --bin mailroom
mkdir -m 0700 -p target/wb-verify && MAILROOM_DISABLE_NATIVE_PICKER=1 node ui/scripts/verify-workbench.mjs "$PWD/target/debug/mailroom" "$PWD/target/wb-verify"

# Sieve proof & launcher scripts
./scripts/verify-sieve.sh
./scripts/verify-sieve-file.sh path/to/rules.sieve
scripts/verify-local-web-launcher.sh
bash -n scripts/connect-account.sh && shellcheck -e SC2329 scripts/connect-account.sh
```

`verify-workbench.mjs` runs 19 headless Chromium checks across vault lock/unlock, Senders bulk/sort/scope, Light/Dark WCAG 2.1 AA + border contrast, EN/DE `320`/`390`/`768`/`1440`px reflow, accessibility across all 10 views, and strict CSP compliance. `./scripts/verify-sieve.sh` runs `check-sieve` and Dovecot Pigeonhole (`sievec` / `sieve-test`) inside an isolated `--network none` container without mounting credentials.

## Documentation & Layout

| Area | Docs |
| --- | --- |
| **Readiness & Setup** | [Personal-use readiness](docs/personal-readiness.md) · [Local web setup](docs/local-web.md) · [macOS confirmation](docs/macos-confirmation.md) · [Browser workspace](docs/browser-workspace.md) |
| **Census, Rules & Sieve** | [Census](docs/census.md) · [Census reports](docs/census-reports.md) · [Rules & proof](docs/rules-proof.md) · [ManageSieve transactions](docs/managesieve-transactions.md) · [Placement comparison](docs/landing-comparison.md) · [IMAP folders](docs/imap-folder-provisioning.md) |
| **Mail Workflows** | [Backlog sorting](docs/backlog.md) · [Invoices](docs/invoices.md) · [Subscriptions](docs/unsubscribe.md) · [Address ledger](docs/address-ledger.md) · [Gmail forwarding](docs/gmail-forwarding.md) · [Wind-down](docs/winddown.md) |
| **Lifecycle, Recovery & UI** | [Account lifecycle](docs/account-lifecycle.md) · [Encrypted recovery](docs/recovery-format.md) · [Agent exchange](docs/agent-exchange.md) · [Design system](DESIGN.md) |

Repo layout: `src/` (Rust core & server), `ui/` (React + Base UI workbench), `data/` (CC0 sender catalog), `scripts/` (launchers, setup & Sieve verifiers), `docs/` (protocol & architecture specs).

## License

[MIT](LICENSE)
