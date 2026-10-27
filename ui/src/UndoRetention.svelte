<script lang="ts">
  import type { RunSummary, UndoDeadline } from './api'
  let { lang, days, deadline, undone = false, summaries }: { lang: 'en' | 'de'; days: number; deadline?: UndoDeadline; undone?: boolean; summaries?: RunSummary[] } = $props()
  const text = {
    en: {
      open: (d: string) => `Undo available until ${d}.`,
      closed: (d: string) => `Undo window closed on ${d}. The next action replaces this run's journal with a summary.`,
      pending: (n: number) => `Undo stays available for ${n} days after this run completes.`,
      kept: (d: string) => `Undone. The journal is kept until ${d}, then replaced by a summary.`,
      compacted: 'Compacted runs',
      compactedNote: (n: number) => `Runs completed more than ${n} days ago keep only this summary. Undo is no longer offered for them.`,
      line: (s: RunSummary, c: string, k: string) => `${s.messages} messages in ${s.batches} batches · ${s.applied} applied · ${s.restored} restored · completed ${c} · compacted ${k}`,
      status: { complete: 'complete', undone: 'undone' },
    },
    de: {
      open: (d: string) => `Rückgängigmachen möglich bis ${d}.`,
      closed: (d: string) => `Frist zum Rückgängigmachen endete am ${d}. Die nächste Aktion ersetzt das Journal dieses Ablaufs durch eine Zusammenfassung.`,
      pending: (n: number) => `Rückgängigmachen bleibt ${n} Tage nach Abschluss dieses Ablaufs möglich.`,
      kept: (d: string) => `Rückgängig gemacht. Das Journal bleibt bis ${d} erhalten und wird dann durch eine Zusammenfassung ersetzt.`,
      compacted: 'Zusammengefasste Abläufe',
      compactedNote: (n: number) => `Abläufe, die vor mehr als ${n} Tagen abgeschlossen wurden, behalten nur diese Zusammenfassung. Rückgängigmachen wird für sie nicht mehr angeboten.`,
      line: (s: RunSummary, c: string, k: string) => `${s.messages} Nachrichten in ${s.batches} Stapeln · ${s.applied} angewendet · ${s.restored} zurückgesetzt · abgeschlossen ${c} · zusammengefasst ${k}`,
      status: { complete: 'abgeschlossen', undone: 'rückgängig gemacht' },
    },
  }
  let t = $derived(text[lang])
  let format = $derived(new Intl.DateTimeFormat(lang === 'de' ? 'de-DE' : 'en-GB', { dateStyle: 'long', timeStyle: 'short' }))
  const at = (seconds: number) => format.format(new Date(seconds * 1000))
</script>
{#if deadline}
  {#if deadline.undo_until === null}<p class="undo-deadline">{t.pending(days)}</p>
  {:else if undone}<p class="undo-deadline" data-undo-until={deadline.undo_until}>{t.kept(at(deadline.undo_until))}</p>
  {:else if deadline.undo_open}<p class="undo-deadline" data-undo-until={deadline.undo_until}>{t.open(at(deadline.undo_until))}</p>
  {:else}<p class="undo-deadline closed" data-undo-until={deadline.undo_until}>{t.closed(at(deadline.undo_until))}</p>{/if}
{/if}
{#if summaries?.length}
  <h5>{t.compacted}</h5><p>{t.compactedNote(days)}</p>
  {#each summaries as s (s.id)}<article class="run-summary" data-run-summary={s.id}><p>{t.status[s.status]} · {t.line(s, at(s.completed_at), at(s.compacted_at))}</p><ul>{#each Object.entries(s.folders) as [source, targets]}{#each Object.entries(targets) as [target, count]}<li>{source} → {target} · {count}</li>{/each}{/each}</ul></article>{/each}
{/if}
<style>
  p, li { overflow-wrap: anywhere; }
  .closed { font-weight: 600; }
</style>
