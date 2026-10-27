import { recipientEvidence, sortingPost, missingLabel, type BacklogOptions } from './api'
import { z } from 'zod'
import { localFetch } from './api'
const name = z.object({ decoded: z.string().max(1024), wire: z.string().max(2048) })
const mailbox = z.object({ name, delimiter: z.string().nullable(), attributes: z.array(z.string()).max(32) })
const namespace = z.object({ prefix: z.string(), delimiter: z.string().nullable() })
const inventory = z.object({ capabilities: z.array(z.string()).max(128), namespaces: z.object({ personal: z.array(namespace).max(8), other: z.array(namespace).max(8), shared: z.array(namespace).max(8) }), mailboxes: z.array(mailbox).max(256) })
const outcome = z.enum(['intent', 'acknowledged', 'observed', 'refused', 'uncertain', 'conflict', 'absent'])
const operation = z.object({ id: z.string(), artifact: z.string(), proof_binding: z.string(), receipt: z.string(), before: inventory, plan: z.object({ logical_leaves: z.array(z.string()).max(64), entries: z.array(z.object({ name, role: z.enum(['ancestor', 'leaf']), existing: mailbox.nullable() })).max(64) }), status: z.enum(['running', 'complete', 'refused', 'uncertain', 'stopped', 'conflict', 'fresh_review']), steps: z.array(z.object({ name, outcome, acknowledgment: z.boolean(), failure: z.string().nullable(), before: inventory, observed: inventory.nullable() })).max(64) })
const journal = z.object({ history: z.array(operation).max(8) })
const view = z.object({ installed: z.literal(true), recovery_hold: z.boolean(), journal, limitations: z.string() })
const preview = z.object({ operation, receipt: z.string(), proof: z.object({ gate: z.string(), semantics: z.string(), pins: z.array(z.object({ message_id_hash: z.string(), status: z.string() })), warnings: z.array(z.string()), runtime_cross_check: z.string(), recipient_evidence: recipientEvidence, capabilities: z.array(z.string()), context_binding: z.string().length(64) }), limitations: z.string() })
export type FolderView = z.infer<typeof view>
export type FolderPreview = z.infer<typeof preview>
type Command = { action: 'preview' | 'status' | 'cancel' } | { action: 'confirm'; receipt: string; confirmed: boolean } | { action: 'reconcile'; operation: string }
export async function folderCommand(account: string, command: Command): Promise<unknown> {
  const response = await localFetch('/api/accounts/folders', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, command }) })
  if (!response.ok) throw Error('rejected')
  return response.json()
}
export async function folderStatus(account: string): Promise<FolderView> { return view.parse(await folderCommand(account, { action: 'status' })) }
export async function folderPreview(account: string): Promise<FolderPreview> { return preview.parse(await folderCommand(account, { action: 'preview' })) }
export async function folderReconcile(account: string, operation: string): Promise<FolderView> { return view.parse(await folderCommand(account, { action: 'reconcile', operation })) }
export const labelJournal = journal
const labelPreview = z.object({ operation, receipt: z.string(), proof: z.object({ saved_proof: z.string().length(64), options: z.object({ scope: z.string(), protect_flagged: z.boolean(), selected: z.array(z.string()).nullable() }), missing: z.array(missingLabel) }), limitations: z.string() })
export type LabelPreview = z.infer<typeof labelPreview>
export type LabelOperation = z.infer<typeof operation>
export async function labelPreviewFor(account: string, options: BacklogOptions): Promise<LabelPreview> { return labelPreview.parse(await sortingPost('gmail', account, { action: 'label_preview', options })) }
