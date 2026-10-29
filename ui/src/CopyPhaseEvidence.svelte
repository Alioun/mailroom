<script lang="ts">
  import type { ProtocolEvidence } from './api'
  let { evidence, lang }: { evidence: NonNullable<ProtocolEvidence['copy_journal']>; lang: 'en' | 'de' } = $props()
  const text = {
    en: { title: 'COPY phases', mapped: 'COPYUID mapping', flags: 'Conditional flag attempt', expunge: 'Exact UID expunge attempt', readback: 'Readback recorded', missing: 'Readback missing', before: 'Observed per-message MODSEQ', lost: 'Acknowledgment missing', rejected: 'Response rejected', pending: 'No acknowledgment recorded' },
    de: { title: 'COPY-Phasen', mapped: 'COPYUID-Zuordnung', flags: 'Bedingte Markierungsänderung versucht', expunge: 'Gezieltes UID EXPUNGE versucht', readback: 'Prüfung gespeichert', missing: 'Prüfung fehlt', before: 'Beobachteter MODSEQ pro Nachricht', lost: 'Bestätigung fehlt', rejected: 'Antwort abgelehnt', pending: 'Keine Bestätigung gespeichert' },
  }
  let t = $derived(text[lang])
  function completion(result: { kind: 'tagged'; status: string } | { kind: 'lost' | 'rejected' } | null) {
    if (result === null) return t.pending
    if (result.kind === 'tagged') return result.status.toUpperCase()
    return result.kind === 'lost' ? t.lost : t.rejected
  }
</script>
<div class="copy-phases">
  <h5>{t.title}</h5>
  {#if evidence.copy.mapping.kind === 'validated'}<p>{t.mapped} · {JSON.stringify(evidence.copy.mapping.mapping.pairs)}</p>{/if}
  <p>{completion(evidence.copy.completion)} · {evidence.readback ? t.readback : t.missing}</p>
  {#each evidence.deletions as batch}
    <div data-copy-phase="store">
      <p>{t.flags} · {Object.keys(batch.evidence.flag.intent.before.messages).join(', ')}</p>
      <p>{t.before} · {Object.entries(batch.evidence.flag.intent.before.messages).map(([uid, value]) => `${uid} = ${value.modseq}`).join(', ')}</p>
      <p>{completion(batch.evidence.flag.result)} · {batch.evidence.flag.readback ? t.readback : t.missing}</p>
    </div>
    {#if batch.evidence.expunge}
      <div data-copy-phase="expunge"><p>{t.expunge} · {Object.keys(batch.evidence.expunge.before.messages).join(', ')}</p><p>{completion(batch.evidence.expunge.completion)} · {batch.evidence.expunge.readback ? t.readback : t.missing}</p></div>
    {/if}
  {/each}
</div>
