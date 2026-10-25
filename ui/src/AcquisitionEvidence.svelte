<script lang="ts">
  import type { EvidenceView, CountObservation } from './acquisition-evidence'
  let { evidence, lang }: { evidence: EvidenceView; lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Local acquisition evidence', empty: 'No versioned protocol observations saved.', latest: 'Latest attempt', success: 'Last successful selected scan', older: 'Older successful observations. No refreshed observation is available for this view.',
      age: 'Age in seconds at report time', selected: 'Selected folders', excluded: 'Discovered folders outside header acquisition', partial: 'Partial checkpoint from the latest attempt', observed: 'Observed', unknown: 'Unknown', spam: 'Current Spam count', time: 'Observed at', source: 'Count source', delimiter: 'Hierarchy delimiter', flat: 'No hierarchy delimiter', roles: 'Advertised roles', noRoles: 'No special-use role advertised', capabilities: 'Advertised IMAP capabilities', synthetic: 'Synthetic adapter. These are not provider observations.',
      limits: 'The topology contains the complete returned LIST, which may omit hidden server mailboxes. Counts are point-in-time mailbox totals, not 12/13-month coverage. Selected headers remain untrusted claims. Recipient authority, DKIM coverage, direct delivery, and Sieve support remain unknown. No account change is authorized.',
      selectedReason: 'Selected for headers', notSelectable: 'Not selectable. No count or headers requested.', systemRole: 'Excluded system role. No headers requested.', gmailLabel: 'Label excluded because All Mail is selected. No headers requested.', complete: 'Selected reads complete', incomplete: 'Selected reads incomplete',
      not_requested: 'Not requested', not_selectable: 'Not selectable', budget: 'Optional count limit reached', denied: 'Count request denied', unavailable: 'Count unavailable', malformed: 'Malformed count response', missing: 'Count missing from response', ambiguous: 'Multiple or conflicting observations', no_junk_role: 'No Junk role advertised', older_snapshot: 'No current successful observation',
      idle: 'Idle', running: 'Running', cancelled: 'Cancelled or interrupted', failed: 'Failed',
    },
    de: {
      title: 'Lokale Erfassungsbelege', empty: 'Keine versionierten Protokollbeobachtungen gespeichert.', latest: 'Letzter Versuch', success: 'Letzte erfolgreiche Erfassung ausgewählter Ordner', older: 'Ältere erfolgreiche Beobachtungen. Für diese Ansicht fehlt eine erneuerte Beobachtung.',
      age: 'Alter in Sekunden zum Berichtszeitpunkt', selected: 'Ausgewählte Ordner', excluded: 'Entdeckte Ordner außerhalb der Kopfzeilenerfassung', partial: 'Unvollständiger Zwischenstand des letzten Versuchs', observed: 'Beobachtet', unknown: 'Unbekannt', spam: 'Aktuelle Spamanzahl', time: 'Beobachtet am', source: 'Quelle der Anzahl', delimiter: 'Hierarchietrennzeichen', flat: 'Kein Hierarchietrennzeichen', roles: 'Gemeldete Rollen', noRoles: 'Keine besondere Rolle gemeldet', capabilities: 'Gemeldete IMAP-Fähigkeiten', synthetic: 'Synthetischer Adapter. Dies sind keine Anbieterbeobachtungen.',
      limits: 'Die Struktur enthält die vollständige LIST-Antwort. Versteckte Serverordner können fehlen. Zahlen sind momentane Ordnerbestände und keine Abdeckung über 12/13 Monate. Kopfzeilen bleiben unbestätigte Angaben. Empfängerautorität, DKIM-Abdeckung, direkte Zustellung und Sieve-Unterstützung bleiben unbekannt. Keine Kontoänderung ist erlaubt.',
      selectedReason: 'Für Kopfzeilen ausgewählt', notSelectable: 'Nicht auswählbar. Keine Anzahl oder Kopfzeilen angefordert.', systemRole: 'Systemrolle ausgeschlossen. Keine Kopfzeilen angefordert.', gmailLabel: 'Label ausgeschlossen, weil Alle Nachrichten ausgewählt ist. Keine Kopfzeilen angefordert.', complete: 'Ausgewählte Lesezugriffe vollständig', incomplete: 'Ausgewählte Lesezugriffe unvollständig',
      not_requested: 'Nicht angefordert', not_selectable: 'Nicht auswählbar', budget: 'Grenze optionaler Anzahlen erreicht', denied: 'Abfrage der Anzahl verweigert', unavailable: 'Anzahl nicht verfügbar', malformed: 'Fehlerhafte Antwort zur Anzahl', missing: 'Anzahl fehlt in der Antwort', ambiguous: 'Mehrere oder widersprüchliche Beobachtungen', no_junk_role: 'Keine Junk-Rolle gemeldet', older_snapshot: 'Keine aktuelle erfolgreiche Beobachtung',
      idle: 'Bereit', running: 'Läuft', cancelled: 'Abgebrochen oder unterbrochen', failed: 'Fehlgeschlagen',
    },
  }
  const t = $derived(strings[lang])
  function date(value: number | null) { return value === null ? t.unknown : new Date(value * 1000).toISOString() }
  function count(value: CountObservation) { return value.kind === 'observed' ? `${t.observed} ${value.messages} · ${date(value.observed_at)} · ${value.source === 'status_messages' ? 'STATUS MESSAGES' : 'EXAMINE / UID SEARCH'}` : `${t.unknown} · ${t[value.reason]}` }
  const snapshots = $derived([
    ...(evidence.last_successful ? [{ label: t.success, value: evidence.last_successful }] : []),
    ...(evidence.latest?.checkpoint && evidence.latest.state !== 'complete' ? [{ label: t.partial, value: evidence.latest.checkpoint }] : []),
  ])
