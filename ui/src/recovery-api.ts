import { z } from 'zod'
import { localFetch } from './api'
const inventory = z.object({ resource: z.string(), bytes: z.number(), sha256: z.string() })
const keyMaterial = z.object({ resource: z.string(), bytes: z.number() })
const accountManifest = z.object({ version: z.literal(1), state_schema: z.number().int().min(1).max(33), captured_at: z.number(), account: z.string(), provider: z.enum(['mailbox_org', 'gmail']), login: z.string(), generation: z.number(), revision: z.number(), resources: z.array(inventory), key_material: z.array(keyMaterial), dependencies: z.array(z.string()), shared_state: z.literal('excluded_preserve_current'), excluded: z.array(z.string()), limitations: z.string() })
const ledger = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('excluded'), source_version: z.string().nullable() }),
  z.object({ kind: z.literal('included'), inventory, schema: z.literal(1), entries: z.number(), semantics: z.string() }),
])
const manifest = z.object({ version: z.literal(2), captured_at: z.number(), accounts: z.array(accountManifest).min(1).max(8), excluded_accounts: z.array(z.string()), ledger, limitations: z.string() })
const ledgerTarget = z.discriminatedUnion('kind', [z.object({ kind: z.literal('preserve_current'), version: z.string().nullable() }), z.object({ kind: z.literal('absent') }), z.object({ kind: z.literal('identical'), version: z.string() })])
const restorePreview = z.object({ manifest, bundle: z.string(), target: z.string(), accounts_absent: z.literal(true), ledger_target: ledgerTarget, consequences: z.string() })
const confirmation = z.object({ display: z.string(), synthetic: z.boolean() })
const captureOffer = z.object({ preview: manifest, receipt: z.string(), confirmation })
const restoreOffer = z.object({ preview: restorePreview, receipt: z.string(), confirmation, authenticated: z.literal(true) })
const bundleInfo = z.object({ id: z.string(), filename: z.string(), modified_at: z.number(), bytes: z.number() })
const status = z.object({ pending: z.array(z.string()), selected_pending: z.array(z.string()), restored: z.array(z.object({ account: z.string(), captured_at: z.number(), authenticated: z.boolean(), reconciliation_required: z.boolean(), bundle: z.string(), resources: z.array(inventory), key_material: z.array(keyMaterial) })), bundles: z.array(z.string()), bundles_dir: z.string().optional(), bundle_info: z.array(bundleInfo).default([]), production_available: z.boolean(), limitations: z.string() })
export type BundleInfo = z.infer<typeof bundleInfo>
export type CaptureOffer = z.infer<typeof captureOffer>
export type RestoreOffer = z.infer<typeof restoreOffer>
export type RecoveryStatus = z.infer<typeof status>
async function post(body: unknown): Promise<unknown> {
  const response = await localFetch('/api/accounts/recovery', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  const data: unknown = await response.json().catch(() => null)
  if (!response.ok) {
    const parsed = z.object({ error: z.string() }).safeParse(data)
    throw Error(parsed.success ? parsed.data.error : 'Recovery rejected. Preserve existing files and the separate secret.')
  }
  return data
}
export async function recoveryStatus() { return status.parse(await post({ action: 'status' })) }
export async function previewCapture(accounts: string[], includeLedger: boolean, passphrase?: string) { return captureOffer.parse(await post({ action: 'selected_capture_preview', accounts, include_ledger: includeLedger, ...(passphrase ? { passphrase } : {}) })) }
export async function capture(offer: CaptureOffer, passphrase?: string) { return z.object({ captured: z.literal(true), bundle: z.string() }).parse(await post({ action: 'selected_capture', preview: offer.preview, receipt: offer.receipt, ...(passphrase ? { passphrase } : {}) })) }
export async function previewRestore(bundle: string, restoreLedger: boolean, passphrase?: string) { return restoreOffer.parse(await post({ action: 'selected_restore_preview', bundle, restore_ledger: restoreLedger, ...(passphrase ? { passphrase } : {}) })) }
export async function restore(offer: RestoreOffer, passphrase?: string) { return post({ action: 'selected_restore', preview: offer.preview, receipt: offer.receipt, ...(passphrase ? { passphrase } : {}) }) }
export async function resumeRestore(passphrase?: string) { return post({ action: 'selected_resume', ...(passphrase ? { passphrase } : {}) }) }
export async function resumeLegacyRestore(account: string, passphrase?: string) { return post({ action: 'resume', account, ...(passphrase ? { passphrase } : {}) }) }
