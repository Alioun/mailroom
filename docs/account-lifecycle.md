# Local account lifecycle

The protected macOS owner CLI supports local account creation, password replacement, forgetting, key rotation and reconciliation of interrupted lifecycle operations. It uses exact native consent and fixed result records. The frozen setup wizard offers creation and password replacement. Its optional maintenance phase offers the remaining operations. These paths require exact version approval and owner-run installed verification before real credentials are used. See [Native confirmation and protected Keychain credentials](macos-confirmation.md).

The HTTP lifecycle handler remains synthetic-only. Production HTTP lifecycle requests and new browser connections return 403 before credential access. Owner commands never connect to a provider. No lifecycle result enables a provider write.

## Durable account states

A version-1 lifecycle record describes unfinished creation, password replacement, local forgetting or key rotation. A completed forget leaves an account-ID tombstone. Records contain account identity, credential references, generations and owned file information. They contain no passwords or decryption material. Account metadata defaults older records to generation zero and an unverified connection.

Creation records intent before credential or data writes and publishes account metadata last. An interrupted unpublished creation is cleaned, never silently completed with new credentials. A published creation is authenticated before its intent is cleared. The connection state records unverified, observed or failed separately from local creation. Password replacement resets connection evidence to unverified and makes no network request.

Password replacement writes a new credential reference, durably selects it in metadata, then retires the old reference. Recovery deletes the unselected reference. The account ID, encryption identity and ciphertext remain unchanged.

## Owned resources and exact forgetting

Each account owns its metadata, password reference, encryption-generation reference, stable identity reference when present, primary encrypted snapshot and three independent synthetic effect snapshots. The effect stores hold mailbox changes, invoice transport acknowledgments and unsubscribe acknowledgments. The primary snapshot includes the complete census, workspace, pins, exports, backlog journal, invoice originals and history, unsubscribe history, ledger projection and wind-down decisions.

The forget preview lists exact existing files, sizes, digests and credential references. It binds both English and German consequences, the account, state revision, generation and resource manifest with a distinct full HMAC receipt. Confirmation recomputes the preview. An altered, stale, cross-account, cross-purpose, unconfirmed or replayed request rejects.

The synthetic HTTP flow stops account-work admission, cancels and joins that account's acquisition worker, and writes deletion intent before removing resources. The separate protected owner command instead refuses a busy account lease. The owner stops active work first. An unrelated acquisition finishes without cancellation. A stable cross-process account lease and every store lock remain held through deletion. Lock files are retained permanently so a held lock can never split across two inodes. These files hold no account content. A tombstone prevents stale account objects from recreating removed resources.

Deletion removes the account-owned encrypted projection, not the shared authoritative `addresses.yaml`. Other accounts and owner-authored ledger entries survive. Reports recompute their source bindings and expose a missing comparison account as a blocker. The UI clears account previews and reloads account and subscription lists after a successful change.

Forgetting loses local undo and reconciliation history. It performs no remote undo, unsubscribe or account shutdown. Browser-managed downloads, prior exports and backups have no tracked destination and cannot be erased here. Unknown files survive. Anonymous staging files from older versions cannot be attributed safely and also survive. New metadata and ciphertext staging files carry an account prefix. Synthetic credential staging belongs to the injected fake backend.

## Encryption generations and stable identity

Generation zero uses the existing ciphertext paths and derivation. Later generations use `<id>.g<generation>.census.enc` in each store directory. All generations share the original stable lock path. A new generation retains the old key and ciphertext until every existing dependent snapshot is staged and authenticated. Metadata selects the generation atomically. Each selected snapshot is reread before obsolete ciphertext and credentials are retired.

Recovery handles each mixed-generation boundary. Before selection, original stores remain readable. After selection, the complete new generation is readable even if old-file cleanup is incomplete. Missing credentials, unsupported records, authentication failures and failed cleanup deny normal work while preserving retry information. Recovery never generates a replacement key for unreadable data.

The first rotation preserves the existing main and synthetic HMAC identity material in a separate credential reference. Encryption keys can then rotate without retaining the obsolete decrypting secret. Placeholders, subscription identities, wind-down row identities and stored operation/effect identities remain stable. Approval bindings include the selected generation, so old previews cannot authorize new actions. Histories, uncertainty, completed effects and conditional-undo observations survive. A recorded completed send or unsubscribe cannot become eligible through rotation.

Account-state versions 1 through 7 retain their existing decoding. The ciphertext envelope is unchanged. Older binaries cannot use generation-aware metadata and must not be used to downgrade a rotated account. This unit makes no lost-key recovery promise. The separate encrypted-recovery feature must capture the selected encryption and identity references, consistent snapshots and reconciliation histories. No backup is created here.

## Synthetic verification

With port 4173 free, run the HTTP verifier, then its matching built-browser verifier. Both retain their own synthetic fixture and metadata under `.audit`.

```sh
CARGO_PROFILE_TEST_OPT_LEVEL=1 cargo test --locked
npm --prefix ui run check
npm --prefix ui run build
CARGO_PROFILE_TEST_OPT_LEVEL=1 node scripts/verify-account-lifecycle.mjs
node ui/scripts/verify-account-lifecycle.mjs
node ui/scripts/verify-account-lifecycle-recovery.mjs
```

The Rust matrix injects deterministic durable-boundary failures and credential failures, reconstructs the file-backed fake credential backend, checks lock inodes and cross-process exclusion, and compares all encrypted aggregates. The HTTP verifier creates actual synthetic mailbox, invoice and unsubscribe effects, replaces the process with SIGKILL at representative lifecycle boundaries, and checks decryptability, recovery convergence, exact forgetting, source invalidation and worker joining. The browser verifier drives the built English and German controls with no browser storage or provider requests.

These checks cover cooperating application processes and injected local failures. They do not establish native signature or prompt behavior, real Keychain durability, arbitrary disk or power-loss completeness, hostile same-user filesystem races, production provider semantics or backup restoration.
