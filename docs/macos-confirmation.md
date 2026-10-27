# Native confirmation and protected Keychain credentials on macOS

The mailroom binary itself holds Keychain access. A small Swift helper in `native/macos`, `mailroom-confirm`, shows one exact action and returns a signed answer. Mailroom launches it only when the running process has the cdhash pinned into the mailroom binary at build time. Both binaries are ad-hoc signed with the hardened runtime and installed root-owned.

## Exact-action requests

Invoice sends, unsubscribe sends, subscription actions, recovery, and the consequential mailbox changes (folder creation, Sieve upload, activation, removal and compensation, backlog moves and Gmail label changes, and their undo) all pass through one confirmation boundary. The server derives each request from its own plan. A request names the account, purpose, target, sending and receiving identities, revisions and content digest. Its public receipt is a random single-use identity valid for five minutes. Mailbox changes name the operation and the exact preview receipt, which is a keyed hash of the full previewed plan, so the native dialog shows that receipt rather than every folder or message.

After a person approves, the boundary checks the answer's HMAC under a fresh 32-byte challenge, reloads state and recomputes the plan. Only an unchanged plan yields the internal consent capability. A cancelled, failed, malformed, unauthenticated, replayed, expired or changed confirmation denies. Intent is saved before an effect and observation afterwards. An interrupted send stays uncertain until reconciled. A native yes never authorizes a retry.

The synthetic fixture server approves in process and labels its requests `synthetic`. Production builds use the native helper. A build without a pinned helper issues no request. Production provider writes remain denied before any helper access, independently of confirmation.

## Helper launch

macOS refuses to execute an already-open descriptor (`/dev/fd/N` returns EACCES), so mailroom checks the started process instead. It rejects symlinks, non-regular files, and files or parent directories that other users can write. It starts the helper suspended with a fixed argv, an empty environment, stdin and stdout pipes, `/dev/null` as stderr and every other descriptor closed. It reads the kernel's cdhash and code-signing flags for that process and resumes it only when the cdhash matches the pin and the hardened runtime, kill and hard flags are set. The answer must carry an HMAC under a challenge sent only on the helper's stdin. Any process may start the helper and see a dialog, but no answer it obtains is accepted by mailroom. The Developer ID requirement is a typed slot (`Pin::DeveloperIdRequirement`) that this build refuses.

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

Same-user processes can still delete items or plant items whose access list names this binary. Access lists cannot prevent either. The item's creator and value are not authenticated by the Keychain. Here is what this means:

- **Deletion.** A deleted item makes the account unavailable. That loses availability, not secrecy.
- **Planted password.** A planted password item is read only after a person approves. Mailroom then logs in with it, which fails against the provider.
- **Planted key, existing state.** A planted key on an account with existing encrypted state does not decrypt that state, so the account refuses to open.
- **Planted key, replaced state.** If the attacker also replaces the user-writable account state with ciphertext under the planted key, mailroom accepts both. Later data for that account would then be encrypted under a key the attacker knows. Mailroom cannot detect this, because neither the Keychain nor the account files are outside same-user control.
- **Moves and copies.** `migrate` and `retrust` move or copy a key only after it opens the owning account's existing authenticated state (`check-store`). A planted legacy or staged key is therefore never published. An account with no saved state yet cannot be checked, and its keys are not moved.

A key trusting exactly one other binary is reported as `other_binary`, which is what a rebuilt mailroom sees. `mailroom keychain retrust <ref>` moves it after the person allows one read and the store check passes. It stages a copy in `<ref>-retrust`, so a rerun after an interruption finishes the move or cleans a leftover copy.

Legacy items from the earlier `keyring` service `mailroom` are listed by name only. `migrate` copies one, then reads the protected copy back and compares it inside the binary. `retire-legacy` deletes an original only when the protected item passes the access check and holds the same value.

## Owner commands

The hardened binary prints only fixed records (`status=…`, `outcome=…`, `ref=… class=…`) and nothing on stderr.

```sh
mailroom keychain refs | legacy
mailroom keychain status | probe | check-store | enroll | migrate | retire-legacy | retrust <ref>
mailroom native-check
```

Options are parsed before any backend opens. A missing value, a repeated option or an unknown option prints `status=usage`. `enroll` reads the password twice from the controlling terminal with echo off and never from a pipe.

