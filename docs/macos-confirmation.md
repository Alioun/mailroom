# Native confirmation and protected Keychain credentials on macOS

This page covers the native setup. The browser-only [local web setup](local-web.md) keeps its credentials in an encrypted vault instead and has its own threat model.

The mailroom binary itself holds Keychain access. A small Swift helper in `native/macos`, `mailroom-confirm`, shows one exact action and returns a signed answer. Mailroom launches it only when the running process has the cdhash pinned into the mailroom binary at build time. Both binaries are ad-hoc signed with the hardened runtime and published, owned by the owner, in the checkout's `build/local/<commit>`.

## Exact-action requests

Invoice sends, unsubscribe sends, subscription actions, recovery, and the consequential mailbox changes (folder creation, Sieve upload, activation, removal and compensation, backlog moves and Gmail label changes, and their undo) all pass through one confirmation boundary. The server derives each request from its own plan. A request names the account, purpose, target, sending and receiving identities, revisions and content digest. Its public receipt is a random single-use identity valid for five minutes. Mailbox changes name the operation and the exact preview receipt, which is a keyed hash of the full previewed plan, so the native dialog shows that receipt rather than every folder or message.

After a person approves, the boundary checks the answer's HMAC under a fresh 32-byte challenge, reloads state and recomputes the plan. Only an unchanged plan yields the internal consent capability. A cancelled, failed, malformed, unauthenticated, replayed, expired or changed confirmation denies. Intent is saved before an effect and observation afterwards. An interrupted send stays uncertain until reconciled. A native yes never authorizes a retry.

The synthetic fixture server approves in process and labels its requests `synthetic`. Production builds use the native helper. A build without a pinned helper issues no request. Production provider writes remain denied before any helper access, independently of confirmation.

## Local account setup

The owner wizard can add one account or replace one saved password through the runtime's mailroom. Both commands require disabled core dumps, a hardened running main image and a supported compiled helper pin before resolving private paths. They accept no alternate config or Keychain path. The login and password enter through the controlling terminal with echo off. Password confirmation uses constant-time comparison. No credential enters argv, a browser request, stdout or stderr.

The native dialog names the exact local operation, account, provider and login. A fresh private HMAC key binds the immutable metadata, saved revision and exact password without exposing a reusable password digest. Consent runs inside the existing transaction lease, before any lifecycle intent or credential write. Duplicate provider/login creation is refused under the installation lease. Creation publishes metadata last. Replacement keeps the encrypted history and local key, selects a new password reference before retiring the old one, and preserves the restored-account hold. Both leave connectivity unverified and make no provider connection.

A refused consent prints the same code `native-check` prints, as `status=denied_<check>` or `status=unavailable`. A Keychain failure, after consent for an addition or before it for a replacement, prints `status=keychain_failed`. Before this split both printed `status=denied`.

Setup creates mode-0700 directories relative to checked parent descriptors. It rejects symlinks, granting ACLs, unsafe ownership and writable shared parents. An existing final directory with broader permissions is refused, not silently repaired. The credential-free checks exercise injected input, native replies and memory credentials. Actual hidden terminal entry, Keychain dialogs and the runtime's helper invocation still need approved owner-run acceptance. Changing this code or its wizard requires fresh secret-flow review and exact approval before real use.

## Local maintenance

The setup wizard has an optional `maintenance` subcommand. It first checks the runtime against its pins. It then offers reconciliation of recorded local lifecycle operations, key rotation and local forgetting. Each consequential operation gets exact native confirmation. The binary accepts only an operation and account ID on argv. It chooses fixed private local directories and prints one fixed result record. It reads the selected protected mailbox credential only when reconciling a published password replacement, to verify it before retiring the previous reference. It never contacts a provider.

Forgetting derives its resource preview while the account lease and every dependent store lock remain held. The native request binds that exact preview and names the local deletion consequences. Rotation binds the current account, revision and resource manifest before any new key or lifecycle intent is written. Local recovery binds the validated recorded intent before cleanup or key retirement. It describes the pending phase, including continued deletion when applicable. No pending intent means no confirmation or effect. An incomplete recovery-bundle restore blocks these lifecycle actions.

