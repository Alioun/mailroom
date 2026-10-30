import { z } from 'zod'

const count = z.number().int().nonnegative()
const destination = z.discriminatedUnion('kind', [z.object({ kind: z.literal('inbox') }), z.object({ kind: z.literal('folder'), name: z.string() })])
export const landingReport = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('unavailable'), reason: z.enum(['no_recorded_deployment', 'transaction_unsettled', 'no_active_composed_deployment', 'historical_semantics', 'artifact_not_retained', 'recorded_bytes_differ', 'no_completed_refresh', 'evaluation_unavailable']) }),
  z.object({
    kind: z.literal('compared'), semantics: z.literal('mailroom/recorded-placement-comparison/v1'), account: z.string(), census_sha256: z.string().regex(/^[a-f0-9]{64}$/), refreshed_at: z.number().int().min(-8640000000000).max(8640000000000), scan_state: z.enum(['idle', 'running', 'cancelled', 'complete', 'failed']), complete: z.boolean(), historical: z.boolean(), transaction: z.string(), artifact: z.string(), baseline_source: z.enum(['retained_deployment', 'legacy_export']), target: z.string(),
    counts: z.object({ matches: count, elsewhere: count, additional_folders: count, uncheckable: count }),
    rows: z.array(z.object({ id: z.string(), subject: z.string(), from: z.string(), observed: z.array(z.string()), prediction: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('folder'), destination, origin: z.enum(['mailroom', 'existing_rule']), comparison: z.enum(['matches', 'elsewhere', 'additional_folders']) }),
      z.object({ kind: z.literal('uncheckable'), reason: z.enum(['recipient_evidence', 'existing_rule_unknown', 'not_a_folder_destination', 'no_observed_folder']) }),
    ]) })),
  }),
]).superRefine((report, context) => {
  if (report.kind !== 'compared') return
  const actual = { matches: 0, elsewhere: 0, additional_folders: 0, uncheckable: 0 }
  for (const row of report.rows) actual[row.prediction.kind === 'uncheckable' ? 'uncheckable' : row.prediction.comparison]++
  const kinds = ['matches', 'elsewhere', 'additional_folders', 'uncheckable'] satisfies (keyof typeof actual)[]
  if (kinds.some(kind => report.counts[kind] !== actual[kind])) {
    context.addIssue({ code: 'custom', message: 'Placement counts disagree with the compared rows' })
  }
  if (report.complete && report.scan_state !== 'complete') context.addIssue({ code: 'custom', message: 'Incomplete acquisition cannot claim complete comparison coverage' })
})

export type LandingReport = z.infer<typeof landingReport>
