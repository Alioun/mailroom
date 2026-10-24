import { z } from 'zod'

z.config({ jitless: true })

const decision = z.object({
  id: z.string(), subject: z.string(), from: z.string(),
  destination: z.discriminatedUnion('kind', [z.object({ kind: z.literal('inbox') }), z.object({ kind: z.literal('folder'), name: z.string() })]),
  matched_rule: z.string().nullable(), reason: z.string(),
  copies: z.array(z.object({ rule: z.string(), recipient: z.string() })),
})
const preview = z.object({
  source: z.literal('synthetic'), semantics: z.string(),
  counts: z.object({ messages: z.number(), filed: z.number(), inbox: z.number(), copies: z.number(), copied_messages: z.number() }),
  decisions: z.array(decision), trace: z.array(z.string()), sieve: z.string(), omissions: z.array(z.string()),
})
const operation = z.enum(['preview', 'synthetic_census', 'connect', 'census', 'upload_rules', 'activate_rules', 'create_folder', 'move_messages', 'change_flags', 'change_forwarding', 'send_message', 'unsubscribe'])
const availability = z.array(z.object({ operation, effect: z.enum(['synthetic_read', 'account_read', 'account_write']), allowed: z.boolean(), reason: z.string() }))
const sample = z.object({
  preset: z.unknown(),
  overrides: z.object({ preset: z.string(), major: z.number(), patches: z.array(z.unknown()), additions: z.array(z.unknown()) }),
  messages: z.array(z.unknown()),
})
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
