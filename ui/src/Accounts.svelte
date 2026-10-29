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
  import { readAccountStatus, listAccounts, syntheticAccount, scanAccount, cancelAccount, confirmAccount, type Account, type AccountStatus } from './api'
  import { accountWorkspace, type SavedAccountState } from './account-workspace'
  let unsubscribe: Unsubscribe | undefined = $state()
  let { lang }: { lang: 'en' | 'de' } = $props()
  const strings = {
    en: {
      title: 'Accounts', detail: 'Saved account census and local lifecycle. Production account creation requires a protected installation. Real provider writes remain locked.',
      protectedUnavailable: 'Add or update accounts through the approved protected setup. This page opens saved accounts.',
      synthetic: 'Synthetic account verification. Temporary files and fake credentials only. No provider or OS credential store is contacted.', add: 'Add synthetic account', addGmail: 'Add synthetic Gmail account',
      load: 'Load saved accounts', choose: 'Choose an account', scan: 'Run or resume account census', cancel: 'Cancel account census', open: 'Open saved census',
      idle: 'Ready', running: 'Account action running', cancelled: 'Account census cancelled', complete: 'Account action complete', failed: 'Account action failed', saved: 'headers saved', folders: 'folders',
      error: 'The account action failed. Check the existing credential privately, or restore the original census key. Saved files were retained. Retry later for provider or network failures.',
      addresses: 'Confirm own addresses', ownership: 'To and Cc are unverified evidence. Confirmation assigns ownership only. It does not assign a business role or forwarding permission.', confirm: 'Confirm own address', details: 'Saved local headers',
      limits: 'Saved headers do not prove delivery-recipient authority, provider authentication, existing-rule coexistence or accounting acceptance. Real provider writes remain locked. Personal-use acceptance is still pending.',
      loadError: 'Could not read the local response. Reload or retry the action.',
      loading: 'Loading saved accounts…', retry: 'Retry loading accounts', working: 'Working…',
      tools: 'Account tasks', overview: 'Census and addresses', rules: 'Rules and address policy', backlog: 'Existing mail', invoices: 'Invoices', subscriptions: 'Unsubscribe and mute', winddown: 'Wind-down', setup: 'Setup and recovery', workspaceLoading: 'Opening the saved census…', workspaceError: 'Could not open or update this account. Saved files were retained. Retry opening its census.', workspaceRetry: 'Retry opening census', waiting: 'The saved workspace will open when the account activity ends.', activity: 'Account activity', empty: 'No saved accounts yet.', emptySynthetic: 'Add a synthetic account above to explore the workflow with invented mail.',
    },
    de: {
      title: 'Konten', detail: 'Gespeicherte Kontoerfassung und lokale Kontoverwaltung. Neue produktive Konten erfordern eine geschützte Installation. Anbieteränderungen bleiben gesperrt.',
      protectedUnavailable: 'Konten werden über die freigegebene geschützte Einrichtung angelegt oder geändert. Hier werden gespeicherte Konten geöffnet.',
      synthetic: 'Synthetische Kontoprüfung. Nur temporäre Dateien und erfundene Zugangsdaten. Kein Anbieter oder Zugangsdaten-Speicher des Betriebssystems wird kontaktiert.', add: 'Synthetisches Konto hinzufügen', addGmail: 'Synthetisches Gmail-Konto hinzufügen',
      load: 'Gespeicherte Konten laden', choose: 'Konto wählen', scan: 'Kontoerfassung starten oder fortsetzen', cancel: 'Kontoerfassung abbrechen', open: 'Gespeicherte Erfassung öffnen',
      idle: 'Bereit', running: 'Kontoaktion läuft', cancelled: 'Kontoerfassung abgebrochen', complete: 'Kontoaktion abgeschlossen', failed: 'Kontoaktion fehlgeschlagen', saved: 'Kopfzeilen gespeichert', folders: 'Ordner',
      error: 'Die Kontoaktion ist fehlgeschlagen. Prüfe die vorhandenen Zugangsdaten vertraulich oder stelle den ursprünglichen Erfassungsschlüssel wieder her. Gespeicherte Dateien bleiben erhalten. Bei Anbieter- oder Netzwerkfehlern später erneut versuchen.',
      addresses: 'Eigene Adressen bestätigen', ownership: 'An und Cc sind unbestätigte Belege. Die Bestätigung weist nur die Zugehörigkeit zu. Sie weist keine geschäftliche Rolle oder Weiterleitungsberechtigung zu.', confirm: 'Eigene Adresse bestätigen', details: 'Lokal gespeicherte Kopfzeilen',
      limits: 'Gespeicherte Kopfzeilen belegen weder Zustellempfänger noch Anbieter-Authentifizierung, Regel-Koexistenz oder Buchhaltungsannahme. Echte Anbieteränderungen bleiben gesperrt. Die Freigabe für den persönlichen Alltag steht noch aus.',
      loadError: 'Die lokale Antwort konnte nicht gelesen werden. Lade neu oder wiederhole die Aktion.',
      loading: 'Gespeicherte Konten werden geladen…', retry: 'Konten erneut laden', working: 'Wird bearbeitet…',
      tools: 'Kontoaufgaben', overview: 'Erfassung und Adressen', rules: 'Regeln und Adressrichtlinie', backlog: 'Vorhandene Nachrichten', invoices: 'Rechnungen', subscriptions: 'Abmelden und stummschalten', winddown: 'Stilllegung', setup: 'Einrichtung und Wiederherstellung', workspaceLoading: 'Gespeicherte Erfassung wird geöffnet…', workspaceError: 'Dieses Konto konnte nicht geöffnet oder aktualisiert werden. Gespeicherte Dateien bleiben erhalten. Öffne die Erfassung erneut.', workspaceRetry: 'Erfassung erneut öffnen', waiting: 'Der gespeicherte Arbeitsbereich öffnet sich, sobald die Kontoaktion endet.', activity: 'Kontoaktivität', empty: 'Noch keine gespeicherten Konten.', emptySynthetic: 'Füge oben ein synthetisches Konto hinzu, um den Ablauf mit erfundenen Nachrichten auszuprobieren.',
    },
  }
  let t = $derived(strings[lang])
  let status = $state<AccountStatus | null>(null)
  let accounts = $state<Account[]>([])
  let selected = $state('')
  let savedAccount = $state<SavedAccountState>({ kind: 'empty' })
  let view = $derived(savedAccount.kind === 'ready' && savedAccount.account === selected ? savedAccount.view : null)
  const workspace = accountWorkspace(next => { savedAccount = next })
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
    workspace.clear()
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
    cacheEpoch += 1
    await loadAccounts()
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
  $effect(() => {
    const account = selected
    if (accountLoad.kind !== 'ready' || running) { workspace.clear(); return }
    void workspace.load(account)
  })
  onMount(() => {
    active = true; void loadAccounts()
    const timer = setInterval(() => { if (status?.job?.progress.state === 'running') void refresh() }, 250)
    return () => { active = false; workspace.dispose(); clearInterval(timer) }
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
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={() => act(async () => { selected = ''; workspace.clear(); accept(await syntheticAccount()) })}>{t.add}</button>
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={() => act(async () => { selected = ''; workspace.clear(); accept(await syntheticAccount(true)) })}>{t.addGmail}</button>
  {:else if status}
    <p>{t.protectedUnavailable}</p>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <div class="census-actions">
    <button disabled={busy || running || accountLoad.kind === 'loading'} onclick={loadAccounts}>{t.load}</button>
    {#if accounts.length}
      <label>{t.choose}<select bind:value={selected} disabled={busy || running}><option value="">{t.choose}</option>{#each accounts as account (account.id)}<option value={account.id}>{account.login} · {account.provider === 'gmail' ? 'Gmail' : 'mailbox.org'}</option>{/each}</select></label>
      <button disabled={busy || running || !selected} onclick={() => act(async () => { workspace.clear(); accept(await scanAccount(selected)) })}>{t.scan}</button>
      <button disabled={busy || !running} onclick={() => act(async () => { accept(await cancelAccount()) })}>{t.cancel}</button>
      <button disabled={busy || running || !selected || savedAccount.kind === 'loading'} onclick={() => workspace.load(selected)}>{t.open}</button>
    {/if}
  </div>
  {#if accountLoad.kind === 'ready' && !accounts.length}
    <div class="account-empty"><p class="scan-state">{t.empty}</p>{#if status?.synthetic}<p>{t.emptySynthetic}</p>{/if}</div>
  {/if}
  {#if busy}<p role="status">{t.working}</p>{/if}
  {#if status?.job}
    <div aria-live="polite">{#if status.job.account}<p>{t.activity} · {status.job.account.login}</p>{/if}<p class="scan-state">{t[status.job.progress.state]}</p><p>{status.job.progress.saved} {t.saved} · {status.job.progress.folders} {t.folders}</p></div>
    {#if status.job.error}<p role="alert">{t.error}</p>{/if}
  {/if}
  {#if selected}
    <nav class="account-tasks" aria-label={t.tools}>
      {#if view}
        <a href="#account-overview">{t.overview}</a><a href="#account-rules">{t.rules}</a><a href="#account-backlog">{t.backlog}</a><a href="#account-invoices">{t.invoices}</a>
      {/if}
      <a href="#account-subscriptions">{t.subscriptions}</a>
      {#if view}<a href="#account-winddown">{t.winddown}</a>{/if}
      <a href="#account-setup">{t.setup}</a>
    </nav>
  {/if}
  {#if savedAccount.kind === 'loading'}
    <p class="workspace-state" role="status">{t.workspaceLoading}</p>
  {:else if savedAccount.kind === 'error' && savedAccount.account === selected}
    <div class="workspace-state"><p role="alert">{t.workspaceError}</p><button disabled={busy || running} onclick={() => workspace.load(selected)}>{t.workspaceRetry}</button></div>
  {:else if selected && running}
    <p class="workspace-state" role="status">{t.waiting}</p>
  {/if}
  {#if view}
    <div id="account-overview" class="account-task" tabindex="-1">
      <p class="scan-state">{t[view.census.acquisition_state]}</p>
      <AcquisitionEvidence evidence={view.evidence} {lang} />
      <h3>{t.addresses}</h3><p class="help">{t.ownership}</p>
      <ul class="account-addresses">{#each view.addresses as address (address.address)}<li><label><input type="checkbox" checked={address.confirmed} disabled={busy || running} onchange={(event) => { const checked = event.currentTarget.checked; const account = selected; void act(async () => { await workspace.load(account, () => confirmAccount(account, address.address, checked)) }) }} />{t.confirm} {address.address}</label><p class="help">{address.evidence.join(', ')}</p></li>{/each}</ul>
      {#key view}<Reports account={selected} {lang} />{/key}
    </div>
    {#key view}
      <div id="account-rules" class="account-task" tabindex="-1"><Ledger account={selected} {lang} /><RulesProof account={selected} {lang} onWorkspaceChange={() => workspaceRevision += 1} /></div>
      <div id="account-backlog" class="account-task" tabindex="-1"><Backlog account={selected} {lang} {workspaceRevision} /></div>
      <div id="account-invoices" class="account-task" tabindex="-1"><Invoices account={selected} {lang} /></div>
    {/key}
  {/if}
  <div id="account-subscriptions" class="account-task" tabindex="-1">{#key cacheEpoch}<Unsubscribe {lang} bind:this={unsubscribe} />{/key}</div>
  {#if view}
    <div id="account-winddown" class="account-task" tabindex="-1">{#key view}<Winddown account={selected} {lang} onBulkHandoff={async (senders, asOf) => { await unsubscribe?.prepareHandoff(selected, senders, asOf); document.getElementById('account-subscriptions')?.focus() }} />{/key}</div>
  {/if}
  <div id="account-setup" class="account-task account-setup" tabindex="-1">
    <h3>{t.setup}</h3>
    {#key status?.synthetic}<Recovery {accounts} account={selected} synthetic={status?.synthetic ?? false} {lang} onChanged={lifecycleChanged} />{/key}
    {#key `${selected}:${status?.synthetic}`}<Lifecycle account={selected} synthetic={status?.synthetic ?? false} {lang} onChanged={lifecycleChanged} />{/key}
    {#key `${selected}:${cacheEpoch}`}<ManageSieve account={selected} {lang} /><SieveTransactions account={selected} {lang} /><FolderProvisioning account={selected} {lang} />{/key}
    {#if view}
      <details><summary>{t.details}</summary>{#each Object.entries(view.census.folders) as [name, folder] (name)}<h3>{name}</h3><p>{Object.keys(folder.messages).length} {t.saved} · UIDVALIDITY {folder.uidvalidity}</p>{#each Object.values(folder.messages) as message (message.uid)}<p>{message.headers.subject} · {message.headers.direction} · UID {message.uid}</p>{/each}{/each}</details>
    {/if}
  </div>
  <p class="help">{t.limits}</p>
</section>
