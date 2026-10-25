<script lang="ts">
  import { openWinddown, reviewWinddown, confirmWinddown, previewWinddownExport, exportWinddown, type WinddownView, type WinddownReview, type WinddownExport, type WinddownSettings, type WinddownChange, type Disclosure } from './winddown-api'
  let { account, lang, onBulkHandoff }: { account: string; lang: 'en' | 'de'; onBulkHandoff: (senders: string[], asOf: number) => Promise<void> } = $props()
  const strings = {
    en: {
      title: 'Wind-down report', intro: 'Review local evidence before considering account retirement. This report changes no provider settings or mail.', time: 'As of, UTC', load: 'Open or refresh wind-down report',
      winding: 'Designate this account as winding down', comparisons: 'Explicit newer owned comparison addresses', comparisonHelp: 'Select only reviewed newer addresses on another owned account. Header observations and service associations do not confirm ownership. No available confirmed address means no comparison evidence.',
      selectedSources: 'Selected comparison sources', removeSource: 'Remove comparison source', reviewSettings: 'Review designation and comparison sources', people: 'People you have written to', service: 'Transactional services', bulk: 'Bulk for unsubscribe review', unresolved: 'Unresolved identities or classes',
      waiting: 'Waiting', moved: 'Moved directly', dismissed: 'Dismissed by owner', quiet: 'Quiet with historical coverage', known: 'Historical row retained', noRows: 'No rows in this group.', waitingDetail: 'Direct migration, an owner dismissal or sufficient quiet history has not been established.', movedDetail: 'A distinct message has trusted direct-delivery evidence to a selected newer owned address.', dismissedDetail: 'An exact owner decision excludes this sender from waiting. Coverage and identity uncertainties still apply.', quietDetail: 'No message falls within the rolling 13-month window, with sufficient source history and usable dates.',
      invalid: 'Invalid or missing dates', future: 'Future dates', count: 'Count / last 13 months', dates: 'First / last message date', observations: 'Exact address observations and source fields', migration: 'Direct migration evidence', reasons: 'Unresolved reasons', settings: 'Reviewed settings link or observed domain', associations: 'Private service associations',
      dismiss: 'Review dismissal', undismiss: 'Review undismissal', exact: 'Exact local decision review', before: 'Current counts', after: 'Resulting counts', confirm: 'I approve exactly this local decision and its displayed consequences', save: 'Save confirmed local decision',
      local: 'Preview private local file', summary: 'Preview agent summary', exportConfirm: 'I approve exactly these output bytes and this disclosure', download: 'Download confirmed output', privacy: 'Local files contain private observations. Agent summaries use aggregates and placeholders, with small senders grouped. Addresses, service associations, settings links and notes are withheld. Each export needs separate review.',
      evidence: 'Source versions and coverage', spam: 'Spam count', unknown: 'Unknown', blockers: 'Readiness blockers', ready: 'Evidence supports an owner retirement review. Actual retirement remains your action.', blocked: 'Evidence is insufficient or senders remain waiting. No retirement recommendation.',
      handoff: 'Prepare bulk handoff', handoffText: 'Bulk handoff prepared for the existing unsubscribe review below. Every account, receiving address and list remains a separate approval target. No unsubscribe is executed.',
      error: 'The action failed or its evidence changed. Reopen the report and review again. Keep existing encrypted files. If the original key is unavailable, later encrypted recovery support is required.', terminal: 'Local terminal rendering', change: 'Exact requested change',
    },
    de: {
      title: 'Bericht zur Kontostilllegung', intro: 'Prüfe lokale Belege vor einer möglichen Kontostilllegung. Dieser Bericht ändert weder Anbietereinstellungen noch Nachrichten.', time: 'Stand, UTC', load: 'Stilllegungsbericht öffnen oder aktualisieren',
      winding: 'Dieses Konto zur Stilllegung vormerken', comparisons: 'Explizite neuere eigene Vergleichsadressen', comparisonHelp: 'Wähle nur geprüfte neuere Adressen eines anderen eigenen Kontos. Kopfzeilen und Dienstzuordnungen bestätigen keine Zugehörigkeit. Ohne bestätigte Adresse fehlen Vergleichsbelege.',
      selectedSources: 'Ausgewählte Vergleichsquellen', removeSource: 'Vergleichsquelle entfernen', reviewSettings: 'Vormerkung und Vergleichsquellen prüfen', people: 'Personen mit eigener Korrespondenz', service: 'Transaktionsdienste', bulk: 'Massenmail zur Abmeldeprüfung', unresolved: 'Ungeklärte Identitäten oder Klassen',
      waiting: 'Wartend', moved: 'Direkt umgezogen', dismissed: 'Vom Inhaber ausgeschlossen', quiet: 'Ruhig mit historischer Abdeckung', known: 'Historischer Eintrag bleibt erhalten', noRows: 'Keine Einträge in dieser Gruppe.', waitingDetail: 'Direkter Umzug, Inhaberausschluss oder ausreichende Ruhehistorie sind nicht belegt.', movedDetail: 'Eine getrennte Nachricht belegt eine vertrauenswürdige direkte Zustellung an eine ausgewählte neuere eigene Adresse.', dismissedDetail: 'Eine exakte Inhaberentscheidung nimmt diesen Absender aus der Warteliste. Ungewissheiten zu Abdeckung und Identität bleiben bestehen.', quietDetail: 'Keine Nachricht liegt im gleitenden 13-Monats-Zeitraum, bei ausreichender Quellenhistorie und nutzbaren Daten.',
      invalid: 'Ungültige oder fehlende Daten', future: 'Zukünftige Daten', count: 'Anzahl / letzte 13 Monate', dates: 'Erstes / letztes Nachrichtendatum', observations: 'Exakte Adressbeobachtungen und Quellfelder', migration: 'Direkte Umzugsbelege', reasons: 'Ungeklärte Gründe', settings: 'Geprüfter Einstellungslink oder beobachtete Domain', associations: 'Private Dienstzuordnungen',
      dismiss: 'Ausschluss prüfen', undismiss: 'Aufhebung prüfen', exact: 'Exakte lokale Entscheidung prüfen', before: 'Bisherige Zahlen', after: 'Resultierende Zahlen', confirm: 'Ich genehmige genau diese lokale Entscheidung und ihre angezeigten Folgen', save: 'Bestätigte lokale Entscheidung speichern',
      local: 'Private lokale Datei vorschauen', summary: 'Agentenzusammenfassung vorschauen', exportConfirm: 'Ich genehmige genau diese Ausgabebytes und diese Freigabe', download: 'Bestätigte Ausgabe herunterladen', privacy: 'Lokale Dateien enthalten private Beobachtungen. Agentenzusammenfassungen nutzen Summen und Platzhalter, kleine Absender werden zusammengefasst. Adressen, Dienstzuordnungen, Einstellungslinks und Notizen bleiben verborgen. Jede Ausgabe benötigt eine eigene Prüfung.',
      evidence: 'Quellversionen und Abdeckung', spam: 'Spamanzahl', unknown: 'Unbekannt', blockers: 'Hindernisse für die Bereitschaft', ready: 'Die Belege erlauben eine Stilllegungsprüfung durch den Inhaber. Die tatsächliche Stilllegung bleibt deine Aktion.', blocked: 'Die Belege reichen nicht aus oder Absender warten noch. Keine Stilllegungsempfehlung.',
      handoff: 'Massenmail-Übergabe vorbereiten', handoffText: 'Übergabe für die vorhandene Abmeldeprüfung unten vorbereitet. Jedes Konto, jede Empfangsadresse und jede Liste bleibt ein separates Genehmigungsziel. Keine Abmeldung wird ausgeführt.',
      error: 'Die Aktion ist fehlgeschlagen oder die Belege haben sich geändert. Öffne den Bericht und prüfe erneut. Bewahre vorhandene verschlüsselte Dateien auf. Ohne ursprünglichen Schlüssel ist die spätere verschlüsselte Wiederherstellung nötig.', terminal: 'Lokale Terminaldarstellung', change: 'Exakt angeforderte Änderung',
    },
  }
  const t = $derived(strings[lang])
  let asOf = $state(new Date().toISOString().slice(0, 19))
  let view = $state<WinddownView | null>(null)
  let settings = $state<WinddownSettings>({ winding_down: false, comparisons: {} })
  let decision = $state<WinddownReview | null>(null)
  let output = $state<WinddownExport | null>(null)
  let confirmed = $state(false)
  let exportConfirmed = $state(false)
  let busy = $state(false)
  let error = $state('')
  let handoff = $state(false)
  const groups = ['people', 'service', 'bulk', 'unresolved'] satisfies WinddownView['report']['rows'][number]['group'][]
  function reset() { decision = null; output = null; confirmed = false; exportConfirmed = false }
  async function act(action: () => Promise<void>) {
    busy = true; error = ''
    try { await action() } catch { error = t.error; reset() } finally { busy = false }
  }
  async function reload() {
    reset(); handoff = false
    view = await openWinddown(account, Math.floor(Date.parse(`${asOf}Z`) / 1000))
    settings = structuredClone($state.snapshot(view.report.settings))
  }
  function selectComparison(id: string, address: string, checked: boolean) {
    reset()
    const chosen = new Set(settings.comparisons[id] ?? [])
    if (checked) chosen.add(address); else chosen.delete(address)
    if (chosen.size) settings.comparisons[id] = [...chosen]; else delete settings.comparisons[id]
  }
  async function review(change: WinddownChange) {
    reset()
    if (view) decision = await reviewWinddown(account, view.report, change)
  }
  async function preview(disclosure: Disclosure) {
    reset()
    if (view) output = await previewWinddownExport(account, view.report, disclosure)
  }
  async function download() {
    if (!view || !output || !exportConfirmed) return
    const expected = output
    const result = await exportWinddown(account, view.report, expected)
    if (result.text !== expected.text || result.disclosure !== expected.disclosure) throw new Error('Output changed')
    const url = URL.createObjectURL(new Blob([result.text], { type: 'text/plain;charset=utf-8' }))
    const link = document.createElement('a'); link.href = url; link.download = result.filename; link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    reset()
  }
  const explanations: Record<string, { en: string; de: string }> = {
    account_not_designated: { en: 'This account is not designated as winding down.', de: 'Dieses Konto ist nicht zur Stilllegung vorgemerkt.' },
    comparison_accounts_absent: { en: 'No newer owned comparison account is selected.', de: 'Kein neueres eigenes Vergleichskonto ist ausgewählt.' },
    selected_comparison_unavailable: { en: 'A previously selected comparison account is unavailable. Review the source selection.', de: 'Ein zuvor ausgewähltes Vergleichskonto fehlt. Prüfe die Quellenauswahl.' },
    comparison_ownership_changed: { en: 'Selected newer addresses are no longer confirmed as distinct owned addresses. Review the selection.', de: 'Ausgewählte neuere Adressen sind nicht mehr als getrennte eigene Adressen bestätigt. Prüfe die Auswahl.' },
    incomplete_acquisition: { en: 'At least one selected census is incomplete.', de: 'Mindestens eine ausgewählte Erfassung ist unvollständig.' },
    spam_count_unknown: { en: 'Spam coverage and its count are unknown, not zero.', de: 'Spamabdeckung und Spamanzahl sind unbekannt, nicht null.' },
    historical_coverage_unknown: { en: 'Coverage does not establish the full 13-month history.', de: 'Die Abdeckung belegt nicht die gesamten letzten 13 Monate.' },
    source_stale_or_coverage_unknown: { en: 'A source is stale or has no coverage through the report time.', de: 'Eine Quelle ist veraltet oder deckt den Berichtszeitpunkt nicht ab.' },
    ownership_unconfirmed: { en: 'An account has no confirmed owned addresses.', de: 'Ein Konto hat keine bestätigten eigenen Adressen.' },
    invalid_future_or_missing_message_dates: { en: 'Invalid, future or missing message dates remain unknown.', de: 'Ungültige, zukünftige oder fehlende Nachrichtendaten bleiben ungeklärt.' },
    sender_identity_or_class_unresolved: { en: 'Sender identity or class is unresolved. Platform mail alone does not prove a transactional service.', de: 'Absenderidentität oder Klasse ist ungeklärt. Plattformmail allein belegt keinen Transaktionsdienst.' },
    comparison_forwarded_ambiguous_duplicate_or_untrusted: { en: 'Some comparison messages are forwarded, ambiguous, duplicated or lack trusted direct-delivery evidence.', de: 'Einige Vergleichsnachrichten sind weitergeleitet, mehrdeutig, dupliziert oder ohne vertrauenswürdigen direkten Zustellbeleg.' },
    quietness_not_proven: { en: 'No recent mail is observed, but sufficient quiet-history evidence is missing.', de: 'Keine aktuelle Nachricht beobachtet, aber ausreichende historische Ruhebelege fehlen.' },
    known_row_absent_from_current_census: { en: 'This known sender is absent from the current census. Its retained history does not prove quietness.', de: 'Dieser bekannte Absender fehlt in der aktuellen Erfassung. Sein aufbewahrter Verlauf belegt keine Ruhe.' },
    unresolved_rows_or_dates: { en: 'Unresolved rows or dates prevent a readiness claim.', de: 'Ungeklärte Einträge oder Daten verhindern eine Bereitschaftsempfehlung.' },
    senders_waiting: { en: 'People or services are still waiting for direct migration or an owner decision.', de: 'Personen oder Dienste warten auf einen direkten Umzug oder eine Inhaberentscheidung.' },
    bulk_requires_separate_unsubscribe_review: { en: 'Bulk mail still needs its separate unsubscribe review.', de: 'Massenmail benötigt weiterhin eine eigene Abmeldeprüfung.' },
  }
  function explain(value: string) { return explanations[value]?.[lang] ?? value }
  function date(value: number | null) { return value === null ? t.unknown : new Date(value * 1000).toISOString() }
