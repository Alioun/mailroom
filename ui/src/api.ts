import { invoiceDiagnostic, invoiceHandoff, invoiceRole } from './invoice-diagnostics'
import { z } from 'zod'
import { evidenceView } from './acquisition-evidence'
import { proposalRationale, proposalReason } from './proposal-diagnostics'

z.config({ jitless: true })

const destination = z.discriminatedUnion('kind', [z.object({ kind: z.literal('inbox') }), z.object({ kind: z.literal('folder'), name: z.string() })])
const documents = z.object({ preset_yaml: z.string(), override_yaml: z.string() })
const pin = z.object({ account: z.string(), message_id_hash: z.string(), expected: destination })
const deliverySource = z.object({ account: z.string(), provider: z.string(), identity: z.string(), profile: z.string(), epoch: z.string(), authority: z.string() })
export const recipientEvidence = z.union([z.enum(['unknown', 'synthetic_delivered_to', 'synthetic_original_to']), z.object({ synthetic_envelope: z.object({ source: deliverySource }) })])
const sample = z.object({
  documents,
  context: z.object({ account: z.string(), engine: z.enum(['mailbox_sieve', 'gmail_backlog']), recipient_evidence: recipientEvidence, capabilities: z.array(z.string()) }),
  messages: z.array(z.object({ id: z.string(), headers: z.string(), current_folder: z.string() })),
  as_of: z.number(), pins: z.array(pin), baseline: z.enum(['current', 'previous']), previous: documents.nullable(),
})
const preview = z.object({
  source: z.literal('local_census'), semantics: z.string(),
  artifact: z.object({ identity: z.string(), context: sample.shape.context, executable: z.string(), trace: z.array(z.string()), omissions: z.array(z.object({ rule: z.string(), reason: z.string() })), folders: z.array(z.string()), effective: z.array(z.object({ id: z.string(), origin: z.string(), enabled: z.boolean(), accounts: z.array(z.string()) })) }),
  counts: z.object({ messages: z.number(), filed: z.number(), inbox: z.number(), copies: z.number(), copied_messages: z.number(), review: z.number() }),
  decisions: z.array(z.object({ id: z.string(), message_id_hash: z.string().nullable(), subject: z.string(), from: z.string(), destination, matched_rule: z.string().nullable(), review: z.boolean(), mark_read: z.boolean(), delivery_status: z.enum(['known', 'unavailable', 'unsupported', 'ambiguous', 'incompatible']), keyword_additions: z.array(z.string()), observed_flags: z.array(z.string()).nullable(), delivery_tags: z.array(z.object({ rule: z.string(), status: z.enum(['match', 'no_match', 'unknown', 'omitted', 'held_tagged']), reason: z.string() })), copies: z.array(z.object({ rule: z.string(), recipient: z.string() })), current_folders: z.array(z.string()), copy_eligibility: z.array(z.object({ rule: z.string(), status: z.string(), reason: z.string() })) })),
  baseline: sample.shape.baseline, diff: z.array(z.object({ id: z.string(), before: z.array(z.string()), after: z.string() })),
  rules: z.array(z.object({ id: z.string(), matched: z.number(), candidates: z.number(), monthly: z.record(z.string(), z.number()), zero_match: z.boolean(), fully_shadowed: z.boolean() })),
  months: z.array(z.string()), unmatched_senders: z.record(z.string(), z.number()), pins: z.array(pin.extend({ status: z.enum(['pass', 'fail', 'stale', 'unknown']), actual: z.array(destination) })),
  gate: z.enum(['blocked', 'warnings']), warnings: z.array(z.string()), runtime_cross_check: z.string(),
})
const operation = z.enum(['preview', 'synthetic_census', 'connect', 'census', 'upload_rules', 'activate_rules', 'create_folder', 'move_messages', 'change_flags', 'change_forwarding', 'send_message', 'unsubscribe'])
const availability = z.array(z.object({ operation, effect: z.enum(['synthetic_read', 'account_read', 'account_write']), allowed: z.boolean(), reason: z.string() }))
export type Preview = z.infer<typeof preview>
export type Availability = z.infer<typeof availability>
export type Operation = z.infer<typeof operation>
export type Sample = z.infer<typeof sample>

async function responseJson(response: Response): Promise<unknown> {
  const value: unknown = await response.json()
  if (!response.ok) {
    const error = z.object({ error: z.string() }).safeParse(value)
    throw new Error(error.success ? error.data.error : `HTTP ${response.status}`)
  }
  return value
}
let sessionToken: Promise<string> | undefined
export async function localFetch(path: string, options: RequestInit = {}): Promise<Response> {
  sessionToken ??= fetch('/api/session').then(responseJson).then(value => z.object({ token: z.string().length(64) }).parse(value).token).catch(() => {
    sessionToken = undefined
    throw new Error('Local session unavailable. Reload the page.')
  })
  const headers = new Headers(options.headers)
  headers.set('X-Mailroom-Token', await sessionToken)
  return fetch(path, { ...options, headers })
}
export async function load() {
  const [a, b] = await Promise.all([localFetch('/api/sample'), localFetch('/api/availability')])
  return { sample: sample.parse(await responseJson(a)), availability: availability.parse(await responseJson(b)) }
}
export async function evaluate(input: Sample): Promise<Preview> {
  const response = await localFetch('/api/preview', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input) })
  return preview.parse(await responseJson(response))
}

