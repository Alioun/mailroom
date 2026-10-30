<script lang="ts">
  import { copyUndoRows, type CopyUndoReview } from './api'
  let { review, lang, disabled, onconfirm, oncancel }: { review: CopyUndoReview; lang: 'en' | 'de'; disabled: boolean; onconfirm: () => Promise<void>; oncancel: () => void } = $props()
  const text = {
    en: { initial: 'Review COPY undo', after: 'Review extra-copy removal', separate: 'This is a separate approval after clearing Deleted. The original Undo approval stays recorded.', restore: 'Restore messages', remove: 'Remove extra copies', clear: 'Clear run-owned Deleted flags', conflicts: 'Messages left unchanged by safety checks', retained: 'Keep originals in', from: 'From', to: 'To', messages: 'Message UIDs', extra: 'Clearing Deleted leaves an extra copy. Removing that copy requires another review and approval.', confirm: 'I confirm only these exact actions', apply: 'Apply reviewed Undo actions', applyAfter: 'Remove these extra copies', cancel: 'Close review', detail: 'Exact local review record', account: 'Account', run: 'Run', receipt: 'Confirmation receipt', empty: 'No eligible action remains. Close this review and reload the journal.' },
    de: { initial: 'COPY rückgängig machen prüfen', after: 'Zusätzliche Kopien entfernen prüfen', separate: 'Dies ist eine separate Freigabe nach dem Entfernen von Deleted. Die ursprüngliche Rückgängig-Freigabe bleibt gespeichert.', restore: 'Nachrichten wiederherstellen', remove: 'Zusätzliche Kopien entfernen', clear: 'Vom Ablauf gesetzte Deleted-Markierungen entfernen', conflicts: 'Nachrichten bleiben nach Sicherheitsprüfung unverändert', retained: 'Originale behalten in', from: 'Aus', to: 'Nach', messages: 'Nachrichten-UIDs', extra: 'Nach dem Entfernen von Deleted bleibt eine zusätzliche Kopie. Ihre Entfernung braucht eine neue Prüfung und Freigabe.', confirm: 'Ich bestätige nur diese genauen Aktionen', apply: 'Geprüfte Rückgängig-Aktionen anwenden', applyAfter: 'Diese zusätzlichen Kopien entfernen', cancel: 'Prüfung schließen', detail: 'Genauer lokaler Prüfbeleg', account: 'Konto', run: 'Ablauf', receipt: 'Bestätigungsbeleg', empty: 'Keine geeignete Aktion bleibt übrig. Prüfung schließen und Journal neu laden.' },
  }
  let t = $derived(text[lang])
  let rows = $derived(copyUndoRows(review))
  let actions = $derived(review.review.actions)
  let confirmed = $state(false)
  async function apply() { if (confirmed && !disabled && rows.length > 0) await onconfirm() }
</script>
<fieldset class="copy-review" disabled={disabled}>
  <legend>{review.kind === 'initial' ? t.initial : t.after}</legend>
  {#if review.kind === 'after_clear'}<p>{t.separate}</p>{/if}
  <dl>
    <dt>{t.restore}</dt><dd>{actions.restore}</dd>
    <dt>{t.remove}</dt><dd>{actions.remove_copy}</dd>
    <dt>{t.clear}</dt><dd>{actions.clear_deleted}</dd>
    <dt>{t.conflicts}</dt><dd>{actions.conflicts}</dd>
  </dl>
  {#if actions.clear_deleted > 0}<p>{t.extra}</p>{/if}
  <ul>
    {#each rows as row}
      <li>
        {#if row.kind === 'restore'}
          <strong>{t.restore}</strong><p>{t.from} {row.intent.source.name.decoded} · {t.to} {row.intent.destination.name.decoded}</p><p>{t.messages} · {Object.keys(row.intent.messages).join(', ')}</p>
        {:else if row.kind === 'remove_copy'}
          <strong>{t.remove}</strong><p>{t.from} {row.plan.deletion.before.identity.name.decoded} · {t.messages} {Object.keys(row.plan.deletion.before.messages).join(', ')}</p><p>{t.retained} {row.plan.retained.identity.name.decoded} · {t.messages} {Object.keys(row.plan.retained.messages).join(', ')}</p>
        {:else}
          <strong>{t.clear}</strong><p>{row.plan.before.identity.name.decoded} · {t.messages} {Object.keys(row.plan.before.messages).join(', ')}</p>
        {/if}
      </li>
    {/each}
  </ul>
  <details><summary>{t.detail}</summary><dl><dt>{t.account}</dt><dd>{review.account}</dd><dt>{t.run}</dt><dd>{review.target.run}</dd><dt>{t.receipt}</dt><dd>{review.review.receipt}</dd></dl></details>
  {#if rows.length > 0}
    <label><input type="checkbox" bind:checked={confirmed} />{t.confirm}</label>
    <button disabled={disabled || !confirmed} onclick={apply}>{review.kind === 'initial' ? t.apply : t.applyAfter}</button>
  {:else}<p>{t.empty}</p>{/if}
</fieldset>
<button onclick={oncancel}>{t.cancel}</button>
<style>
  .copy-review { border: 0; padding: 0; margin-block: 1rem; min-width: 0; }
  legend { font-weight: 650; }
  dl { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: .25rem 1rem; font-variant-numeric: tabular-nums; }
  dd { margin: 0; }
  details dl { grid-template-columns: minmax(5rem, auto) minmax(0, 1fr); }
  li { margin-block: 1rem; }
  p, dt, dd, label, legend { overflow-wrap: anywhere; }
  label { display: block; margin-block: 1rem; }
</style>
