<script lang="ts">
  import { onMount } from 'svelte'
  import { readAccountStatus, listAccounts, connectAccount, syntheticAccount, scanAccount, cancelAccount, openAccount, confirmAccount, type Account, type AccountStatus, type AccountView } from './api'
  let { lang }: { lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Connect an account', detail: 'Owner-run, read-only access to mailbox.org or Gmail. Use an existing password or app password. Connection checks TLS, login and folder discovery. No account settings change.',
      provider: 'Provider', login: 'Login address', password: 'Existing password or app password', connect: 'Connect account privately', secret: 'The password goes to the OS credential store. It is never saved in configuration or browser storage.',
      synthetic: 'Synthetic account verification. Temporary files and fake credentials only. No provider or OS credential store is contacted.', add: 'Add synthetic account',
      load: 'Load saved accounts', choose: 'Choose an account', scan: 'Run or resume account census', cancel: 'Cancel account census', open: 'Open saved census',
      idle: 'Ready', running: 'Account action running', cancelled: 'Account census cancelled', complete: 'Account action complete', failed: 'Account action failed', saved: 'headers saved', folders: 'folders',
      error: 'The account action failed. Check the existing credential privately, or restore the original census key. Saved files were retained. Retry later for provider or network failures.',
      addresses: 'Confirm own addresses', ownership: 'To and Cc are unverified evidence. Confirmation assigns ownership only. It does not assign a business role or forwarding permission.', confirm: 'Confirm own address', details: 'Saved local headers',
      limits: 'This is acquisition only. Reports, classification, Takeout and trusted DKIM correlation remain incomplete. No result proves rule coexistence, delivery-recipient behavior or accounting acceptance. All account writes stay locked.',
      loadError: 'Could not read the local response. Reload or retry the action.',
    },
    de: {
      title: 'Konto verbinden', detail: 'Nur vom Kontoinhaber ausgeführter Lesezugriff auf mailbox.org oder Gmail. Nutze ein vorhandenes Passwort oder App-Passwort. Die Verbindung prüft TLS, Anmeldung und Ordner. Kontoeinstellungen bleiben unverändert.',
      provider: 'Anbieter', login: 'Anmeldeadresse', password: 'Vorhandenes Passwort oder App-Passwort', connect: 'Konto vertraulich verbinden', secret: 'Das Passwort geht in den Zugangsdaten-Speicher des Betriebssystems. Es wird weder in der Konfiguration noch im Browserspeicher abgelegt.',
      synthetic: 'Synthetische Kontoprüfung. Nur temporäre Dateien und erfundene Zugangsdaten. Kein Anbieter oder Zugangsdaten-Speicher des Betriebssystems wird kontaktiert.', add: 'Synthetisches Konto hinzufügen',
      load: 'Gespeicherte Konten laden', choose: 'Konto wählen', scan: 'Kontoerfassung starten oder fortsetzen', cancel: 'Kontoerfassung abbrechen', open: 'Gespeicherte Erfassung öffnen',
      idle: 'Bereit', running: 'Kontoaktion läuft', cancelled: 'Kontoerfassung abgebrochen', complete: 'Kontoaktion abgeschlossen', failed: 'Kontoaktion fehlgeschlagen', saved: 'Kopfzeilen gespeichert', folders: 'Ordner',
      error: 'Die Kontoaktion ist fehlgeschlagen. Prüfe die vorhandenen Zugangsdaten vertraulich oder stelle den ursprünglichen Erfassungsschlüssel wieder her. Gespeicherte Dateien bleiben erhalten. Bei Anbieter- oder Netzwerkfehlern später erneut versuchen.',
      addresses: 'Eigene Adressen bestätigen', ownership: 'An und Cc sind unbestätigte Belege. Die Bestätigung weist nur die Zugehörigkeit zu. Sie weist keine geschäftliche Rolle oder Weiterleitungsberechtigung zu.', confirm: 'Eigene Adresse bestätigen', details: 'Lokal gespeicherte Kopfzeilen',
      limits: 'Dies ist nur die Erfassung. Berichte, Klassifizierung, Takeout und vertrauenswürdige DKIM-Zuordnung fehlen noch. Kein Ergebnis belegt Regel-Koexistenz, Zustellempfänger oder Buchhaltungsannahme. Alle Kontoänderungen bleiben gesperrt.',
      loadError: 'Die lokale Antwort konnte nicht gelesen werden. Lade neu oder wiederhole die Aktion.',
    },
  }
  let t = $derived(strings[lang])
  let status = $state<AccountStatus | null>(null)
  let accounts = $state<Account[]>([])
  let selected = $state('')
  let provider = $state<Account['provider']>('mailbox_org')
  let login = $state('')
  let password = $state('')
  let view = $state<AccountView | null>(null)
  let busy = $state(false)
  let error = $state('')
  let running = $derived(status?.job?.progress.state === 'running')
  let active = true
  let polling = false
  let revision = 0
  function accept(next: AccountStatus) {
    status = next
    if (next.job?.account) {
      const account = next.job.account
      if (!accounts.some(a => a.id === account.id)) accounts = [...accounts, account]
      if (!selected) selected = account.id
    }
  }
  async function act(action: () => Promise<void>) {
    revision += 1; busy = true; error = ''
    try { await action() } catch { error = t.loadError } finally { busy = false }
  }
  async function refresh() {
    if (polling || busy) return
    polling = true
    const requestedRevision = revision
    try {
      const next = await readAccountStatus()
      if (active && requestedRevision === revision) accept(next)
    } catch { if (active) error = t.loadError } finally { polling = false }
  }
  async function connect() {
    const input = { provider, login, password }; password = ''; view = null; selected = ''
    await act(async () => { accept(await connectAccount(input)) })
    input.password = ''
  }
  onMount(() => {
    active = true; void refresh()
    const timer = setInterval(() => { if (status?.job?.progress.state === 'running') void refresh() }, 250)
    return () => { active = false; password = ''; clearInterval(timer) }
  })