Active account work keeps its account lease, so the separate owner command refuses competing work. It cannot cancel or join another process's worker. Stop active work first and reopen the app after maintenance. Production HTTP lifecycle stays closed. These controls require exact approval and owner-run verification of the runtime before real use.

## Owner-held encrypted recovery

The optional `recovery` subcommand prepares private transfer directories, selects up to eight accounts, and offers capture, absent-account restoration or interrupted local restoration. Ledger inclusion and restoration each start disabled. The script receives account IDs, a bundle ID and fixed result codes. The runtime's mailroom reads the separate random 256-bit recovery key from the controlling terminal with echo disabled. Capture requires the same key twice. Encoded and decoded key buffers have erasing owners. The key never travels through argv, a browser request or result output.

Native consent names each account, capture time, schema, generation, revision and resource count. Shared exclusions appear once. Restore and resume explicitly say whether the installed ledger is preserved or the bundled ledger is restored. The immutable receipt binds the complete source plan and entered key using a fresh private HMAC key. Capture and restoration recompute under their transaction leases. Resume obtains new consent under installation and account leases before continuing recorded local effects.

Completed captures appear at `~/.config/mailroom/recovery-transfer/outgoing/<bundle-id>.bundle`. The owner keeps the encrypted bundle outside this Mac and its key separately. Imports use the fixed `incoming` sibling directory. The binary rejects symlinks, nonregular files, extra hard links, other owners, granting ACLs and permissions other than 0600. It retains the checked descriptor through preview, restore and durable staging. Renaming or replacing the import pathname does not select a different input. Authentication and exact bundle identity remain required. The restore target may be absent or identical, and differing owner ledger bytes reject.

Provider passwords and uncaptured changes remain excluded. Restoration keeps accounts read-only and cannot resume sends, filing, unsubscribe or other provider actions. Production HTTP recovery stays denied. These additions require passing dummy checks, a separate secret-flow review, exact owner approval and owner-run acceptance of the runtime before real use. No current execution approval is implied by this reference.

On a fresh installation, recovery records an uninitialized reservation in the stable private ledger lock. Normal startup can then create its empty disabled ledger after a ledger-excluded restore or a failed recovery action. Initialization permission is cleared and synchronized before either startup or restoration publishes authoritative YAML. A missing initialized ledger is never recreated automatically. Legacy empty locks retain that established meaning, and incomplete or unknown reservation records reject. This correction preserves the lock inode and grants no provider authority.

## Helper launch

macOS refuses to execute an already-open descriptor (`/dev/fd/N` returns EACCES), so mailroom checks the started process instead. It rejects symlinks, non-regular files, and files or parent directories that other users can write. The file and its parent may belong to the owner or to root. It starts the helper suspended with a fixed argv, an empty environment, stdin and stdout pipes, `/dev/null` as stderr and every other descriptor closed. It reads the kernel's cdhash and code-signing flags for that process and resumes it only when the cdhash matches the pin and the hardened runtime, kill and hard flags are set. The answer must carry an HMAC under a challenge sent only on the helper's stdin. Any process may start the helper and see a dialog, but no answer it obtains is accepted by mailroom. The Developer ID requirement is a typed slot (`Pin::DeveloperIdRequirement`) that this build refuses.

## Keychain items

Items use the generic-password service `mailroom-protected-v1` and legacy access lists, because keychain access groups need an entitlement that ad-hoc signing cannot carry.

- Mailbox passwords trust no application, so macOS asks the person at every read.
- Census and identity keys trust exactly this binary's cdhash, because every store open reads them.

Before every read, mailroom reads the item's access-list entries and applies one policy, a pure function tested on every edited shape.
- Encryption-only entries are ignored.
- Every other entry must keep the prompt selector the item was created with.
- The change-ACL entry must trust nobody.
- The reading entries must agree with one another.
- For a password, the reading entries must trust no application. For a key, they must trust exactly `cdhash H"<own cdhash>"`, checked against the kernel's cdhash of the running process.

