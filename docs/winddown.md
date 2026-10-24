# Local wind-down report

The account interface compares saved local census evidence for an explicitly designated winding-down account and explicitly selected newer owned addresses. The report grants no retirement, forwarding, rule deployment, unsubscribe, alias disabling or sending authority. Actual retirement remains an owner action. All production mutations remain denied.

Only synthetic accounts have a wind-down evidence adapter in this unit. The production endpoint rejects requests before constructing the account repository or reading credentials. Takeout acquisition, provider recipient trust, forwarding setup, route tagging, native confirmation, encrypted recovery bundles and owner-authorized live validation remain separate work. Software readiness does not require waiting 13 months.

## Report model

`src/winddown.rs` owns the report, source versions, grouping, statuses, uncertainties, owner decisions and purpose-specific receipts. The terminal table, private file and interface use this report. Rendering does not derive another status or verdict.

The input binding includes the source and selected comparison accounts, census identity, completeness, confirmed exact owned addresses, account revisions, acquisition coverage, the current authoritative ledger bytes and projection, sender-data content and version, analysis version, as-of time and local decisions. Comparison identities join privately before any placeholder rendering. Keyed placeholders from separate accounts never form a join key.

The report reuses census canonical-message deduplication, correspondence classification, date accounting and its rolling calendar window. The 13-month start is the as-of instant minus 13 calendar months, inclusive. This differs from the proof's month-bucket diagnostic. First and last dates mean valid nonfuture header Message Dates. They do not mean arrival, observation or read time. Invalid, missing and future dates remain explicit unknowns.

Exact dotted local parts and distinct domains remain distinct observations. Each observation carries its account, original header field, original raw values and valid Message Date. Existing census parsing lowercases parsed addresses. The retained original field values expose that limitation. To, Cc, Delivered-To and X-Original-To observations alone never establish delivery authority.

A `Direct` acquisition fact binds a specific message fingerprint and exact delivery address. Only an installed adapter can supply it. The HTTP request cannot install facts, choose source paths or trust a provider. Current production acquisition supplies none. Forwarded, ambiguous and missing facts cannot establish migration. Conflicting copies under a Gmail canonical identity are ambiguous. A message already present in the old account by Message-ID does not establish direct migration at the new account.

## Groups and statuses

People require confirmed own-address Sent correspondence and consistent census classification. Transactional services require reviewed sender data. A platform or ESP observation alone is insufficient. Bulk goes to the existing unsubscribe review. Mixed classes, relay claims, multiple From identities and insufficient classification stay unresolved.

People and service rows have these statuses.

- `waiting` means neither direct migration, an explicit owner dismissal nor adequate quiet-history evidence establishes another status.
- `moved` requires the same exact sender in a selected comparison census, writing directly to an explicitly selected newer owned address in the rolling window. A forwarded copy is insufficient. Its message fingerprint, account, date and exact delivery address remain local evidence.
- `dismissed` records an exact confirmed owner choice. It does not remove uncertainties or grant unsubscribe authority. Undismissal restores the evidence-derived status and remains possible if the sender's current class changes.
- `quiet` requires no mail in the rolling window, a valid older last Message Date, complete acquisition and sufficient historical and current coverage, including a known Spam count.

Bulk and unresolved rows remain explicit groups. A bulk handoff refreshes the existing unsubscribe review, highlights matching subscriptions and clears every selection to Keep. It selects and approves nothing. Every account, receiving address and list retains Q20's separate approval boundary.

Known rows survive aging and census removal. A missing row retains its recorded history with an explicit unresolved reason. Absence from a newer census never proves quietness. A removed comparison account or changed ownership produces a blocker while allowing source selection to be repaired.

The report recommends only an owner retirement review when designation, selected comparisons, source completeness, ownership, historical coverage, current coverage, dates, classes and the waiting set support it. Missing Spam is unknown, not zero. Any unresolved source or bulk handoff blocks readiness. A dismissal never erases a date, identity or coverage blocker.

## Private ledger and sender data

The installed private `addresses.yaml` remains authoritative. Wind-down consumes its current reviewed projection and content version. It neither edits the ledger nor copies an editable ledger into its own aggregate. Service associations explain local observations. Ledger membership and retired filing status establish neither ownership, migration nor alias disabling. External source edits invalidate confirmations without requiring a manual ledger refresh.

`SenderData` is an installed, reviewed and versioned catalog. Transactional classification and settings links come from that catalog. The generic catalog is empty. The test-only fixture installs a synthetic reviewed service and settings link. No personal data is a public default. Missing links show the observed domain as text. Links with credentials, query strings, fragments or a non-HTTPS scheme fall back to the domain. The interface never launches or fetches them. A production sender-data installation remains later integration work.

## Durable decisions and disclosure

Encrypted account-state version 7 adds wind-down settings, known-row history, exact dismissal membership and confirmed decision records. Versions 1 through 6 load without inventing a designation, dismissal, direct-delivery fact or coverage. All account writers retain the aggregate through the existing revision-checked replacement boundary. Decision records retain the input binding, exact change and full receipt.

The account-operation mutex and authoritative ledger lock cover reload, derivation, exact review, source rechecks and decision save. Selected account stores remain locked during comparison and confirmation. Another process cannot modify their encrypted states through the cooperating store while the action runs. The inherited advisory ledger boundary cannot promise an atomic compare-and-swap against an uncooperative hand editor at the exact replacement instant. Completed edits are checked before confirmation and after report generation.

The decision HMAC binds the full current report, exact change, resulting report, all participating versions and displayed consequences. Altered, stale, replayed, cross-account and cross-purpose submissions fail. A source or private ledger edit invalidates an outstanding review. A process restart preserves exactly confirmed decisions. It does not approve an earlier unconfirmed preview.

A private local-file download and an agent-summary download have separate exact previews. Each export HMAC binds the full current report, selected disclosure and exact output bytes. Confirmation recomputes immediately before returning those bytes. A local-file receipt cannot authorize summary disclosure or a local decision. The interface verifies downloaded text against the displayed preview.

Private files include exact local observations and associations. Agent summaries contain counts, status and uncertainty, keyed placeholders for senders with at least three messages, and a grouped small-sender tail. They withhold raw addresses, service associations, links and ledger notes. This unit provides no clear-address or free-text disclosure selection. Existing census summaries remain a separate purpose. No data enters browser storage, URLs or background jobs. HTTP keeps the existing no-store, CSP, Host, Origin and launch-session protections.

## Local verification commands

Run all commands in the application checkout. Shared mutable fixtures run sequentially. Before replacement, verify the retained fake server's exact argv, cwd and binary SHA-256 against its metadata. Never use the production-default launcher for these checks.

After the fake port is free, `CARGO_PROFILE_TEST_OPT_LEVEL=1 node scripts/verify-winddown.mjs` builds the test-only launcher and verifies protected HTTP, receipts, exact output and actual SIGKILL/replacement. It retains a fake process and metadata in `.audit/winddown-retained.json`. Prior fixture directories remain intact.

With that matching process retained, `node ui/scripts/verify-winddown.mjs` drives the actual built Svelte interface with repository Playwright Chromium. It saves exact review, summary and narrow German screenshots under `.audit/winddown-browser/`. T3 preview is attempted first. This fallback is used only after the allowed preview failures.

`node scripts/winddown-report.mjs --account ACCOUNT --as-of UTC_SECONDS` prints the server's local terminal rendering. It contacts only the fixed local server and never prints its session token. Terminal output is private and can contain addresses and service associations. The synthetic verifier captures it and compares it byte for byte with the local-file preview. Browser download remains the explicit file-export path.
