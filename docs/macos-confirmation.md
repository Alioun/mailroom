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

Same-user processes can still delete items or plant items whose access list names this binary. Access lists cannot prevent either. The Keychain does not authenticate an item's creator or value. Here is what this means:

- **Deletion.** A deleted item makes the account unavailable. That loses availability, not secrecy.
- **Planted password.** A planted password item is read only after a person approves. Mailroom then logs in with it, which fails against the provider.
- **Planted encryption key, existing state.** A planted key on an account with existing encrypted state does not decrypt that state, so the account refuses to open.
- **Planted identity key.** A rotated account keeps a separate 128-byte identity key that the saved state never checks. Rotation therefore saves an HMAC of the identity bytes, keyed by the account's encryption key, in the account metadata. Every store open recomputes it and refuses identity bytes that do not match. Forging it needs the encryption key, which the saved state authenticates.
- **Planted key, replaced state.** If the attacker also replaces the user-writable account state with ciphertext under a planted encryption key, and writes a matching commitment, mailroom accepts all of it. Later data for that account would then be encrypted under a key the attacker knows. Mailroom cannot detect this, because neither the Keychain nor the account files are outside same-user control.
- **Accounts saved before commitments.** An account with an identity key but no commitment does not open. `mailroom keychain adopt <account>` commits it to the identity bytes found now, after its encryption key opens its saved state. Nothing authenticates those identity bytes, so adoption trusts them as they are, and the owner decides.
- **Moves.** `mailroom keychain settle <account>` gathers every key of one account from wherever it is: current, staged by an interrupted move, trusting an earlier build, or left in the legacy service. It checks the whole set against the account's saved state and commitment, and only then moves each key. A planted legacy, staged or current key is therefore never published. An account with no saved state yet cannot be checked, and its keys are not moved.

A key trusting exactly one other binary is reported as `other_binary`, which is what a rebuilt mailroom sees. Reading it once needs a person. A move from an earlier build goes through a staged copy in `<ref>-retrust`, so a rerun after an interruption gathers the key again from where it was left and finishes.

Legacy items from the earlier `keyring` service `mailroom` are listed by name only. Legacy keys move with their account through `settle`. `migrate` copies a legacy password, then reads the protected copy back and compares it inside the binary. `retire-legacy` deletes an original only when the protected item passes the access check and holds the same value.

## Owner commands

The hardened binary prints only fixed records (`status=…`, `outcome=…`, `ref=… class=…`) and nothing on stderr.

```sh
mailroom keychain refs | legacy
mailroom keychain status | probe | check-store | enroll | migrate | retire-legacy <ref>
mailroom keychain settle | adopt <account>
mailroom native-check
```

Options are parsed before any backend opens. A missing value, a repeated option or an unknown option prints `status=usage`. `enroll` reads the password twice from the controlling terminal with echo off and never from a pipe.

`--keychain <file>` names an explicit keychain for tests. It must be an existing file with one link, owned by the user, in a directory nobody else can write. Every directory above it must belong to the user or root, without group or other write and without an ACL entry that grants anything. It must lie outside the system and user keychain folders and off the search list. Mailroom compares its device, inode and link count, read from an open descriptor, before and after Security opens it. Search-list errors and search-list paths that do not resolve refuse, and nothing ever falls back to the default keychain. Security opens keychains by path, at open and again at later use. These checks therefore bind a path, not a handle: a process running as the user can still replace the file afterwards. The option guards tests against reaching the login keychain by mistake. It is not a boundary against code running as the user.

## Owner setup

Setup has a freeze step and three phases. The owner types every command from the approval record.

1. **Freeze.** The approval record names a commit and a tree digest: the SHA-256 of the sorted per-file SHA-256 list of `git archive <commit>`, without `ui/dist`. The block below runs only system tools. Git runs as the owner with a cleared environment and `--no-replace-objects`, so replacement objects cannot swap the archived bytes. Root then checks that `/usr/local/share/mailroom` and every directory above it is a root-owned directory without group or other write and without ACLs, creating it if absent. It extracts into a fresh `mktemp -d` directory there, without owners, ACLs, extended attributes or flags from the archive. It refuses anything but plain files and directories, compares the digest with `/usr/bin/shasum` before anything from the tree runs, and publishes by rename to `setup-<commit>`, which must not exist yet. Any refusal removes the fresh directory.
2. **Build, as the owner.** Owner decision Q28, answered on 2026-10-27, reads: "The owner explicitly accepts, for this build step only, that any program running as the owner, agents included, could alter the binaries or their build inputs before they are signed and pinned. This is a deliberate waiver of the secrets.md frozen-copy requirement for the build step; it is not a waiver for anything else." The build is therefore not protected. It runs the Homebrew Rust and npm, Node and Xcode toolchains as installed, from a copy of the frozen tree in a fresh `/private/tmp/mailroom-build.*` directory, with a cleared environment, a fresh `CARGO_HOME` and npm cache, and npm configuration set to `/dev/null`. It signs both binaries ad hoc with the hardened runtime and prints the mailroom cdhash, the helper cdhash, the UI digest and the install command.
3. **Install, as root.** Nothing from the build directory runs as root. `tar` copies the three outputs into a fresh root-only staging directory without following links or reading special files. The install refuses anything but plain files and directories, and refuses copies whose `codesign --verify` cdhash or UI digest differs from the values the owner typed from the build output. It writes `pins` as a new file and renames the directory into `/usr/local/libexec/mailroom`. An existing installation is moved aside, never written into or followed.
4. **Keychain stages, as the owner.** The phase refuses an installation that is not root-owned, has ACLs, symlinks or special files, or does not match its pins. It verifies `mailroom` with `codesign` against its cdhash before every launch and accepts from each command exactly one known result with its expected exit code. It reads the expected references from the account metadata with `plutil`. `refs` must list exactly those, `legacy` names outside them are dropped and counted, and a `status` class must be the reference's own. It settles each account, migrates legacy passwords, enrolls missing passwords and checks the prompt at every password read.

