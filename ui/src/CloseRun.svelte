<script lang="ts">
  import { previewClose, sortingPost, type BacklogKind, type ClosePreview } from './api'
  type Run = { id: string; status: string; closed?: { at: number; receipt: string } }
  let { kind, account, run, closable, lang, disabled, onchange }: { kind: BacklogKind; account: string; run: Run; closable: boolean; lang: 'en' | 'de'; disabled: boolean; onchange: () => Promise<void> } = $props()
  const text = {
    en: {
      preview: 'Check whether this run can be closed',
      confirm: 'I confirm closing this run as acknowledged. No mail changes, the conflict stays recorded and the journal is kept.',
      close: 'Close run without changing mail',
      closed: 'Closed by the owner on {date}. The conflict stays recorded; the journal is kept and no longer blocks new runs.',
      refused: 'This run cannot be closed: something is still undoable or uncertain. Reconcile or undo it first.',
      reason: {
        nothing_to_undo: 'A fresh check found nothing left to undo. Every remaining message is in conflict and stays as it is.',
        undo_ended: 'Undo already ran. The messages it could not restore stay in conflict as they are.',
        undo_unavailable: 'Undo is no longer offered for this run: its approval inputs changed or the undo window closed. Completed changes stay as they are.',
      },
    },
    de: {
      preview: 'Prüfen, ob dieser Ablauf geschlossen werden kann',
      confirm: 'Ich bestätige, diesen Ablauf als zur Kenntnis genommen zu schließen. Keine Nachricht ändert sich, der Konflikt bleibt verzeichnet und das Journal bleibt erhalten.',
      close: 'Ablauf schließen, ohne Nachrichten zu ändern',
      closed: 'Vom Inhaber am {date} geschlossen. Der Konflikt bleibt verzeichnet; das Journal bleibt erhalten und blockiert keine neuen Abläufe mehr.',
      refused: 'Dieser Ablauf kann nicht geschlossen werden: Etwas ist noch rückgängig zu machen oder ungewiss. Zuerst abgleichen oder rückgängig machen.',
      reason: {
        nothing_to_undo: 'Eine frische Prüfung fand nichts mehr rückgängig zu machen. Alle übrigen Nachrichten stehen im Konflikt und bleiben, wie sie sind.',
        undo_ended: 'Das Rückgängigmachen lief bereits. Nachrichten, die es nicht wiederherstellen konnte, bleiben im Konflikt, wie sie sind.',
        undo_unavailable: 'Rückgängigmachen wird für diesen Ablauf nicht mehr angeboten: Die Freigabegrundlagen haben sich geändert oder die Frist ist abgelaufen. Abgeschlossene Änderungen bleiben, wie sie sind.',
      },
    },
  }
  let t = $derived(text[lang])
  let preview = $state<ClosePreview | null>(null)
  let confirmed = $state(false)
  let refused = $state(false)
  let busy = $state(false)
  async function inspect() {
    busy = true; refused = false; preview = null; confirmed = false
    try { preview = await previewClose(kind, account, run.id) } catch { refused = true } finally { busy = false }
  }
  async function close() {
    if (!preview || !confirmed) return
    busy = true
    try { await sortingPost(kind, account, { action: 'close', run: run.id, receipt: preview.receipt, confirmed: true }); preview = null; await onchange() } catch { refused = true } finally { busy = false }
  }
  const date = (seconds: number) => new Date(seconds * 1000).toLocaleDateString(lang === 'de' ? 'de-DE' : 'en-GB', { year: 'numeric', month: 'long', day: 'numeric' })
</script>
{#if run.closed}
  <p class="run-closed">{t.closed.replace('{date}', date(run.closed.at))}</p>
{:else if closable}
  <button class="close-preview" disabled={disabled || busy} onclick={inspect}>{t.preview}</button>
  {#if refused}<p role="status">{t.refused}</p>{/if}
  {#if preview}<div class="confirmation"><p>{t.reason[preview.reason]}</p><label><input type="checkbox" bind:checked={confirmed} disabled={disabled || busy} />{t.confirm}</label><button disabled={disabled || busy || !confirmed} onclick={close}>{t.close}</button></div>{/if}
{/if}
<style>
  .confirmation { display: flex; flex-wrap: wrap; gap: .75rem; margin-block: 1rem; }
  p, label { overflow-wrap: anywhere; }
  label { display: block; }
</style>