</script>

<section class="acquisition-evidence" aria-label={t.title}>
  <h4>{t.title}</h4>
  {#if evidence.latest}
    <p>{t.latest} · {evidence.latest.state === 'complete' ? t.complete : t[evidence.latest.state]} · {date(evidence.latest.started_at)} / {date(evidence.latest.finished_at)}</p>
    {#if evidence.latest.session}<p>{t.capabilities} · {date(evidence.latest.session.observed_at)} · {evidence.latest.session.imap_capabilities.join(', ') || t.unknown}</p>{/if}
  {:else}<p>{t.empty}</p>{/if}
  {#if evidence.last_successful && !evidence.current}<p class="help">{t.older}</p>{/if}
  {#if evidence.age_seconds !== null}<p>{t.age} · {evidence.age_seconds}</p>{/if}
  <p class="spam-observation">{t.spam} · {count(evidence.spam)}</p>
  {#each snapshots as snapshot}
    <details>
      <summary>{snapshot.label} · {date(snapshot.value.completed_at ?? snapshot.value.topology_observed_at)}</summary>
      {#if snapshot.value.adapter !== 'mailroom/read-only-imap/v1'}<p>{t.synthetic}</p>{/if}
      <p>{t.capabilities} · {snapshot.value.imap_capabilities.join(', ') || t.unknown}</p>
      {#each ['selected', 'excluded'] as group}
        <h5>{group === 'selected' ? t.selected : t.excluded}</h5>
        {#each snapshot.value.folders.filter(f => (f.selection === 'selected') === (group === 'selected')) as folder (folder.name)}
          <div class="folder-observation">
            <p>{folder.name} · {t.delimiter} {folder.delimiter ?? t.flat}</p>
            <p>{t.roles} · {folder.roles.join(', ') || t.noRoles}</p>
            <p>{folder.selection === 'selected' ? t.selectedReason : folder.selection === 'not_selectable' ? t.notSelectable : folder.selection === 'system_role' ? t.systemRole : t.gmailLabel}</p>
            <p>{count(folder.count)}{folder.selection === 'selected' ? ` · ${folder.selected_complete ? t.complete : t.incomplete}` : ''}</p>
          </div>
        {/each}
      {/each}
    </details>
  {/each}
  <p class="help">{t.limits}</p>
</section>

<style>
  .acquisition-evidence { margin-block: 1rem; overflow-wrap: anywhere; }
  .folder-observation { border-inline-start: 2px solid currentColor; padding-inline-start: .75rem; margin-block: .75rem; }
</style>