Anything else is refused without reading: an "Always Allow" grant, an added application, several applications, any application allowed, or an edited prompt selector. Changing an access list needs a person, because `SecKeychainItemSetAccess` waits for the system dialog. Production use also requires the hardened runtime, which blocks debugger attachment and injected libraries, and a zero core-file limit, which `main` sets and verifies first.

Same-user processes can still delete items or plant items whose access list names this binary. Access lists cannot prevent either. The Keychain does not authenticate an item's creator or value. Here is what this means:

- **Deletion.** A deleted item makes the account unavailable. That loses availability, not secrecy.
- **Planted password.** A planted password item is read only after a person approves. Mailroom then logs in with it, which fails against the provider.
- **Planted encryption key, existing state.** A planted key on an account with existing encrypted state does not decrypt that state, so the account refuses to open.
- **Planted identity key.** A rotated account keeps a separate 128-byte identity key that the saved state never checks. Rotation therefore saves an HMAC of the identity bytes, keyed by the account's encryption key, in the account metadata. Every store open recomputes it and refuses identity bytes that do not match. Forging it needs the encryption key, which the saved state authenticates.
- **Planted key, replaced state.** If the attacker also replaces the user-writable account state with ciphertext under a planted encryption key, and writes a matching commitment, mailroom accepts all of it. Later data for that account would then be encrypted under a key the attacker knows. Mailroom cannot detect this, because neither the Keychain nor the account files are outside same-user control.
- **Accounts saved before commitments.** An account with an identity key but no commitment does not open. `mailroom keychain adopt <account>` commits it to the identity bytes found now, after its encryption key opens its saved state. Nothing authenticates those identity bytes, so adoption trusts them as they are, and the owner decides. A recovery bundle of such an account is refused before any key is imported, and so is a bundle whose identity does not match its commitment; the bundle is left as it is. An interrupted rotation never uses a next-generation key that nothing authenticates. A rotation refuses to start if its next key, a first identity or any next-generation file already exists. While the metadata still selects the earlier generation, the earlier data and key are intact, so recovery rolls the rotation back, deleting only keys it proves it created: the next key when it reproduces the commitment recorded before it was created, and a first identity equal to the bytes derived from the earlier key. Each key deletion reads and removes the same Keychain item. A planted key, or a key from a rotation recorded before commitments, stays, and a later rotation refuses until it is removed. Rollback never unlinks a file, because a path cannot be bound to the file a check read. Whatever entry holds a recorded store's next-generation name is moved, with an exclusive rename between open directories, into the first free numbered slot of the account's rollback folder in that store directory. A destination already taken refuses the rollback and leaves both entries where they are. Once the metadata selects the next generation, the next-generation data already exists and is never re-encrypted; a planted key cannot decrypt it, so recovery refuses and keeps everything. Metadata written before commitments is repaired in one write only when the identity derives from the earlier key, the earlier data still authenticates that key, and the next-generation data equals the earlier data under the next key; the repair commits the identity bytes it derived, never a second read of the identity item. Otherwise recovery refuses, and the intent, keys and files are kept.

**Rollback copies.** Each store directory can hold one rollback folder per account, `.<account>.rollback`, with numbered slots `1` to `8`: `<data>/.<account>.rollback/<n>/<account>.g<generation>.census.enc` for the primary store, and the same under the mailbox, invoice-transport and unsubscribe-transport effect directories. A slot holds what a rotation interrupted before publication had written under that store's next-generation name. For a genuine rotation that is a complete copy of the account's encrypted state for that store, encrypted under the next-generation key. Rollback deletes that key when it can prove the rotation created it, which leaves the copy undecryptable by any key mailroom keeps; deleting a Keychain item does not prove that no other copy of the key exists. When the key is kept, because it could not be proven, the copy stays decryptable with that key. A slot can also hold whatever another process put under that name, including a link, which is moved as a link, or a directory. Mailroom never deletes a rollback copy by itself. The folder goes away only when `forget` removes it with the account, or when the owner deletes it by hand (for example `rm -r <data>/.<account>.rollback`) after deciding that nothing in it is needed. A slot is taken by creating it, and creation refuses an existing name, so mailroom makes at most 8 slots per folder. A rotation refuses to start while slot `8` of any of the account's rollback folders exists, and a rollback that finds every slot taken refuses and keeps the rotation pending; both report how to clear the folder. Anyone running as the owner can create slot `8`, or fill the folder, and so stop key changes of that account until the owner clears it. That is an availability loss like deleting a key, not a disclosure. Entries others put in the folder under other names are kept and do not count. The forget preview lists each rollback folder with its regular-file bytes and a digest of everything in it, read through directory descriptors without following links: every entry's type and name, each file's length and SHA-256, each link's target, and each directory's entries in order, nested ones included. Any other entry type refuses the preview. Forget recomputes the preview and refuses if anything changed, then removes the folder by name with everything in it, as the account's own namespace. Something another process changes inside it after that last comparison is removed as well; the digest binds what the owner confirmed, not the moment of deletion.

