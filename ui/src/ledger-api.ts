import { z } from 'zod'
import { localFetch } from './api'

const association = z.object({ sender_domains: z.array(z.string()), approved_evidence: z.string() })
export const ledgerEntry = z.object({
  address: z.string(), service: z.string(), folder: z.string(), first_seen: z.string(),
  status: z.enum(['active', 'retired']), note: z.string(),
  category: z.enum(['money', 'contracts', 'orders', 'travel', 'social', 'newsletters']),
  invoice: z.boolean(), association: association.nullable(),
}).catchall(z.unknown())
const document = z.object({ version: z.number(), enabled: z.boolean(), entries: z.array(ledgerEntry) }).catchall(z.unknown())
const observation = z.object({ account: z.string(), folder: z.string(), uidvalidity: z.number(), uid: z.number(), recipient_header: z.string(), delivery_authority: z.string(), sender_domain: z.string().nullable(), message_date: z.string().nullable() })
const batch = z.object({ content_version: z.string(), revision: z.number(), enabled: z.boolean(), entries: z.array(ledgerEntry) })
const view = z.object({
  snapshot: z.object({ content_version: z.string(), document }), revision: z.number(), census_state: z.string(),
  proposals: z.array(z.object({ address: z.string(), first_seen_proposal: z.string(), first_seen_meaning: z.string(), ownership: z.string(), role: z.string(), service: z.string().nullable(), evidence: z.array(observation) })),
  leaks: z.array(z.object({ address: z.string(), service: z.string(), approved: association, observation, signal: z.string() })),
  unknown_sender_observations: z.number(), projection_changed: z.boolean(), semantics: z.string(), limitation: z.string(),
})
const review = z.object({ receipt: z.string(), batch, resulting_document: document, resulting_overrides: z.string(), effects: z.string(), proof: z.object({ receipt: z.string(), proof: z.object({ gate: z.string(), artifact: z.object({ identity: z.string(), executable: z.string() }), warnings: z.array(z.string()), counts: z.object({ messages: z.number(), filed: z.number(), inbox: z.number(), copies: z.number(), review: z.number() }) }) }) })
export type LedgerEntry = z.infer<typeof ledgerEntry>
export type LedgerView = z.infer<typeof view>
export type LedgerReview = z.infer<typeof review>
export type LedgerBatch = z.infer<typeof batch>
async function command(account: string, command: unknown): Promise<unknown> {
  const response = await localFetch('/api/accounts/ledger', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, command }) })
  if (!response.ok) throw new Error('Ledger action rejected')
  return response.json()
}
export async function openLedger(account: string): Promise<LedgerView> { return view.parse(await command(account, { action: 'open' })) }
export async function reviewLedger(account: string, batch: LedgerBatch): Promise<LedgerReview> { return review.parse(await command(account, { action: 'preview', batch })) }
export async function confirmLedger(account: string, review: LedgerReview): Promise<LedgerView> { return view.parse(await command(account, { action: 'confirm', batch: review.batch, receipt: review.receipt, confirmed: true })) }
