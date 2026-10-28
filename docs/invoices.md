# Invoice reports and synthetic send-on

This slice provides destination-level automatic-copy safety, a protected missed-invoice report, and exact synthetic send-on. Every production write remains denied. It does not establish provider readiness or personal daily readiness.

## Copy and evidence semantics

Compiler semantics `mailroom-sieve-subset-v3` and evaluator semantics `parsed-emitted-sieve-subset-v3` replace the earlier per-rule duplicate behavior. For each destination, the compiler records prior trigger intent and excludes it from subsequent rules before emitting redirects. Same-message suppression therefore does not depend on duplicate state becoming visible during the message. A shared destination handle guards repeated Message-IDs across later synthetic arrivals. Targets compare with ASCII case folding for copy suppression, consistent with the existing address matcher. Redirects preserve the configured destination spelling. Earlier per-rule cache entries do not populate the new target handles. No real deployment or cache migration is authorized by this change.

Stable rule IDs attribute the selected copy. Distinct destinations remain independent. Filing still selects one folder after copying. Unknown or mixed roles retain Inbox with no copy. Invoice-address triggers are independent of sender and bulk evidence. Approved senders still require business eligibility. Keyword forwards need the engine's Sieve `mime` extension and a PDF or XML part; see `docs/rules-proof.md`. Gmail backlog emits no automatic invoice forward.

Proof predictions and prepared Sieve downloads are separate from observed transport handoff. The synthetic transport's acknowledgment establishes only its own exact fake effect. Real handoff and accounting acceptance stay unknown. Qonto receipt-body conversion means a body-only receipt is not automatically an unusable submission.

## Protected report

`/api/accounts/invoices` derives its report from the current encrypted census, immutable compilation, and invoice records on each open. No timer generates it. The selected account shows a count badge, business candidates, unresolved roles, rule attribution, named targets, evidence reasons, and unapproved sender suggestions. Suggestions never edit rules, approve senders, or classify business recipients.

Known content and attachment bytes, filename hints, and unknown evidence have separate types. Missing headers do not prove attachment absence, link-only content, or ZUGFeRD. Only the installed fake adapter supplies the seven PDF, XML, receipt-body, link-only, filename-hint, unsupported-format, and unknown-content fixtures. Ordinary accounts retain unknown recipient and MIME provenance.

Reports bind state revision, exact contents, compilation, and observations through a full account HMAC. Changed state invalidates send previews. A stale report remains visible until refreshed, but cannot authorize a changed send. Census refresh followed by opening the account derives a new report.

## Exact send and recovery

Account-state v4 adds a concrete invoice aggregate. Versions 1, 2, and 3 migrate with no invented invoice evidence or approval and preserve existing fields. Original synthetic MIME, attachments, targets, and send plans remain in authenticated encrypted state. They do not enter summaries, account metadata, or browser storage.

A dedicated full HMAC binds the account, complete state, source location and Message-ID hash, original content and attachments, named target, fixed approved synthetic sender, report selection and version, semantics, and confirmation action. Confirmation reloads and recomputes under account-operation exclusion and the existing store lock. The HTTP request cannot replace the content, sender, adapter, or endpoints. A proof or backlog receipt is not send authority.

Only server-installed synthetic transport and action-naming synthetic confirmation adapters execute. The server persists intent, independently persists the fake received plan under separately derived encryption keys, and then records observation. Process replacement turns an unobserved intent into uncertainty. Recovery reconciles an exact independently durable fake acknowledgment. A missing acknowledgment stays uncertain and never triggers a blind retry. Completed sends have no Undo. An unfinished invoice operation blocks starting a backlog operation and another invoice send.

The synthetic checkbox confirms a synthetic action only. There is no blank compose substitution.

## Real adapters

Original-message acquisition reads one message through the read-only IMAP session. It runs `EXAMINE`, checks the folder's UIDVALIDITY, reads `RFC822.SIZE` and refuses more than 25 MiB before fetching `BODY.PEEK[]`. No command sets `\Seen`. `original::Original` parses the exact bytes into census headers, Message-ID and every non-body part with its name and content type. `invoice::record_original` keeps the original, encrypted in account state, only when its Message-ID matches the census record. Acquired evidence feeds the report and the keyword-forward MIME test in proof.

