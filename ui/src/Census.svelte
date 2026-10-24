<script lang="ts">
  import { onMount } from 'svelte'
  import { censusAction, readCensus, type CensusStatus } from './api'

  let { lang }: { lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Header census', detail: 'Explore eight invented headers. This demo parses mail, checkpoints an encrypted SQLite snapshot, and resumes after cancellation or a simulated failure. It never connects to an account.',
      unavailable: 'Connect a real account', unavailableDetail: 'Unavailable until the OS credential adapter and read-only IMAP acquisition are implemented.',
      start: 'Start synthetic census', resume: 'Resume synthetic census', cancel: 'Cancel census', failure: 'Simulate scan error',
      progress: 'Scan progress', messages: 'headers saved', idle: 'Ready for synthetic data', running: 'Scanning synthetic headers', cancelled: 'Cancelled. Saved headers retained.', complete: 'Synthetic census complete', failed: 'Synthetic scan stopped',
      addresses: 'Review discovered addresses', addressDetail: 'Evidence suggests these may be own addresses. To and Cc are unverified candidates. Confirmation does not assign a personal or business role.',
      confirm: 'Confirm own address', local: 'Local header details', localDetail: 'Full Subjects and sender addresses stay in this local view. The report below omits them.',
      received: 'Received', sent: 'Sent', unknown: 'DKIM coverage unknown', summary: 'Summary preview', summaryDetail: 'All values are keyed placeholders. No clear values are approved. Review a report before sharing it. Full monthly and sender statistics are pending.',
      limitations: 'No live acquisition, Takeout import, pinned PSL, trusted DKIM correlation, sender classification or complete summary yet. The temporary demo key disappears when the server stops.',
      retry: 'Reload census status', loadError: 'Could not load the local census.',
    },
    de: {
      title: 'Kopfzeilen erfassen', detail: 'Acht erfundene Kopfzeilen erkunden. Das Beispiel verarbeitet Nachrichten, speichert verschlüsselte SQLite-Prüfpunkte und setzt nach Abbruch oder simuliertem Fehler fort. Es verbindet kein Konto.',
      unavailable: 'Echtes Konto verbinden', unavailableDetail: 'Nicht verfügbar, bis Betriebssystem-Zugangsdaten und schreibgeschützte IMAP-Erfassung implementiert sind.',
      start: 'Synthetische Erfassung starten', resume: 'Synthetische Erfassung fortsetzen', cancel: 'Erfassung abbrechen', failure: 'Erfassungsfehler simulieren',
      progress: 'Fortschritt', messages: 'Kopfzeilen gespeichert', idle: 'Bereit für synthetische Daten', running: 'Synthetische Kopfzeilen werden erfasst', cancelled: 'Abgebrochen. Gespeicherte Kopfzeilen bleiben erhalten.', complete: 'Synthetische Erfassung abgeschlossen', failed: 'Synthetische Erfassung gestoppt',
      addresses: 'Entdeckte Adressen prüfen', addressDetail: 'Die Belege deuten auf eigene Adressen hin. An und Cc sind unbestätigte Kandidaten. Eine Bestätigung weist keine private oder geschäftliche Rolle zu.',
      confirm: 'Eigene Adresse bestätigen', local: 'Lokale Kopfzeilendetails', localDetail: 'Vollständige Betreffzeilen und Absenderadressen bleiben in dieser lokalen Ansicht. Der Bericht darunter lässt sie aus.',
      received: 'Empfangen', sent: 'Gesendet', unknown: 'DKIM-Abdeckung unbekannt', summary: 'Berichtsvorschau', summaryDetail: 'Alle Werte sind schlüsselgebundene Platzhalter. Keine Klartextwerte sind freigegeben. Prüfe den Bericht vor dem Teilen. Vollständige Monats- und Absenderstatistiken fehlen noch.',
      limitations: 'Noch keine echte Erfassung, Takeout-Import, fixierte PSL, vertrauenswürdige DKIM-Zuordnung, Absenderklassifizierung oder vollständiger Bericht. Der temporäre Beispielschlüssel verschwindet beim Serverstopp.',
      retry: 'Erfassungsstatus neu laden', loadError: 'Die lokale Erfassung konnte nicht geladen werden.',
    },
  }
  let t = $derived(strings[lang])
  let census = $state<CensusStatus | null>(null)
  let error = $state('')
  let busy = $state(false)
  let polling = false
  let active = true
  let revision = 0
  async function reload() {
    if (polling || busy) return
    const requestedRevision = revision
    polling = true
    try { const next = await readCensus(); if (active && revision === requestedRevision) { census = next; error = '' } }
    catch { if (active) error = t.loadError }
    finally { polling = false }
  }
  async function act(action: 'start' | 'cancel' | 'confirm', input: unknown) {
    revision += 1
    busy = true
    error = ''
    try { census = await censusAction(action, input) }
    catch (e) { error = e instanceof Error ? e.message : t.loadError }
    finally { busy = false }
  }
  onMount(() => {
    active = true
    void reload()
    const timer = setInterval(() => { if (census?.state === 'running') void reload() }, 250)
    return () => { active = false; clearInterval(timer) }
  })
</script>

<section class="census" aria-labelledby="census-heading">
  <h2 id="census-heading">{t.title}</h2>
  <p>{t.detail}</p>
  <p class="source">{lang === 'en' ? 'Synthetic data only' : 'Nur synthetische Daten'}</p>
  <div class="census-actions">
    <button disabled>{t.unavailable}</button>
    <p class="help">{t.unavailableDetail}</p>
  </div>
  {#if error}<div class="error" role="alert"><p>{error}</p><button onclick={reload}>{t.retry}</button></div>{/if}
  {#if census}
    <div class="census-actions">
      <button class="primary" disabled={busy || census.state === 'running'} onclick={() => act('start', { mode: 'normal' })}>{census.state === 'cancelled' || census.state === 'failed' ? t.resume : t.start}</button>
      <button disabled={busy || census.state !== 'running'} onclick={() => act('cancel', {})}>{t.cancel}</button>
      <button disabled={busy || census.state === 'running'} onclick={() => act('start', { mode: 'error' })}>{t.failure}</button>
    </div>
    <div class="census-progress" aria-live="polite">
      <p class="scan-state">{t[census.state]}</p>
      <progress aria-label={t.progress} value={census.completed} max={census.total}></progress>
      <p>{census.completed} / {census.total} {t.messages}</p>
      {#if census.error}<p role="alert">{census.error}</p>{/if}
    </div>
    {#if census.addresses.length}
      <h3>{t.addresses}</h3><p class="help">{t.addressDetail}</p>
      <ul class="address-candidates">
        {#each census.addresses as address (address.address)}
          <li><label><input type="checkbox" checked={address.confirmed} disabled={busy || census.state === 'running'} onchange={(event) => act('confirm', { address: address.address, confirmed: event.currentTarget.checked })} /> <span>{t.confirm} <strong>{address.address}</strong><small>{address.evidence.join(', ')}</small></span></label></li>
        {/each}
      </ul>
      <details><summary>{t.local}</summary><p class="help">{t.localDetail}</p>
        {#each census.messages as message (message.uid)}
          <article class="census-message"><h3>{message.subject}</h3><p>{message.sender} · {t[message.direction]} · UID {message.uid}</p><p class="help">{t.unknown}</p></article>
        {/each}
      </details>
      <details><summary>{t.summary}</summary><p class="help">{t.summaryDetail}</p><pre class="census-summary">{census.summary}</pre></details>
    {/if}
    <p class="help">{t.limitations}</p>
  {/if}
</section>
