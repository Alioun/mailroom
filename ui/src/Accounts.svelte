<script lang="ts">
  import { onMount } from 'svelte'
  import ManageSieve from './ManageSieve.svelte'
  import SieveTransactions from './SieveTransactions.svelte'
  import FolderProvisioning from './FolderProvisioning.svelte'
  import AcquisitionEvidence from './AcquisitionEvidence.svelte'
  import Reports from './Reports.svelte'
  import Lifecycle from './Lifecycle.svelte'
  import Recovery from './Recovery.svelte'
  import Ledger from './Ledger.svelte'
  import Winddown from './Winddown.svelte'
  import Backlog from './Backlog.svelte'
  import Invoices from './Invoices.svelte'
  import Unsubscribe from './Unsubscribe.svelte'
  import RulesProof from './RulesProof.svelte'
  import { readAccountStatus, listAccounts, syntheticAccount, scanAccount, cancelAccount, openAccount, confirmAccount, type Account, type AccountStatus, type AccountView } from './api'
  let unsubscribe: Unsubscribe | undefined = $state()
  let { lang }: { lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Accounts', detail: 'Saved account census and local lifecycle. Production account creation requires a protected installation. Real provider writes remain locked.',
      protectedUnavailable: 'Protected account installation is unavailable. New production connections are locked.',
      synthetic: 'Synthetic account verification. Temporary files and fake credentials only. No provider or OS credential store is contacted.', add: 'Add synthetic account', addGmail: 'Add synthetic Gmail account',
      load: 'Load saved accounts', choose: 'Choose an account', scan: 'Run or resume account census', cancel: 'Cancel account census', open: 'Open saved census',
      idle: 'Ready', running: 'Account action running', cancelled: 'Account census cancelled', complete: 'Account action complete', failed: 'Account action failed', saved: 'headers saved', folders: 'folders',
      error: 'The account action failed. Check the existing credential privately, or restore the original census key. Saved files were retained. Retry later for provider or network failures.',
      addresses: 'Confirm own addresses', ownership: 'To and Cc are unverified evidence. Confirmation assigns ownership only. It does not assign a business role or forwarding permission.', confirm: 'Confirm own address', details: 'Saved local headers',
      limits: 'Takeout and trusted DKIM correlation remain incomplete. The full personal workflow is not ready for daily use. No result proves rule coexistence, delivery-recipient behavior or accounting acceptance. All account writes stay locked.',
      loadError: 'Could not read the local response. Reload or retry the action.',
      loading: 'Loading saved accounts…', retry: 'Retry loading accounts', working: 'Working…',
      empty: 'No saved accounts yet.', emptySynthetic: 'Add a synthetic account above to explore the workflow with invented mail.',
    },
    de: {
      title: 'Konten', detail: 'Gespeicherte Kontoerfassung und lokale Kontoverwaltung. Neue produktive Konten erfordern eine geschützte Installation. Anbieteränderungen bleiben gesperrt.',
      protectedUnavailable: 'Geschützte Kontoinstallation fehlt. Neue produktive Verbindungen sind gesperrt.',
      synthetic: 'Synthetische Kontoprüfung. Nur temporäre Dateien und erfundene Zugangsdaten. Kein Anbieter oder Zugangsdaten-Speicher des Betriebssystems wird kontaktiert.', add: 'Synthetisches Konto hinzufügen', addGmail: 'Synthetisches Gmail-Konto hinzufügen',
      load: 'Gespeicherte Konten laden', choose: 'Konto wählen', scan: 'Kontoerfassung starten oder fortsetzen', cancel: 'Kontoerfassung abbrechen', open: 'Gespeicherte Erfassung öffnen',
      idle: 'Bereit', running: 'Kontoaktion läuft', cancelled: 'Kontoerfassung abgebrochen', complete: 'Kontoaktion abgeschlossen', failed: 'Kontoaktion fehlgeschlagen', saved: 'Kopfzeilen gespeichert', folders: 'Ordner',
      error: 'Die Kontoaktion ist fehlgeschlagen. Prüfe die vorhandenen Zugangsdaten vertraulich oder stelle den ursprünglichen Erfassungsschlüssel wieder her. Gespeicherte Dateien bleiben erhalten. Bei Anbieter- oder Netzwerkfehlern später erneut versuchen.',
      addresses: 'Eigene Adressen bestätigen', ownership: 'An und Cc sind unbestätigte Belege. Die Bestätigung weist nur die Zugehörigkeit zu. Sie weist keine geschäftliche Rolle oder Weiterleitungsberechtigung zu.', confirm: 'Eigene Adresse bestätigen', details: 'Lokal gespeicherte Kopfzeilen',
      limits: 'Takeout und vertrauenswürdige DKIM-Zuordnung fehlen noch. Der gesamte persönliche Ablauf ist noch nicht für den Alltag bereit. Kein Ergebnis belegt Regel-Koexistenz, Zustellempfänger oder Buchhaltungsannahme. Alle Kontoänderungen bleiben gesperrt.',
      loadError: 'Die lokale Antwort konnte nicht gelesen werden. Lade neu oder wiederhole die Aktion.',
      loading: 'Gespeicherte Konten werden geladen…', retry: 'Konten erneut laden', working: 'Wird bearbeitet…',
      empty: 'Noch keine gespeicherten Konten.', emptySynthetic: 'Füge oben ein synthetisches Konto hinzu, um den Ablauf mit erfundenen Nachrichten auszuprobieren.',
    },
  }
  let t = $derived(strings[lang])
  let status = $state<AccountStatus | null>(null)
  let accounts = $state<Account[]>([])
  let selected = $state('')
  let view = $state<AccountView | null>(null)
  let busy = $state(false)
  let error = $state('')
  type AccountLoad = { kind: 'loading'; revision: number } | { kind: 'ready' } | { kind: 'error' }
  let accountLoad = $state<AccountLoad>({ kind: 'loading', revision: 0 })
  let running = $derived(status?.job?.progress.state === 'running')
  let active = true
  let polling = false
  let cacheEpoch = $state(0)
  let workspaceRevision = $state(0)
  let revision = 0
  async function loadAccounts() {
    const requestedRevision = ++revision
    accountLoad = { kind: 'loading', revision: requestedRevision }
    error = ''
    try {
      const [next, saved] = await Promise.all([readAccountStatus(), listAccounts()])
      if (!active || requestedRevision !== revision) return
      accounts = saved
      if (!accounts.some(account => account.id === selected)) selected = ''
      accept(next)
      selected ||= accounts[0]?.id ?? ''
      accountLoad = { kind: 'ready' }
    } catch {
      if (active && requestedRevision === revision) accountLoad = { kind: 'error' }
    } finally {
      if (active && accountLoad.kind === 'loading' && accountLoad.revision === requestedRevision) accountLoad = { kind: 'ready' }
    }
  }
  async function lifecycleChanged() {
    revision += 1; view = null; cacheEpoch += 1; status = await readAccountStatus(); accounts = await listAccounts(); if (!accounts.some(a => a.id === selected)) selected = ''
  }
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
    } catch { if (active && requestedRevision === revision) error = t.loadError } finally { polling = false }
  }
  onMount(() => {
    active = true; void loadAccounts()
    const timer = setInterval(() => { if (status?.job?.progress.state === 'running') void refresh() }, 250)
    return () => { active = false; clearInterval(timer) }
  })
