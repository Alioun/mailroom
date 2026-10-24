# Rules and proof

This unit compiles YAML rules and evaluates the emitted Sieve subset over normalized saved census records. It is a local compiler and proof slice. The complete personal workflow and provider deployment remain unfinished.

## Source model

[The preset](../examples/preset.yaml) and [one override document](../examples/override.yaml) deserialize through one JSON model. Generated structural schemas are in [schemas](../schemas). The compiler then validates semantic constraints such as unique IDs, existing patch targets, folder depth and mailbox syntax. Regenerate schemas with `cargo run --locked --example rules-schema -- schemas`.

Stable IDs have dot-separated components. Overrides pin the preset ID and major version. A known-ID patch can change enabled state, folder, AND conditions, condition extensions, account scope and the read action. Rename chains resolve before patching. Cycles, missing destinations and duplicate migrated patches fail. New rules require an explicit `user.*` ID, category and conditions. Unknown fields fail. The trace and effective metadata retain origins, disabled rules and account scopes.

The generic vocabulary has Money, Contracts, Orders, Travel, Social and Newsletters, with German equivalents. Personal and business roots are configurable. Topic and special folders are direct root children. Overrides select a special folder without renaming a preset topic. Unicode path segments and MIME-encoded Subjects are supported. Sender and Subject comparisons use Sieve's ASCII casemap, so non-ASCII letters do not gain Unicode case folding.

Approved address mappings override exact domain mappings. Ownership confirmation alone assigns no role. Sign-up domains must already have an explicit personal or business role. Active and retired ledger entries generate recipient rules. A new local part on a classified sign-up domain goes to its configured new-address folder. A classified domain outside the sign-up policy sorts normally.

## Compilation and matching

Role safety runs first. The implemented executable accepts exactly one approved synthetic delivery recipient from Delivered-To or X-Original-To. Unknown roles, missing evidence, multiple recipients and unfiltered roles retain Inbox with internal review status. No Flagged mutation is implied. There is no fallback to To, Cc, relay claims or account ownership.

Forward copies run after role safety and before guards and ordinary filing. Filing then runs guards, user overrides, ledger rules, new-address policy, bulk, From and Subject rules, followed by Inbox retention. One filing action stops execution. Bulk means List-Id or both an exact one-click Post value and an HTTPS URI marker. This is routing evidence, not authenticated unsubscribe authorization.

Named private forwarding targets exist only in the override. Stable forward IDs can be enabled and scoped there. Invoice-address and explicitly approved raw From senders require business eligibility, Message-ID evidence and the copy and duplicate capabilities. Every copy has a per-rule duplicate handle. Gmail backlog compilation emits no forwards. Keyword forwards require nonbulk and attachment evidence. Since header census cannot prove PDF or XML attachments, the whole keyword forward is omitted. No attachment or body read is added. Other unsupported required predicates omit the entire rule with a reason.

`CompiledArtifact` has private fields and no deserializer. Its SHA-256 identity binds the compiler semantics version, normalized source documents, account and engine context, scopes, capabilities, effective metadata and executable text. YAML formatting alone does not change its identity. Proof parses the emitted executable into Sieve tests and actions. It never invokes a source rule matcher. A test changes the executable folder and observes the changed decision; an unknown executable action fails.

The parser supports only the emitted subset, not arbitrary Sieve. The compiled Sieve is also the internal matching representation for Gmail backlog decisions. No Gmail XML or Gmail filter deployment is implemented.

## Proof and privacy

`GET /api/sample` returns the complete synthetic request. `POST /api/preview` compiles it. `POST /api/accounts/proof` accepts documents, account, UTC as-of time, census pins and baseline input. It reads the encrypted saved census under the existing operation lock without acquiring more mail. The account route rejects caller engine or evidence injection. Its recipient provenance and provider capabilities remain unknown, so it retains all messages for review.

