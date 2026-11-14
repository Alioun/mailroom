# Browser workspace

The default interface uses React and Base UI. Overview, Senders, Recommendations, and Local rules share one selected account. Adding an account starts a read-only scan and opens saved findings automatically. Running another scan keeps the previous findings visible.

Recommendations start with a browser preview of the exact summary. Identifiers stay masked unless the user selects an organization domain. Sharing saves the request before starting T3. Returned proposals appear automatically, survive reloads, and require local review before saving rule edits. Dismissal is separate from permanent rule rejection.

The T3 adapter uses the installed `/opt/homebrew/bin/t3` CLI and the configured `codex_home_v2` provider with `gpt-6.1-sol`. It registers an isolated project for each request. The exchange contains only the reviewed summary, packaged instructions, MCP configuration, and returned proposals. It never receives the browser session token, account credentials, raw mail, or private rule documents.

The displayed states distinguish a prepared request, a T3 launch, an actual MCP summary read, and received proposals. Interrupted launches reconcile existing threads before retrying. An uncertain result never starts another thread blindly. A thread link appears only when the installed CLI returns one.

Advanced tools remain at `/advanced.html` during the migration. They include invoices, subscriptions, recovery, provider diagnostics, and detailed rule tools. Existing specialist browser tests target this entry. Core workspace tests use the default entry.

## Verification

`npm --prefix ui run check` checks TypeScript and retained Svelte components. `npm --prefix ui run build` builds both entries.

`node ui/scripts/verify-workbench.mjs <mailroom-binary> <artifact-directory>` runs the core journey in Chromium against the real server with invented local-vault and synthetic accounts. Port 4173 must be free. The test intercepts only the T3 launch action, then uses the real MCP client to submit a proposal. Rust dummy-CLI tests cover launch, retry reconciliation, and environment handling. This test does not establish a completed round trip through the installed T3 agent.

The core test covers browser vault creation and saved-account unlock, automatic scanning, 1,000 sender rows, search, filtering, pagination, drafts across account switches, failed-account recovery, first-time rule creation during proposal review, exact masked sharing, automatic proposal arrival, local rule saving, mobile keyboard focus, narrow layouts, German, and provider-write refusal.

An additional installed-T3 check used an isolated invented-data exchange. The installed agent read the reviewed summary and submitted a proposal through the CLI MCP fallback. This independently verifies the agent exchange. The browser test still intercepts launch, so it does not establish one uninterrupted browser-to-installed-agent journey.