- **Moves.** `mailroom keychain settle <account>` gathers every key of one account from wherever it is: current, staged by an interrupted move, trusting an earlier build, or left in the legacy service. It checks the whole set against the account's saved state and commitment before the first key moves, then moves the keys one at a time; a rerun after an interruption finishes the rest. A planted legacy, staged or current key is refused when the account's saved state is unchanged and its identity already has a genuine commitment. Two cases are the exceptions above: an attacker who also replaces the state and writes a matching commitment, and an identity the owner adopts. An account with no saved state yet cannot be checked, and its keys are not moved.

A key trusting exactly one other binary is reported as `other_binary`, which is what a rebuilt mailroom sees. Reading it once needs a person. A move from an earlier build goes through a staged copy in `<ref>-retrust`, so a rerun after an interruption gathers the key again from where it was left and finishes.

Legacy items from the earlier `keyring` service `mailroom` are listed by name only. Legacy keys move with their account through `settle`. `migrate` copies a legacy password, then reads the protected copy back and compares it inside the binary. `retire-legacy` deletes an original only when the protected item passes the access check and holds the same value.

## Owner commands

The hardened binary prints only fixed records (`status=…`, `outcome=…`, `ref=… class=…`) and writes nothing to stderr itself. If writing to stdout fails, the Rust runtime can still print a panic message there; no such message carries a key or password.

```sh
mailroom keychain refs | legacy
mailroom keychain status | probe | check-store | enroll | migrate | retire-legacy <ref>
mailroom keychain settle | adopt <account>
mailroom native-check
mailroom account add gmail | mailbox_org
mailroom account replace-password <account-id>
mailroom account forget | rotate-key | recover-local <account-id>
```

`native-check` prints `outcome=approved` or `outcome=cancelled` with exit 0, and `outcome=unavailable` or a denial with exit 1. A denial names the first check that refused the answer: `denied_failed` (the helper exited non-zero or by a signal), `denied_timeout` (no answer and exit within 300 seconds), `denied_expired` (the answer came more than 300 seconds after the request was issued), `denied_malformed`, `denied_unauthenticated` (wrong request id, digest or tag), `denied_channel` (a pipe or wait error), `denied_oversized`, `denied_launch` (the file check, start or identity check failed), or `denied_other` for any other error, whose text is never printed.

Options are parsed before any backend opens. A missing value, a repeated option or an unknown option prints `status=usage`. `enroll` reads the password twice from the controlling terminal with echo off and never from a pipe.

`--keychain <file>` names an explicit keychain for tests. It must be an existing file with one link, owned by the user, in a directory nobody else can write. Every directory above it must belong to the user or root, without group or other write and without an ACL entry that grants anything. It must lie outside the system and user keychain folders and off the search list. Mailroom compares its device, inode and link count, read from an open descriptor, before and after Security opens it. Search-list errors and search-list paths that do not resolve refuse, and nothing ever falls back to the default keychain. Security opens keychains by path, at open and again at later use. These checks therefore bind a path, not a handle: a process running as the user can still replace the file afterwards. The option guards tests against reaching the login keychain by mistake. It is not a boundary against code running as the user.

## Owner setup

The owner runs one command from the checkout. Nothing is installed. There is no `sudo`, no root step and no write outside the checkout's ignored `build/` directory, the owner's `~/.config/mailroom` and a temporary build directory. The owner answers only what a person must answer: the native dialogs, hidden password entry and yes or no questions.