Proof excludes Sent mail and deduplicates Gmail by X-GM-MSGID while combining known placement labels. It returns one filing destination, independent copy eligibility and proposed read status. Counts derive from those displayed decisions. The 13 calendar months include the as-of month. Missing, invalid and future Date values are excluded from those monthly counts and reported. Diagnostics include zero-selected rules, fully shadowed rules, unmatched Inbox senders, partial census and engine omissions.

Pins reference account and Message-ID hash. Missing or cross-account references are stale. Any matching record in the wrong folder fails its pin and blocks the proof. Compile errors reject the request. Stale pins and other diagnostics warn. A warning result never grants deployment authority.

The synthetic library preview can compare current placement or a previous configuration compiled for the same account and engine. Saved-account proof compares current placement or the exact artifact from the last confirmed local export. The alternative comparison has a one-click control. Sensitive decisions and targets stay in the protected local UI response. There is no proof CLI summary or agent export. Existing reviewed census summary and placeholder protections remain intact.

## Durable local workspace and download

The encrypted singleton SQLite row uses account-state version 2. It contains the complete census, an optional workspace, a monotonic account revision and at most eight confirmed export records. The workspace retains raw YAML, its own revision, account-scoped pins, the exact UTC as-of time and baseline mode. Each export retains the exact compiled artifact, source workspace, proof receipt, preparation time and local-download provenance. The private storage decoder restores the immutable artifact without recompiling it. An obsolete semantics version or different engine makes a recorded baseline unavailable.

Version 1 encrypted SQLite snapshots load with their original census and no invented workspace or approvals. The next successful write migrates them atomically. Unknown versions, invalid JSON and authentication failures retain the existing file. All census checkpoints, refreshes and ownership confirmations preserve workspace and export fields.

An exclusive OS file lock covers the account store's entire lifetime, including validation and replacement. A competing process fails immediately. An empty account-ID lock file remains in the private data directory. Whole-state writes also compare revisions. Census replacement requires loading within that store's lock lifetime. The HTTP workspace save compares the displayed account revision, so a stale tab must reload instead of overwriting a newer scan or save.

`workspace/open` and `workspace/save` accept only the selected account and typed workspace input. Save validates YAML, pin references and proof inputs before replacing a workspace. The UI marks unsaved input and requires an explicit encrypted save before proof. It keeps documents, private targets and receipts out of browser storage.

`proof/preview` reloads the selected account's saved workspace. Its full purpose-separated HMAC binds account, provider, complete census and state revision, raw documents and workspace revision, artifact, compiler and evaluator semantics, pins and expectations, exact UTC as-of, baseline mode and recorded artifact, displayed warnings and gate, filename and exact executable bytes. `proof/download` requires that receipt and explicit confirmation. Under the same account lock it reloads state, recompiles and regenerates the proof, compares the receipt in constant time, then records the baseline before returning the exact bytes. Formatting-only saves, identical census refreshes, pin changes, ownership changes and prior exports invalidate an old receipt.

Only mailbox.org accounts with a nonblocking settled gate can download `rules.sieve`. Failing pins and compile errors block. Stale pins and other diagnostics warn. Gmail backlog can save and prove a workspace but has no Sieve-download action. The browser requires confirmation under the displayed proof. Reload clears local confirmation. A recorded export means bytes were prepared for an owner-confirmed local download. It proves neither filesystem delivery nor provider upload or activation.

Run `node scripts/verify-durable.mjs` and `node ui/scripts/verify-durable.mjs` against the built fake-only server. With that server stopped, `node ui/scripts/verify-durable-restart.mjs` starts and replaces a test-only HTTP process using temporary file-backed fake credentials. It verifies UI recovery, stable proof receipts and bytes, rejected prior launch tokens, and synthetic private-marker absence in encrypted state and metadata. The fixture and credential adapter are excluded from production builds. No real provider or credential store is used.

## Independent synthetic checks

Run `./scripts/verify-sieve.sh`. It generates five variants and 70 independent header fixtures from the public sample, lints each executable with check-sieve and compares Pigeonhole results with fresh-cache evaluator decisions. Variants cover standard rules, German folders with read flags, unknown evidence, Gmail backlog and missing capabilities. Comparisons assert exactly one destination, every copy target and Seen actions.

