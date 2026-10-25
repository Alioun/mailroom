import { z } from 'zod'
import { localFetch } from './api'

const context = z.object({ account: z.string(), provider: z.enum(['mailbox_org', 'gmail']), profile: z.string(), source_epoch: z.string(), tls_identity: z.string(), authority: z.string(), adapter: z.literal('managesieve-read-v1') })
const capabilities = z.record(z.string(), z.string())
const names = z.array(z.object({ name: z.string(), active: z.boolean() })).max(256)
const body = z.object({ name: z.string(), bytes: z.array(z.number().int().min(0).max(255)).max(1048576), sha256: z.string().regex(/^[a-f0-9]{64}$/) })
const baseline = z.object({ attempt: z.string(), observed_at: z.number(), context, capabilities, names, bodies: z.array(body).max(2) })
const attempt = z.object({ id: z.string(), started_at: z.number(), finished_at: z.number().nullable(), context, outcome: z.enum(['running', 'complete', 'failed', 'cancelled', 'interrupted']), failure: z.string().nullable(), capabilities: capabilities.nullable(), names: names.nullable(), bodies: z.array(body).max(2) })
const view = z.object({ installed: z.literal(true), current: z.boolean(), recovery_hold: z.boolean(), facts: z.object({ version: z.number().int().min(0).max(1), latest: attempt.nullable(), last_successful: baseline.nullable() }), limits: z.object({ script_bytes: z.number(), attempt_bytes: z.number(), names: z.number() }), limitations: z.string() })
const status = z.object({ installed: z.literal(true), running: z.boolean(), error: z.string().nullable() })
export type SieveView = z.infer<typeof view>
export type SieveBody = z.infer<typeof body>
async function command(account: string, action: 'open' | 'start' | 'cancel' | 'status'): Promise<unknown> {
  const response = await localFetch('/api/accounts/managesieve', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ account, command: { action } }) })
  if (response.status === 403) throw new Error('unavailable')
  if (!response.ok) throw new Error('rejected')
  return response.json()
}
export async function openSieve(account: string): Promise<SieveView> { return view.parse(await command(account, 'open')) }
export async function startSieve(account: string): Promise<void> { z.object({ started: z.literal(true) }).parse(await command(account, 'start')) }
export async function cancelSieve(account: string): Promise<void> { z.object({ cancel_requested: z.literal(true) }).parse(await command(account, 'cancel')) }
export async function sieveStatus(account: string): Promise<z.infer<typeof status>> { return status.parse(await command(account, 'status')) }
export function exactReview(body: SieveBody): string {
  let text: string
  try { text = new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(body.bytes)) } catch { text = '(UTF-8 unavailable)' }
  const hex = body.bytes.map(b => b.toString(16).padStart(2, '0')).join(' ')
  return `UTF-8\n${text}\n\nHEX\n${hex}`
}
