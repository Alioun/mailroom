# Census implementation and remaining work

The current executable exposes a synthetic census only. It does not access an account, credential backend, personal configuration or mail dataset. The account actions remain unavailable in the interface and denied by the Rust policy.

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

On Unix the store enforces directory mode 0700 and file mode 0600. Windows ACL enforcement is not implemented or verified. The library currently accepts an application-supplied directory. Production per-user data-directory selection, an OS-backed master secret, account forgetting, key rotation and large-census behavior remain incomplete. The serialized SQLite snapshot is limited to 128 MiB. Saves rewrite the whole snapshot. The caller must serialize writes to an account store. The demo does so through one server mutex.

## Synthetic interface and report

The demo processes eight bundled synthetic headers in bounded steps. Each completed step saves the actual encrypted snapshot. Resume reopens that file. Cancellation preserves records. A generation identifier prevents an old worker from advancing a restarted scan. Simulated failure stops after two saved headers. The demo's master secret stays in memory and disappears at server exit. No real credential backend is invoked.

The local interface shows progress, decoded Subjects, sender details and own-address confirmation. It supports English and German. Connection to real accounts is visibly unavailable.

The summary is deliberately conservative. Its approval list is empty, so it emits no clear sender, address, domain, folder, Subject or unsubscribe target. Placeholder values use a kind label and six lowercase base32 characters from account-scoped HMAC-SHA256. Normalization trims whitespace and lowercases the value. Confirmed addresses use separate alias and own-domain placeholders. Senders with fewer than three messages are pooled. This is a preview, not the complete required summary.

## HTTP boundary

The server generates a random token per launch and keeps it out of stdout, URLs, arguments and persistent browser storage. A same-origin session bootstrap supplies it to the interface. All subsequent API requests require `X-Mailroom-Token`. Host, Origin and fetch-site checks apply to bootstrap and API requests. There is no wildcard CORS.

Synthetic census requests accept only a demo mode, cancellation or confirmation of a discovered synthetic address. They reject account identifiers, credential fields, source URLs and authorization fields. No request can enable an account adapter or override the policy. All account operations remain denied.

## Required follow-up units

- Implement the OS credential backend and person-run connection interface with hidden input or stored references. Wire production census keys to that backend and select the per-user data directory. No real account path exists yet.
- Add an internal read capability and a structurally read-only IMAP adapter. Require validated TLS, EXAMINE, explicit BODY.PEEK header fields, bounded reads, cancellation and sanitized failures. Test actual commands against fake endpoints before enabling owner-run acquisition.
- Connect UID reconciliation to complete server inventories, including metadata refresh, removed folders, vanished messages and UIDVALIDITY resets. Implement Gmail All Mail selection, deduplication, labels and Junk/Trash exclusion. The current code provides only local checkpoint reconciliation.
- Add Takeout MBOX import without body or attachment persistence.
- Pin a PSL snapshot before deriving registrable domains. The current model explicitly stores no registrable domain.
- Implement trusted authentication provenance and signature correlation. Preserve unknown coverage when evidence is insufficient.
- Add sender classification, ESP attribution, Sent-derived correspondence, full monthly/read/flag/list statistics, approved clear-value handling and the repeated-keyword summary rules. Add report approval and export. Until then the summary omits all clear values and Subjects.
- Add unsubscribe logs and backlog journals with those concrete workflows. They are absent from this schema.
- Verify production persistence, credentials, account connection and acquisition through synthetic credential backends and fake TLS endpoints. Current verification covers only the local slice.
