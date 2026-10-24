# Census report semantics

The account interface derives reports from the saved encrypted census. It performs no provider reads and does not save plaintext report caches. Opening a report does not write a file. Explicit export downloads `census-summary.md` through the browser after review and confirmation. The full census and personal workflow remain unfinished.

## Counts and dates

Every report has an explicit UTC as-of timestamp and an analysis version. Counts describe all saved canonical messages. Date windows and monthly volume omit invalid, missing and future header dates and report their counts separately. Header dates use strict RFC 2822 parsing and years 1970 through 2100. A syntactically invalid calendar date stays unknown. Nonstandard dates are not guessed. Flags describe the current saved acquisition, rather than historical flags at the as-of time.

Received statistics exclude Sent messages. Sender and domain monthly volumes use header Date. Rolling 6, 12 and 13 calendar-month windows include their lower boundary and the as-of instant. Read rate is current `\Seen` count divided by saved received count. Flagged and answered counts use `\Flagged` and `\Answered`. The latest read message date is the newest valid header Date among currently Seen messages. It is not a recorded read timestamp. No actual read timestamp is available.

Gmail messages deduplicate by X-GM-MSGID across saved folders. Duplicate records union current folders, labels and flags. Any Sent direction makes that canonical message Sent. The first saved canonical header supplies other content and dates. Gmail arrivals use received canonical messages' INTERNALDATE, parsed in IMAP format and grouped in UTC. Raw saved-record and canonical-message counts remain separate. Other providers retain folder-local copies because Message-ID hashes alone do not prove message identity.

Acquisition and each folder retain their state and saved/inventory counts. A report is complete only when the account and every scanned folder are complete and their saved counts match inventories. Cancelled, failed, running and idle states remain partial. Complete describes the selected saved acquisition. Full Gmail label topology and excluded Spam/Trash counts are not available.

Own-address candidates and their evidence remain local. Confirmation records ownership only. Confirmed registrable own domains appear in local details. This assigns no business role, signup policy, accounting target or forwarding permission.

## Registrable domains

`psl = "=2.1.245"` pins the generated Public Suffix List snapshot in code, with its exact crate checksum in Cargo.lock. The immutable data identifier is that package version, rather than a guessed upstream publication date. The package's recorded source commit is `9246c0683216b5912ef3f4340e259208b65434fb`. Its generated `src/list.rs` SHA-256 is `0513e49566e252cad1f2def4d308a24fcb3104aa0f2c933194afa38986e858cf`.