SMTP submission uses implicit TLS or STARTTLS with certificate verification. A server that does not offer STARTTLS, or that sends bytes after its STARTTLS reply, is refused before anything else is sent. AUTH PLAIN goes only over TLS. The envelope sender and recipient are the confirmed plan's sending address and target. The DATA payload is the exact previewed original, dot-stuffed on the wire. A message with bare LF or CR is refused. The intent is saved before submission and the outcome after. A refusal before message data records `not_sent`. A 2xx reply after the data records `completed`, another reply records `not_sent`, and a missing reply records `uncertain`. Only reply codes and enhanced status codes are kept. Nothing is ever retried, and an uncertain send still blocks the next one.

`send::smtp_secret` is the only code that handles the account password. It reads it through `CredentialBackend::get`, builds one zeroizing AUTH PLAIN line, writes it and wipes every buffer. It returns only the reply code and wipes the reply text. Tests use a dummy password and check it never reaches output, argv, errors, saved state or fake-server logs.

There is no invoice Mail handoff. Mail can find a message only by Message-ID across every account and picks the sender itself, so it cannot be held to the confirmed original bytes and sending identity. `preview` refuses `SendMode::MailAppHandoff`; the mode stays only so older records load. Forward from Mail yourself when SMTP is not configured.

## Production routes

The invoice route's `acquire` action runs original-message acquisition through the account's IMAP login, as a census does. `preview` and `send` with `mode: smtp` use the real adapter: SMTP goes to the provider's submission server (`smtp.mailbox.org` or `smtp.gmail.com`, port 465, implicit TLS) with the account login as sending identity and its password reference read by `send::smtp_secret`. It needs an acquired original. The confirmation boundary names it `smtp_send`, binding account, operation, target, sending identity and the SHA-256 of the exact message bytes. Socket deadlines hold from the first byte, the TLS handshake included, and every SMTP reply line is held in a zeroizing buffer sized once, so an echoed credential is wiped on every return path.

Every wired action first asks `send::wiring::Endpoints` for a `policy::WritePermit`, before any repository, credential, confirmation or network access, and every real adapter requires that permit. `policy::authorize` denies every account write, so production answers 403 with the policy's reason. Tests install loopback fakes through `policy::fake_write_permit`, which exists only in test builds and refuses any endpoint off loopback.

`resolve` records the owner's finding for an uncertain SMTP send as `sent` or `not_sent`, after checking the Sent folder. The route offers census messages in Sent mail with the original's Message-ID as `sent_evidence`, read from the saved census only. Nothing is resent. A send recorded as not sent can be sent again only through a fresh preview and confirmation, whose receipt differs because the state changed. Only an uncertain send can be resolved.

## Verification and limits

`scripts/verify-invoices.mjs` drives real protected HTTP. `scripts/verify-invoice-restart.mjs` drives the test-only persistent fake fixture and actual SIGKILL and process replacement before submission and after durable effect before observation. `ui/scripts/verify-invoices.mjs` drives the built interface, stale-state rejection, reload, recovered process state, German controls, CSP, and empty browser storage. The restart verifier retains its fake fixture and process in local `.audit/invoice-retained.json`.

The independent Sieve verifier covers overlapping triggers, destination aliases, and distinct destinations in fresh caches. It does not establish persistent provider cache behavior, expiry, concurrency, actual recipient provenance, or Qonto acceptance. Synthetic proof ordering does not reproduce historical arrival order.

Production wiring of the real adapters, trusted recipient evidence, deployed-engine observations, arbitrary accounting profiles, conflict repair, and scale remain incomplete. Sources are bounded synthetic fixtures. The send history stops at 64 records without pruning. A before-submission crash can leave an uncertain operation blocked until a future inspection workflow exists. Synthetic source reconstruction does not prove real raw-mail preservation. Existing acquisition, report, backlog, provider, and personal-workflow limitations continue. Backlog Cancel stops new sorting batches and retains completed changes. Backlog Undo requires its own preview and confirmation.
