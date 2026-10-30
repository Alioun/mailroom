<script lang="ts">
  import type { LandingReport } from './landing'
  let { report, lang = 'en' }: { report: LandingReport; lang?: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Observed folders and recorded rules',
      detail: 'Recomputed from saved received messages and the exact last recorded deployment. This includes older messages and later moves. It does not prove initial delivery, continued activation, copy receipt or keyword storage.',
      unavailable: { no_recorded_deployment: 'No deployment is recorded. A downloaded script alone is not a deployment.', transaction_unsettled: 'The latest transaction is unresolved or has no complete observed result.', no_active_composed_deployment: 'The latest transaction does not record an active composed deployment.', historical_semantics: 'This historical deployment uses an earlier evaluation order.', artifact_not_retained: 'The exact compiled artifact is no longer in retained exports. Current edited rules cannot replace it.', recorded_bytes_differ: 'The retained artifact does not reproduce the recorded script bytes.', no_completed_refresh: 'There is no completed census refresh to compare.', evaluation_unavailable: 'The recorded composition cannot be evaluated safely.' },
      historical: 'Restored historical data. This is not a current provider observation.', partial: 'The current acquisition is incomplete. This comparison covers saved records only.',
      matches: 'match the predicted folder', elsewhere: 'observed elsewhere', additional: 'also have other observed folders', unknown: 'cannot be checked',
      show: 'Inspect the local comparison', subject: 'Message', observed: 'Observed folders', expected: 'Predicted folder', result: 'Comparison', mailroom: 'Mailroom filing', existing_rule: 'Existing rule',
      comparisons: { matches: 'Matches', elsewhere: 'Observed elsewhere', additional_folders: 'Additional folders observed' },
      reasons: { recipient_evidence: 'Recipient evidence is unknown', existing_rule_unknown: 'Existing-rule outcome is unknown', not_a_folder_destination: 'The existing rule does not predict a folder', no_observed_folder: 'No folder was observed' },
      empty: 'No received messages in the saved comparison.', previous: 'Previous messages', next: 'Next messages', range: 'Messages', of: 'of', snapshot: 'Census SHA-256', deployment: 'Recorded transaction', refreshed: 'Last completed refresh',
    },
    de: {
      title: 'Beobachtete Ordner und gespeicherte Regeln',
      detail: 'Aus gespeicherten empfangenen Nachrichten und dem exakt zuletzt gespeicherten Einsatz neu berechnet. Enthält ältere Nachrichten und spätere Verschiebungen. Belegt weder ursprüngliche Zustellung noch weitere Aktivierung, Kopieempfang oder Stichwortspeicherung.',
      unavailable: { no_recorded_deployment: 'Kein Einsatz gespeichert. Ein heruntergeladenes Skript allein ist kein Einsatz.', transaction_unsettled: 'Die letzte Transaktion ist ungeklärt oder hat kein vollständig beobachtetes Ergebnis.', no_active_composed_deployment: 'Die letzte Transaktion enthält keinen aktiven zusammengesetzten Einsatz.', historical_semantics: 'Dieser ältere Einsatz verwendet eine frühere Auswertungsreihenfolge.', artifact_not_retained: 'Das exakte kompilierte Artefakt fehlt in den aufbewahrten Exporten. Aktuell bearbeitete Regeln können es nicht ersetzen.', recorded_bytes_differ: 'Das aufbewahrte Artefakt erzeugt nicht die gespeicherten Skriptbytes.', no_completed_refresh: 'Es gibt keine abgeschlossene Erfassung für den Vergleich.', evaluation_unavailable: 'Der gespeicherte zusammengesetzte Einsatz lässt sich nicht sicher auswerten.' },
      historical: 'Wiederhergestellte historische Daten. Keine aktuelle Anbieterbeobachtung.', partial: 'Die aktuelle Erfassung ist unvollständig. Der Vergleich umfasst nur gespeicherte Datensätze.',
      matches: 'entsprechen dem vorhergesagten Ordner', elsewhere: 'anderswo beobachtet', additional: 'haben weitere beobachtete Ordner', unknown: 'nicht prüfbar',
      show: 'Lokalen Vergleich ansehen', subject: 'Nachricht', observed: 'Beobachtete Ordner', expected: 'Vorhergesagter Ordner', result: 'Vergleich', mailroom: 'Mailroom-Ablage', existing_rule: 'Vorhandene Regel',
      comparisons: { matches: 'Stimmt überein', elsewhere: 'Anderswo beobachtet', additional_folders: 'Weitere Ordner beobachtet' },
      reasons: { recipient_evidence: 'Empfängerbelege unbekannt', existing_rule_unknown: 'Ergebnis vorhandener Regeln unbekannt', not_a_folder_destination: 'Die vorhandene Regel sagt keinen Ordner vorher', no_observed_folder: 'Kein Ordner beobachtet' },
      empty: 'Keine empfangenen Nachrichten im gespeicherten Vergleich.', previous: 'Vorherige Nachrichten', next: 'Nächste Nachrichten', range: 'Nachrichten', of: 'von', snapshot: 'Zensus SHA-256', deployment: 'Gespeicherte Transaktion', refreshed: 'Letzte abgeschlossene Erfassung',
    },
  }
  const t = $derived(strings[lang])
  let page = $state(1)
  let pageCount = $derived(report.kind === 'compared' ? Math.max(1, Math.ceil(report.rows.length / 50)) : 1)
  let current = $derived(Math.min(pageCount, Math.max(1, page)))
  let rows = $derived(report.kind === 'compared' ? report.rows.slice((current - 1) * 50, current * 50) : [])