const censusStatus = z.object({
  source: z.literal('synthetic'), state: z.enum(['idle', 'running', 'cancelled', 'complete', 'failed']),
  completed: z.number(), total: z.number(), error: z.string().nullable(), storage: z.string(), summary: z.string(),
  addresses: z.array(z.object({ address: z.string(), evidence: z.array(z.string()), confirmed: z.boolean() })),
  messages: z.array(z.object({ uid: z.number(), direction: z.enum(['received', 'sent']), subject: z.string(), sender: z.string(), dkim: z.literal('unknown') })),
})
export type CensusStatus = z.infer<typeof censusStatus>
export async function readCensus(): Promise<CensusStatus> {
  return censusStatus.parse(await responseJson(await localFetch('/api/census/demo')))
}
export async function censusAction(action: 'start' | 'cancel' | 'confirm', input: unknown): Promise<CensusStatus> {
  return censusStatus.parse(await responseJson(await localFetch(`/api/census/demo/${action}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input),
  })))
}

const account = z.object({ id: z.string(), provider: z.enum(['mailbox_org', 'gmail']), login: z.string() })
const accountStatus = z.object({
  synthetic: z.boolean(),
  job: z.object({ account: account.nullable(), progress: z.object({ state: z.enum(['idle', 'running', 'cancelled', 'complete', 'failed']), saved: z.number(), folders: z.number() }), error: z.string().nullable() }).nullable(),
})
const accountView = z.object({
  evidence: evidenceView,
  addresses: censusStatus.shape.addresses,
  census: z.object({ account: z.string(), acquisition_state: censusStatus.shape.state, folders: z.record(z.string(), z.object({
    uidvalidity: z.number(), state: censusStatus.shape.state,
    messages: z.record(z.string(), z.object({ uid: z.number(), headers: z.object({ subject: z.string(), direction: z.enum(['received', 'sent']) }) })),
  })) }),
})
export type Account = z.infer<typeof account>
export type AccountStatus = z.infer<typeof accountStatus>
export type AccountView = z.infer<typeof accountView>
async function accountPost(action: string, input: unknown): Promise<unknown> {
  return responseJson(await localFetch(`/api/accounts/${action}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input) }))
}
export async function readAccountStatus(): Promise<AccountStatus> { return accountStatus.parse(await responseJson(await localFetch('/api/accounts/status'))) }
export async function listAccounts(): Promise<Account[]> { return z.array(account).parse(await accountPost('list', {})) }
export async function connectAccount(input: { provider: Account['provider']; login: string; password: string }): Promise<AccountStatus> { return accountStatus.parse(await accountPost('connect', input)) }
export async function syntheticAccount(gmail = false): Promise<AccountStatus> { return accountStatus.parse(await accountPost(gmail ? 'synthetic-gmail' : 'synthetic', {})) }
export async function scanAccount(account: string): Promise<AccountStatus> { return accountStatus.parse(await accountPost('scan', { account })) }
export async function cancelAccount(): Promise<AccountStatus> { return accountStatus.parse(await accountPost('cancel', {})) }
export async function openAccount(account: string): Promise<AccountView> { return accountView.parse(await accountPost('open', { account })) }
export async function confirmAccount(account: string, address: string, confirmed: boolean): Promise<AccountView> { return accountView.parse(await accountPost('confirm', { account, address, confirmed })) }

const reportCounts = z.object({
  messages: z.number(), seen: z.number(), flagged: z.number(), answered: z.number(),
  monthly: z.record(z.string(), z.number()), invalid_dates: z.number(), future_dates: z.number(),
  latest_read_message_date: z.number().nullable(), first_date: z.number().nullable(), last_date: z.number().nullable(),
  last_12_months: z.number(), last_13_months: z.number(), read_last_6_months: z.number(),
})
const report = z.object({
  acquisition: evidenceView,
  version: z.string(), analysis_version: z.string(), as_of: z.number(), state: censusStatus.shape.state, complete: z.boolean(),
  folders: z.array(z.object({ folder: z.string(), state: censusStatus.shape.state, inventory: z.number(), saved: z.number() })),
  saved_records: z.number(), canonical_messages: z.number(), received: reportCounts, sent: z.number(),
  own_addresses: censusStatus.shape.addresses, own_domains: z.array(z.string()),
  senders: z.array(z.object({
    sender: z.string().nullable(), domain: z.string().nullable(), class: z.enum(['bulk', 'platform', 'list_with_people', 'person', 'unknown']), wrote_to_sender: z.boolean(), counts: reportCounts,
    raw_from: z.array(z.string()), relay_claims: z.array(z.string()), one_click_pair: z.number(), list_unsubscribe: z.number(), list_id_messages: z.number(), list_ids: z.array(z.string()),
    esp_observations: z.array(z.string()), folders: z.array(z.string()), gmail_labels: z.array(z.string()), recipient_addresses: z.array(z.string()), subject_keywords: z.record(z.string(), z.number()),
  })),
  domains: z.record(z.string(), reportCounts), gmail_arrivals: z.record(z.string(), z.number()), invalid_arrival_dates: z.number(), future_arrival_dates: z.number(),
  clear_candidates: z.array(z.object({ id: z.string(), domain: z.string() })), limitations: z.array(z.enum(['unknown_authentication', 'current_flags_and_dates', 'gmail_topology', 'subject_tokens', 'classification'])),
})
const summaryPreview = z.object({ summary: z.string(), receipt: z.string() })
export type CensusReport = z.infer<typeof report>
export type SummaryPreview = z.infer<typeof summaryPreview>
export async function readReport(account: string, as_of: number): Promise<CensusReport> { return report.parse(await accountPost('report', { account, as_of })) }
export async function reviewSummary(account: string, report: CensusReport, approved: string[]): Promise<SummaryPreview> {
  return summaryPreview.parse(await accountPost('summary/review', { account, as_of: report.as_of, version: report.version, approved }))
}
export async function exportSummary(account: string, report: CensusReport, approved: string[], receipt: string) {
  return z.object({ summary: z.string(), filename: z.literal('census-summary.md') }).parse(await accountPost('summary/export', { account, as_of: report.as_of, version: report.version, approved, receipt, confirmed: true }))
}

