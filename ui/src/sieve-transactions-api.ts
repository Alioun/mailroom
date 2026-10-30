import { z } from 'zod'
import { localFetch } from './api'
const body = z.object({ name: z.string(), bytes: z.array(z.number().int().min(0).max(255)).max(65536), sha256: z.string() })
const image = z.object({ capabilities: z.record(z.string(), z.string()), names: z.array(z.object({ name: z.string(), active: z.boolean() })).max(8), bodies: z.array(body).max(8) })
const effect = z.enum(['upload', 'activate', 'disable', 'upload_owned'])
const step = z.object({ effect, before: image, after: image, outcome: z.enum(['intent', 'acknowledged', 'observed', 'refused', 'uncertain', 'conflict', 'preimage']), acknowledgment: z.boolean(), failure: z.string().nullable() })
const placement = z.discriminatedUnion('kind', [z.object({ kind: z.literal('block') }), z.object({ kind: z.literal('include'), script: z.string() })])
const count = z.number().int().nonnegative()
const filingCounts = z.object({ mailroom: count, existing_wins: count, conflicting: count, undetermined: count })
const legacyCounts = filingCounts.extend({ suppressed_copies: count, copies: z.never().optional() }).strict()
const currentCounts = filingCounts.extend({ suppressed_copies: z.never().optional(), copies: z.object({ copied: count, beside_existing: count, undetermined: count, guard_violations: count }).strict() }).strict()
const counts = z.union([legacyCounts, currentCounts])
const compositionBase = z.object({ purpose: z.enum(['deploy', 'removal', 'compensation']), placement: placement.nullable(), owned: z.string().nullable(), foreign_sha256: z.string(), existing_rules: z.array(z.string()).max(256) })
const composition = z.discriminatedUnion('semantics', [
  compositionBase.extend({ semantics: z.literal('mailroom-foreign-composition-v1'), precedence: legacyCounts.nullable() }),
  compositionBase.extend({ semantics: z.literal('mailroom-foreign-composition-v2'), precedence: currentCounts.nullable() }),
])
const transaction = z.object({ id: z.string(), compensates: z.string().nullable(), target: z.string(), ownership_epoch: z.string(), artifact: z.string(), proof_binding: z.string(), receipt: z.string(), before: image, after: image, status: z.enum(['running', 'complete', 'refused', 'uncertain', 'stopped', 'conflict', 'fresh_review']), validation: z.union([z.object({ Ok: z.null() }), z.object({ Err: z.string() })]).nullable(), space: z.union([z.object({ Ok: z.null() }), z.object({ Err: z.string() })]).nullable(), steps: z.array(step).max(2), composition: composition.optional() })
const journal = z.object({ history: z.array(transaction).max(4) })
const view = z.object({ installed: z.literal(true), recovery_hold: z.boolean(), placement: placement.nullable(), journal, limitations: z.string() })
const destination = z.discriminatedUnion('kind', [z.object({ kind: z.literal('inbox') }), z.object({ kind: z.literal('folder'), name: z.string() })])
const existing = z.discriminatedUnion('kind', [z.object({ kind: z.literal('inbox') }), z.object({ kind: z.literal('folder'), name: z.string() }), z.object({ kind: z.literal('discard') }), z.object({ kind: z.literal('redirect') }), z.object({ kind: z.literal('reject') })])
const precedence = z.object({ semantics: z.literal('mailroom-foreign-composition-v2'), placement, host: z.string(), foreign_rules: z.array(z.object({ index: z.number(), name: z.string(), line: z.number(), may_stop: z.boolean(), dispositions: z.array(existing) })), decisions: z.array(z.object({ id: z.string(), message_id_hash: z.string().nullable(), rule: z.string(), status: z.enum(['existing_rule_wins', 'undetermined']), existing: existing.nullable(), mailroom: destination, conflicting: z.boolean(), copies: z.number() })), copies: z.array(z.object({ id: z.string(), message_id_hash: z.string().nullable(), rule: z.string(), recipient: z.string(), status: z.enum(['copied', 'undetermined']), filed_by: z.enum(['mailroom', 'existing_rule', 'undetermined']) })), counts: currentCounts, pins: z.array(z.object({ message_id_hash: z.string(), status: z.string() })), gate: z.string(), warnings: z.array(z.string()) })
const proof = z.union([z.object({ gate: z.string(), semantics: z.string(), composed_gate: z.string(), pins: z.array(z.object({ message_id_hash: z.string(), status: z.string() })), warnings: z.array(z.string()), runtime_cross_check: z.string() }), z.object({ compensates: z.string(), fresh_provider_match: z.literal(true) }), z.object({ removal: z.string() })])
const ready = z.object({ outcome: z.literal('ready'), transaction, receipt: z.string(), proof, precedence: precedence.nullable(), effects: z.array(effect), limitations: z.string() })
const refusal = z.object({ reason: z.enum(['placement_unchosen', 'invalid_placement', 'host_missing', 'host_inactive', 'unparseable', 'marker_damaged', 'owned_name_foreign', 'unsafe_rule', 'mailroom_unreachable', 'mailroom_content_invalid', 'proof_blocked', 'nothing_to_remove']), script: z.string().optional(), rule: z.string().optional(), line: z.number().optional(), detail: z.string().optional(), active: z.string().nullable().optional() })
const refused = z.object({ outcome: z.literal('refused'), refusal, precedence: precedence.nullable(), limitations: z.string() })
const preview = z.discriminatedUnion('outcome', [ready, refused])
export type Placement = z.infer<typeof placement>
export type TransactionView = z.infer<typeof view>
export type TransactionPreview = z.infer<typeof preview>
export type Precedence = z.infer<typeof precedence>
export type Counts = z.infer<typeof counts>
type Command = { action: 'preview' | 'status' | 'cancel' | 'removal_preview' } | { action: 'confirm' | 'removal_confirm'; receipt: string; confirmed: boolean } | { action: 'reconcile' | 'compensation_preview'; operation: string } | { action: 'compensation_confirm'; operation: string; receipt: string; confirmed: boolean } | { action: 'record_placement'; placement: Placement | null }
export async function transactionCommand(account: string, command: Command): Promise<unknown> {
  const response = await localFetch('/api/accounts/sieve-transactions', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, command }) })
  if (response.status === 403) throw Error('unavailable')
  if (!response.ok) throw Error('rejected')
  return response.json()
}
export async function transactionStatus(account: string): Promise<TransactionView> { return view.parse(await transactionCommand(account, { action: 'status' })) }
export async function transactionPreview(account: string, kind: 'deploy' | 'removal' | { compensates: string }): Promise<TransactionPreview> {
  return preview.parse(await transactionCommand(account, kind === 'deploy' ? { action: 'preview' } : kind === 'removal' ? { action: 'removal_preview' } : { action: 'compensation_preview', operation: kind.compensates }))
}
export async function reconcileTransaction(account: string, operation: string): Promise<TransactionView> { return view.parse(await transactionCommand(account, { action: 'reconcile', operation })) }
export async function recordPlacement(account: string, placement: Placement | null): Promise<TransactionView> { return view.parse(await transactionCommand(account, { action: 'record_placement', placement })) }
