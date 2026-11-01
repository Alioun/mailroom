import { z } from 'zod'

const text = (limit: number) => z.string().refine(value =>
  value.trim().length > 0 && Array.from(value).length <= limit && !/[\u0000-\u0008\u000b-\u001f\u007f-\u009f\ud800-\udfff]/u.test(value),
)

const value = z.strictObject({ value: z.string() })
const condition = z.discriminatedUnion('kind', [
  value.extend({ kind: z.literal('from_address') }),
  value.extend({ kind: z.literal('from_domain') }),
  z.strictObject({ kind: z.literal('sender_set'), addresses: z.array(z.string()).optional(), domains: z.array(z.string()).optional() }),
  value.extend({ kind: z.literal('to_address') }),
  value.extend({ kind: z.literal('subject_contains') }),
  value.extend({ kind: z.literal('list_id_exact') }),
  ...(['list_id_exists', 'list_id_absent', 'one_click', 'non_bulk', 'attachment'] as const).map(kind => z.strictObject({ kind: z.literal(kind) })),
])
const category = z.enum(['money', 'contracts', 'orders', 'travel', 'social', 'newsletters'])
const patch = z.strictObject({
  id: z.string(),
  enabled: z.boolean().nullable().optional(),
  folder: z.string().nullable().optional(),
  when: z.array(condition).nullable().optional(),
  extend: z.array(condition).optional(),
  accounts: z.array(z.string()).nullable().optional(),
  mark_read: z.boolean().nullable().optional(),
})
const rule = z.strictObject({
  id: z.string(),
  category,
  stage: z.enum(['guard', 'from', 'subject']),
  when: z.array(condition),
  folder: z.string().nullable().optional(),
  enabled: z.boolean().optional(),
  accounts: z.array(z.string()).optional(),
  retain: z.boolean().optional(),
  mark_read: z.boolean().optional(),
})
export const ruleEdits = z.strictObject({ patches: z.array(patch).optional(), additions: z.array(rule).optional() })
  .refine(edits => { const count = (edits.patches?.length ?? 0) + (edits.additions?.length ?? 0); return count >= 1 && count <= 20 })

export const agentProposal = z.strictObject({
  version: z.literal(1),
  summary_sha256: z.string().regex(/^[0-9a-f]{64}$/u),
  recommendations: z.array(z.strictObject({
    title: text(120),
    reason: text(2000),
    suggested_change: text(2000),
  })).min(1).max(20),
  rule_edits: ruleEdits.optional(),
})

export type AgentProposal = z.infer<typeof agentProposal>

export async function readAgentProposal(document: string, summary: string): Promise<AgentProposal> {
  const encoder = new TextEncoder()
  if (encoder.encode(document).length > 131_072) throw new Error('Proposal too large')
  const proposal = agentProposal.parse(JSON.parse(document))
  const hash = await crypto.subtle.digest('SHA-256', encoder.encode(summary))
  const expected = Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, '0')).join('')
  if (proposal.summary_sha256 !== expected) throw new Error('Proposal summary changed')
  return proposal
}