```sh
cd ~/projects/mailroom
scripts/setup.sh        # first run, and after each new commit
scripts/setup.sh run    # daily: start the local server
```

Owner decision Q30, answered on 2026-11-04 with "Waive the freeze", waives the secrets.md frozen-copy requirement for this setup wizard. The decision record reads: "The owner waives the secrets.md frozen-copy requirement for the personal mailroom setup wizard, as Q28 did for the build step. The wizard runs straight from the checkout. The owner accepts that any program running as the owner, agents included, could change the wizard before it runs." The consequence is that nothing protects the wizard itself. A program running as the owner can change `scripts/setup.sh` or `scripts/setup-macos-protection.sh` in the checkout, and the changed wizard then runs as the owner. The runtime checks below bind the runtime to what a run built, but they cannot show that the wizard or the build was untouched. Q30 does not waive the credential-free tests, the separate secret-flow review, owner approval of the exact resulting commit, the pins, the cdhash checks before every launch, fixed-record output, or the rule that no secret enters argv or output.

`scripts/setup.sh` starts the wizard as `/usr/bin/env -i HOME="$HOME" TERM="$TERM" PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p scripts/setup-macos-protection.sh`. The wizard refuses any other environment, PATH or missing `-p` before its first stage, and refuses to run as root. One run has two parts:

1. **The runtime, as the owner.** Before anything builds, the wizard refuses a checkout with any uncommitted change, tracked or untracked, outside `docs/`, top-level Markdown files and `.DS_Store`. That set covers what the build reads: `src/`, `ui/`, `native/`, `scripts/`, the Cargo files, `.cargo`, and the `agent/`, `data/`, `examples/` and `tests/` files that `src/` embeds. It then shows the full HEAD commit and asks "Use this commit?". The owner compares it with the approved commit, and the default answer declines. When `build/local/<commit>` already holds a runtime that verifies, as described below, the wizard reuses it and builds nothing. A runtime there that does not verify stops the wizard, which names the directory for the owner to delete. Otherwise the build runs under owner decision Q28, answered on 2026-10-27: "The owner explicitly accepts, for this build step only, that any program running as the owner, agents included, could alter the binaries or their build inputs before they are signed and pinned. This is a deliberate waiver of the secrets.md frozen-copy requirement for the build step; it is not a waiver for anything else." The build extracts `git archive` of that commit, with `--no-replace-objects`, into a fresh `/private/tmp/mailroom-build.*` directory. It runs the Homebrew Rust and npm, Node and Xcode toolchains as installed, with a cleared environment, a fresh `CARGO_HOME` and npm cache, and fresh empty npm user and global configuration files. It compiles the helper path `build/local/<commit>/mailroom-confirm`, the helper's cdhash and the UI path `build/local/<commit>/ui` into mailroom, and signs both binaries ad hoc with the hardened runtime. `tar` then copies the three outputs into a fresh `build/local/.staging.*` directory without following links or reading special files. Publishing refuses anything but plain files and directories, and copies whose `codesign` cdhash, hardened runtime or UI digest differs from what the build produced. It writes `pins` as a new file and renames the staging directory to `build/local/<commit>`. An existing runtime is never written into or replaced. The build directory is removed whatever the result.
2. **Keychain stages, as the owner.** The phase checks the runtime and refuses one whose pins differ from the identities this run built. It verifies `mailroom` with `codesign` against its cdhash before every launch, launches it with `env -i` and discards its stderr, and accepts from each command exactly one known result with its expected exit code. It reads the expected references from the account metadata with `plutil`. `refs` must list exactly those, `legacy` names outside them are dropped and counted, and a `status` class must be the reference's own. It optionally creates a local account, settles each account, migrates legacy passwords, enrolls missing passwords, optionally replaces a saved password and checks the prompt at every password read. After adding an account or replacing a password, it reloads the expected references before accepting later command output.

