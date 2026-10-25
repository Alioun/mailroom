# Backlog sorting and protocol MOVE

The local interface previews filing changes against saved rules. Real-account writes remain denied. Legacy synthetic histories and the protocol MOVE journal are separate. Protocol execution requires an explicitly installed, account-bound loopback source and separately installed synthetic workspace evidence. HTTP requests and recovered data cannot install either authority.

## Scope and proof

The default scope is Inbox plus unfiled mail. Gmail's legacy synthetic adapter treats messages without a user label as unfiled. Protocol mailbox.org execution requires an acquired Archive special-use role. Names alone do not establish that role. Inbox-only and Advanced every-folder scopes are available. Sent, Drafts, Spam and Trash remain excluded. Flagged messages are protected by default, and protocol filing excludes preexisting Deleted messages.

Protocol preview resolves the saved workspace through `Accounts::workspace_context`. It retains the exact compiled artifact, executable and saved proof receipt, including pins, as-of and the selected Current or Previous baseline. A separate execution proof evaluates that same parsed artifact against fresh TLS observations. Ordinary IMAP headers and write permission grant no recipient or delivery-envelope authority. Without usable independent evidence, the proof cannot authorize a move.

Filing projects only the proof's destination decisions. Predicted accounting copies, read changes and keywords do not execute. Preview includes destination selections, counts, exclusions and diagnostic reasons. Missing destinations never cause CREATE.

A purpose-specific HMAC binds the exact approval to the saved proof, execution proof, material account state, options, sparse UIDs, topology and installed authority. Source identity includes the account, login, loopback endpoint, TLS name/root, profile and epoch. Runtime authority also binds credential generation, local keys, recovery identity and workspace context. Material changes invalidate continuation without rewriting historical artifacts or operation IDs. Sieve-download receipts cannot start filing or Undo.

## Protocol journal and execution

`AccountState::protocol_backlog` owns schema15 `Journal`, `Run` and `SavedPlan` records. Each batch reuses the validated `MoveEvidence` model. Approval consumption and exact membership persist before execution. The worker then saves attempted Intent before transmission, validated COPYUID mapping before later response processing, completion or loss, and identity readback as distinct evidence. Each command contains at most 500 distinct approved UIDs. Sparse sets never expand to include other messages.

The verified-TLS transport requires MOVE, usable UIDPLUS, stable mailbox identities and writable selected mailboxes. It emits UID MOVE with exact quoted wire names. It never uses COPY, STORE, EXPUNGE, CLOSE or CREATE. There is one execution connection per worker. Local account admission and store locks exclude competing application work.

An entry is verified only when a compatible validated mapping identifies the destination, the source UID is absent and mapped destination readback matches. Tagged OK, source absence, UIDNEXT, counts and Message-ID matching cannot replace COPYUID. A partial mapping resolves only its mapped entries. Missing mapping after possible transmission remains uncertain even if the source is still present. The worker never repeats an attempted MOVE automatically.

Cancel signals active socket work, joins the worker and saves Stop before returning. It stops new batches and retains completed or uncertain effects. Cancel remains available with an invalid ledger and creates no Undo. Status, refresh and process replacement do not resume work. Explicit Reconcile observes unresolved attempts without replacing verified historical readback. Repeated identical reconciliation avoids a new journal revision. Explicit Resume can execute only the compatible, never-attempted remainder. Unresolved attempted work blocks it.

Conditional Undo has a separate exact preview and initially unchecked confirmation. It reverses eligible mapped messages still at their recorded destination while the original mailbox instance remains compatible. Missing or moved-away messages remain conflicts without withholding other eligible messages. Reverse MOVE preserves current flags, records newly assigned UIDs and uses the same durable evidence transitions. A potentially effected reverse MOVE cannot repeat. Undo is not snapshot rollback.

IMAP MOVE cannot atomically prevent another client from changing flags after the final read. Local locks do not exclude other clients. Provider durability and external-client races require independent acceptance.

## Persistence, refresh and recovery

