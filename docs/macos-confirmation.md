# Native confirmation and protected Keychain credentials on macOS

Two small Swift helpers in `native/macos` do the macOS work. `mailroom-confirm` shows one exact action and returns a signed answer. `mailroom-keychain` performs one Keychain operation. Mailroom launches either only when the running process has the code identity pinned into the mailroom binary at build time.

## Exact-action requests

Invoice sends, unsubscribe sends, subscription actions, recovery, and the consequential mailbox changes (folder creation, Sieve upload, activation, removal and compensation, backlog moves and Gmail label changes, and their undo) all pass through one confirmation boundary. The server derives each request from its own plan. A request names the account, purpose, target, sending and receiving identities, revisions and content digest. Its public receipt is a random single-use identity valid for five minutes. Mailbox changes name the operation and the exact preview receipt, which is a keyed hash of the full previewed plan, so the native dialog shows that receipt rather than every folder or message.

After a person approves, the boundary checks the answer's HMAC under a fresh 32-byte challenge, reloads state and recomputes the plan. Only an unchanged plan yields the internal consent capability. A cancelled, failed, malformed, unauthenticated, replayed, expired or changed confirmation denies. Intent is saved before an effect and observation afterwards. An interrupted send stays uncertain until reconciled. A native yes never authorizes a retry.

The synthetic fixture server uses an in-process backend that approves and labels its requests `synthetic`. Production builds use the native backend. A build without a pinned helper issues no request. Production provider writes remain denied before any helper access, independently of confirmation.

## Launch and channel

macOS refuses to execute an already-open descriptor (`/dev/fd/N` returns EACCES), so mailroom checks the started process instead. It rejects symlinks, non-regular files, and files or parent directories that other users can write. It then starts the helper suspended with `posix_spawn`, a fixed argv, an empty environment, stdin and stdout pipes, `/dev/null` as stderr and every other descriptor closed. It reads the kernel's cdhash and code-signing flags for that process (`csops`) and resumes it only when the cdhash matches the pin and the hardened runtime, kill and hard flags are set. A file replaced at any moment before the spawn produces a different cdhash and is killed before it runs.

The request and challenge travel only over the private pipes. Another local process cannot answer in the helper's place, because the answer must carry an HMAC under a challenge it never sees. The Developer ID requirement is a typed slot (`Pin::DeveloperIdRequirement`) that this build refuses.

## Keychain items

Items use the generic-password service `mailroom-protected-v1`. The helper creates each item with an explicit access list.

- Mailbox passwords trust no application, so macOS asks the person at every read. A read that cannot ask returns nothing. This is the per-access requirement for the assisted workflow.
- Local census keys and identity keys trust only the helper's own cdhash, because every store open reads them. Any other binary, a rebuilt helper included, triggers a system prompt.

Items are created, never overwritten. Deletion is idempotent. Secrets reach the helper only through its stdin pipe, come back only in an authenticated reply, are zeroized after use and never appear in argv, logs, errors or files. Errors are fixed codes.

Legacy items from the earlier `keyring` service `mailroom` are detected by name only. Migration copies each into a protected item when the owner runs it. Originals stay until the owner retires them.

## Owner setup

Run `bash scripts/setup-macos-protection.sh` in a terminal. `--plan` previews it. The wizard builds and signs both helpers, records their cdhashes in `~/.config/mailroom/native-pins.env`, builds mailroom with the pins, shows an invented confirmation to approve and one to cancel, migrates legacy items, takes any missing mailbox password through the helper's hidden terminal prompt and checks the prompt at every password read. `bash scripts/verify-setup-macos-protection.sh` drives it against fake tools.

## Verification

```sh
(cd native/macos && swift test)
cargo test --locked
bash scripts/verify-setup-macos-protection.sh
```

The Swift tests and the Rust Keychain tests create a throwaway keychain in a temporary directory and delete it. They never touch the login keychain or the search list. A test cannot click a real dialog, so the visible native prompt, per-access approval and migration of real items remain owner checks.
