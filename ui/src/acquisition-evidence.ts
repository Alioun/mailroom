import { z } from 'zod'

z.config({ jitless: true })

export const countObservation = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('observed'), messages: z.number().int().nonnegative(), observed_at: z.number(), source: z.enum(['status_messages', 'examine_search']) }),
  z.object({ kind: z.literal('unknown'), reason: z.enum(['not_requested', 'not_selectable', 'budget', 'denied', 'unavailable', 'malformed', 'missing', 'ambiguous', 'no_junk_role', 'older_snapshot']) }),
])
const snapshot = z.object({
  version: z.literal(1), adapter: z.string(), account: z.string(), provider: z.enum(['gmail', 'mailbox_org']), attempt_id: z.string(),
  topology_observed_at: z.number(), completed_at: z.number().nullable(), imap_capabilities: z.array(z.string()),
  folders: z.array(z.object({ wire: z.string(), name: z.string(), delimiter: z.string().nullable(), attributes: z.array(z.string()), roles: z.array(z.string()), selectable: z.boolean(), selection: z.enum(['selected', 'not_selectable', 'system_role', 'gmail_label']), count: countObservation, uidvalidity: z.number().nullable(), selected_complete: z.boolean() })),
})
export const evidenceView = z.object({
  latest: z.object({ id: z.string(), adapter: z.string(), provider: z.enum(['gmail', 'mailbox_org']), started_at: z.number(), finished_at: z.number().nullable(), state: z.enum(['idle', 'running', 'cancelled', 'complete', 'failed']), session: z.object({ observed_at: z.number(), imap_capabilities: z.array(z.string()) }).nullable(), checkpoint: snapshot.nullable() }).nullable(),
  last_successful: snapshot.nullable(), age_seconds: z.number().nullable(), current: z.boolean(), spam: countObservation,
})
export type EvidenceView = z.infer<typeof evidenceView>
export type CountObservation = z.infer<typeof countObservation>
