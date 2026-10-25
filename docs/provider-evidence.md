# Read-only acquisition evidence

The private IMAP adapter retains authenticated-session advertisements, the complete returned LIST, and current mailbox counts. These observations grant no authentication, recipient, Sieve, or write authority. Production endpoints remain fixed and TLS verifies certificates and hostnames.

## Protocol observations

The command choices follow these primary specifications.

- [RFC 3501 section 6.3.2](https://www.rfc-editor.org/rfc/rfc3501#section-6.3.2) defines read-only EXAMINE. Selected folders still require nonzero UIDVALIDITY and a complete bounded UID SEARCH matching EXISTS. Metadata and explicit BODY.PEEK header fields follow. No body or attachment is requested.
- [RFC 3501 section 6.3.10](https://www.rfc-editor.org/rfc/rfc3501#section-6.3.10) defines STATUS and MESSAGES. STATUS does not select the mailbox or change message state. Optional counts use `STATUS "mailbox" (MESSAGES)` on excluded selectable mailboxes. They never request headers or bodies.
- [RFC 3501 section 6.3.8](https://www.rfc-editor.org/rfc/rfc3501#section-6.3.8) defines LIST wildcards and permits hidden mailboxes. A complete returned LIST is not proof that every server mailbox was exposed.
- [RFC 6154 section 2](https://www.rfc-editor.org/rfc/rfc6154#section-2) defines special-use attributes. Multiple mailboxes may share a role. All may omit Junk and Trash. [RFC 5258 section 3](https://www.rfc-editor.org/rfc/rfc5258#section-3) distinguishes selection and return options. The adapter requests `LIST "" "*" RETURN (SPECIAL-USE)` when advertised, without a special-use selection filter.

The installed `imap` client owns response and literal framing. The `imap-proto` parser produces typed CAPABILITY, LIST and STATUS responses. Advertisements are normalized into a bounded IMAP capability set. Delimiters retain NIL separately from a single delimiter character. Wire mailbox names and decoded modified-UTF7 names remain local. Roles come from parsed attributes, never translated names. All parsed LIST attributes are retained, including extensions.

NoSelect and NonExistent mailboxes are never examined or counted. Junk, Trash and Drafts remain outside ordinary header matching. Gmail selects its unique selectable All Mail folder when available. Multiple All Mail candidates fail required selection rather than silently choosing one. Other Gmail labels stay visible as excluded topology. The scanner never sums overlapping label counts.

At most 100 excluded selectable folders receive optional count requests per attempt, with Junk candidates first. Other counts retain an explicit limit reason. Each command keeps the existing 4 MiB response budget and 15-second timeout. LIST permits at most 10,000 folders, with bounded names and attributes. Capability sets permit 256 entries of at most 256 bytes. Counts accept exactly one matching STATUS response and one explicit MESSAGES value. Zero is an observation. Missing, denied, malformed, unavailable and conflicting responses remain unknown. Some malformed framing is surfaced by the installed client as an unavailable command response. That failure does not become zero.

## Attempts, checkpoints and successful snapshots

`Census.acquisition` holds one latest attempt and one last successful snapshot. Both carry adapter semantics and version. Attempts have random observation identities, provider identity, timestamps, session advertisements and optional partial checkpoints. Snapshots bind the account, provider, attempt, LIST observation time, wire and decoded names, delimiters, attributes, roles, selectability, selection reasons, count sources and times, UIDVALIDITY and selected-read completeness.

A checkpoint can contain useful complete-LIST observations while selected header reads remain incomplete. It cannot replace the successful snapshot. Connection or LIST failures retain the previous successful facts as older. Retry starts a new attempt and re-inventories the server. Reopening a running attempt marks it interrupted without inventing a completion time. Final publication saves a candidate only after required selected reads finish. Optional count uncertainty does not invalidate otherwise complete selected reads.

The legacy `folder_roles` map is a compatibility projection of the last successful topology. Success replaces it together with the snapshot, removing vanished roles. Failure leaves that projection unchanged and visibly older. Per-folder message checkpoints still resume holes and reset on UIDVALIDITY changes. Complete acquisition refers to selected reads, not continuous historical retention or excluded message contents.

Local views show the latest attempt separately from successful observations and their age at the view's timestamp. They show selected and excluded folders separately. A failed refresh cannot present an old Spam count as newly observed. Multiple Junk-role candidates make the aggregate unknown, while their individual observations remain visible. No role also means unknown. A restored account's captured observations remain historical in protected account and report views. Restored-account write holds stay enforced.

Wind-down consumes an unambiguous current protocol count and its observation source and timestamp. Restored accounts cannot supply current comparison observations until their existing reconciliation hold is resolved. It never fills historical coverage, direct-versus-forwarded delivery or DKIM coverage. Existing explicitly installed synthetic wind-down fixtures remain separate. Current count totals do not establish a rolling 12/13-month history. Raw topology and capabilities stay out of agent summaries. Local report and proof bindings already cover the whole Census or AccountState, so an evidence change invalidates prior approvals even when executable rules are unchanged.

## Persistence and recovery

Internal encrypted-state schema 8 adds these facts. Schemas 1 through 7 still decode with empty acquisition evidence. The later proposal aggregate advances new saves to schema 9. Both single-account and selected-account recovery validators now accept schemas 1 through 10. Older readers reject the newer schema instead of silently ignoring new semantics. Outer recovery envelope versions and resource bounds remain unchanged. See [Recovery bundle format](recovery-format.md).

Every census save preserves the other AccountState aggregates through the existing authenticated whole-state save. Rotation and recovery carry the captured facts without changing stable identities. Restore changes approval authority and retains the reconciliation hold. A passive scan cannot clear it. No reconciliation bypass is installed.

## Local verification and limits

Rust tests install local fake TLS connections through a private test-only factory. Production code has no endpoint override or test-installation route. The test server fixture is compiled only into the test executable. Its protected HTTP scan uses `Accounts::scan`, `ReadOnlyImap`, the ordinary scanner and the authenticated encrypted store. Test roots contain invented data only.

`scripts/verify-provider-evidence.mjs` builds and starts that fixture. It verifies ciphertext reopening in a separate process, server replacement, current and failed refreshes, topology changes, zero and ambiguous counts, conservative saved-account proof, stale report and proof rejection, HTTP protections and redacted wire commands. It retains paired process metadata under `.audit/provider-evidence-retained.json`. Stop and verify that process before rebuilding its executable. `ui/scripts/verify-provider-evidence.mjs` drives the actual built UI against the retained fixture, including English, German, failed refresh, reload, reports and narrow layout. Existing recovery and lifecycle verifiers remain applicable.

These checks establish local fake-server behavior. Actual provider behavior, credentials, protected Keychain access, protected native launch and authenticated IPC remain unimplemented or unperformed. No provider query or mutation is authorized by these facts. ManageSieve, deployment, trusted recipient and DKIM provenance, historical delivery evidence, personal-scale acceptance and daily readiness remain later work.

[Organisational filing proposals](organisational-proposals.md) now consume saved observations for local review. They add no provider or recipient authority.
