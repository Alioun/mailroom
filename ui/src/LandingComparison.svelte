<script lang="ts">
  import type { LandingReport } from './landing'
  let { report, lang = 'en' }: { report: LandingReport; lang?: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Observed folders and recorded rules',
      detail: 'Recomputed from saved received messages and the exact last recorded deployment. This includes older messages and later moves. It does not prove initial delivery, continued activation, copy receipt or keyword storage.',
      unavailable: { no_recorded_deployment: 'No deployment is recorded. A downloaded script alone is not a deployment.', transaction_unsettled: 'The latest transaction is unresolved or has no complete observed result.', no_active_composed_deployment: 'The latest transaction does not record an active composed deployment.', historical_semantics: 'This historical deployment uses an earlier evaluation order.', artifact_not_retained: 'The exact deployment artifact is unavailable. Current edited rules cannot replace it.', recorded_bytes_differ: 'The retained artifact does not reproduce the recorded script bytes.', no_completed_refresh: 'There is no completed census refresh to compare.', evaluation_unavailable: 'The recorded composition cannot be evaluated safely.' },
      historical: 'Restored historical data. This is not a current provider observation.', partial: 'The current acquisition is incomplete. This comparison covers saved records only.',
      baseline: { retained_deployment: 'Exact rules retained with this deployment, independent of downloads.', legacy_export: 'Historical baseline from export history. Export eviction can make this comparison unavailable.' },
      windowTitle: 'Mailbox dates after the active-script observation', windowDetail: 'Uses IMAP INTERNALDATE and the local observation clock. Copying or importing can preserve or assign this date. This does not prove first delivery or continuous activation.',
      windowMissing: { no_active_observation: 'No active-script observation time was recorded. Older mail cannot be separated by deployment time.', refresh_predates_observation: 'The last completed refresh predates the active-script observation. Refresh before checking later mailbox dates.' },
      windowOnly: 'Show only mailbox dates in this interval', windowRange: 'Exclusive start and inclusive end', via: { readback: 'Observed after the recorded effect', reconciliation: 'Observed during later reconciliation' },
      arrival: 'Mailbox date', periods: { unavailable: 'No timing evidence', before_or_at_observation: 'At or before observation', after_observation: 'Within the interval', after_refresh: 'After the completed refresh', invalid_date: 'Invalid or missing INTERNALDATE' },
      matches: 'match the predicted folder', elsewhere: 'observed elsewhere', additional: 'also have other observed folders', unknown: 'cannot be checked',
      show: 'Inspect the local comparison', subject: 'Message', observed: 'Observed folders', expected: 'Predicted folder', result: 'Comparison', mailroom: 'Mailroom filing', existing_rule: 'Existing rule',
      comparisons: { matches: 'Matches', elsewhere: 'Observed elsewhere', additional_folders: 'Additional folders observed' },
      reasons: { recipient_evidence: 'Recipient evidence is unknown', existing_rule_unknown: 'Existing-rule outcome is unknown', not_a_folder_destination: 'The existing rule does not predict a folder', no_observed_folder: 'No folder was observed' },
      emptyWindow: 'No mailbox dates fall inside this interval.', empty: 'No received messages in the saved comparison.', previous: 'Previous messages', next: 'Next messages', range: 'Messages', of: 'of', snapshot: 'Census SHA-256', deployment: 'Recorded transaction', refreshed: 'Last completed refresh',
    },
    de: {
      title: 'Beobachtete Ordner und gespeicherte Regeln',
      detail: 'Aus gespeicherten empfangenen Nachrichten und dem exakt zuletzt gespeicherten Einsatz neu berechnet. Enthält ältere Nachrichten und spätere Verschiebungen. Belegt weder ursprüngliche Zustellung noch weitere Aktivierung, Kopieempfang oder Stichwortspeicherung.',
      unavailable: { no_recorded_deployment: 'Kein Einsatz gespeichert. Ein heruntergeladenes Skript allein ist kein Einsatz.', transaction_unsettled: 'Die letzte Transaktion ist ungeklärt oder hat kein vollständig beobachtetes Ergebnis.', no_active_composed_deployment: 'Die letzte Transaktion enthält keinen aktiven zusammengesetzten Einsatz.', historical_semantics: 'Dieser ältere Einsatz verwendet eine frühere Auswertungsreihenfolge.', artifact_not_retained: 'Das exakte Einsatzartefakt fehlt. Aktuell bearbeitete Regeln können es nicht ersetzen.', recorded_bytes_differ: 'Das aufbewahrte Artefakt erzeugt nicht die gespeicherten Skriptbytes.', no_completed_refresh: 'Es gibt keine abgeschlossene Erfassung für den Vergleich.', evaluation_unavailable: 'Der gespeicherte zusammengesetzte Einsatz lässt sich nicht sicher auswerten.' },
      historical: 'Wiederhergestellte historische Daten. Keine aktuelle Anbieterbeobachtung.', partial: 'Die aktuelle Erfassung ist unvollständig. Der Vergleich umfasst nur gespeicherte Datensätze.',
      baseline: { retained_deployment: 'Exakte Regeln mit diesem Einsatz aufbewahrt, unabhängig von Downloads.', legacy_export: 'Historische Grundlage aus dem Exportverlauf. Entfernte Exporte können diesen Vergleich unverfügbar machen.' },
      windowTitle: 'Mailbox-Daten nach der Beobachtung des aktiven Skripts', windowDetail: 'Verwendet IMAP INTERNALDATE und die lokale Beobachtungszeit. Kopieren oder Importieren kann dieses Datum erhalten oder setzen. Belegt weder erste Zustellung noch durchgehende Aktivierung.',
      windowMissing: { no_active_observation: 'Keine Beobachtungszeit des aktiven Skripts gespeichert. Ältere Nachrichten lassen sich nicht nach Einsatzzeit abgrenzen.', refresh_predates_observation: 'Die letzte abgeschlossene Erfassung liegt vor der Beobachtung des aktiven Skripts. Vor dem Prüfen späterer Mailbox-Daten neu erfassen.' },
      windowOnly: 'Nur Mailbox-Daten in diesem Zeitraum anzeigen', windowRange: 'Start ausgeschlossen, Ende eingeschlossen', via: { readback: 'Nach der gespeicherten Änderung beobachtet', reconciliation: 'Bei späterem Abgleich beobachtet' },
      arrival: 'Mailbox-Datum', periods: { unavailable: 'Keine Zeitbelege', before_or_at_observation: 'Bei oder vor Beobachtung', after_observation: 'Innerhalb des Zeitraums', after_refresh: 'Nach der abgeschlossenen Erfassung', invalid_date: 'INTERNALDATE ungültig oder fehlt' },
      matches: 'entsprechen dem vorhergesagten Ordner', elsewhere: 'anderswo beobachtet', additional: 'haben weitere beobachtete Ordner', unknown: 'nicht prüfbar',
      show: 'Lokalen Vergleich ansehen', subject: 'Nachricht', observed: 'Beobachtete Ordner', expected: 'Vorhergesagter Ordner', result: 'Vergleich', mailroom: 'Mailroom-Ablage', existing_rule: 'Vorhandene Regel',
      comparisons: { matches: 'Stimmt überein', elsewhere: 'Anderswo beobachtet', additional_folders: 'Weitere Ordner beobachtet' },
      reasons: { recipient_evidence: 'Empfängerbelege unbekannt', existing_rule_unknown: 'Ergebnis vorhandener Regeln unbekannt', not_a_folder_destination: 'Die vorhandene Regel sagt keinen Ordner vorher', no_observed_folder: 'Kein Ordner beobachtet' },
      emptyWindow: 'Keine Mailbox-Daten liegen in diesem Zeitraum.', empty: 'Keine empfangenen Nachrichten im gespeicherten Vergleich.', previous: 'Vorherige Nachrichten', next: 'Nächste Nachrichten', range: 'Nachrichten', of: 'von', snapshot: 'Zensus SHA-256', deployment: 'Gespeicherte Transaktion', refreshed: 'Letzte abgeschlossene Erfassung',
    },
  }
  const t = $derived(strings[lang])
  let page = $state(1)
  let windowOnly = $state(false)
  let selected = $derived(report.kind === 'compared' ? report.rows.filter(row => !windowOnly || report.arrival_window.kind !== 'timed' || row.arrival_period === 'after_observation') : [])
  let pageCount = $derived(Math.max(1, Math.ceil(selected.length / 50)))
  let current = $derived(Math.min(pageCount, Math.max(1, page)))
  let rows = $derived(selected.slice((current - 1) * 50, current * 50))
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
      <p class="help">{t.baseline[report.baseline_source]}</p>
      <h4>{t.windowTitle}</h4>
      {#if report.arrival_window.kind === 'timed'}
        <p class="help">{t.windowDetail}</p>
        <p>{t.via[report.arrival_window.via]} · {t.windowRange} · {new Date(report.arrival_window.after * 1000).toISOString()} / {new Date(report.arrival_window.through * 1000).toISOString()}</p>
        <p>{report.arrival_window.counts.matches} {t.matches} · {report.arrival_window.counts.elsewhere} {t.elsewhere} · {report.arrival_window.counts.additional_folders} {t.additional} · {report.arrival_window.counts.uncheckable} {t.unknown}</p>
        <label><input type="checkbox" bind:checked={windowOnly} onchange={() => page = 1} /> {t.windowOnly}</label>
      {:else}<p>{t.windowMissing[report.arrival_window.reason]}</p>{/if}
      <p>{t.refreshed} · {new Date(report.refreshed_at * 1000).toISOString()}</p>
      <p class="help">{t.deployment} · {report.transaction} · {report.target}</p>
      <p class="help">{t.snapshot} · <code>{report.census_sha256}</code></p>
      {#if rows.length === 0}<p>{windowOnly && report.arrival_window.kind === 'timed' ? t.emptyWindow : t.empty}</p>{:else}
        <p aria-live="polite">{t.range} {(current - 1) * 50 + 1}–{Math.min(current * 50, selected.length)} {t.of} {selected.length}</p>
        <div class="proof-pagination"><button disabled={current === 1} onclick={() => page = current - 1}>{t.previous}</button><button disabled={current === pageCount} onclick={() => page = current + 1}>{t.next}</button></div>
        <div class="table-scroll"><table><thead><tr><th>{t.subject}</th><th>{t.observed}</th><th>{t.expected}</th><th>{t.result}</th></tr></thead><tbody>
          {#each rows as row (row.id)}<tr><td>{row.subject}<br /><span class="help">{row.from}</span><br /><span class="help">{t.arrival} · {t.periods[row.arrival_period]}</span></td><td>{row.observed.join(', ')}</td>
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
