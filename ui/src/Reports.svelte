<script lang="ts">
  import { readReport, reviewSummary, exportSummary, type CensusReport, type SummaryPreview } from './api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Local census report', time: 'As of (UTC)', load: 'Generate or refresh local report', local: 'Local detail stays on this device. Nothing is sent to an agent. Export writes a file only after you review and confirm.',
      complete: 'Complete saved acquisition', partial: 'Partial acquisition. Counts describe saved progress only.', messages: 'canonical messages', received: 'received', sent: 'sent', saved: 'saved records', folders: 'Current folders', domains: 'Sender domains', arrivals: 'Gmail arrivals by month', empty: 'None observed',
      senders: 'Senders', unknown: 'Unknown', bulk: 'Bulk', platform: 'Platform', list_with_people: 'List with people', person: 'Person', monthly: 'Volume by month', flags: 'Current flags', seen: 'seen', flagged: 'flagged', answered: 'answered', latest: 'Latest read message date', invalid: 'Invalid or missing dates', future: 'Future dates', wrote: 'User wrote to sender', yes: 'Yes', no: 'No',
      raw: 'Raw From', relay: 'Relay claims (unauthenticated)', lists: 'List markers', oneClick: 'Complete one-click pair', unsubscribe: 'List-Unsubscribe', listIds: 'List-Id', esp: 'ESP observations (unauthenticated)', labels: 'Gmail labels', recipients: 'Addresses written to (as observed)', keywords: 'Repeated Subject tokens (local only, digits masked)', windows: '12 / 13 months; read in 6 months',
      review: 'Review agent summary', privacy: 'Only selected organisational domains appear in clear text. Own addresses, people, folders, labels, List-Ids and keywords stay keyed placeholders. Senders below three messages pool into the long tail. Relay identity and DKIM coverage remain unknown.',
      approve: 'Allow clear domain', preview: 'Preview exact shared text', confirm: 'I reviewed this exact summary and want to export it', download: 'Export reviewed summary', failure: 'Report action failed. The census or selection may have changed. Generate a fresh report and review it again.',
      limits: 'Report semantics and limitations', ownDomains: 'Confirmed own domains',
      unknown_authentication: 'Header observations are unauthenticated. Relay originals are claims. DKIM unsubscribe coverage remains unknown.',
      current_flags_and_dates: 'Monthly volume uses header Date and current Seen flags. The latest read message date is not the time it was read. Invalid and future dates stay outside date windows.',
      gmail_topology: 'Gmail arrivals use INTERNALDATE for received canonical messages. Full label topology and excluded Spam/Trash counts are unavailable.',
      subject_tokens: 'Keywords occur in at least three distinct received messages per sender, with digits masked. Keywords, folders, labels and List-Ids always use keyed placeholders in the summary.',
      classification: 'A discussion list suggests people on a list. A complete one-click pair suggests bulk. ESP or Auto-Submitted suggests platform. Confirmed Sent correspondence without list/bulk markers suggests person. Mixed or insufficient evidence stays unknown. No class grants authentication or permission.',
    },
    de: {
      title: 'Lokaler Erfassungsbericht', time: 'Stand (UTC)', load: 'Lokalen Bericht erstellen oder aktualisieren', local: 'Lokale Details bleiben auf diesem Gerät. Nichts geht an einen Agenten. Erst nach Prüfung und Bestätigung wird eine Datei exportiert.',
      complete: 'Gespeicherte Erfassung vollständig', partial: 'Unvollständige Erfassung. Die Zahlen beschreiben nur den gespeicherten Fortschritt.', messages: 'eindeutige Nachrichten', received: 'empfangen', sent: 'gesendet', saved: 'gespeicherte Datensätze', folders: 'Aktuelle Ordner', domains: 'Absenderdomains', arrivals: 'Gmail-Eingänge pro Monat', empty: 'Keine beobachtet',
      senders: 'Absender', unknown: 'Unbekannt', bulk: 'Massenmail', platform: 'Plattform', list_with_people: 'Diskussionsliste', person: 'Person', monthly: 'Volumen pro Monat', flags: 'Aktuelle Markierungen', seen: 'gelesen', flagged: 'markiert', answered: 'beantwortet', latest: 'Datum der neuesten gelesenen Nachricht', invalid: 'Ungültige oder fehlende Datumswerte', future: 'Zukünftige Datumswerte', wrote: 'Eigene Nachricht an Absender', yes: 'Ja', no: 'Nein',
      raw: 'Ursprüngliches From', relay: 'Relay-Angaben (unbestätigt)', lists: 'Listenmerkmale', oneClick: 'Vollständiges One-Click-Paar', unsubscribe: 'List-Unsubscribe', listIds: 'List-Id', esp: 'ESP-Beobachtungen (unbestätigt)', labels: 'Gmail-Labels', recipients: 'Angeschriebene Adressen (wie beobachtet)', keywords: 'Wiederholte Betreffwörter (nur lokal, Ziffern maskiert)', windows: '12 / 13 Monate; gelesen in 6 Monaten',
      review: 'Agentenzusammenfassung prüfen', privacy: 'Nur ausgewählte Organisationsdomains erscheinen im Klartext. Eigene Adressen, Personen, Ordner, Labels, Listen-IDs und Stichwörter bleiben verschlüsselt abgeleitete Platzhalter. Absender mit weniger als drei Nachrichten werden zusammengefasst. Relay-Identität und DKIM-Abdeckung bleiben unbekannt.',
      approve: 'Domain im Klartext erlauben', preview: 'Exakten Freigabetext anzeigen', confirm: 'Ich habe diese Zusammenfassung geprüft und möchte sie exportieren', download: 'Geprüfte Zusammenfassung exportieren', failure: 'Berichtsaktion fehlgeschlagen. Erfassung oder Auswahl können sich geändert haben. Erstelle einen neuen Bericht und prüfe ihn erneut.',
      limits: 'Bedeutung und Grenzen des Berichts', ownDomains: 'Bestätigte eigene Domains',
      unknown_authentication: 'Kopfzeilen sind unbestätigte Angaben. Relay-Originale sind Behauptungen. Die DKIM-Abdeckung der Abmeldekopfzeilen bleibt unbekannt.',
      current_flags_and_dates: 'Monatsvolumen nutzt das Nachrichtendatum und aktuelle Gelesen-Markierungen. Das Datum der neuesten gelesenen Nachricht ist nicht der Lesezeitpunkt. Ungültige und zukünftige Daten bleiben außerhalb der Zeitfenster.',
      gmail_topology: 'Gmail-Eingänge nutzen INTERNALDATE der eindeutigen empfangenen Nachrichten. Vollständige Label-Struktur und Zahlen ausgeschlossener Spam/Papierkorb-Nachrichten fehlen.',
      subject_tokens: 'Stichwörter kommen in mindestens drei verschiedenen empfangenen Nachrichten pro Absender vor, mit maskierten Ziffern. Stichwörter, Ordner, Labels und Listen-IDs bleiben in der Zusammenfassung verschlüsselt abgeleitete Platzhalter.',
      classification: 'Diskussionslisten deuten auf Personen in Listen. Ein vollständiges One-Click-Paar deutet auf Massenmail. ESP oder Auto-Submitted deutet auf Plattformmail. Bestätigte eigene Korrespondenz ohne Listenmerkmale deutet auf Personen. Gemischte oder unzureichende Belege bleiben unbekannt. Keine Klasse weist Echtheit oder Berechtigungen nach.',
    },
  }
  const t = $derived(strings[lang])
  let asOf = $state(new Date().toISOString().slice(0, 19))
  let report = $state<CensusReport | null>(null)
  let approved = $state<string[]>([])
  let preview = $state<SummaryPreview | null>(null)
  let confirmed = $state(false)
  let busy = $state(false)
  let error = $state('')
  async function act(action: () => Promise<void>) {
    busy = true; error = ''
    try { await action() } catch { error = t.failure; preview = null; confirmed = false } finally { busy = false }
  }
  function resetPreview() { preview = null; confirmed = false }
  async function load() {
    resetPreview(); report = null; approved = []
    await act(async () => {
      report = await readReport(account, Math.floor(Date.parse(`${asOf}Z`) / 1000))
      approved = report.clear_candidates.map(c => c.id)
    })
  }
  function date(value: number | null) { return value === null ? t.unknown : new Date(value * 1000).toISOString() }
  async function download() {
    if (!report || !preview || !confirmed) return
    const currentReport = report, currentPreview = preview
    await act(async () => {
      const result = await exportSummary(account, currentReport, approved, currentPreview.receipt)
      if (result.summary !== currentPreview.summary) throw new Error('Summary changed')
      const url = URL.createObjectURL(new Blob([result.summary], { type: 'text/markdown;charset=utf-8' }))
      const link = document.createElement('a'); link.href = url; link.download = result.filename; link.click()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
      confirmed = false
    })
  }
