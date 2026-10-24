# Census implementation and remaining work

The executable supports an owner-run read-only account path and two independent synthetic paths. The owner can connect mailbox.org or Gmail, run or resume header acquisition, cancel it, inspect encrypted saved data, derive local reports and review a private summary export. The complete census and personal workflow remain incomplete. Verification uses fake credentials and local synthetic TLS servers. No real account or OS credential backend has been exercised by an agent.

## Ownership and data flow

`census::Census` owns one opaque account identifier, folder checkpoints and confirmed own addresses. Each folder stores UIDVALIDITY, a live UID set, saved records and an explicit idle, running, cancelled, complete or failed state. Reconciliation removes vanished UIDs, clears records when UIDVALIDITY changes and returns bounded pending UID batches. Missing UIDs remain pending even when a higher UID was saved. Reopening converts an interrupted running checkpoint to cancelled.

`census::parse_headers` accepts a header block of at most 64 KiB and rejects a trailing body. It decodes MIME encoded words and Unicode Subjects and mailbox display names. It retains full Subjects, Date, From, Sender, Reply-To, To, Cc, Bcc, delivery and original-recipient addresses, Return-Path domain, relay originals, list fields, authentication results, DKIM signatures and Received evidence. Duplicate evidence fields remain separate. Message-ID is stored only as SHA-256. Message metadata stores direction, size, internal date, flags, Gmail message and thread IDs, and Gmail labels.

Raw evidence strings retain encoded words. Invalid UTF-8 bytes in raw evidence use replacement characters. Malformed address fields retain their raw evidence but may produce no parsed address. This parser is separate from the foundation's intentionally ASCII-only rule compiler.

Relay unwrapping is an observed header claim. It is not authenticated identity. DKIM unsubscribe-header coverage always remains unknown. Neither an Authentication-Results field naming a provider nor a signature listing unsubscribe fields proves coverage. Trusted provider provenance and correlation with a passing signature remain required future work.

Own-address candidates retain their evidence source. To and Cc remain unverified. A confirmation records only that an address belongs to the owner. It assigns no recipient role, folder, forwarding target or accounting behavior.

## Encrypted SQLite snapshots

`census::EncryptedStore` opens SQLite only in memory. Temporary storage and journaling use memory. Schema version 1 contains one account snapshot in a singleton SQLite row. The current model is loaded as a whole. It is not a relational query or journal implementation.

Each account uses a distinct `<opaque-account-id>.census.enc` file. A 32-byte random local secret derives distinct encryption and placeholder keys through HKDF-SHA256 with account-scoped salt and separate purpose labels. Secret and derived-key buffers zeroize on drop. The AES implementation enables its key-zeroization feature.

The file contains the versioned `MAILROOM-CENSUS` marker, a fresh random 96-bit nonce and AES-256-GCM ciphertext with its authentication tag. The version marker and expected account identifier are authenticated as additional data. AES-GCM, HKDF and HMAC come from RustCrypto crates pinned by Cargo.lock.

Saving authenticates any existing file before replacement. It serializes the in-memory SQLite database, encrypts it, writes only ciphertext to a private temporary file, synchronizes that file, atomically replaces the account file and synchronizes the directory. Interrupted staging leaves the previous snapshot readable. Orphan staging files are ignored. Wrong keys, account mixups, unknown versions and tampering fail without replacing the existing file. Plaintext SQLite, WAL files, plaintext temporary copies and raw message logs are never written by this implementation.

