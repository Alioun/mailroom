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

Before every read, mailroom reads the item's access lists and checks each one that grants more than encryption. Passwords must trust no application. Keys must trust exactly the requirement `cdhash H"<own cdhash>"`, checked against the kernel's cdhash of the running process. Anything else is refused without reading: an "Always Allow" grant, an added application, several applications, any application allowed, or a change list that trusts someone. Changing an access list needs a person, because `SecKeychainItemSetAccess` waits for the system dialog. Production use additionally requires the hardened runtime, which blocks debugger attachment and injected libraries, and a zero core-file limit, which `main` sets and verifies first.

Same-user processes can still delete items or create look-alike items. Access lists cannot prevent that. A deleted item fails closed. A look-alike key cannot decrypt existing stores. Items are created, never overwritten.

A key trusting exactly one other binary is reported as `other_binary`, which is what a rebuilt mailroom sees. `mailroom keychain retrust <ref>` moves it after the person allows one read. It stages a copy in `<ref>-retrust`, so a rerun after an interruption finishes the move.

Legacy items from the earlier `keyring` service `mailroom` are listed by name only. `migrate` copies one, then reads the protected copy back and compares it inside the binary. `retire-legacy` deletes an original only when the protected item passes the access check and holds the same value.

## Owner commands

The hardened binary prints only fixed records (`status=…`, `outcome=…`, `ref=… class=…`) and nothing on stderr.

```sh
mailroom keychain refs | legacy
mailroom keychain status | probe | enroll | migrate | retire-legacy | retrust <ref>
mailroom native-check
```

`enroll` reads the password twice from the controlling terminal with echo off and never from a pipe. `--keychain <file>` names an explicit keychain for tests. It must exist, and it may not be the login keychain, a system keychain or any keychain on the search list.

## Owner setup

The owner runs setup only from a root-owned copy of the approved commit:

```sh
COMMIT=<approved commit from the approval record>
sudo mkdir -p /usr/local/share/mailroom/setup-$COMMIT
git -C ~/projects/mailroom archive --format=tar "$COMMIT" | sudo tar -x -C /usr/local/share/mailroom/setup-$COMMIT
sudo chown -R root:wheel /usr/local/share/mailroom
sudo chmod -R go-w /usr/local/share/mailroom
/usr/local/share/mailroom/setup-$COMMIT/scripts/setup-macos-protection.sh
```

The wizard refuses a copy that is not root-owned at every level or whose tree digest differs from the one the owner types from the approval record. It builds in a private temporary directory and installs `mailroom` and `mailroom-confirm` root-owned in `/usr/local/libexec/mailroom`. It runs them only after `codesign --verify` against their exact cdhash, and it reduces their output to fixed records. Each stage must report its expected result, or the wizard stops with "Setup incomplete" and keeps its records in `~/.config/mailroom/native-setup-records`. Daily use runs `/usr/local/libexec/mailroom/mailroom`.

## Verification

```sh
(cd native/macos && swift test)
cargo test --locked
bash scripts/verify-setup-macos-protection.sh
```

The Rust tests run the Keychain code in process, and the real owner commands of a hardened signed copy, against throwaway keychains they create and delete. They never touch the login keychain. A test cannot click a real dialog, so the visible prompts, per-access approval, retrust and migration of real items remain owner checks.

## Residual exposure

- Security framework and Foundation keep their own copies of values they read or store, which mailroom cannot wipe.
- So do the Swift JSON decoder and kernel pipe buffers for the confirmation challenge.
- Mailroom wipes its own buffers.
- The build toolchains (Xcode, cargo, npm) are user-writable and trusted as installed.