</script>

<section class="reports" aria-labelledby="report-heading">
  <h3 id="report-heading">{t.title}</h3>
  <p>{t.local}</p>
  <label>{t.time}<input type="datetime-local" step="1" bind:value={asOf} disabled={busy} onchange={() => { report = null; resetPreview() }} /></label>
  <button disabled={busy || !asOf} onclick={load}>{t.load}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if report}
    <p class="scan-state" role="status">{report.complete ? t.complete : t.partial} · {report.state}</p>
    <p>{report.canonical_messages} {t.messages} · {report.received.messages} {t.received} · {report.sent} {t.sent} · {report.saved_records} {t.saved}</p>
    <p>{t.flags}: {report.received.seen}/{report.received.messages} {t.seen} · {report.received.flagged} {t.flagged} · {report.received.answered} {t.answered}</p>
    <p>{t.ownDomains}: {report.own_domains.join(', ') || t.empty}</p>
    <p>{t.lists}: {t.oneClick} {report.senders.reduce((n, s) => n + s.one_click_pair, 0)} · {t.unsubscribe} {report.senders.reduce((n, s) => n + s.list_unsubscribe, 0)} · {t.listIds} {report.senders.reduce((n, s) => n + s.list_id_messages, 0)} / {report.received.messages}</p>
    <p>{t.invalid}: {report.received.invalid_dates} · {t.future}: {report.received.future_dates}</p>
    <details><summary>{t.folders}</summary>{#each report.folders as folder (folder.folder)}<p>{folder.folder} · {folder.saved}/{folder.inventory} · {folder.state}</p>{/each}</details>
    <details><summary>{t.domains}</summary>{#each Object.entries(report.domains) as [domain, counts] (domain)}<p>{domain} · {counts.messages} · {JSON.stringify(counts.monthly)}</p>{/each}</details>
    <details><summary>{t.arrivals}</summary><p>{JSON.stringify(report.gmail_arrivals)}</p><p>{t.invalid}: {report.invalid_arrival_dates} · {t.future}: {report.future_arrival_dates}</p></details>
    <h4>{t.senders}</h4>
    {#each report.senders as sender, i (i)}
      <details class="sender-report"><summary>{sender.sender ?? t.unknown} · {t[sender.class]} · {sender.counts.messages}</summary>
        <p>{sender.domain ?? t.unknown} · {t.wrote}: {sender.wrote_to_sender ? t.yes : t.no}</p>
        <p>{t.flags}: {sender.counts.seen}/{sender.counts.messages} {t.seen} · {sender.counts.flagged} {t.flagged} · {sender.counts.answered} {t.answered}</p>
        <p>{t.latest}: {date(sender.counts.latest_read_message_date)}</p>
        <p>{t.windows}: {sender.counts.last_12_months} / {sender.counts.last_13_months}; {sender.counts.read_last_6_months}</p>
        <p>{t.monthly}: {JSON.stringify(sender.counts.monthly)}</p>
        <p>{t.invalid}: {sender.counts.invalid_dates} · {t.future}: {sender.counts.future_dates}</p>
        <p>{t.raw}: {sender.raw_from.join(', ') || t.empty}</p><p>{t.relay}: {sender.relay_claims.join(', ') || t.empty}</p>
        <p>{t.lists}: {t.oneClick} {sender.one_click_pair} · {t.unsubscribe} {sender.list_unsubscribe} · {t.listIds} {sender.list_id_messages}</p>
        <p>{sender.list_ids.join(', ')}</p><p>{t.esp}: {sender.esp_observations.join(', ') || t.empty}</p>
        <p>{t.folders}: {sender.folders.join(', ')}</p><p>{t.labels}: {sender.gmail_labels.join(', ') || t.empty}</p>
        <p>{t.recipients}: {sender.recipient_addresses.join(', ') || t.empty}</p><p>{t.keywords}: {JSON.stringify(sender.subject_keywords)}</p>
      </details>
    {/each}
    <details><summary>{t.limits}</summary>{#each report.limitations as limitation (limitation)}<p>{t[limitation]}</p>{/each}</details>
    <h4>{t.review}</h4><p>{t.privacy}</p>
    <div class="approved-values">{#each report.clear_candidates as candidate (candidate.id)}<label><input type="checkbox" value={candidate.id} bind:group={approved} disabled={busy} onchange={resetPreview} />{t.approve} {candidate.domain}</label>{/each}</div>
    <button disabled={busy} onclick={() => { const current = report; if (current) void act(async () => { resetPreview(); preview = await reviewSummary(account, current, approved) }) }}>{t.preview}</button>
    {#if preview}
      <pre class="summary-preview">{preview.summary}</pre>
      <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label>
      <button class="primary" disabled={busy || !confirmed} onclick={download}>{t.download}</button>
    {/if}
  {/if}
</section>