</script>

<section class="accounts" aria-labelledby="accounts-heading">
  <h2 id="accounts-heading">{t.title}</h2>
  <p>{t.detail}</p>
  {#if status?.synthetic}
    <p class="source">{t.synthetic}</p>
    <button disabled={busy || running} onclick={() => act(async () => { selected = ''; view = null; accept(await syntheticAccount()) })}>{t.add}</button>
  {:else if status}
    <form class="account-form" onsubmit={(event) => { event.preventDefault(); void connect() }}>
      <label>{t.provider}<select bind:value={provider} disabled={busy || running}><option value="mailbox_org">mailbox.org</option><option value="gmail">Gmail</option></select></label>
      <label>{t.login}<input type="email" bind:value={login} autocomplete="off" spellcheck={false} required disabled={busy || running} /></label>
      <label>{t.password}<input type="password" bind:value={password} autocomplete="off" required maxlength="1024" disabled={busy || running} /></label>
      <p class="help">{t.secret}</p>
      <button class="primary" type="submit" disabled={busy || running || !password || !login}>{t.connect}</button>
    </form>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <div class="census-actions">
    <button disabled={busy || running} onclick={() => act(async () => { accounts = await listAccounts(); selected ||= accounts[0]?.id ?? '' })}>{t.load}</button>
    {#if accounts.length}
      <label>{t.choose}<select bind:value={selected} disabled={busy || running} onchange={() => view = null}><option value="">{t.choose}</option>{#each accounts as account (account.id)}<option value={account.id}>{account.login} · {account.provider === 'gmail' ? 'Gmail' : 'mailbox.org'}</option>{/each}</select></label>
      <button disabled={busy || running || !selected} onclick={() => act(async () => { view = null; accept(await scanAccount(selected)) })}>{t.scan}</button>
      <button disabled={busy || !running} onclick={() => act(async () => { accept(await cancelAccount()) })}>{t.cancel}</button>
      <button disabled={busy || running || !selected} onclick={() => act(async () => { view = await openAccount(selected) })}>{t.open}</button>
    {/if}
  </div>
  {#if status?.job}
    <div aria-live="polite"><p class="scan-state">{t[status.job.progress.state]}</p><p>{status.job.progress.saved} {t.saved} · {status.job.progress.folders} {t.folders}</p></div>
    {#if status.job.error}<p role="alert">{t.error}</p>{/if}
  {/if}
  {#if view}
    <p class="scan-state">{t[view.census.acquisition_state]}</p>
    <h3>{t.addresses}</h3><p class="help">{t.ownership}</p>
    <ul class="account-addresses">{#each view.addresses as address (address.address)}<li><label><input type="checkbox" checked={address.confirmed} disabled={busy || running} onchange={(event) => { const checked = event.currentTarget.checked; void act(async () => { view = await confirmAccount(selected, address.address, checked) }) }} />{t.confirm} {address.address}</label><p class="help">{address.evidence.join(', ')}</p></li>{/each}</ul>
    <details><summary>{t.details}</summary>{#each Object.entries(view.census.folders) as [name, folder] (name)}<h3>{name}</h3><p>{Object.keys(folder.messages).length} {t.saved} · UIDVALIDITY {folder.uidvalidity}</p>{#each Object.values(folder.messages) as message (message.uid)}<p>{message.headers.subject} · {message.headers.direction} · UID {message.uid}</p>{/each}{/each}</details>
  {/if}
  <p class="help">{t.limits}</p>
</section>
