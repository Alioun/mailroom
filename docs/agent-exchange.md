# Agent exchange boundary

The development exchange lets an agent read a supplied census-summary export and return text recommendations for local review. It includes separate input and proposal directories, `AGENTS.md`, an agent skill and a stdio MCP command. This unit uses no account repository, census key, credential backend, native helper or provider connection.

The existing summary download now requires browser review and [native action-naming confirmation](census-reports.md#reviewed-summary-boundary), followed by census revalidation. Automatic publication into the exchange, installed agent restrictions, rule-patch acceptance and agent proof exports remain unfinished. Only invented exports and injected confirmation backends have acceptance coverage. Installed native and owner acceptance of this disclosure action remain unverified. This is not personal daily-use acceptance.

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
| `submit_proposal` | The strict version-1 proposal document in the advertised JSON Schema. | A proposal digest, recommendation count and `staged_for_local_review` status. |

Neither tool accepts a path, account identifier, host, credential, execution command or approval. Unsupported tools and malformed arguments fail with fixed codes. Mail observations remain untrusted data.

The proposal contains `version`, `summary_sha256` and 1 to 20 text recommendations. Each recommendation has `title`, `reason` and `suggested_change`. Titles have at most 120 Unicode characters. The other fields each have at most 2,000. Empty fields and control characters other than newline and tab fail. Unknown fields fail. The document itself is at most 128 KiB.

Submission checks the currently supplied summary digest and saves `proposals/<document-sha256>.json`. It synchronizes the file and parent directory before acknowledging success. An exact retry returns the same digest without replacing the file. A changed or partial existing file fails. Interrupted proposals confer no authority, and a malformed partial file cannot open in the UI. The exchange does not compact or delete proposal history.

## Local review

The report panel offers **Open agent recommendations** beside the exact summary preview. It accepts only a valid proposal whose digest matches those exact summary bytes. Imported text stays in browser memory. Changing the report or summary removes that review. The panel escapes text and creates no links, commands, approvals or rule changes.

The digest detects mismatched summary bytes. It is not a signature, account binding, owner decision or provider evidence. A matching recommendation is still untrusted. The owner can use it while editing local rules, then save the existing workspace and run fresh proof. Unrelated overrides stay untouched by the import because import changes none of them.

## Verification

Rust tests cover folder reopen, exact retries, interrupted files, changed summaries, malformed proposals, link and permission refusals, protocol initialization and bounded errors. The SDK interoperability runner is `scripts/verify-agent-mcp.mjs`. It uses the actual MCP client vendored by the installed Playwright distribution, with invented data and isolated process directories. No MCP dependency is added to the executable.

The UI verifier is `ui/scripts/verify-agent-exchange.mjs`. It parses the document written by that actual command, rejects changed and malformed documents, and renders escaped English and German reviews. Server rendering and a successful build do not establish mounted-browser acceptance. Public specification refresh and installed agent-client acceptance remain separate checks.

The summary-download Router tests use invented accounts and an injected confirmation backend. `ui/scripts/verify-summary-disclosure.mjs` consumes their exact export fixture. `scripts/verify-native-model.py` compiles and runs the existing Swift XCTest model cases directly with this Mac's installed Xcode libraries. These checks launch no native helper and establish no installed confirmation or Keychain acceptance.