</script>

<section class="accounts" aria-labelledby="accounts-heading" aria-busy={accountLoad.kind === 'loading'}>
  <h2 id="accounts-heading">{t.title}</h2>
  <p>{t.detail}</p>
  {#if accountLoad.kind === 'loading'}
    <p role="status">{t.loading}</p>
  {:else if accountLoad.kind === 'error'}
    <div class="account-load-error"><p role="alert">{t.loadError}</p><button onclick={loadAccounts}>{t.retry}</button></div>
  {/if}
  {#if status?.synthetic}
    <p class="source">{t.synthetic}</p>
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={() => act(async () => { selected = ''; view = null; accept(await syntheticAccount()) })}>{t.add}</button>
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={() => act(async () => { selected = ''; view = null; accept(await syntheticAccount(true)) })}>{t.addGmail}</button>
  {:else if status}
    <p>{t.protectedUnavailable}</p>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <div class="census-actions">
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={loadAccounts}>{t.load}</button>
    {#if accounts.length}
      <label>{t.choose}<select bind:value={selected} disabled={busy || running} onchange={() => view = null}><option value="">{t.choose}</option>{#each accounts as account (account.id)}<option value={account.id}>{account.login} · {account.provider === 'gmail' ? 'Gmail' : 'mailbox.org'}</option>{/each}</select></label>
      <button disabled={busy || running || !selected} onclick={() => act(async () => { view = null; accept(await scanAccount(selected)) })}>{t.scan}</button>
      <button disabled={busy || !running} onclick={() => act(async () => { accept(await cancelAccount()) })}>{t.cancel}</button>
      <button disabled={busy || running || !selected} onclick={() => act(async () => { view = await openAccount(selected) })}>{t.open}</button>
    {/if}
  </div>
  {#if accountLoad.kind === 'ready' && !accounts.length}
    <div class="account-empty"><p class="scan-state">{t.empty}</p>{#if status?.synthetic}<p>{t.emptySynthetic}</p>{/if}</div>
  {/if}
  {#if busy}<p role="status">{t.working}</p>{/if}
  {#if status?.job}
    <div aria-live="polite"><p class="scan-state">{t[status.job.progress.state]}</p><p>{status.job.progress.saved} {t.saved} · {status.job.progress.folders} {t.folders}</p></div>
    {#if status.job.error}<p role="alert">{t.error}</p>{/if}
  {/if}
  {#key status?.synthetic}<Recovery {accounts} account={selected} synthetic={status?.synthetic ?? false} {lang} onChanged={lifecycleChanged} />{/key}
  {#key `${selected}:${status?.synthetic}`}<Lifecycle account={selected} synthetic={status?.synthetic ?? false} {lang} onChanged={lifecycleChanged} />{/key}
  {#key `${selected}:${cacheEpoch}`}<ManageSieve account={selected} {lang} /><SieveTransactions account={selected} {lang} /><FolderProvisioning account={selected} {lang} />{/key}
  {#if view}
    <p class="scan-state">{t[view.census.acquisition_state]}</p>
    <AcquisitionEvidence evidence={view.evidence} {lang} />
    <h3>{t.addresses}</h3><p class="help">{t.ownership}</p>
    <ul class="account-addresses">{#each view.addresses as address (address.address)}<li><label><input type="checkbox" checked={address.confirmed} disabled={busy || running} onchange={(event) => { const checked = event.currentTarget.checked; void act(async () => { view = await confirmAccount(selected, address.address, checked) }) }} />{t.confirm} {address.address}</label><p class="help">{address.evidence.join(', ')}</p></li>{/each}</ul>
    {#key view}<Reports account={selected} {lang} /><Winddown account={selected} {lang} onBulkHandoff={async (senders, asOf) => { await unsubscribe?.prepareHandoff(selected, senders, asOf) }} /><Ledger account={selected} {lang} /><RulesProof account={selected} {lang} onWorkspaceChange={() => workspaceRevision += 1} /><Backlog account={selected} {lang} {workspaceRevision} /><Invoices account={selected} {lang} />{/key}
    <details><summary>{t.details}</summary>{#each Object.entries(view.census.folders) as [name, folder] (name)}<h3>{name}</h3><p>{Object.keys(folder.messages).length} {t.saved} · UIDVALIDITY {folder.uidvalidity}</p>{#each Object.values(folder.messages) as message (message.uid)}<p>{message.headers.subject} · {message.headers.direction} · UID {message.uid}</p>{/each}{/each}</details>
  {/if}
  {#key cacheEpoch}<Unsubscribe {lang} bind:this={unsubscribe} />{/key}
  <p class="help">{t.limits}</p>
</section>
