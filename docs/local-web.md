# Local web setup

The local web setup runs Mailroom on your own computer from one command. Creating the vault, unlocking it, adding accounts and the census then all happen in the browser at `http://127.0.0.1:4173`. Nothing is installed system-wide: no `sudo`, no launch agent, no global package, no Keychain entry and no native dialog.

## Start it

The first start of a version builds that exact commit once. Running this command, with the full 40-character commit, is the owner's approval of that version:

```sh
/usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin /usr/bin/git --no-replace-objects -C <checkout> show <commit>:scripts/local-web.sh |
  /usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin /bin/bash -s -- approve <checkout> <commit>
```

The script that runs comes from the commit, never from the working tree. The command ignores Git replacement objects and clears the environment before either Git or Bash starts. It exports the commit with `git archive`, builds the UI and the release binary in a cleared environment with a private home, cargo home and npm cache under `/private/tmp`, and refuses to build below any `.cargo` directory. It publishes the result read-only to `~/Library/Application Support/Mailroom Web Runtime/versions/<commit>` with one rename. It pins the binary, the UI tree and the launcher by SHA-256, installs the launcher as `start` and starts the server.

Every later start is one command, with no build, install or checkout access:

```sh
"$HOME/Library/Application Support/Mailroom Web Runtime/start"
```

It checks that the runtime belongs to you, that nobody else can write to it and that its entries match their pins, then starts the binary with only `HOME` and `PATH`. A mismatch prints `status=changed` and starts nothing. Approving a commit that is already built verifies it and skips the build. In a terminal, the start opens the browser once the server answers.

The build uses the toolchain the native setup uses, at fixed paths: Homebrew `cargo` and `rustc`, `/usr/local/bin/node` and Homebrew `npm`. A missing tool prints `status=toolchain_missing`. The first build downloads crates and npm packages into its private caches.

## The vault

Account passwords and census keys live in one encrypted file, `~/.config/mailroom-web/vault.mrv`. The browser asks for a master password of at least 12 characters, typed twice, and shows that a forgotten master password cannot be recovered. After every server start, the browser asks for the master password once. Until then, every account route answers `423` without reading account files or credentials.

The format is fixed by its version: Argon2id v0x13 with 64 MiB, 3 passes and 4 lanes derives the AES-256-GCM key from the password and a random salt. The header is authenticated. Every write uses a fresh random nonce and a higher generation, is synced to disk and replaces the file with one rename. Bounds are checked before any key derivation, so a damaged or oversized file costs nothing to refuse. A wrong password and a modified file give the same refusal. Mailroom refuses, and never repairs, a vault file that is a link, has another owner, more than one link, or permissions beyond `0600`, and a root directory that others can write.

One server owns the vault for its whole life. A second server stops with `status=vault_in_use` before it binds the port. Before changing credentials, Mailroom checks whether another program replaced the vault file. If so, it forgets the key and asks for the master password again. Reads use the unlocked copy in memory and do not detect replacement on their own.

## Accounts

The page adds an account from its provider, login and app password, saves it in the vault and runs a read-only IMAP folder listing. A refused sign-in, a network failure and a duplicate account each show their own message. The page offers to retry the connection or to replace the app password, which uses the existing transactional password replacement. An account setup interrupted by a crash shows a button that finishes or removes exactly that setup.

Web accounts live in `~/.config/mailroom-web` and `~/Library/Application Support/mailroom-web`. Accounts of the native setup in `~/.config/mailroom` and the Keychain stay where they are, unread and unchanged. Nothing migrates between the two.

Provider writes stay denied by the same policy as before. The web setup grants no generic lifecycle actions, no recovery bundles and no synthetic consent.

## Limits

- **Same-user programs.** While the vault is unlocked, any program running under your user account can fetch the local session token and use the read routes, or try to read process memory. Browser origin checks stop other websites, not malware on the computer. The native setup's per-action dialogs have no equivalent here.
- **Offline guessing.** A copy of `vault.mrv` can be attacked offline. The Argon2id cost and the strength of the master password are the only defenses.
- **Secret copies.** Mailroom keeps keys and decrypted entries in erasing buffers and returns only fixed codes. The browser, the HTTP stack and the kernel still hold copies of a password while it travels, and memory can reach swap.
- **The runtime folder.** The pins detect a changed runtime at the next start. Whether an agent sandbox on this computer can write to the runtime folder has not been verified, so no claim is made that it cannot.
- **Before real use**, the exact commit needs passing credential-free tests, a separate secret-flow review and the owner's approval.

## Verify

```sh
scripts/verify-local-web-launcher.sh          # launcher with a dummy home and fake npm and cargo
cargo test --locked --lib -- acquisition::accounts::vault server::accounts::web
npm --prefix ui run build && cargo build --release --locked
node ui/scripts/verify-local-web.mjs "$PWD/target/release/mailroom"
```

The last command starts the real binary with a dummy home on `127.0.0.1:4173`, so nothing else may use that port. It drives the forms in Chromium when Playwright's browser is installed and falls back to HTTP otherwise. It adds no account and contacts no provider.