</script>

<section class="landing-comparison" aria-label={t.title}>
  <h3>{t.title}</h3>
  <p class="help">{t.detail}</p>
  {#if report.kind === 'unavailable'}
    <p>{t.unavailable[report.reason]}</p>
  {:else}
    {#if report.historical}<p class="warning">{t.historical}</p>{/if}
    {#if !report.complete}<p class="warning">{t.partial}</p>{/if}
    <p>{report.counts.matches} {t.matches} · {report.counts.elsewhere} {t.elsewhere} · {report.counts.additional_folders} {t.additional} · {report.counts.uncheckable} {t.unknown}</p>
    <details><summary>{t.show}</summary>
      <p>{t.refreshed} · {new Date(report.refreshed_at * 1000).toISOString()}</p>
      <p class="help">{t.deployment} · {report.transaction} · {report.target}</p>
      <p class="help">{t.snapshot} · <code>{report.census_sha256}</code></p>
      {#if rows.length === 0}<p>{t.empty}</p>{:else}
        <p aria-live="polite">{t.range} {(current - 1) * 50 + 1}–{Math.min(current * 50, report.rows.length)} {t.of} {report.rows.length}</p>
        <div class="proof-pagination"><button disabled={current === 1} onclick={() => page = current - 1}>{t.previous}</button><button disabled={current === pageCount} onclick={() => page = current + 1}>{t.next}</button></div>
        <div class="table-scroll"><table><thead><tr><th>{t.subject}</th><th>{t.observed}</th><th>{t.expected}</th><th>{t.result}</th></tr></thead><tbody>
          {#each rows as row (row.id)}<tr><td>{row.subject}<br /><span class="help">{row.from}</span></td><td>{row.observed.join(', ')}</td>
            {#if row.prediction.kind === 'folder'}<td>{row.prediction.destination.kind === 'inbox' ? 'INBOX' : row.prediction.destination.name}<br /><span class="help">{t[row.prediction.origin]}</span></td><td>{t.comparisons[row.prediction.comparison]}</td>
            {:else}<td>—</td><td>{t.reasons[row.prediction.reason]}</td>{/if}
          </tr>{/each}
        </tbody></table></div>
      {/if}
    </details>
  {/if}
</section>

<style>
  .landing-comparison { margin-block: 28px; min-width: 0; }
  .table-scroll { max-width: 100%; overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; text-align: start; }
  th, td { padding: 12px 16px 12px 0; border-bottom: 1px solid var(--line); vertical-align: top; overflow-wrap: anywhere; min-width: 150px; }
  code { overflow-wrap: anywhere; }
</style>