The runtime image mounts only the synthetic output and the harness, both read-only. Its root filesystem is read-only, temporary writes use tmpfs and networking is disabled. No SMTP or IMAP service starts. Dovecot package installation is confined to image construction and blocks service startup. The synthetic CI workflow runs tests, UI checks and this harness without mail credentials. CI execution itself has not been observed in this local unit.

The verified local tools were check-sieve 1.0.1 at source revision `3a99728878994a7f6f2f14032013fc0f06bacbc1` and Pigeonhole 0.5.19 from Debian Dovecot package `1:2.3.19.1+dfsg1-2.1+deb12u6`. The base image digest is pinned. Apt repositories remain an external version dependency.

Primary tool references:

- [check-sieve usage and supported extensions](https://github.com/dburkart/check-sieve/blob/3a99728878994a7f6f2f14032013fc0f06bacbc1/README.md). It is used for syntax checking, not its experimental simulator.
- [check-sieve MIT license](https://github.com/dburkart/check-sieve/blob/3a99728878994a7f6f2f14032013fc0f06bacbc1/LICENSE).
- [Pigeonhole sieve-test manual](https://github.com/dovecot/pigeonhole/blob/0.5.21/doc/man/sieve-test.1.in). The harness uses simulation and trace output without `-e`. It never sends a message.
- [RFC 5231 section 7](https://www.rfc-editor.org/rfc/rfc5231#section-7) specifies the address count test used by role safety.
- [RFC 7352](https://www.rfc-editor.org/rfc/rfc7352) specifies duplicate state and its prior-execution semantics.

## Incomplete acceptance criteria

1. Real provider recipient evidence, envelope support and capability receipts are unimplemented. Local saved-account proof therefore holds every message. Multiple same-role recipients are conservatively held too. Full mixed-recipient evaluation remains incomplete.
2. Persistent duplicate-cache true results, cache expiry, concurrent deliveries and actual provider state are not cross-checked. The evaluator starts an empty in-memory cache for each proof and orders messages by parsed Date, UID and stable location. It does not reproduce arrival time or the provider's historical cache. The Pigeonhole harness deliberately checks independent fresh-cache messages only.
3. Keyword forwarding and attachment-dependent rules remain omitted. Provider MIME capability discovery, missed-invoice and unsupported-format reports, sender-check warnings and intake acceptance remain unfinished. Actual invoice sending is a later guarded operation.
4. The source schema is a coherent subset of the complete authoring specification. It has no preset forward patches, category-wide enable controls, major-upgrade approval workflow or ledger service, first-seen and leak-report fields. General TOML, Jsonnet, alternative layouts and weak-provider engines remain deferred product scope.
5. Deterministic organisational proposals are unimplemented. Nothing infers roles, approves invoice senders or changes private rule files.
6. Durable local documents, pins, confirmed Sieve downloads and recorded export comparisons are implemented. Provider deployment history, after-deploy landing comparison and strict/warnings-only advanced gates remain unimplemented. Download history records prepared bytes only. It does not prove successful delivery or provider application.
7. Generated system filing rules lack individual 13-month diagnostic rows. Omitted rules are reported but engine-gap attribution to individual Inbox messages remains incomplete. Backend diagnostic prose and copy-eligibility states are not fully localized, though controls and proof headings support English and German.
8. T3 preview opened but both snapshot attempts timed out. The actual built UI was verified through the repository's Playwright fallback. T3 preview verification remains unverified.
9. Provider folder separators, existing folder inventory, rule coexistence, accounting acceptance and full-scale census behavior remain unknown. Full Gmail topology and earlier census lifecycle limitations remain as documented in the census receipts. No synthetic check settles these facts.

Deployment, Gmail forwarding setup, journaled backlog moves and undo, unsubscribe, complete address lifecycle, wind-down reporting and owner-authorized live validation remain later dependencies. Every live-write denial remains enforced.
