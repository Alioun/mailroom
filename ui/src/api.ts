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
const operation = z.enum(['preview', 'connect', 'census', 'upload_rules', 'activate_rules', 'create_folder', 'move_messages', 'change_flags', 'change_forwarding', 'send_message', 'unsubscribe'])
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
export async function load() {
  const [a, b] = await Promise.all([fetch('/api/sample'), fetch('/api/availability')])
  return { sample: sample.parse(await responseJson(a)), availability: availability.parse(await responseJson(b)) }
}
export async function evaluate(input: Sample): Promise<Preview> {
  const response = await fetch('/api/preview', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input) })
  return preview.parse(await responseJson(response))
}
