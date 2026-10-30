<script lang="ts">
  import { onDestroy } from 'svelte'
  import { previewProtocolCopyUndo, confirmProtocolCopyUndo, type CopyUndoTarget, type CopyUndoReview } from './api'
  import Review from './CopyUndoReview.svelte'
  let { account, targets, installed, disabled, lang, onstart }: { account: string; targets: CopyUndoTarget[]; installed: false; disabled: boolean; lang: 'en' | 'de'; onstart: () => Promise<void> } = $props()
  const text = {
    en: { initial: 'Preview COPY undo', after: 'Review remaining extra copies', batch: 'Batch', clear: 'flag clear', loading: 'Reading fresh message evidence…', sending: 'Applying the confirmed actions…', error: 'The review could not complete. Reload the journal and request a fresh review. Saved history is retained.' },
    de: { initial: 'COPY-Rückgängigvorschau', after: 'Verbleibende zusätzliche Kopien prüfen', batch: 'Stapel', clear: 'Markierungsentfernung', loading: 'Frische Nachrichtennachweise werden gelesen…', sending: 'Bestätigte Aktionen werden angewendet…', error: 'Die Prüfung konnte nicht abgeschlossen werden. Journal neu laden und eine frische Prüfung anfordern. Der gespeicherte Verlauf bleibt erhalten.' },
  }
  let t = $derived(text[lang])
  let preview = $state<CopyUndoReview | null>(null)
  let phase = $state<'idle' | 'previewing' | 'confirming' | 'failed'>('idle')
  let busy = $derived(phase === 'previewing' || phase === 'confirming')
  let alive = true
  onDestroy(() => { alive = false })
  async function inspect(target: CopyUndoTarget) {
    if (!installed || disabled || busy) return
    preview = null; phase = 'previewing'
    try { const result = await previewProtocolCopyUndo(account, target); if (alive) { preview = result; phase = 'idle' } } catch { if (alive) phase = 'failed' }
  }
  async function confirm() {
    if (!preview || !installed || disabled || busy) return
    const approved = preview; preview = null; phase = 'confirming'
    try { await confirmProtocolCopyUndo(approved); if (alive) { await onstart(); phase = 'idle' } } catch { if (alive) phase = 'failed' }
  }
</script>
<div class="copy-undo">
  {#each targets as target}
    <button disabled={disabled || !installed || busy} onclick={() => inspect(target)}>{target.kind === 'initial' ? t.initial : `${t.after} · ${t.batch} ${target.forward + 1}, ${t.clear} ${target.clear + 1}`}</button>
  {/each}
  {#if busy}<p role="status">{phase === 'previewing' ? t.loading : t.sending}</p>{/if}
  {#if phase === 'failed'}<p role="alert">{t.error}</p>{/if}
  {#if preview}{#key preview.review.receipt}<Review review={preview} {lang} disabled={disabled || !installed || busy} onconfirm={confirm} oncancel={() => { preview = null; phase = 'idle' }} />{/key}{/if}
</div>
