import { z } from 'zod'

const text = (limit: number) => z.string().refine(value =>
  value.trim().length > 0 && Array.from(value).length <= limit && !/[\u0000-\u0008\u000b-\u001f\u007f-\u009f\ud800-\udfff]/u.test(value),
)

export const agentProposal = z.strictObject({
  version: z.literal(1),
  summary_sha256: z.string().regex(/^[0-9a-f]{64}$/u),
  recommendations: z.array(z.strictObject({
    title: text(120),
    reason: text(2000),
    suggested_change: text(2000),
  })).min(1).max(20),
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
