# Fixture-owned ManageSieve transactions

The private production-compiled adapter implements CHECKSCRIPT, HAVESPACE, PUTSCRIPT and SETACTIVE over the existing verified STARTTLS session. There is no production installation. Ordinary synthetic mode and a discovery-only installation do not grant write authority. HTTP callers select an account and an action. They cannot select bytes, target names, endpoints, ownership, TLS roots or trust context.

A separate construction-only fixture source owns one exact existing target and ownership epoch. Its non-deserializable write permit binds that account and installed source identity. Replacement supports an owned active target, or an owned inactive target with no other active script. A missing target or foreign active selection rejects. No ownership inference, foreign-rule composition, include placement, script creation, deletion, rename or standalone fallback exists.

## Exact review and effects

Preview derives the immutable candidate and fresh proof from the server-owned saved workspace. Saved-account recipient authority remains conservative. The preview displays complete before/after byte and digest review, active selection, proof gate, pin results, warnings, exact effect order and limitations. Replacing an active script affects future mail immediately. Uploading an inactive script does not activate it. Explicit activation is a separate command.

Dedicated purpose-separated receipts bind the entire saved state, workspace/census revisions, ledger authority, proof binding and semantics, exact executable, complete observed script inventory and bytes, capabilities, active selection, installed source, ownership epoch, credential/key/recovery context and operation identity. Compensation has a distinct purpose and original journal binding. Confirmation recomputes proof and actual provider observations, rechecks external ledger freshness and consumes approval through an encrypted revision before effects. Download, altered, stale, replayed and cross-purpose approvals cannot grant these writes.

CHECKSCRIPT validates syntax without storing. Its success is not delivery, coexistence, provider acceptance of a subsequent upload, or independent Sieve-engine verification. Unsupported protocol semantics and command refusal have no upload fallback. HAVESPACE reserves nothing and cannot mask a later PUTSCRIPT quota refusal. Client literals count exact octets, including CRLF and UTF-8, without newline conversion. The adapter never sends credentials before verified TLS and never follows referrals.

## Journal and recovery

Concrete transaction and step records live in encrypted whole-account state. Each effect records its exact preimage, intended postimage and intent before sending. Acknowledgment is persisted separately from independent list/get readback. Outcomes distinguish intent, acknowledgment, observation, refusal, uncertainty, conflict and matching preimage. Validation failures retain the previous successful discovery baseline. Effect intent makes prior discovery observations historical until a fresh successful discovery.

Timeout, EOF, cancellation or process death after an effect might have arrived leaves uncertainty. Reopen never calls the operation complete, resends an effect or continues activation. Explicit reconciliation uses actual TLS observations. A matching postimage establishes observed state, without attribution. A matching preimage does not establish historical non-occurrence and requires renewed exact review. A third state or changed authority blocks further effects. Repeated status and converged reconciliation do not save the primary journal again or issue effect commands.

Stop cancels further commands and preserves completed effects. It never compensates automatically. Separate compensation preview compares fresh provider observations with the eligible recorded postimage. Confirmation restores only the original target bytes and original active selection. Returning to an originally empty active selection displays and separately confirms the empty SETACTIVE effect. Interrupted compensation requires reconciliation and another exact approval before remaining effects. External edits block compensation. Compensation cannot reverse forwards or mail processed while a script was active.

ManageSieve has no compare-and-swap or multi-command transaction. Another editor can change bytes after the final read. Local locks cannot prevent that race. The runtime verifier injects an edit at this boundary and demonstrates the overwrite rather than claiming preservation across it.

## Persistence and limits

The durable TLS fixture stores provider script bytes, active selection, TLS key/root, socket profile, ownership epoch and command-name-only counts in the existing independent encrypted synthetic mailbox aggregate. Application death therefore preserves provider effects and the exact installed authority. It adds no lifecycle-owned file or encrypted store. Existing rotation, forgetting and recovery inventories own the same four stores. Restored accounts retain their read-only reconciliation hold.

Internal schema 11 advances the account loader/stager, single and selected recovery validators and browser manifest validator together. Schemas 1 through 10 invent no transactions, write invalidation or fixture authority and reject new semantics under their version numbers. Transaction history never silently prunes. Limits are four operations, two effect steps per operation, eight script names, 64 KiB total bytes per observed inventory, 32 KiB candidate and 4 MiB serialized journal. Reaching a limit fails and preserves history. The wire adapter independently limits each script to 1 MiB and total input and output to 4 MiB per session. Existing response, absolute deadline and cancellation bounds remain.

## Verification

Run these workflows sequentially, stopping the identity-verified retained fake before a build.

- `cargo fmt --check`
- `cargo check --locked`
- `CARGO_PROFILE_TEST_OPT_LEVEL=1 cargo test --locked`
- `npm --prefix ui run check`
- `npm --prefix ui run build`
- `node scripts/verify-managesieve-transactions.mjs`
- `node ui/scripts/verify-managesieve-transactions.mjs`

The HTTP workflow leaves safe retained metadata in `.audit/managesieve-transactions-retained.json`. The built-browser workflow updates that same identity after actual process replacement. Use `python3 .audit/provider-evidence-stop.py .audit/managesieve-transactions-retained.json` before the next build. The scripts preserve all historical fixture directories. They never print authentication payloads or script contents. Private byte review stays escaped in memory and out of browser storage and screenshots.

The fixture establishes protocol and application behavior. Docker/Pigeonhole remains unavailable and is not retried or provisioned. Independent Sieve execution, delivery, coexistence, actual provider facts, production access, native executable continuity, protected Keychain and daily-use readiness remain unverified. No native confirmation is claimed by synthetic confirmation controls.
