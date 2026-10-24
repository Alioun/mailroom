# Private address ledger

The installed `addresses.yaml` is the authority for address bookkeeping. Account-state version 6 stores an encrypted source snapshot, content version and the entries it projected into the existing compiler. That snapshot cannot be edited through workspace save. Versions 1 through 5 still load without invented ledger approvals.

Only synthetic launchers install this adapter in this implementation unit. The default launcher installs no ledger adapter. `/api/accounts/ledger` rejects production requests before constructing an account repository or accessing credentials. Requests cannot select paths, install an adapter, trust a provider or enable production writes. Enabling the ledger does not grant account-write authority.

## Format version 1

```yaml
version: 1
enabled: false
entries:
  - address: purchases+one@signup.example.test
    service: Example service
    folder: Purchases
    first_seen: '2001-01-01, supplied by owner'
    status: active
    note: Private owner note
    category: orders
    invoice: false
    association:
      sender_domains: [example.org]
      approved_evidence: Explicitly reviewed service association
```

`folder` is a single leaf under the recipient's classified root. The existing rule layout selects the roots and special folder leaves. Generic defaults are English. No personal folder preferences are installed by this feature. Retired entries use the configured retired leaf. Unlisted recipients use the configured new-address leaf only on explicitly designated signup domains. Other classified domains retain topic routing. Unknown or mixed recipient roles retain Inbox without accounting copies. Guards and user overrides, including existing mute rules, precede ledger filing. Accounting copies remain independent of retirement.

The file and its switch are shared across installed accounts. Saving updates their common source. Every account reconciles independently before proof and actions. Roles and recipient authority stay account-specific. The file switch enables projection of its entries. Independently authored rule entries and signup-domain policy remain independently configured. Disabling this projection removes only entries previously managed by this file. It does not remove unrelated rules or disable provider aliases.

Required fields are `address`, `service`, `folder`, `first_seen`, `status` and `note`. `category` defaults to `orders` for a new entry and remains explicit in review. `invoice` defaults to false. Discovery batches cannot grant a new invoice designation or change an existing category/invoice designation. Those remain separate approvals. An association is optional. When supplied it needs explicit organisational sender domains and approval evidence. The original approved association is immutable through batch refresh. A different observed domain produces a review signal, never automatic approval or a proven abuse finding. Services remain separate even if they share a sender domain.

Existing address spelling, supplied `first_seen`, category, invoice designation and unknown entry fields must survive an upsert unchanged. Unknown JSON-compatible root and entry fields survive file replacements, including nested values. Unsupported YAML tags, duplicate mapping keys, unsupported versions, invalid destinations and ambiguous address duplicates reject without rewriting the file. Serialization can change YAML formatting and comments. The adapter preserves field values, not textual formatting. Existing census parsing lowercases mailbox addresses; this feature does not rewrite owner local-part spelling or strip plus tags. Case-folding collisions reject because the current compiler cannot safely distinguish them. Provider-specific local-part identity and plus-tag semantics remain unverified.

`first_seen` is an owner-supplied historical value and is retained verbatim. A discovery proposal explicitly offers the UTC time that the current review generated it. This is not Message Date, first receipt or first census observation. The UI includes that meaning in the new entry as a preserved `first_seen_meaning` extension. The census does not record an earlier address-observation timestamp. Each observation separately names Message Date, source account, folder, UIDVALIDITY, UID and recipient header. To/Cc, Delivered-To and X-Original-To remain discovery hints here. This adapter grants no provider recipient trust. Sender domains come from the visible From mailbox with the pinned `psl` 2.1.245; absent, malformed or multiple sender evidence remains unknown. It is not authenticated sender evidence.

## Exact review and saving

The protected POST endpoint accepts `open`, `preview` and `confirm`. A batch contains the current file content version, account revision, policy switch and exact upserts. It never deletes entries implicitly. Retirement is filing bookkeeping only. Service, folder, status, first_seen, notes, unknown fields and association evidence appear in the exact batch.

The exact review discloses the shared-file effect. A dedicated full HMAC binds the account state, census and workspace revisions, exact source version, batch, resulting document, projection, compiled proof and displayed effects. Confirmation recomputes that review under the same shared-file lock. Altered, stale, cross-account, replayed and other-purpose receipts reject. Rule download, backlog, invoice and subscription actions reread the installed file at their request boundary. An external edit invalidates prior proof approval without a manual projection refresh, including an initially empty disabled ledger. A prepared sorting plan also retains its ledger version and cannot Resume or Advance after that version changes. Stop remains available even if the ledger is invalid. Existing uncertain effects remain recorded; this feature does not automatically retarget or repair an obsolete sorting plan.

The private directory has mode 0700. Ledger and lock files have mode 0600. The adapter uses a stable OS lock shared across accounts and processes, a pinned directory identity, descriptor-relative operations, no-follow opens, regular-file and ownership checks, one-link checks, bounded reads, synced staging, a content-version precondition and an atomic file rename. Directory, lock and ledger symlink replacements reject. Lock contention fails rather than waiting while another account holds its store. Hand editors should coordinate on `addresses.lock`; advisory locks cannot serialize an editor that ignores them. Content is checked again immediately before replacement, but this does not claim a filesystem compare-and-swap against an uncooperative writer editing at that exact instant.

The YAML replacement and encrypted account-state replacement are two separate durable operations. They are not one atomic transaction. If the file saves and the account save fails or the process exits, the next open/proof request derives a fresh projection from the current file. It preserves any newer owner edit, advances revisions and requires fresh proof. It never blindly writes a queued old document during recovery. Repeated reconciliation converges. Previously managed entries must still match their saved projection; independently authored conflicts reject rather than being adopted or discarded.

The local saved-census proof retains unknown recipient provenance in Inbox. Synthetic compiler tests separately exercise emitted Sieve with explicitly synthetic provenance. Neither establishes real provider routing, alias lifecycle, account associations, roles or accounting acceptance. No address-generation, alias-provisioning, catch-all, forwarding change or unsubscribe action is implemented by this ledger feature.

## Privacy and verification

Ledger entries, associations and notes stay in the installed private YAML and encrypted derived account state. Only the session-protected local views expose them. They do not enter agent summaries, URLs, logs or browser storage. Private ledger model types have no Debug implementation. The endpoint retains the existing no-store, CSP, session, Host and Origin boundary.

Run mutable fixtures serially. After stopping a directly verified fake server, run `CARGO_PROFILE_TEST_OPT_LEVEL=1 node scripts/verify-address-ledger.mjs`. It builds the test-only launcher, exercises HTTP and actual SIGKILL/replacement, and retains its fake server and directory in `.audit/address-ledger-retained.json`. Then run `node ui/scripts/verify-address-ledger.mjs` against that server. It drives the built interface with the repository's Playwright Chromium workflow and saves screenshots under `.audit/address-ledger-browser/`. The optimized test profile changes no fixtures or persistence limits and is used only for correctness checks.
