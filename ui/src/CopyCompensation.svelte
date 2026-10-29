<script lang="ts">
  import type { CompensationCounts } from './api'
  let { counts, lang }: { counts: CompensationCounts; lang: 'en' | 'de' } = $props()
  const text = {
    en: {
      title: 'COPY undo history',
      restored: 'Messages restored',
      copies_removed: 'Extra copies removed',
      deleted_cleared: 'Deleted flags cleared',
      pending: 'Pending',
      uncertain: 'Uncertain',
      conflicts: 'Conflicts',
      clear: 'Clearing a Deleted flag leaves an extra copy. Undo is still incomplete.',
    },
    de: {
      title: 'COPY-Rückgängigverlauf',
      restored: 'Wiederhergestellte Nachrichten',
      copies_removed: 'Entfernte zusätzliche Kopien',
      deleted_cleared: 'Entfernte Deleted-Markierungen',
      pending: 'Ausstehend',
      uncertain: 'Ungewiss',
      conflicts: 'Konflikte',
      clear: 'Nach dem Entfernen einer Deleted-Markierung bleibt eine zusätzliche Kopie. Das Rückgängigmachen ist noch nicht abgeschlossen.',
    },
  }
  let t = $derived(text[lang])
</script>
<div class="copy-compensation">
  <h5>{t.title}</h5>
  <dl>
    <dt>{t.restored}</dt><dd>{counts.restored}</dd>
    <dt>{t.copies_removed}</dt><dd>{counts.copies_removed}</dd>
    <dt>{t.deleted_cleared}</dt><dd>{counts.deleted_cleared}</dd>
    <dt>{t.pending}</dt><dd>{counts.pending}</dd>
    <dt>{t.uncertain}</dt><dd>{counts.uncertain}</dd>
    <dt>{t.conflicts}</dt><dd>{counts.conflicts}</dd>
  </dl>
  {#if counts.deleted_cleared > 0}<p>{t.clear}</p>{/if}
</div>
<style>
  dl { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: .25rem 1rem; }
  dt { overflow-wrap: anywhere; }
  dd { margin: 0; }
</style>