The [psl package](https://crates.io/crates/psl/2.1.245) compiles the [Public Suffix List](https://publicsuffix.org/list/) into Rust. The package declares MIT or Apache-2.0 licensing. The upstream list uses MPL-2.0, as stated in its [source header](https://publicsuffix.org/list/public_suffix_list.dat). ICANN and private suffix rules, including wildcard and exception rules, are included. Dependency installation does not perform a runtime list update. Updating the pinned package requires an analysis-version change and renewed verification.

Hostnames pass IDNA conversion to ASCII, lowercase normalization and DNS label validation. IP literals and malformed domains stay unknown. For example, `mail.shop.co.uk` groups as `shop.co.uk`. These are derived report fields. Existing encrypted snapshot schema version 1 is unchanged and its previous optional registrable-domain field remains untouched.

## Conservative sender classes

Classification first groups each received message's available evidence, then groups by its observed effective address. A sender with conflicting message classes stays unknown.

- `list_with_people` requires List-Id and a List-Post value containing a mailto posting target.
- `bulk` requires exactly one List-Unsubscribe-Post value equal to `List-Unsubscribe=One-Click` and an HTTPS target in List-Unsubscribe. It counts header syntax only. DKIM coverage remains unknown and no request is sent.
- `platform` requires a known ESP observation or an Auto-Submitted value other than `no`.
- `person` requires a Sent message from a confirmed own address to that sender in To, Cc or Bcc, without list, unsubscribe or Precedence markers on the received message.
- `unknown` covers insufficient or mixed evidence. List-Id alone, an In-Reply-To field alone, an unfamiliar sender, or a bulk-looking address does not establish a class.

ESP observations currently cover the researched evidence available in the acquired fields. These are the Feedback-ID AmazonSES suffix and exact or subdomain Return-Path matches for amazonses.com, mandrillapp.com and klaviyomail.com. Each observation is a typed claim, not a verified platform identity or a topic category. No new vendor headers or provider reads are introduced.

Raw From addresses and relay-claimed originals remain separate local fields. Relay claims may supply an effective grouping address, but they establish no authenticated identity. Relay-only domains never become clear-value candidates. Authentication-Results and DKIM fields remain local evidence. No authserv-id is trusted and no signature is correlated with a verified pass.

Subject keywords are lowercase alphanumeric tokens of at least three characters, with every numeric character replaced by `#`. A token counts at most once per canonical received message. It appears only after three distinct messages from that sender contain it. Full Subjects remain in encrypted snapshots and local header detail. Keywords always become keyed placeholders in summaries, even for approved organisational domains.

## Reviewed summary boundary

The interface proposes only registrable domains of bulk or platform senders with at least three messages and no relay claim. Domains observed on own-address candidates or on any person, discussion-list or unknown sender are excluded. The owner can untick every proposed domain before preview. This conservative candidate set is not a claim that every organisation is detected.

Only the selected candidate domain strings may appear in clear text. Addresses, display names, Subjects, raw header values, unsubscribe URIs, folders, Gmail labels, List-Ids, recipient addresses and keywords cannot enter the clear-value list. People and unknown senders use stable account-scoped keyed placeholders. Confirmed own addresses use separate local-part and domain placeholders. Small senders pool into a long tail. Domain-volume aggregates use approved domains or domain placeholders.

`report` accepts only an account ID and as-of time. `summary/review` also accepts the report version and selected opaque candidate IDs. It reloads the existing census and derives all values itself. Caller-supplied text, output names, hosts, URLs, backends and authorization fields are rejected. An account-scoped HMAC binds the report version to the entire saved census, as-of instant, provider mode and analysis version. A second HMAC receipt binds that version, the sorted selection and the exact Markdown preview.

`summary/export` requires that same selection, version, receipt and an explicit true confirmation. It reloads and re-derives the data under the account-operation lock. Changed census data, ownership, selection, receipt or version fails with a generic error. The fixed export name carries no account or private metadata. The UI clears its preview and confirmation when the selection or as-of time changes, and requires fresh review after refreshing. Approval is not persisted and must be reviewed again after reload.

This is a local web confirmation and browser download. It does not implement the wider agent workflow, its native action-naming confirmation, agent folders, proof/ruleset exports, proposal review or background access. No report or classification enables live writes.

## Remaining census work

- Owner-run runtime evidence for the OS credential adapters, mailbox.org and Gmail, provider restrictions, throttling, filesystem behavior and recovery. Synthetic tests do not establish those facts.
- Windows private ACL enforcement and Windows/Linux runtime validation, including directory synchronization. Current Unix permissions remain enforced.
- Credential update, explicit confirmed account forgetting, key rotation, lost-key recovery, interrupted-creation orphan cleanup and lifecycle UI. A missing key must continue to stop without replacement or deletion.
- Full Gmail label topology and excluded-folder counts, including Spam counts needed by the later wind-down workflow.
- Trusted authentication provenance and correlated passing DKIM signatures, including coverage of both unsubscribe headers. All such coverage stays unknown.
- Scale verification and possible persistence changes beyond whole-snapshot rewrites, the 128 MiB snapshot cap and existing acquisition bounds. Report derivation currently holds the snapshot and derived data in memory.
- Takeout import, further provider and credential adapters, and broader platform work remain separate units.
- Wider agent integration and its native confirmation boundary. Clear exports currently support organisational domains only. Other values intentionally remain placeholders.
- Unsubscribe logs and backlog journals belong to their concrete workflows and remain absent. Rules, invoice forwarding, address ledger, unsubscribe execution and full wind-down statuses remain later personal features.

Provider OX coexistence, exact recipient semantics, forwarding behavior and accounting intake acceptance remain owner-run evidence. Business roles, the signup domain/mechanism, invoice sender approvals and accounting targets remain owner decisions. Own-address confirmation and classifier observations do not settle any of them.
