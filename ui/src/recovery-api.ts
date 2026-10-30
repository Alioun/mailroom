import { z } from 'zod'
import { localFetch } from './api'
const inventory = z.object({ resource: z.string(), bytes: z.number(), sha256: z.string() })
const keyMaterial = z.object({ resource: z.string(), bytes: z.number() })
const accountManifest = z.object({ version: z.literal(1), state_schema: z.number().int().min(1).max(30), captured_at: z.number(), account: z.string(), provider: z.enum(['mailbox_org', 'gmail']), login: z.string(), generation: z.number(), revision: z.number(), resources: z.array(inventory), key_material: z.array(keyMaterial), dependencies: z.array(z.string()), shared_state: z.literal('excluded_preserve_current'), excluded: z.array(z.string()), limitations: z.string() })
const ledger = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('excluded'), source_version: z.string().nullable() }),
  z.object({ kind: z.literal('included'), inventory, schema: z.literal(1), entries: z.number(), semantics: z.string() }),
])
const manifest = z.object({ version: z.literal(2), captured_at: z.number(), accounts: z.array(accountManifest).min(1).max(8), excluded_accounts: z.array(z.string()), ledger, limitations: z.string() })
const ledgerTarget = z.discriminatedUnion('kind', [z.object({ kind: z.literal('preserve_current'), version: z.string().nullable() }), z.object({ kind: z.literal('absent') }), z.object({ kind: z.literal('identical'), version: z.string() })])
const restorePreview = z.object({ manifest, bundle: z.string(), target: z.string(), accounts_absent: z.literal(true), ledger_target: ledgerTarget, consequences: z.string() })
const confirmation = z.object({ display: z.string(), synthetic: z.literal(true) })
const captureOffer = z.object({ preview: manifest, receipt: z.string(), confirmation })
const restoreOffer = z.object({ preview: restorePreview, receipt: z.string(), confirmation, authenticated: z.literal(true) })
const status = z.object({ pending: z.array(z.string()), selected_pending: z.array(z.string()), restored: z.array(z.object({ account: z.string(), captured_at: z.number(), authenticated: z.boolean(), reconciliation_required: z.boolean(), bundle: z.string(), resources: z.array(inventory), key_material: z.array(keyMaterial) })), bundles: z.array(z.string()), production_available: z.literal(false), limitations: z.string() })
export type CaptureOffer = z.infer<typeof captureOffer>
export type RestoreOffer = z.infer<typeof restoreOffer>
export type RecoveryStatus = z.infer<typeof status>
async function post(body: unknown): Promise<unknown> {
  const response = await localFetch('/api/accounts/recovery', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  const data: unknown = await response.json()
  if (!response.ok) throw Error('Recovery rejected. Preserve existing files and the separate secret.')
  return data
}
export async function recoveryStatus() { return status.parse(await post({ action: 'status' })) }
export async function previewCapture(accounts: string[], includeLedger: boolean) { return captureOffer.parse(await post({ action: 'selected_capture_preview', accounts, include_ledger: includeLedger })) }
export async function capture(offer: CaptureOffer) { return z.object({ captured: z.literal(true), bundle: z.string() }).parse(await post({ action: 'selected_capture', preview: offer.preview, receipt: offer.receipt })) }
export async function previewRestore(bundle: string, restoreLedger: boolean) { return restoreOffer.parse(await post({ action: 'selected_restore_preview', bundle, restore_ledger: restoreLedger })) }
export async function restore(offer: RestoreOffer) { return post({ action: 'selected_restore', preview: offer.preview, receipt: offer.receipt }) }
export async function resumeRestore() { return post({ action: 'selected_resume' }) }
export async function resumeLegacyRestore(account: string) { return post({ action: 'resume', account }) }
