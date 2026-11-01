# Agent exchange boundary

The development exchange lets an agent read a supplied census-summary export and return text recommendations, optionally with structured rule edits, for local review. It includes separate input and proposal directories, `AGENTS.md`, an agent skill and a stdio MCP command. This unit uses no account repository, census key, credential backend, native helper or provider connection.

The existing summary download now requires browser review and [native action-naming confirmation](census-reports.md#reviewed-summary-boundary), followed by census revalidation. Automatic publication into the exchange, installed agent restrictions and agent proof exports remain unfinished. Only invented exports and injected confirmation backends have acceptance coverage. Installed native and owner acceptance of this disclosure action remain unverified. This is not personal daily-use acceptance.

## Files

`mailroom agent init --folder <absolute-path>` creates an empty exchange. The root and subdirectories have mode `0700`. The fixed structure is:

```text
AGENTS.md
skills/mailroom-assistant/SKILL.md
reviewed/
proposals/
```

Initialization copies static agent instructions. It exports no account data and refuses to replace edited instructions. It can finish creating missing static files after an interruption. The server never creates missing directories when it starts.

The supplied input is `reviewed/census-summary.md`, a UTF-8 export no larger than 256 KiB with the census summary heading. The file must have mode `0600`, belong to the current user and have a single hard link. The fixed child directories and files reject symbolic links. Directory handles anchor child access after the exchange opens.

File placement and Unix permissions do not establish approval provenance or isolate another process running as the same user. These checks are not a substitute for an enforced agent sandbox. The owner must approve the exact disclosure before real account data enters an installed exchange. This command does not implement an automatic publication path or authenticate the provenance of a supplied file.

The exchange contains user-managed exported copies. Account forgetting, encrypted rotation and recovery do not manage these copies. Account removal cannot erase exports, backups or copies already read by an agent. The exchange must stay separate from private account storage and the shared address ledger.

## Stdio protocol

`mailroom agent serve --folder <absolute-path>` speaks newline-delimited JSON-RPC over stdin and stdout. It requires MCP initialization followed by `notifications/initialized`. It supports protocol versions `2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05`. Each request is at most 128 KiB. An oversized frame closes the command. Diagnostics use fixed status codes on stderr. They contain no caller-supplied paths or values.

The server advertises two tools:

| Tool | Input | Result |
| --- | --- | --- |
| `read_reviewed_summary` | An empty object. | The supplied summary and its exact UTF-8 SHA-256. |
| `submit_proposal` | The strict version-1 proposal document in the advertised JSON Schema. | A proposal digest, recommendation count, rule edit count and `staged_for_local_review` status. |

Neither tool accepts a path, account identifier, host, credential, execution command or approval. Unsupported tools and malformed arguments fail with fixed codes. Mail observations remain untrusted data.

The proposal contains `version`, `summary_sha256` and 1 to 20 text recommendations. Each recommendation has `title`, `reason` and `suggested_change`. Titles have at most 120 Unicode characters. The other fields each have at most 2,000. Empty fields and control characters other than newline and tab fail. Unknown fields fail. The document itself is at most 128 KiB.

An optional `rule_edits` object holds `patches` and `additions`, 1 to 20 entries together. A patch uses the common rule model's `Patch` shape and names an existing preset rule. An addition uses the `Rule` shape with a fresh `user.*` ID. The packaged skill lists the constraints. Staging checks only the closed schema, the counts and the summary digest. The exchange has no rule documents, so it cannot detect conflicts. Local review does that.

Submission checks the currently supplied summary digest and saves `proposals/<document-sha256>.json`. It synchronizes the file and parent directory before acknowledging success. An exact retry returns the same digest without replacing the file. A changed or partial existing file fails. Interrupted proposals confer no authority, and a malformed partial file cannot open in the UI. The exchange does not compact or delete proposal history.

## Local review

The report panel offers **Open agent recommendations** beside the exact summary preview. It accepts only a valid proposal whose digest matches those exact summary bytes. Imported text stays in browser memory. Changing the report or summary removes that review. The panel escapes text and creates no links or commands.

The digest detects mismatched summary bytes. It is not a signature, account binding, owner decision or provider evidence. A matching recommendation is still untrusted. Text recommendations change nothing. The owner can use them while editing local rules, then save the workspace and run fresh proof.

## Rule edit import

A proposal with `rule_edits` also offers **Review rule edits against saved local rules**. The browser sends the account, the reviewed summary's `as_of`, report version, approved clear domains and summary receipt, and the proposal to the protected `POST /api/accounts/agent-edits` action. The server regenerates the summary from the saved census and refuses the request unless the receipt matches. A new scan or another selection therefore makes the proposal stale.

Preview validates the edits against the saved workspace:

- Every patch follows the preset's rename chain to an existing preset or bundled core sender rule. Cycles, unknown rules, owner additions and guard rules are refused. A patch must change a value.
- Each rule is claimed once across existing owner patches and additions and the proposal's edits.
- An addition needs a fresh `user.*` filing rule ID that is neither a current rule nor a renamed ID.
- Edits the owner rejected earlier are refused. Matching uses a digest of the edit with its patch ID resolved, so an old alias does not bypass it.
- The resulting documents must pass the authoritative-ledger check and compile and prove like a workspace save.

Patches are written under their current IDs. The resulting override document is reserialized, as with filing proposals. The review shows both override documents side by side, a line diff, and the current and resulting proof side by side in English and German, with counts, gate, pins, omissions and warnings.

Accepting and rejecting use separate purpose-bound HMAC receipts. Both bind the workspace context, the whole saved account state (account, state and workspace revisions, rejection history and census digest), the exact proposal, the reviewed edits, both documents, the diff, both proofs and the effects text. Confirmation needs `confirmed: true` and recomputes the review under the account locks. A stale, altered or cross-purpose receipt is refused, as is any confirmation while recovery reconciliation is pending. Acceptance is unavailable when the resulting proof is blocked.

Acceptance writes the resulting override document, increments the workspace revision and saves through the encrypted account store. That invalidates earlier proof and download receipts. Run fresh proof before any export. Rejection leaves the documents unchanged and records each edit's digest, current target, summary digest and the workspace `as_of` in `proposals.agent_rejections`. That list holds at most 512 entries. When it is full, rejection is unavailable and nothing is pruned. The field is omitted while empty, so the store schema stays 32 and older states read unchanged.

Nothing in a proposal can change roles, layout, ledger entries, targets, forwards, delivery tags, credentials or exports, or start a deployment, send or mailbox action.

## Verification

Rust tests cover folder reopen, exact retries, interrupted files, changed summaries, malformed proposals, closed rule edit schemas, link and permission refusals, protocol initialization and bounded errors. `rules::agent_edits` tests cover invalid edits, rename chains and cycles, double claims, stale, altered, cross-purpose and cross-account receipts, rejection memory and its bound. `server::accounts::agent_edits` tests run the protected action through the Router with an invented synthetic account. The SDK interoperability runner is `scripts/verify-agent-mcp.mjs`. It uses the actual MCP client vendored by the installed Playwright distribution, with invented data and isolated process directories. No MCP dependency is added to the executable.

The UI verifier is `ui/scripts/verify-agent-exchange.mjs`. It parses the document written by that actual command, rejects changed and malformed documents, and renders escaped English and German reviews. `ui/scripts/verify-agent-rule-edits.mjs` reads the review and saved response printed by the Router test, checks the client schema and request bodies, and renders the side-by-side review in English and German. Server rendering and a successful build do not establish mounted-browser acceptance. Public specification refresh and installed agent-client acceptance remain separate checks.

The summary-download Router tests use invented accounts and an injected confirmation backend. `ui/scripts/verify-summary-disclosure.mjs` consumes their exact export fixture. `scripts/verify-native-model.py` compiles and runs the existing Swift XCTest model cases directly with this Mac's installed Xcode libraries. These checks launch no native helper and establish no installed confirmation or Keychain acceptance.