export type Documents = z.infer<typeof documents>
export type Pin = z.infer<typeof pin>

const workspaceInput = z.object({ documents, pins: z.array(pin), as_of: z.number(), baseline: sample.shape.baseline })
const exportInfo = z.object({ identity: z.string(), semantics: z.string(), proof_semantics: z.string(), exported_at: z.number(), provenance: z.string(), usable: z.boolean() })
const workspaceView = z.object({ revision: z.number(), workspace: z.object({ revision: z.number(), input: workspaceInput }).nullable(), exports: z.array(exportInfo), engine: sample.shape.context.shape.engine })
const proofPreview = z.object({ proof: preview, receipt: z.string().length(64), revision: z.number(), workspace_revision: z.number(), as_of: z.number(), download_available: z.boolean(), previous: exportInfo.nullable(), filename: z.literal('rules.sieve') })
export type WorkspaceView = z.infer<typeof workspaceView>
export type ProofPreview = z.infer<typeof proofPreview>
export async function openWorkspace(account: string): Promise<WorkspaceView> { return workspaceView.parse(await accountPost('workspace/open', { account })) }
export async function saveWorkspace(account: string, revision: number, input: z.infer<typeof workspaceInput>): Promise<WorkspaceView> { return workspaceView.parse(await accountPost('workspace/save', { account, revision, input })) }
export async function previewWorkspace(account: string): Promise<ProofPreview> { return proofPreview.parse(await accountPost('proof/preview', { account })) }
export async function downloadSieve(account: string, receipt: string) {
  return z.object({ filename: z.literal('rules.sieve'), sieve: z.string(), identity: z.string(), workspace: workspaceView }).parse(await accountPost('proof/download', { account, receipt, confirmed: true }))
}

const backlogOptions = z.object({ scope: z.enum(['inbox_unfiled', 'inbox', 'every_folder']), protect_flagged: z.boolean(), selected: z.array(z.string()).nullable(), intent: z.enum(['sort', 'archive_unmatched']).optional() })
const backlogIdentity = z.object({ folder: z.string(), uidvalidity: z.number(), uid: z.number(), message_id_hash: z.string(), gmail_id: z.string().nullable() })
const backlogOperation = z.discriminatedUnion('kind', [z.object({ kind: z.literal('imap'), method: z.enum(['uid_move', 'copy_uid_expunge']) }), z.object({ kind: z.literal('gmail'), add: z.array(z.string()), remove: z.array(z.string()) })])
const backlogPlan = z.object({
  account: z.string(), provider: account.shape.provider, source: z.string(), state_revision: z.number(), mailbox_revision: z.number().nullable(), semantics: z.string(),
  artifact: preview.shape.artifact, proof_gate: z.string(), options: backlogOptions,
  changes: z.array(z.object({ source: backlogIdentity, flags: z.array(z.string()), original_labels: z.array(z.string()), destination: z.string(), sender: z.string(), operation: backlogOperation })),
  excluded: z.record(z.string(), z.number()), unmatched: z.number(), review: z.number(), groups: z.record(z.string(), z.object({ count: z.number(), selected: z.boolean(), top_senders: z.array(z.tuple([z.string(), z.number()])) })), stopped_folders: z.array(z.string()), diagnostics: z.array(z.string()),
})
const backlogObservation = z.object({ destination: backlogIdentity, commands: z.array(z.string()) })
const backlogOutcome = z.discriminatedUnion('state', [z.object({ state: z.literal('pending') }), z.object({ state: z.literal('intent') }), z.object({ state: z.literal('applied'), observation: backlogObservation }), z.object({ state: z.literal('conflict'), reason: z.string() })])
const backlogUndo = z.discriminatedUnion('state', [z.object({ state: z.literal('not_requested') }), z.object({ state: z.literal('intent') }), z.object({ state: z.literal('restored'), observation: backlogObservation }), z.object({ state: z.literal('conflict'), reason: z.string() })])
const backlogRun = z.object({ id: z.string(), plan: backlogPlan, status: z.enum(['prepared', 'running', 'stopped', 'completed', 'failed', 'uncertain']), outcomes: z.array(backlogOutcome), undo: z.array(backlogUndo), undo_requested: z.boolean(), batches: z.number(), error: z.string().nullable() })
const backlogView = z.object({ protocol_installed: z.boolean().default(false), protocol_history: z.boolean().default(false), gmail_installed: z.boolean().default(false), gmail_history: z.boolean().default(false), runs: z.array(backlogRun), synthetic: z.boolean() })
const backlogPreview = z.object({ plan: backlogPlan, receipt: z.string(), execution_available: z.boolean() })
const backlogUndoPreview = z.object({ receipt: z.string(), eligible: z.array(z.number()), conflicts: z.record(z.string(), z.string()), warning: z.string() })
export type BacklogOptions = z.infer<typeof backlogOptions>
export type BacklogView = z.infer<typeof backlogView>
export type BacklogPreview = z.infer<typeof backlogPreview>
export type BacklogUndoPreview = z.infer<typeof backlogUndoPreview>
type BacklogAction = { action: 'open' } | { action: 'preview'; options: BacklogOptions } | { action: 'start'; options: BacklogOptions; receipt: string; confirmed: boolean } | { action: 'advance' | 'resume' | 'stop' | 'undo_preview' | 'resume_undo'; run: string } | { action: 'undo'; run: string; receipt: string; confirmed: boolean }
async function backlogPost(account: string, command: BacklogAction): Promise<unknown> { return accountPost('backlog', { account, command }) }
export async function openBacklog(account: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'open' })) }
export async function previewBacklog(account: string, options: BacklogOptions): Promise<BacklogPreview> { return backlogPreview.parse(await backlogPost(account, { action: 'preview', options })) }
export async function startBacklog(account: string, plan: BacklogPreview): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'start', options: plan.plan.options, receipt: plan.receipt, confirmed: true })) }
export async function advanceBacklog(account: string, run: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'advance', run })) }
export async function previewBacklogUndo(account: string, run: string): Promise<BacklogUndoPreview> { return backlogUndoPreview.parse(await backlogPost(account, { action: 'undo_preview', run })) }
export async function undoBacklog(account: string, run: string, receipt: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'undo', run, receipt, confirmed: true })) }
export async function resumeBacklogUndo(account: string, run: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'resume_undo', run })) }