The ACL scans capture each listing whole and match it in bash. A listing or scan error refuses.

<!-- freeze-block -->
```sh
COMMIT=<approved commit from the approval record>
DIGEST=<approved tree digest from the approval record>
/usr/bin/env -i /usr/bin/git --no-replace-objects -C "$HOME/projects/mailroom" archive --format=tar "$COMMIT" \
| /usr/bin/sudo /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p -c '
set -eu
commit=$1 digest=$2 share=/usr/local/share/mailroom me=$(/usr/bin/id -u)
refuse() { echo "refused: $1" >&2; exit 1; }
trusted() {
  [ -d "$1" ] && [ ! -L "$1" ] || return 1
  stat=$(/usr/bin/stat -f "%u %Lp" "$1") || return 1
  case "${stat%% *}" in 0|"$me") ;; *) return 1 ;; esac
  [ $(( 8#${stat##* } & 8#022 )) -eq 0 ] || return 1
  [ "$(/bin/ls -lde "$1" 2>/dev/null | /usr/bin/wc -l)" -eq 1 ]
}
dir=$(/usr/bin/dirname "$share")
while :; do trusted "$dir" || refuse "$dir"; [ "$dir" = / ] && break; dir=$(/usr/bin/dirname "$dir"); done
[ -e "$share" ] || [ -L "$share" ] || /bin/mkdir -m 755 "$share"
trusted "$share" || refuse "$share"
final="$share/setup-$commit"
[ ! -e "$final" ] && [ ! -L "$final" ] || refuse "$final exists"
fresh=$(/usr/bin/mktemp -d "$share/.incoming.XXXXXX")
trap "/bin/rm -rf \"$fresh\"" EXIT
/usr/bin/tar -x -f - -C "$fresh" --no-same-owner --no-acls --no-xattrs --no-fflags --no-mac-metadata
/bin/chmod -R go-w "$fresh"
[ -z "$(/usr/bin/find "$fresh" ! -type f ! -type d -print)" ] || refuse "not plain files"
actual=$(cd "$fresh" && LC_ALL=C /usr/bin/find . -type f ! -path "./ui/dist/*" -print0 | LC_ALL=C /usr/bin/sort -z \
  | /usr/bin/xargs -0 /usr/bin/shasum -a 256 | /usr/bin/shasum -a 256 | /usr/bin/cut -c1-64)
[ "$actual" = "$digest" ] || refuse "digest"
/bin/chmod 755 "$fresh"
/bin/mv "$fresh" "$final"
echo "frozen $final"
' freeze "$COMMIT" "$DIGEST"
```

Then the three phases. The build prints the exact install command with its output directory.

```sh
F=/usr/local/share/mailroom/setup-$COMMIT/scripts/setup-macos-protection.sh
/usr/bin/env -i HOME="$HOME" TERM="$TERM" PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$F" build
/usr/bin/sudo /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$F" install <output directory the build printed>
/usr/bin/env -i HOME="$HOME" TERM="$TERM" PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$F"
```

Daily use runs `/usr/local/libexec/mailroom/mailroom`, which serves the root-owned UI copy installed beside it. Each phase stops with "Setup incomplete" on any unexpected result and keeps fixed records.

## Verification

```sh
(cd native/macos && swift test)
cargo test --locked
bash scripts/verify-setup-macos-protection.sh
bash scripts/verify-setup-real-build.sh
```

The Rust tests run the Keychain code in process, and the real owner commands of a hardened signed copy, against throwaway keychains they create and delete. They never touch the login keychain. The wizard test drives every phase's stage logic with fake tools in a tree owned by the test user, and runs the freeze block from this page without sudo. The real-build test runs the build phase with the installed toolchains and the install phase into a temporary prefix, both as the test user. A test cannot click a real dialog or run sudo. The visible prompts, per-access approval, reading an earlier build's keys, migration of real items and the sudo steps remain owner checks.

## Residual exposure the owner accepts

- **The build (Q28).** The owner builds as the owner, under the waiver quoted in Owner setup. A program running as the owner can change the sources copied for the build, the toolchains or the outputs before they are signed. The install phase proves only that the installed bytes are the ones the build phase signed and printed.
- **Package install scripts.** npm runs install scripts from lock-pinned packages during the build.
- **Adopted identities.** `adopt` trusts an uncommitted identity key as it is when the owner runs it.
- **Planted key with replaced state.** Described under Keychain items.
- **Framework copies.** Security framework, Foundation and CryptoKit keep copies of values they handle, which mailroom cannot wipe.
- **Decoder and pipe copies.** The Swift JSON decoder's intermediate copies and kernel pipe buffers hold the one-exchange confirmation challenge. The helper wipes the envelope buffers it owns. Mailroom wipes its own read and enrollment buffers, and its writes borrow the caller's buffer.
- **Mailbox-change dialogs** show the operation and the exact plan receipt, not every folder or message. The person relies on the browser preview for those targets.