`--keychain <file>` names an explicit keychain for tests. It must be an existing file owned by the user in a directory nobody else can write, outside the system and user keychain folders, and off the search list. The opened keychain must resolve to the same file. Search-list errors refuse, and nothing ever falls back to the default keychain.

## Owner setup

The chain of trust runs from the approval record to the installed bytes:

1. The approval record names a commit and a tree digest. The owner freezes that commit into a root-owned copy with commands typed from the record. ACLs are stripped, and nothing in the checkout is trusted.
2. The owner starts the root phase with an absolute command:
   - `env -i` clears the environment, and `/bin/bash -p` ignores `BASH_ENV` and imported functions.
   - The script refuses any other exported variable or PATH.
   - It refuses a copy that is not root-owned at every level, has an ACL or symlink, or has another digest.
3. The root phase builds as root:
   - The build runs from a copy of the frozen tree in a fresh directory under `/var/root`, which no same-user process can write.
   - It uses an allowlisted environment: a fresh `HOME` and `CARGO_HOME`, npm user and global config set to `/dev/null`, and fixed tool paths.
   - It refuses `.cargo`, `.npmrc` or `.config` above the build directory.
   - Crates come from `cargo fetch --locked`, verified against `Cargo.lock` checksums. npm packages come from `npm ci`, verified against `package-lock.json` integrity hashes.
4. It signs the outputs with the hardened runtime and installs them into a freshly created root-owned directory. An existing installation is moved aside, never written into or followed. The new directory is then renamed into place, and `pins` is a new file in it.
5. The owner phase runs as the owner with the same kind of launch command. It refuses an installation that is not root-owned, has ACLs or symlinks, or does not match its pins. It verifies `mailroom` with `codesign` against its cdhash before every launch. It accepts from each command exactly one known result with its expected exit code.

```sh
COMMIT=<approved commit from the approval record>
sudo /bin/mkdir -p /usr/local/share/mailroom/setup-$COMMIT
/usr/bin/git -C ~/projects/mailroom archive --format=tar "$COMMIT" | sudo /usr/bin/tar -x -C /usr/local/share/mailroom/setup-$COMMIT
sudo /usr/sbin/chown -R root:wheel /usr/local/share/mailroom
sudo /bin/chmod -R -N /usr/local/share/mailroom
sudo /bin/chmod -R go-w /usr/local/share/mailroom
sudo /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p /usr/local/share/mailroom/setup-$COMMIT/scripts/setup-macos-protection.sh install "$USER"
/usr/bin/env -i HOME="$HOME" TERM="$TERM" PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p /usr/local/share/mailroom/setup-$COMMIT/scripts/setup-macos-protection.sh
```

Daily use runs `/usr/local/libexec/mailroom/mailroom`, which serves the root-owned UI copy installed beside it. Each phase stops with "Setup incomplete" on any unexpected result and keeps fixed records.

## Verification

```sh
(cd native/macos && swift test)
cargo test --locked
bash scripts/verify-setup-macos-protection.sh
```

The Rust tests run the Keychain code in process, and the real owner commands of a hardened signed copy, against throwaway keychains they create and delete. They never touch the login keychain. The wizard test drives both phases' stage logic with fake tools in a tree owned by the test user. A test cannot click a real dialog or run sudo, so the visible prompts, per-access approval, retrust, migration of real items and the root build remain owner checks.

## Residual exposure the owner accepts

- **Toolchains.** The root phase shows the SHA-256 of each tool and asks the owner to accept them. These are the Homebrew Rust toolchain, `node`, npm's CLI, Xcode through `xcode-select` and `codesign`. Homebrew's files are writable by the owner account, so a same-user process could have altered them before the build. Running them as root during the build does not make them more trustworthy than they were.
- **Package install scripts.** npm runs install scripts from lock-pinned packages.
- **Framework copies.** Security framework, Foundation and CryptoKit keep copies of values they handle, which mailroom cannot wipe.
- **Decoder and pipe copies.** The Swift JSON decoder's intermediate copies and kernel pipe buffers hold the one-exchange confirmation challenge. The helper wipes the envelope buffers it owns. Mailroom wipes its own read and enrollment buffers, and its writes borrow the caller's buffer.
- **Mailbox-change dialogs** show the operation and the exact plan receipt, not every folder or message. The person relies on the browser preview for those targets.