export async function stopBacklog(account: string, run: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'stop', run })) }
export async function resumeBacklog(account: string, run: string): Promise<BacklogView> { return backlogView.parse(await backlogPost(account, { action: 'resume', run })) }

const invoiceAttachment = z.object({ name: z.string(), media_type: z.string(), bytes: z.array(z.number()) })
const invoiceEvidence = z.discriminatedUnion('kind', [z.object({ kind: z.literal('unknown') }), z.object({ kind: z.literal('filename_hint'), names: z.array(z.string()) }), z.object({ kind: z.literal('known'), body: z.enum(['receipt', 'link_only', 'other']), attachments: z.array(invoiceAttachment) })])
const invoiceSource = z.object({ id: z.string(), folder: z.string(), uidvalidity: z.number(), uid: z.number(), message_id_hash: z.string().nullable(), original: z.string(), evidence: invoiceEvidence })
const invoicePlan = z.object({ account: z.string(), state_revision: z.number(), report_version: z.string(), selection: z.string(), source: invoiceSource, content_digest: z.string(), target_name: z.string(), target: z.string(), sending_address: z.string(), semantics: z.string(), confirmation_action: z.string(), mode: z.string() })
const invoiceRow = z.object({ id: z.string(), subject: z.string(), sender: z.string(), role: invoiceRole, reasons: z.array(invoiceDiagnostic), evidence: invoiceEvidence, predicted_targets: z.array(z.string()), matching_rules: z.array(z.string()), copy_eligibility: z.array(z.object({ rule: z.string(), status: z.string(), reason: z.string() })), handoff: invoiceHandoff, accounting_acceptance: z.literal('unknown'), sender_proposal: z.string().nullable(), send_available: z.boolean() })
const invoiceReport = z.object({ version: z.string(), semantics: z.string(), revision: z.number(), artifact: z.string().nullable(), complete: z.boolean(), count: z.number(), eligible_business: z.number(), unresolved: z.number(), rows: z.array(invoiceRow), warnings: z.array(invoiceDiagnostic) })
const resolution = z.enum(['sent', 'not_sent'])
const invoiceSend = z.object({ id: z.string(), plan: invoicePlan, status: z.enum(['intent', 'uncertain', 'completed', 'not_sent', 'handed_off']), resolved: resolution.optional() })
const invoiceView = z.object({ report: invoiceReport, sends: z.array(invoiceSend), synthetic: z.boolean(), targets: z.array(z.string()).default([]), sent_evidence: z.record(z.string(), z.array(z.string())).default({}) })
const exactConfirmation = z.object({ request_id: z.string().length(64), protocol: z.literal('mailroom-exact-action-v1'), plan_digest: z.string().length(64), display: z.string(), expires_after_seconds: z.number(), synthetic: z.boolean() })
const invoicePreview = z.object({ plan: invoicePlan, receipt: z.string().length(64), native_available: z.literal(false), confirmation: exactConfirmation.nullable() })
export type InvoiceView = z.infer<typeof invoiceView>
export type InvoicePreview = z.infer<typeof invoicePreview>
export async function openInvoices(account: string): Promise<InvoiceView> { return invoiceView.parse(await accountPost('invoices', { account, command: { action: 'open' } })) }
/** The wired real adapter; without one the synthetic fixture transport sends. */
export type InvoiceRoute = 'smtp'
export type Resolution = z.infer<typeof resolution>
const route = (mode: string): InvoiceRoute | undefined => mode === 'smtp' ? 'smtp' : undefined
export async function acquireInvoice(account: string, selection: string): Promise<InvoiceView> { return invoiceView.parse(await accountPost('invoices', { account, command: { action: 'acquire', selection } })) }
export async function previewInvoice(account: string, selection: string, target: string, mode?: InvoiceRoute): Promise<InvoicePreview> { return invoicePreview.parse(await accountPost('invoices', { account, command: { action: 'preview', selection, target, ...(mode ? { mode } : {}) } })) }
export async function sendInvoice(account: string, preview: InvoicePreview): Promise<InvoiceView> { const mode = route(preview.plan.mode); return invoiceView.parse(await accountPost('invoices', { account, command: { action: 'send', selection: preview.plan.selection, target: preview.plan.target_name, receipt: preview.receipt, confirmation_action: preview.plan.confirmation_action, ...(mode ? { mode } : {}) } })) }
export async function recoverInvoice(account: string, send: string): Promise<InvoiceView> { return invoiceView.parse(await accountPost('invoices', { account, command: { action: 'recover', send } })) }
export async function resolveInvoice(account: string, send: string, outcome: Resolution): Promise<InvoiceView> { return invoiceView.parse(await accountPost('invoices', { account, command: { action: 'resolve', send, resolution: outcome } })) }

