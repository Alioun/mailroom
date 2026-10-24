import { z } from 'zod'
import { localFetch } from './api'
const settings = z.object({ winding_down: z.boolean(), comparisons: z.record(z.string(), z.array(z.string())) })
const row = z.object({
  id: z.string(), sender: z.string().nullable(), domain: z.string().nullable(),
  group: z.enum(['people', 'service', 'bulk', 'unresolved']), status: z.enum(['waiting', 'moved', 'dismissed', 'quiet', 'bulk', 'unresolved']),
  messages: z.number(), recent: z.number(), first_date: z.number().nullable(), last_date: z.number().nullable(),
  invalid_dates: z.number(), future_dates: z.number(), wrote_to_sender: z.boolean(), historical: z.boolean(),
  observations: z.array(z.object({ account: z.string(), field: z.string(), address: z.string(), raw_values: z.array(z.string()), message_date: z.number().nullable(), delivery_authority: z.string() })),
  migration: z.array(z.object({ message_identity: z.string(), account: z.string(), address: z.string(), message_date: z.number(), evidence: z.string() })),
  reasons: z.array(z.string()), settings_link: z.string().nullable(), service_associations: z.array(z.string()),
})
export const reportSchema = z.object({ version: z.string(), semantics: z.string(), census_analysis: z.string(), sender_data_version: z.string(), ledger_version: z.string(), account: z.string(), as_of: z.number(), boundary: z.number(), settings,
  sources: z.array(z.object({ account: z.string(), revision: z.number(), census_version: z.string(), complete: z.boolean(), owned_addresses: z.array(z.string()), spam_count: z.number().nullable(), coverage_since: z.number().nullable(), coverage_through: z.number().nullable() })),
  rows: z.array(row), counts: z.record(z.string(), z.number()), blockers: z.array(z.string()), verdict: z.string(), effects: z.string(),
})
const opened = z.object({ report: reportSchema, terminal: z.string(), available: z.array(z.object({ account: z.string(), login: z.string(), owned_addresses: z.array(z.string()) })) })
const change = z.discriminatedUnion('kind', [z.object({ kind: z.literal('settings'), settings }), z.object({ kind: z.literal('dismiss'), rows: z.array(z.string()), dismissed: z.boolean() })])
const review = z.object({ receipt: z.string(), report: reportSchema, resulting_report: reportSchema, change, consequences: z.string() })
const disclosure = z.enum(['local_file', 'agent_summary'])
const exported = z.object({ receipt: z.string(), disclosure, text: z.string(), filename: z.string() })
export type WinddownView = z.infer<typeof opened>
export type WinddownReport = z.infer<typeof reportSchema>
export type WinddownReview = z.infer<typeof review>
export type WinddownChange = z.infer<typeof change>
export type WinddownExport = z.infer<typeof exported>
export type WinddownSettings = z.infer<typeof settings>
export type Disclosure = z.infer<typeof disclosure>
async function command(account: string, as_of: number, command: unknown): Promise<unknown> {
  const response = await localFetch('/api/accounts/winddown', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, as_of, command }) })
  if (!response.ok) throw new Error('Wind-down action rejected')
  return response.json()
}
export async function openWinddown(account: string, asOf: number): Promise<WinddownView> { return opened.parse(await command(account, asOf, { action: 'open' })) }
export async function reviewWinddown(account: string, report: WinddownReport, change: WinddownChange): Promise<WinddownReview> { return review.parse(await command(account, report.as_of, { action: 'preview', version: report.version, change })) }
export async function confirmWinddown(account: string, value: WinddownReview): Promise<void> { z.object({ saved: z.literal(true) }).parse(await command(account, value.report.as_of, { action: 'confirm', version: value.report.version, change: value.change, receipt: value.receipt, confirmed: true })) }
export async function previewWinddownExport(account: string, report: WinddownReport, disclosure: Disclosure): Promise<WinddownExport> { return exported.parse(await command(account, report.as_of, { action: 'export_preview', version: report.version, disclosure })) }
export async function exportWinddown(account: string, report: WinddownReport, value: WinddownExport): Promise<WinddownExport> { return exported.parse(await command(account, report.as_of, { action: 'export', version: report.version, disclosure: value.disclosure, receipt: value.receipt, confirmed: true })) }
