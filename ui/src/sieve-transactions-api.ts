import { z } from 'zod'
import { localFetch } from './api'
const body = z.object({ name: z.string(), bytes: z.array(z.number().int().min(0).max(255)).max(65536), sha256: z.string() })
const image = z.object({ capabilities: z.record(z.string(), z.string()), names: z.array(z.object({ name: z.string(), active: z.boolean() })).max(8), bodies: z.array(body).max(8) })
const effect = z.enum(['upload', 'activate', 'disable'])
const step = z.object({ effect, before: image, after: image, outcome: z.enum(['intent', 'acknowledged', 'observed', 'refused', 'uncertain', 'conflict', 'preimage']), acknowledgment: z.boolean(), failure: z.string().nullable() })
const transaction = z.object({ id: z.string(), compensates: z.string().nullable(), target: z.string(), ownership_epoch: z.string(), artifact: z.string(), proof_binding: z.string(), receipt: z.string(), before: image, after: image, status: z.enum(['running', 'complete', 'refused', 'uncertain', 'stopped', 'conflict', 'fresh_review']), validation: z.union([z.object({ Ok: z.null() }), z.object({ Err: z.string() })]).nullable(), space: z.union([z.object({ Ok: z.null() }), z.object({ Err: z.string() })]).nullable(), steps: z.array(step).max(2) })
const journal = z.object({ history: z.array(transaction).max(4) })
const view = z.object({ installed: z.literal(true), recovery_hold: z.boolean(), journal, limitations: z.string() })
const preview = z.object({ transaction, receipt: z.string(), proof: z.union([z.object({ gate: z.string(), semantics: z.string(), pins: z.array(z.object({ message_id_hash: z.string(), status: z.string() })), warnings: z.array(z.string()), runtime_cross_check: z.string() }), z.object({ compensates: z.string(), fresh_provider_match: z.literal(true) })]), effects: z.array(effect), limitations: z.string() })
export type TransactionView = z.infer<typeof view>
export type TransactionPreview = z.infer<typeof preview>
type Command = { action: 'preview' | 'status' | 'cancel' } | { action: 'confirm'; receipt: string; confirmed: boolean } | { action: 'reconcile' | 'compensation_preview'; operation: string } | { action: 'compensation_confirm'; operation: string; receipt: string; confirmed: boolean }
export async function transactionCommand(account: string, command: Command): Promise<unknown> {
  const response = await localFetch('/api/accounts/sieve-transactions', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, command }) })
  if (response.status === 403) throw Error('unavailable')
  if (!response.ok) throw Error('rejected')
  return response.json()
}
export async function transactionStatus(account: string): Promise<TransactionView> { return view.parse(await transactionCommand(account, { action: 'status' })) }
export async function transactionPreview(account: string, operation?: string): Promise<TransactionPreview> { return preview.parse(await transactionCommand(account, operation ? { action: 'compensation_preview', operation } : { action: 'preview' })) }
export async function reconcileTransaction(account: string, operation: string): Promise<TransactionView> { return view.parse(await transactionCommand(account, { action: 'reconcile', operation })) }