const unsubscribeMethod = z.enum(['one_click', 'browser_preparation', 'mail_app_preparation', 'local_mail', 'retirement_preparation', 'mute', 'keep', 'remove_mute'])
const unsubscribeInput = z.object({ as_of: z.number(), selections: z.array(z.object({ subscription: z.string(), method: unsubscribeMethod })), folders: z.object({ personal: z.string(), business: z.string() }) })
const unsubscribeRow = z.object({ id: z.string(), account: z.string(), sender: z.string(), domain: z.string().nullable(), recipient: z.string(), recipient_evidence: z.string(), list_id: z.string().nullable(), list_evidence: z.string(), class: z.string(), count_12_months: z.number(), read_share: z.number().nullable(), newest_dated_seen_message: z.number().nullable(), unread_volume: z.number(), rank: z.number(), recommended: z.boolean(), flags_known: z.boolean(), authentication: z.string(), original_header_survives: z.boolean(), relay_alias_initially_selected: z.literal(false), methods: z.array(unsubscribeMethod), role: z.string(), source: z.string(), header_digest: z.string(), source_ids: z.array(z.string()), straggler: z.string() })
const unsubscribeEntry = z.object({ id: z.string(), subscription: z.string(), sender_key: z.string(), method: unsubscribeMethod, time: z.number(), status: z.string(), host: z.string().nullable(), payload_digest: z.string(), http_status: z.number().nullable(), success: z.string(), resolved: resolution.optional() })
const unsubscribeView = z.object({ report: z.object({ version: z.string(), complete: z.boolean(), revision: z.number(), as_of: z.number(), last_completed_at: z.number().nullable(), census_age_seconds: z.number().nullable(), rows: z.array(unsubscribeRow), warnings: z.array(z.string()) }), history: z.array(unsubscribeEntry), mutes: z.record(z.string(), z.object({ subscription: z.string(), rule: z.object({ id: z.string() }) })), synthetic: z.boolean(), wired: z.boolean().default(false), production_available: z.literal(false), q20_scope: z.literal('separate_subscriptions') })
const unsubscribePreview = z.object({ account: z.string(), state_revision: z.number(), workspace_revision: z.number().nullable(), semantics: z.string(), receipt: z.string().length(64), confirmation_action: z.string(), local_mail_confirmation: z.string().nullable(), scope: z.string(), confirmation: exactConfirmation.nullable(), proof: preview.nullable(), actions: z.array(z.object({ subscription: z.string(), account: z.string(), sender: z.string(), recipient: z.string(), list_id: z.string().nullable(), source: z.string(), header_digest: z.string(), method: unsubscribeMethod, host: z.string().nullable(), payload_digest: z.string(), role: z.string(), destination: z.string().nullable(), effect: z.string(), copy_card: z.object({ uri: z.string().nullable(), to: z.string().nullable(), from: z.string().nullable(), subject: z.string().nullable(), body: z.string().nullable() }).nullable() })) })
type UnsubscribeView = z.infer<typeof unsubscribeView>
export type UnsubscribeMethod = z.infer<typeof unsubscribeMethod>
export async function recoverUnsubscribe(account: string, entry: string, as_of: number): Promise<UnsubscribeView> { return unsubscribeView.parse(await accountPost('unsubscribe', { account, command: { action: 'recover', entry, as_of } })) }
export async function resolveUnsubscribe(account: string, entry: string, outcome: Resolution, as_of: number): Promise<UnsubscribeView> { return unsubscribeView.parse(await accountPost('unsubscribe', { account, command: { action: 'resolve', entry, resolution: outcome, as_of } })) }