</script>

<details class="winddown">
  <summary>{t.title}</summary>
  <p>{t.intro}</p>
  <label>{t.time}<input type="datetime-local" step="1" bind:value={asOf} disabled={busy} onchange={() => { view = null; reset() }} /></label>
  <button disabled={busy || !asOf} onclick={() => act(reload)}>{t.load}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if view}
    <p role="status">{view.report.verdict === 'evidence_supports_owner_retirement_review' ? t.ready : t.blocked}</p>
    <p>{Object.entries(view.report.counts).map(([key, count]) => `${key} ${count}`).join(' · ')}</p>
    <p>{t.blockers}: {view.report.blockers.map(explain).join(' ') || '0'}</p>
    <fieldset disabled={busy}>
      <legend>{t.comparisons}</legend><p>{t.comparisonHelp}</p>
      <label><input type="checkbox" bind:checked={settings.winding_down} onchange={reset} />{t.winding}</label>
      {#each view.available as source (source.account)}
        <p>{source.login} · {source.account}</p>
        {#each source.owned_addresses as address (address)}<label><input type="checkbox" checked={settings.comparisons[source.account]?.includes(address) ?? false} onchange={event => selectComparison(source.account, address, event.currentTarget.checked)} />{address}</label>{/each}
        {#if !source.owned_addresses.length}<p>{t.unknown}</p>{/if}
      {/each}
      <p>{t.selectedSources}</p>
      {#each Object.entries(settings.comparisons) as [id, addresses] (id)}<p>{id} · {addresses.join(', ')}</p><button onclick={() => { delete settings.comparisons[id]; reset() }}>{t.removeSource} {id}</button>{/each}
      <button onclick={() => act(() => review({ kind: 'settings', settings: $state.snapshot(settings) }))}>{t.reviewSettings}</button>
    </fieldset>
    <details><summary>{t.evidence}</summary>{#each view.report.sources as source (source.account)}<p>{source.account} · {source.census_version} · {source.complete} · {t.spam}: {source.spam_count ?? t.unknown} · {source.spam_observation?.kind === 'observed' ? `${date(source.spam_observation.observed_at)} · STATUS MESSAGES` : ''} · {date(source.coverage_since)} / {date(source.coverage_through)}</p>{/each}<p>{view.report.census_analysis} · {view.report.sender_data_version} · {view.report.ledger_version}</p></details>
    {#each groups as group (group)}
      <h4>{t[group]}</h4>
      {#if !view.report.rows.some(row => row.group === group)}<p>{t.noRows}</p>{/if}
      {#each view.report.rows.filter(row => row.group === group) as row (row.id)}
        <details class="winddown-row"><summary>{row.sender ?? t.unknown} · {t[row.status]} · {row.messages}</summary>
          {#if row.status === 'waiting'}<p>{t.waitingDetail}</p>{:else if row.status === 'moved'}<p>{t.movedDetail}</p>{:else if row.status === 'dismissed'}<p>{t.dismissedDetail}</p>{:else if row.status === 'quiet'}<p>{t.quietDetail}</p>{/if}
          <p>{t.count}: {row.messages} / {row.recent}</p><p>{t.invalid}: {row.invalid_dates} · {t.future}: {row.future_dates}</p><p>{t.dates}: {date(row.first_date)} / {date(row.last_date)}</p>
          {#if row.historical}<p>{t.known}</p>{/if}
          <p>{t.reasons}: {row.reasons.map(explain).join(' ') || '0'}</p>
          <p>{t.settings}: {row.settings_link ?? row.domain ?? t.unknown}</p>
          <p>{t.associations}: {row.service_associations.join(', ') || t.unknown}</p>
          <details><summary>{t.observations}</summary>{#each row.observations as observation}<p>{observation.address} · {observation.field} · {date(observation.message_date)} · {observation.delivery_authority}</p><p>{observation.raw_values.join(' | ')}</p>{/each}</details>
          <details><summary>{t.migration}</summary>{#each row.migration as migration}<p>{migration.account} · {migration.address} · {date(migration.message_date)} · {migration.evidence} · {migration.message_identity}</p>{/each}</details>
          {#if group === 'people' || group === 'service' || row.status === 'dismissed'}<button disabled={busy} onclick={() => act(() => review({ kind: 'dismiss', rows: [row.id], dismissed: row.status !== 'dismissed' }))}>{row.status === 'dismissed' ? t.undismiss : t.dismiss}</button>{/if}
        </details>
      {/each}
      {#if group === 'bulk'}<button disabled={busy} onclick={() => act(async () => { if (view) { await onBulkHandoff(view.report.rows.filter(row => row.group === 'bulk').flatMap(row => row.sender ? [row.sender] : []), view.report.as_of); handoff = true } })}>{t.handoff}</button>{#if handoff}<p role="status">{t.handoffText}</p>{/if}{/if}
    {/each}
    {#if decision}
      <section class="winddown-review"><h4>{t.exact}</h4><p>{decision.consequences}</p>
        <p>{t.before}: {JSON.stringify(decision.report.counts)}</p><p>{t.after}: {JSON.stringify(decision.resulting_report.counts)}</p>
        <p>{t.change}</p><pre>{JSON.stringify(decision.change, null, 2)}</pre>
        {#each decision.report.rows.filter(row => decision?.change.kind === 'dismiss' && decision.change.rows.includes(row.id)) as row (row.id)}<p>{row.sender} · {t[row.status]} · {row.messages}</p>{/each}
        <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label>
        <button disabled={busy || !confirmed} onclick={() => act(async () => { if (decision) { await confirmWinddown(account, decision); await reload() } })}>{t.save}</button>
      </section>
    {/if}
    <details><summary>{t.terminal}</summary><pre>{view.terminal}</pre></details>
    <p>{t.privacy}</p>
    <button disabled={busy} onclick={() => act(() => preview('local_file'))}>{t.local}</button>
    <button disabled={busy} onclick={() => act(() => preview('agent_summary'))}>{t.summary}</button>
    {#if output}
      <section class="winddown-output"><p>{output.disclosure} · {output.filename}</p><pre>{output.text}</pre>
        <label><input type="checkbox" bind:checked={exportConfirmed} disabled={busy} />{t.exportConfirm}</label>
        <button disabled={busy || !exportConfirmed} onclick={() => act(download)}>{t.download}</button>
      </section>
    {/if}
  {/if}
</details>
<style>
  .winddown { margin-top: 30px; border-top: 1px solid #ccd6da; padding-top: 20px; overflow-wrap: anywhere; }
  .winddown-row { padding: 10px 0; border-bottom: 1px solid #ccd6da; }
  fieldset { margin: 20px 0; }
  label { display: block; margin: 10px 0; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 32rem; overflow: auto; }
  h4 { margin-top: 30px; margin-bottom: 10px; }
  .winddown-review, .winddown-output { border-top: 1px solid #ccd6da; margin-top: 20px; padding-top: 20px; }
</style>
