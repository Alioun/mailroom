# Invoice reports and synthetic send-on

This slice provides destination-level automatic-copy safety, a protected missed-invoice report, and exact synthetic send-on. Every production write remains denied. It does not establish provider readiness or personal daily readiness.

## Copy and evidence semantics

Compiler semantics `mailroom-sieve-subset-v3` and evaluator semantics `parsed-emitted-sieve-subset-v3` replace the earlier per-rule duplicate behavior. For each destination, the compiler records prior trigger intent and excludes it from subsequent rules before emitting redirects. Same-message suppression therefore does not depend on duplicate state becoming visible during the message. A shared destination handle guards repeated Message-IDs across later synthetic arrivals. Targets compare with ASCII case folding for copy suppression, consistent with the existing address matcher. Redirects preserve the configured destination spelling. Earlier per-rule cache entries do not populate the new target handles. No real deployment or cache migration is authorized by this change.

Stable rule IDs attribute the selected copy. Distinct destinations remain independent. Filing still selects one folder after copying. Unknown or mixed roles retain Inbox with no copy. Invoice-address triggers are independent of sender and bulk evidence. Approved senders still require business eligibility. Keyword forwards remain omitted in full because the installed compiler has no trustworthy MIME predicate. Gmail backlog emits no automatic invoice forward.

Proof predictions and prepared Sieve downloads are separate from observed transport handoff. The synthetic transport's acknowledgment establishes only its own exact fake effect. Real handoff and accounting acceptance stay unknown. Qonto receipt-body conversion means a body-only receipt is not automatically an unusable submission.

## Protected report

`/api/accounts/invoices` derives its report from the current encrypted census, immutable compilation, and invoice records on each open. No timer generates it. The selected account shows a count badge, business candidates, unresolved roles, rule attribution, named targets, evidence reasons, and unapproved sender suggestions. Suggestions never edit rules, approve senders, or classify business recipients.

Known content and attachment bytes, filename hints, and unknown evidence have separate types. Missing headers do not prove attachment absence, link-only content, or ZUGFeRD. Only the installed fake adapter supplies the seven PDF, XML, receipt-body, link-only, filename-hint, unsupported-format, and unknown-content fixtures. Ordinary accounts retain unknown recipient and MIME provenance.

Reports bind state revision, exact contents, compilation, and observations through a full account HMAC. Changed state invalidates send previews. A stale report remains visible until refreshed, but cannot authorize a changed send. Census refresh followed by opening the account derives a new report.

## Exact send and recovery

Account-state v4 adds a concrete invoice aggregate. Versions 1, 2, and 3 migrate with no invented invoice evidence or approval and preserve existing fields. Original synthetic MIME, attachments, targets, and send plans remain in authenticated encrypted state. They do not enter summaries, account metadata, or browser storage.

A dedicated full HMAC binds the account, complete state, source location and Message-ID hash, original content and attachments, named target, fixed approved synthetic sender, report selection and version, semantics, and confirmation action. Confirmation reloads and recomputes under account-operation exclusion and the existing store lock. The HTTP request cannot replace the content, sender, adapter, or endpoints. A proof or backlog receipt is not send authority.

Only server-installed synthetic transport and action-naming synthetic confirmation adapters execute. The server persists intent, independently persists the fake received plan under separately derived encryption keys, and then records observation. Process replacement turns an unobserved intent into uncertainty. Recovery reconciles an exact independently durable fake acknowledgment. A missing acknowledgment stays uncertain and never triggers a blind retry. Completed sends have no Undo. An unfinished invoice operation blocks starting a backlog operation and another invoice send.

The mail-app action returns preparation containing the original MIME and target. It launches nothing. There is no blank compose substitution. A functioning native forward-original integration, true OS confirmation, approved real sending identities, and SMTP are incomplete. The checkbox explicitly confirms a synthetic action only.

## Verification and limits

`scripts/verify-invoices.mjs` drives real protected HTTP. `scripts/verify-invoice-restart.mjs` drives the test-only persistent fake fixture and actual SIGKILL and process replacement before submission and after durable effect before observation. `ui/scripts/verify-invoices.mjs` drives the built interface, stale-state rejection, reload, recovered process state, German controls, CSP, and empty browser storage. The restart verifier retains its fake fixture and process in local `.audit/invoice-retained.json`.

The independent Sieve verifier covers overlapping triggers, destination aliases, and distinct destinations in fresh caches. It does not establish persistent provider cache behavior, expiry, concurrency, actual recipient provenance, or Qonto acceptance. Synthetic proof ordering does not reproduce historical arrival order.

Real original-message acquisition, trusted MIME and recipient evidence, deployed-engine observations, arbitrary accounting profiles, real native and SMTP adapters, conflict repair, and scale remain incomplete. Sources are bounded synthetic fixtures. The send history stops at 64 records without pruning. A before-submission crash can leave an uncertain operation blocked until a future inspection workflow exists. Synthetic source reconstruction does not prove real raw-mail preservation. Existing acquisition, report, backlog, provider, and personal-workflow limitations continue. Backlog Cancel remains a separate pending owner decision.