A runtime verifies only whole. Every entry belongs to the owner, nobody else can write it, and it holds no symlink, special file or granting ACL entry. Every directory above it up to `/` belongs to the owner or root, nobody else can write it, and it is not a symlink. Deny-only ACL entries, such as the `group:everyone deny delete` every home folder carries, do not count. `pins` names the commit of the directory, both cdhashes and the UI digest. Both binaries must carry exactly those cdhashes and the hardened runtime, and the UI copy must have that digest. The ACL scans capture each listing whole and match it in bash. A listing or scan error refuses.

The same command takes one optional subcommand. Each checks the runtime of the HEAD commit first and stops with "Run scripts/setup.sh first" when that runtime is missing:

```sh
scripts/setup.sh run          # start the server from the runtime: no prompt, no rebuild
scripts/setup.sh keychain     # the Keychain stages again, without a rebuild
scripts/setup.sh maintenance  # approved local account maintenance
scripts/setup.sh recovery     # approved encrypted recovery
```

`run` starts `build/local/<commit>/mailroom` with only `HOME` in its environment, in place of the wizard. The server serves the UI copy beside it at <http://127.0.0.1:4173>, and Ctrl+C stops it. A new commit needs `scripts/setup.sh` once, which builds that commit's runtime and moves each account's keys to the new binary. Earlier runtimes stay in `build/local` until the owner deletes them. Each phase stops with "Setup incomplete" on any unexpected result and keeps fixed records in `~/.config/mailroom/native-*-records`.

An installation from an earlier commit under `/usr/local/libexec/mailroom`, and its records in `/private/var/root/mailroom-install-records`, are no longer used or changed. Its keys move to the local runtime in the "Move each account's keys from an earlier build" stage, where macOS asks the owner before the new mailroom reads each key. The owner may delete the old installation with `sudo` after that.

## Verification

```sh
(cd native/macos && swift test)
cargo test --locked
bash scripts/verify-setup-macos-protection.sh
python3 -I scripts/verify-account-setup.py
bash scripts/verify-setup-real-build.sh
```

The Rust tests run the Keychain code in process, and the real owner commands of a hardened signed copy, against throwaway keychains they create and delete. They never touch the login keychain. The wizard test drives the one-command setup end to end with fake tools in a checkout owned by the test user. It covers publishing, reuse of a verified runtime, refusal of a changed, re-signed, unhardened, moved or group-writable runtime, granting and deny-only ACLs, `run` and the keychain, maintenance and recovery phases. The real-build test runs the build stage with the installed toolchains on a clone of the HEAD commit and checks the published runtime. A test cannot click a real dialog. The visible prompts, per-access approval, reading an earlier build's keys and migration of real items remain owner checks.

## Residual exposure the owner accepts

- **The wizard (Q30).** The wizard runs from the checkout, under the waiver quoted in Owner setup. A program running as the owner can change it before it runs.
- **The build (Q28).** The owner builds as the owner, under the waiver quoted in Owner setup. A program running as the owner can change the checkout's commits, the toolchains or the outputs before they are signed. Publishing proves only that the runtime holds the bytes this run built and signed.
- **The runtime.** The runtime belongs to the owner, as the root-owned installation did not. A program running as the owner can replace the binaries, the UI copy and the pins together, between runs or between a check and a launch. The checks then pass for the replacement. What still holds is in the binaries and the Keychain. Mailroom starts only a helper with the cdhash compiled into it. Keys trust only the cdhash of the mailroom that created or settled them, so a replaced mailroom reads them only after the owner allows it in a dialog. Every password read asks the owner.
- **Package install scripts.** npm runs install scripts from lock-pinned packages during the build.
- **Adopted identities.** `adopt` trusts an uncommitted identity key as it is when the owner runs it.
- **Planted key with replaced state.** Described under Keychain items.
- **Framework copies.** Security framework, Foundation and CryptoKit keep copies of values they handle, which mailroom cannot wipe.
- **Decoder and pipe copies.** The Swift JSON decoder's intermediate copies and kernel pipe buffers hold the one-exchange confirmation challenge. The helper wipes the envelope buffers it owns. Mailroom wipes its own read and enrollment buffers, and its writes borrow the caller's buffer.
- **Mailbox-change dialogs** show the operation and the exact plan receipt, not every folder or message. The person relies on the browser preview for those targets.