const subscriptionReviewInput = unsubscribeInput.extend({ selections: z.array(z.object({ account: z.string(), subscription: z.string(), method: unsubscribeMethod })) })
const subscriptionReviewView = z.object({ accounts: z.array(unsubscribeView.extend({ account: z.string() })), synthetic: z.boolean(), review_available: z.boolean(), q20_scope: z.literal('separate_subscriptions') })
const subscriptionReviewPreview = z.object({ confirmation: exactConfirmation, previews: z.array(unsubscribePreview), receipt: z.string().length(64), confirmation_action: z.string(), local_mail_confirmation: z.string().nullable() })
export type SubscriptionReviewInput = z.infer<typeof subscriptionReviewInput>
export type SubscriptionReviewView = z.infer<typeof subscriptionReviewView>
export type SubscriptionReviewPreview = z.infer<typeof subscriptionReviewPreview>
export async function openSubscriptionReview(as_of: number): Promise<SubscriptionReviewView> { return subscriptionReviewView.parse(await accountPost('subscription-review', { command: { action: 'open', as_of } })) }
export async function previewSubscriptionReview(input: SubscriptionReviewInput): Promise<SubscriptionReviewPreview> { return subscriptionReviewPreview.parse(await accountPost('subscription-review', { command: { action: 'preview', input } })) }
export async function confirmSubscriptionReview(input: SubscriptionReviewInput, plan: SubscriptionReviewPreview): Promise<SubscriptionReviewView> { return subscriptionReviewView.parse(await accountPost('subscription-review', { command: { action: 'confirm', input, receipt: plan.receipt, confirmation_action: plan.confirmation_action, mail_confirmation_action: plan.local_mail_confirmation } })) }

const lifecycleResource = z.object({ path: z.string(), bytes: z.number(), digest: z.string() })
const forgetPreview = z.object({ account: z.string(), revision: z.number(), generation: z.number(), resources: z.array(lifecycleResource), credential_references: z.array(z.string()), consequences: z.object({ en: z.string(), de: z.string() }), receipt: z.string().length(64) })
const lifecycleRecovery = z.object({ account: z.string(), state: z.enum(['creating', 'replacing', 'forgetting', 'rotating', 'forgotten']) })
const lifecycleStatus = z.object({ recovery: z.array(lifecycleRecovery), production_available: z.literal(false) })
const lifecycleSaved = z.object({ saved: z.literal(true), recovery: z.array(lifecycleRecovery) })
export type ForgetPreview = z.infer<typeof forgetPreview>
export type LifecycleStatus = z.infer<typeof lifecycleStatus>
export async function readLifecycle(): Promise<LifecycleStatus> { return lifecycleStatus.parse(await accountPost('lifecycle', { action: 'status' })) }
export async function previewForget(account: string): Promise<ForgetPreview> { return forgetPreview.parse(await accountPost('lifecycle', { action: 'forget_preview', account })) }
export async function forgetAccount(account: string, preview: ForgetPreview): Promise<void> { lifecycleSaved.parse(await accountPost('lifecycle', { action: 'forget', account, preview, confirmed: true })) }
export async function replaceAccountPassword(account: string, password: string): Promise<void> { lifecycleSaved.parse(await accountPost('lifecycle', { action: 'replace_password', account, password })) }
export async function rotateAccount(account: string): Promise<void> { lifecycleSaved.parse(await accountPost('lifecycle', { action: 'rotate', account, confirmed: true })) }
export async function recoverAccount(account: string): Promise<void> { lifecycleSaved.parse(await accountPost('lifecycle', { action: 'recover', account })) }

const proposalChange = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('extend_sender'), id: z.string(), address: z.string() }).strict(),
  z.object({ kind: z.literal('patch'), patch: z.record(z.string(), z.json()) }),
])
const proposalSelection = z.object({ account: z.string(), generation: z.string(), candidates: z.array(z.object({ id: z.string(), change: proposalChange })) })
const proposalGeneration = z.object({
  account: z.string(), generation: z.string(), semantics: z.string(), analysis: z.string(), revision: z.number(),
  candidates: z.array(z.object({ id: z.string(), sender_claim: z.string(), change: proposalChange, observations: z.array(z.object({ message: z.string(), subject: z.string(), folders: z.array(z.string()) })), rationale: proposalRationale, uncertainty: z.string(), rejected: z.boolean(), prior_rejections: z.number(), impact: proofPreview })),
  unresolved: z.array(z.object({ sender_claim: z.string().nullable(), messages: z.number(), reason: proposalReason })),
  current: proofPreview, decisions: z.object({ history: z.array(z.object({ candidate: z.string(), generation: z.string(), disposition: z.enum(['accept', 'reject']), target: z.string(), as_of: z.number() })) }), limitations: z.string(),
})
const proposalReview = z.object({ selection: proposalSelection, disposition: z.enum(['accept', 'reject']), changes: z.array(proposalChange), original_documents: documents, resulting_documents: documents, document_diff: z.string(), current: proofPreview, resulting: proofPreview, receipt: z.string(), acceptance_available: z.boolean(), effects: z.string() })
export type ProposalGeneration = z.infer<typeof proposalGeneration>
export type ProposalReview = z.infer<typeof proposalReview>
export type ProposalSelection = z.infer<typeof proposalSelection>
export async function openProposals(account: string): Promise<ProposalGeneration> { return proposalGeneration.parse(await accountPost('proposals', { account, command: { action: 'open' } })) }
export async function reviewProposals(account: string, selection: ProposalSelection, disposition: ProposalReview['disposition']): Promise<ProposalReview> { return proposalReview.parse(await accountPost('proposals', { account, command: { action: 'preview', selection, disposition } })) }
export async function confirmProposals(account: string, review: ProposalReview): Promise<ProposalGeneration> { return proposalGeneration.parse(await accountPost('proposals', { account, command: { action: 'confirm', selection: review.selection, disposition: review.disposition, receipt: review.receipt, confirmed: true } })) }