On Unix the store enforces directory mode 0700 and file mode 0600. Windows ACL enforcement is not implemented or verified. Production storage uses `~/Library/Application Support/mailroom/` on macOS, `%LOCALAPPDATA%\mailroom\` on Windows, or `$XDG_DATA_HOME/mailroom/` on Linux with the usual user-data fallback. Account forgetting, key rotation and large-census behavior remain incomplete. The serialized SQLite snapshot is limited to 128 MiB. Saves rewrite the whole snapshot. The caller must serialize writes to an account store. The demo does so through one server mutex.

## Synthetic interface and report

The demo processes eight bundled synthetic headers in bounded steps. Each completed step saves the actual encrypted snapshot. Resume reopens that file. Cancellation preserves records. A generation identifier prevents an old worker from advancing a restarted scan. Simulated failure stops after two saved headers. The demo's master secret stays in memory and disappears at server exit. No real credential backend is invoked.

The local interface supports English and German. The eight-header demo shows progress, decoded Subjects, sender details and own-address confirmation. Owner account controls are separate from this demo.

The eight-header demo uses the same report analysis and a fixed as-of time of 25 October 2026 UTC. Its approval list stays empty. The account interface supports a separate report review and export, described in [Census report semantics](census-reports.md). Both paths omit full Subjects and unsubscribe targets from summaries.

Placeholders use a kind label and six lowercase base32 characters from account-scoped HMAC-SHA256. Normalization trims whitespace and lowercases the value. Confirmed addresses use separate alias and own-domain placeholders. Sender identities keep the same person placeholder even when classification changes. Senders with fewer than three messages pool into a long tail. Clear organisational domains require an explicit reviewed selection. Folders, labels, List-Ids and keywords always remain placeholders in exported summaries.

## HTTP boundary

The server generates a random token per launch and keeps it out of stdout, URLs, arguments and persistent browser storage. A same-origin session bootstrap supplies it to the interface. All subsequent API requests require `X-Mailroom-Token`. Host, Origin and fetch-site checks apply to bootstrap and API requests. There is no wildcard CORS.

Synthetic census requests accept only a demo mode, cancellation or confirmation of a discovered synthetic address. They reject account identifiers, credential fields, source URLs and authorization fields. No synthetic request can enable an account adapter or override the policy. All eight categories of account mutation remain denied. Dedicated owner account routes permit read-only work; the generic operations route never starts account work.

## Owner account persistence and credentials

`acquisition::Accounts` stores one private metadata file per random opaque account ID. Metadata contains the selected provider, login and two credential references. It contains no password or census key. On macOS metadata uses `~/.config/mailroom/`. Linux uses the per-user config directory and Windows uses the roaming application-data directory. Census ciphertext stays in the separate data directory described above.

The default backend uses `keyring` 4.2's binary secret API for macOS Keychain, Windows Credential Manager and Linux Secret Service. It initializes only when an owner account action needs credentials. Server construction, session bootstrap, status and both synthetic paths never initialize it. Loading the saved-account list reads metadata only and requires an explicit UI action. Connecting saves the supplied existing password directly to the backend, generates a random 32-byte local secret in the same backend, creates the encrypted census, and then attempts TLS authentication and folder discovery. A failed connection can therefore leave a saved account ready for a later scan retry. Reconnecting creates a separate account ID; it does not replace an existing account's credential or key.

Every later census open or scan requires the existing local secret and an existing readable snapshot. A missing credential, missing snapshot, bad key or unreadable snapshot stops the action. Nothing silently generates a replacement key. Fake-backend tests verify isolation and reconstruction, including two actual subprocesses reopening one synthetic account.

Application secret values redact Debug and zeroize their byte buffers on drop. Derived keys retain the existing zeroization. Native IMAP, TLS, HTTP and browser implementations can create transient copies that these wrappers cannot promise to zeroize. Passwords never enter application logs, returned errors, argv, URLs, config or persistent browser storage. The hidden UI input clears on submission. OS prompts and an unlocked credential store are owner prerequisites. No agent runs those steps. Additional password-manager adapters are deferred. This does not isolate mutually untrusted processes running as the same OS user.

Account operations serialize through one server-side operation lock. One job may run at a time. The fixed listening port prevents two normal server instances from running simultaneously. Direct library callers must serialize access themselves. Account creation is not transactional across the OS backend and filesystem; interruption can leave orphan references or a ciphertext file. Recovery and cleanup of those orphans are not implemented. Existing readable accounts are not overwritten.

## Read-only IMAP acquisition

Only the policy module constructs the internal provider-bound `ReadPermit`. Production transport construction requires that type. The transport and its IMAP session are private. Its only operations expose folder discovery, complete UID inventory, metadata and explicit header fields. HTTP accepts typed account IDs and provider choices. It accepts no host, port, TLS override, raw command, authority or source URL.

The fixed endpoints are `imap.mailbox.org:993` and `imap.gmail.com:993`. `native-tls` validates certificates and hostnames before authentication. The pinned `imap` 3.0.0-alpha.15 client and `imap-proto` parser handle responses and literals. This version is a prerelease; the adapter is intentionally private and tested against actual emitted commands. The application does not implement its own IMAP response framing. Modified UTF-7 folder and label payloads use a bounded codec with Base64 and UTF-16 validation. Unicode names are retained locally, with no guesses based on translated folder names.

The transport requests `LIST` with `RETURN (SPECIAL-USE)` when advertised. A non-Gmail server without SPECIAL-USE is rejected rather than guessing folder roles. Gmail also requires X-GM-EXT-1. Folders marked NoSelect, Junk, Trash or Drafts are excluded. Gmail uses its All Mail special-use folder once when available; otherwise it scans eligible folders and deduplicates by X-GM-MSGID. Message labels also exclude Spam, Junk, Trash and Drafts. Sent direction comes from the Sent attribute or Gmail's system Sent label.

Each folder opens with `EXAMINE`. The adapter requires the server's read-only response, nonzero UIDVALIDITY and a successful `UID SEARCH ALL` whose size matches the EXAMINE count. Missing responses or changing counts cause a retry. Metadata requests contain only UID, size, internal date, flags and the Gmail message ID, thread ID and labels when applicable. Metadata refresh covers existing UIDs on every scan. New header requests contain explicit `BODY.PEEK[HEADER.FIELDS (...)]<0.65537>`. Headers over 64 KiB fail. No body or attachment section is requested. There is no public raw-session or arbitrary-command method, and no reachable mutation command, SMTP or ManageSieve implementation.

The adapter uses one TLS connection at a time, batches of at most 25 UIDs, a 4 MiB response budget per command, a 15-second socket timeout and a command deadline checked between reads. Inventories are limited to 250,000 UIDs per folder and LIST to 10,000 folders. Acquisition pauses at least 100 ms between batches. TLS, protocol and incomplete-inventory failures retry at most twice after 1 and 2 seconds. Authentication, credential and storage failures do not retry. Cancellation is checked between reads, batches and backoff intervals. An in-flight OS DNS lookup or socket operation can delay cancellation; DNS uses the system resolver and is not independently timed out.

Reconciliation begins only after a complete folder UID and metadata inventory. It resumes missing UID holes, removes vanished records, clears records on UIDVALIDITY change, and refreshes flags, labels and direction. Removed folders disappear only after all selected folders finish. Each header batch checkpoints the encrypted snapshot. A persisted account acquisition state becomes complete only after the entire selected inventory finishes. Reopening an interrupted running state marks it cancelled. A failure retains completed valid checkpoints, reports failure and leaves unvisited saved folders intact. A resumed scan re-inventories the provider rather than trusting a previous last UID. Gmail label topology and skipped-folder counts are not separately persisted; reports currently see canonical scanned folders and per-message labels.

Confirmed own addresses survive UID resets and vanished evidence and remain visible for later revocation. The encrypted account login is an unverified candidate. To/Cc, login and relay claims assign no personal or business role, forwarding permission or accounting behavior. DKIM unsubscribe coverage remains unknown. No transport result proves OX coexistence, delivery-recipient behavior or accounting acceptance.

## Synthetic account verification

Starting the binary with `--synthetic-accounts` installs a fake credential backend and a bundled 100-header source in a temporary directory. The UI clearly identifies this mode and offers synthetic mailbox.org and Gmail account buttons instead of a password form. HTTP cannot switch modes or provide fake endpoints. The normal eight-header demo remains separate, with independent ephemeral keys. Both synthetic account credentials and files disappear with the process.

`cargo test --locked` covers the fake backend, actual subprocess restart, key loss, account isolation, exact read-only TLS commands, fragmented header literals, Unicode folders and labels, certificate and hostname rejection, redacted authentication failure, response-size enforcement, cancellation, reconciliation, metadata refresh, Gmail exclusions and deduplication. HTTP tests exercise the fake account lifecycle. `ui/scripts/verify-acquisition.mjs` drives the built synthetic account UI. `ui/scripts/verify-census.mjs` retains the original demo checks. `ui/scripts/verify-reports.mjs` drives partial and complete report states, Sent correspondence, Gmail arrivals, domain selection, exact Markdown download, German narrow layout, CSP and empty browser storage. None of these flows touches a provider or the OS backend.

## Required follow-up units

- Validate the OS adapters and real provider behavior through an owner-run process. Current evidence establishes synthetic behavior only. Windows ACL enforcement and cross-platform runtime behavior remain unverified.
- Add credential update, account forgetting, key rotation, interrupted-creation cleanup and a recovery interface. The current UI retries scans with existing stored credentials; it does not replace them.
- Verify large-census performance and replace whole-snapshot rewrites if needed. The existing 128 MiB limit still applies.
- Add Takeout MBOX import without body or attachment persistence.
- Persist full Gmail label topology and excluded-folder counts. Current canonical scanned-folder counts do not describe every provider label or Spam folder.
- Implement trusted authentication provenance and signature correlation. Preserve unknown coverage when evidence is insufficient.
- Complete the wider agent workflow separately, including native action-naming confirmation, agent enablement, proof/ruleset exports, restricted agent directories and proposal resolution. The report export currently uses explicit local web confirmation and a browser download. It does not enable agent access or live operations.
- Add unsubscribe logs and backlog journals with those workflows. They are absent from this schema.

The person-run `scripts/connect-account.sh` walkthrough starts no credentialed step automatically and reads no secret. It guides an existing local launch and private UI entry without requesting provider setting changes. Its shared wizard library retains unused helpers; shellcheck excludes SC2329 for those template functions only.