Schemas 1 through 14 with absent protocol fields retain legacy history and install no authority. Old schema numbers reject new protocol semantics, including nested injection. Schema15 validates journal lengths, identities, batch membership, evidence phases and resource bounds at save and load. Run history caps at 32 and the protocol journal at 4 MiB. Overflow rejects the save without pruning unresolved evidence. Old export bytes and compiler/proof version strings remain exact.

The installed TLS fake persists actual messages, flags, UIDVALIDITY and monotonic UIDNEXT independently of the application journal. Its read-only acquisition adapter scans the same mutable state after moves. Refresh preserves workspace, exports, ledger and unrelated histories. The ordinary legacy synthetic scan still uses its separate header fixture.

Single and selected recovery retain populated protocol journals and immutable IDs. Later independent mailbox, send, unsubscribe, Sieve and folder effects remain outside the captured primary state. Restored accounts remain held and install no source or usable approval. Restoration never reconciles, resumes or reverses provider work. Outer bundle versions and resource bounds are unchanged.

## Repeatable verification

The following scripts use invented fixtures and the actual protected saved-workspace workflow. Each starts with port 4173 free and stops its process after checking its current identity. Set `CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_OPT_LEVEL=1` in the execution environment.

- `node scripts/verify-imap-backlog-move.mjs` exercises saved proof, exact download, sparse forward/reverse MOVE, actual SIGKILL boundaries and 501-message Cancel/replacement/Resume.
- `node scripts/verify-imap-backlog-faults.mjs` checks protected saved-workspace failures, mapping loss and conflict, identity changes, unsupported capabilities and exact encoded mailbox names across replacement and reverse MOVE.
- `node scripts/verify-imap-backlog-scope.mjs` verifies actual observed Archive roles, Inbox and Advanced scopes, mixed-role exclusions, protected flags and exact sparse commands.
- `node scripts/verify-imap-backlog-projection.mjs` verifies that predicted accounting copies and read changes remain outside protocol filing.
- `node scripts/verify-imap-backlog-reconcile.mjs` retains verified history through reconciliation and preserves later client flags during separately confirmed Undo.
- `node scripts/verify-imap-backlog-material.mjs` checks stale documents, pins, dates, baselines, options and workspace authority before approval consumption and after process replacement.
- `node scripts/verify-imap-backlog-admission.mjs` checks observed unknown-recipient holds, cross-account approval denial and changed topology or mailbox instances after preview.
- `node scripts/verify-imap-backlog-batches.mjs` checks actual forward and reverse process deaths between 500-message batches, reverse Cancel and explicit safe remainder continuation.
- `node scripts/verify-imap-backlog-safety.mjs` checks protected requests, stale and wrong-purpose receipts, conditional conflicts, current flags, reverse crash boundaries, live-socket Cancel, rotation and exact forgetting. `--tail` runs only the Cancel/lifecycle cases.
- `node scripts/verify-imap-backlog-refresh.mjs` compares complete non-census state and independent effects across actual read-only TLS refresh and replacement.
- `node scripts/verify-imap-backlog-recovery.mjs` exercises populated single and selected HTTP capture/restore, exact primary state, later effects and restored holds.
- `node ui/scripts/verify-imap-backlog-move.mjs` drives the built English/German protocol controls through replacement, Cancel, Reconcile, Resume and confirmed Undo.
- `node ui/scripts/verify-imap-backlog-recovery.mjs` drives built capture/restore with populated journals and verifies the held protocol controls.

`node scripts/verify-imap-move-core.mjs` covers the reusable domain and TLS transport directly. Those direct-intent tests do not substitute for the saved application workflows. Legacy scripts `verify-backlog.mjs`, `verify-backlog-restart.mjs` and `ui/scripts/verify-backlog-cancel.mjs` retain their separate synthetic adapter coverage.

## Remaining limits

COPY fallback, Gmail protocol labels and the separate unchecked unmatched-mail archive offer remain unimplemented. Production source installation and actual provider acceptance remain unavailable. Native confirmation, protected Keychain access, independent Sieve execution and whole-personal-workflow readiness are separate gates. Controls support English and narrow German layouts. Backend diagnostic reason codes remain partly English. Local fixture results grant no real-account write authority.