const moveMailbox = z.object({ name: z.object({ decoded: z.string(), wire: z.string() }), uidvalidity: z.number().int().positive() })
const moveMetadata = z.object({ message_id_hash: z.string(), size: z.number(), internal_date: z.string(), flags: z.array(z.string()) })
const moveIntent = z.object({ purpose: z.union([z.literal('conditional_undo'), z.object({ filing: z.object({ protect_flagged: z.boolean() }) })]), source: moveMailbox, destination: moveMailbox, messages: z.record(z.string(), moveMetadata) })
const copyUid = z.object({ uidvalidity: z.number().int().positive(), pairs: z.record(z.string(), z.number().int().positive()) })
const moveReadback = z.object({ identity: moveMailbox, messages: z.record(z.string(), moveMetadata) })
const moveEvidence = z.object({ intent: moveIntent, attempted: z.boolean(), mapping: z.discriminatedUnion('kind', [z.object({ kind: z.literal('missing') }), z.object({ kind: z.literal('validated'), mapping: copyUid }), z.object({ kind: z.literal('rejected'), prior: copyUid.nullable() })]), completion: z.discriminatedUnion('kind', [z.object({ kind: z.literal('lost') }), z.object({ kind: z.literal('tagged'), status: z.enum(['ok', 'no', 'bad']), code: z.string().nullable() })]).nullable(), readback: z.tuple([moveReadback, moveReadback]).nullable() })
const modSeq = z.string().regex(/^[1-9][0-9]{0,19}$/).refine(value => value.length < 20 || value <= '18446744073709551615').brand<'ModSeq'>()
const versionedMetadata = z.object({ metadata: moveMetadata, modseq: modSeq }).strict()
const copyReadback = z.object({ identity: moveMailbox, highest_modseq: modSeq, messages: z.record(z.string(), versionedMetadata) }).strict()
const flagIntent = z.object({ before: copyReadback, change: z.enum(['add_deleted', 'remove_owned_deleted']) }).strict()
const storeResult = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('lost') }).strict(),
  z.object({ kind: z.literal('rejected') }).strict(),
  z.object({ kind: z.literal('tagged'), status: z.enum(['ok', 'no', 'bad']), modified: z.array(z.number().int().positive()).nullable(), updated: z.record(z.string(), modSeq) }).strict(),
])
const flagEvidence = z.object({ intent: flagIntent, result: storeResult.nullable(), readback: copyReadback.nullable() }).strict()
const copyJournal = z.object({
  before: copyReadback, copy: moveEvidence,
  readback: z.object({ source: copyReadback, destination: copyReadback }).strict().nullable(),
  deletions: z.array(z.object({
    evidence: z.object({ flag: flagEvidence, expunge: z.object({ before: copyReadback, completion: moveEvidence.shape.completion, readback: copyReadback.nullable() }).strict().nullable() }).strict(),
    destination: copyReadback, final_destination: copyReadback.nullable(),
  }).strict()),
}).strict()
const protocolEvidence = z.union([
  z.object({ kind: z.literal('copy_uid_expunge'), evidence: copyJournal }).strict().transform(batch => ({ ...batch.evidence.copy, copy_journal: batch.evidence })),
  moveEvidence.strict().transform(batch => ({ ...batch, copy_journal: null })),
])
export type ProtocolEvidence = z.infer<typeof protocolEvidence>
const reverseMove = z.object({ forward: z.number().int().nonnegative(), evidence: protocolEvidence })
export const undoDeadline = z.object({ run: z.string(), completed_at: z.number().nullable(), undo_until: z.number().nullable(), undo_open: z.boolean() })
export const runSummary = z.object({ id: z.string(), receipt: z.string(), status: z.enum(['complete', 'undone']), completed_at: z.number(), compacted_at: z.number(), messages: z.number(), batches: z.number(), applied: z.number(), restored: z.number(), folders: z.record(z.string(), z.record(z.string(), z.number())) })
export type UndoDeadline = z.infer<typeof undoDeadline>
export type RunSummary = z.infer<typeof runSummary>
const retained = { undo_deadlines: z.array(undoDeadline).default([]), undo_window_days: z.number().default(90), closable: z.array(z.string()).default([]) }
export const closure = z.object({ at: z.number(), receipt: z.string().length(64) })
const protocolRun = z.object({ id: z.string(), receipt: z.string(), plan: z.object({ filing: backlogPlan, binding: z.object({ account: z.string(), source: z.string(), authority: z.string(), workspace: z.string(), material: z.string() }), saved_proof: z.string(), execution_proof: z.string() }), status: z.enum(['prepared', 'running', 'stopped', 'complete', 'uncertain', 'conflict', 'undo_running', 'undone']), stop_requested: z.boolean(), batches: z.array(protocolEvidence), reverse: z.array(reverseMove), undo_receipt: z.string().nullable(), undo_conflicts: z.number().int().nonnegative().default(0), completed_at: z.number().optional(), closed: closure.optional() })
const protocolView = z.object({ installed: z.boolean(), recovery_hold: z.boolean(), journal: z.object({ runs: z.array(protocolRun), summaries: z.array(runSummary).default([]) }), ...retained, limitations: z.string() })
const protocolPreview = z.object({ operation: protocolRun, receipt: z.string(), saved: proofPreview, execution: preview, limitations: z.string() })
const protocolUndo = z.object({ run: z.string(), reverse: z.array(reverseMove), conflicts: z.number(), receipt: z.string(), limitations: z.string() })
export type ProtocolView = z.infer<typeof protocolView>
export type ProtocolPreview = z.infer<typeof protocolPreview>
export type ProtocolUndo = z.infer<typeof protocolUndo>
type ProtocolAction = { action: 'status' | 'cancel' } | { action: 'preview' | 'label_preview'; options: BacklogOptions } | { action: 'confirm' | 'label_confirm'; options: BacklogOptions; receipt: string; confirmed: boolean } | { action: 'reconcile' | 'resume' | 'undo_preview' | 'resume_undo' | 'close_preview'; run: string } | { action: 'undo' | 'close'; run: string; receipt: string; confirmed: boolean } | { action: 'label_reconcile'; operation: string }
export const closePreview = z.object({ run: z.string(), reason: z.enum(['nothing_to_undo', 'undo_ended', 'undo_unavailable']), receipt: z.string().length(64) })
export type ClosePreview = z.infer<typeof closePreview>
export type BacklogKind = 'protocol' | 'gmail'
export async function sortingPost(kind: BacklogKind, account: string, command: ProtocolAction): Promise<unknown> { return accountPost(`backlog/${kind}`, { account, command }) }
export async function previewClose(kind: BacklogKind, account: string, run: string): Promise<ClosePreview> { return closePreview.parse(await sortingPost(kind, account, { action: 'close_preview', run })) }
export async function protocolAction(account: string, command: ProtocolAction): Promise<void> { await accountPost('backlog/protocol', { account, command }) }
export async function openProtocolBacklog(account: string): Promise<ProtocolView> { return protocolView.parse(await accountPost('backlog/protocol', { account, command: { action: 'status' } })) }
export async function previewProtocolBacklog(account: string, options: BacklogOptions): Promise<ProtocolPreview> { return protocolPreview.parse(await accountPost('backlog/protocol', { account, command: { action: 'preview', options } })) }
export async function previewProtocolUndo(account: string, run: string): Promise<ProtocolUndo> { return protocolUndo.parse(await accountPost('backlog/protocol', { account, command: { action: 'undo_preview', run } })) }

const gmailLabel = z.object({ decoded: z.string(), wire: z.string() })
const gmailSnapshot = z.object({ gmail_id: z.string(), message_id_hash: z.string(), size: z.number(), internal_date: z.string(), flags: z.array(z.string()), labels: z.array(z.string()) })
const gmailIntent = z.object({ archive_only: z.boolean().optional(), purpose: moveIntent.shape.purpose, context: moveMailbox, destination: gmailLabel, messages: z.record(z.string(), gmailSnapshot) })
const gmailStep = z.object({ kind: z.enum(['add_label', 'remove_inbox', 'restore_inbox', 'remove_label']), uids: z.array(z.number().int().positive()), attempted: z.boolean(), completion: moveEvidence.shape.completion, readback: z.object({ identity: moveMailbox, messages: z.record(z.string(), gmailSnapshot) }).nullable() })
const gmailBatch = z.object({ intent: gmailIntent, steps: z.array(gmailStep) })
const gmailReverse = z.object({ forward: z.number().int().nonnegative(), batch: gmailBatch })
const gmailRun = protocolRun.extend({ batches: z.array(gmailBatch), reverse: z.array(gmailReverse) })
const gmailOutcome = z.enum(['pending', 'verified', 'changed', 'unapplied', 'uncertain', 'conflict'])
const gmailOutcomes = z.record(z.string(), z.object({ forward: z.array(z.array(z.record(z.string(), gmailOutcome))), reverse: z.array(z.array(z.record(z.string(), gmailOutcome))) }))
const gmailView = z.object({ installed: z.boolean(), recovery_hold: z.boolean(), journal: z.object({ runs: z.array(gmailRun), summaries: z.array(runSummary).default([]) }), outcomes: gmailOutcomes, ...retained, labels: z.object({ history: z.array(z.any()) }).default({ history: [] }), label_limitations: z.string().default(''), limitations: z.string() })
export const missingLabel = z.object({ name: z.string(), messages: z.number().int().positive(), blocked: z.string().nullable() })
const gmailPreview = z.object({ operation: gmailRun, receipt: z.string(), saved: proofPreview, execution: preview, missing_labels: z.array(missingLabel).default([]), limitations: z.string() })
const gmailUndo = z.object({ run: z.string(), reverse: z.array(gmailReverse), conflicts: z.number(), receipt: z.string(), limitations: z.string() })
export type GmailView = z.infer<typeof gmailView>
export type GmailPreview = z.infer<typeof gmailPreview>
export type GmailUndo = z.infer<typeof gmailUndo>
export type GmailOutcome = z.infer<typeof gmailOutcome>
export async function gmailAction(account: string, command: ProtocolAction): Promise<void> { await accountPost('backlog/gmail', { account, command }) }
export async function openGmailBacklog(account: string): Promise<GmailView> { return gmailView.parse(await accountPost('backlog/gmail', { account, command: { action: 'status' } })) }
export async function previewGmailBacklog(account: string, options: BacklogOptions): Promise<GmailPreview> { return gmailPreview.parse(await accountPost('backlog/gmail', { account, command: { action: 'preview', options } })) }
export async function previewGmailUndo(account: string, run: string): Promise<GmailUndo> { return gmailUndo.parse(await accountPost('backlog/gmail', { account, command: { action: 'undo_preview', run } })) }
