<script lang="ts">
  import { connectAccount, reconnectAccount, finishInterrupted, type Account, type AccountStatus } from './api'
  let { status, account, disabled, lang, onStatus }: { status: AccountStatus; account: Account | undefined; disabled: boolean; lang: 'en' | 'de'; onStatus: (next: AccountStatus) => void } = $props()
  const strings = {
    en: {
      title: 'Add an account', provider: 'Provider', login: 'Email address', password: 'App password', add: 'Add and connect',
      readOnly: 'Mailroom signs in read-only: it lists folders and saves message headers. It never changes, moves or sends mail.',
      hints: { mailbox_org: 'Use your mailbox.org password, or an app password if you created one.', gmail: 'Gmail needs a Google app password. Google offers app passwords only with 2-Step Verification turned on.' },
      connecting: 'Connecting to the provider…', connected: 'Connected. Run the census to save headers.',
      unverified: 'The connection to this account was not checked yet.', failedAccount: 'The last connection to this account failed.',
      retry: 'Retry connection', correct: 'Correct app password', newPassword: 'New app password', save: 'Save and reconnect',
      interrupted: 'An account setup stopped before it finished. Clean it up to add the account again.', cleanUp: 'Clean up interrupted setup',
      failed: 'Mailroom did not accept the request. Reload the page and try again.',
      codes: {
        authentication: 'The provider refused the sign-in. Check the email address and the app password, then correct the password below.',
        tls: 'Mailroom could not reach the provider securely. Check the internet connection, then retry.',
        protocol: 'The provider gave an incomplete answer. Retry in a few minutes.',
        incomplete: 'The provider gave an incomplete answer. Retry in a few minutes.',
        invalid_input: 'This account is already saved, or the address or password is not valid. Pick the saved account to retry or correct it.',
        vault_locked: 'The vault locked itself. Unlock it again; nothing was lost.',
        lifecycle_pending: 'Another account action is still running. Wait for it, then retry.',
        cancelled: 'The connection was cancelled. Retry when ready.',
      } as Record<string, string>,
      other: 'The connection failed. Retry, or correct the app password below.',
    },
    de: {
      title: 'Konto hinzufügen', provider: 'Anbieter', login: 'E-Mail-Adresse', password: 'App-Passwort', add: 'Hinzufügen und verbinden',
      readOnly: 'Mailroom meldet sich nur lesend an: Es listet Ordner auf und speichert Kopfzeilen. Es ändert, verschiebt oder sendet nie Nachrichten.',
      hints: { mailbox_org: 'Verwende dein mailbox.org-Passwort oder ein App-Passwort, falls du eines angelegt hast.', gmail: 'Gmail braucht ein Google-App-Passwort. Google bietet App-Passwörter nur mit aktivierter Bestätigung in zwei Schritten an.' },
      connecting: 'Verbindung zum Anbieter wird aufgebaut…', connected: 'Verbunden. Starte die Erfassung, um Kopfzeilen zu speichern.',
      unverified: 'Die Verbindung zu diesem Konto wurde noch nicht geprüft.', failedAccount: 'Die letzte Verbindung zu diesem Konto ist fehlgeschlagen.',
      retry: 'Verbindung erneut versuchen', correct: 'App-Passwort korrigieren', newPassword: 'Neues App-Passwort', save: 'Speichern und neu verbinden',
      interrupted: 'Eine Kontoeinrichtung wurde vor dem Abschluss unterbrochen. Räume sie auf, um das Konto erneut hinzuzufügen.', cleanUp: 'Unterbrochene Einrichtung aufräumen',
      failed: 'Mailroom hat die Anfrage nicht angenommen. Lade die Seite neu und versuche es erneut.',
      codes: {
        authentication: 'Der Anbieter hat die Anmeldung abgelehnt. Prüfe E-Mail-Adresse und App-Passwort und korrigiere das Passwort unten.',
        tls: 'Mailroom konnte den Anbieter nicht sicher erreichen. Prüfe die Internetverbindung und versuche es erneut.',
        protocol: 'Der Anbieter hat unvollständig geantwortet. Versuche es in einigen Minuten erneut.',
        incomplete: 'Der Anbieter hat unvollständig geantwortet. Versuche es in einigen Minuten erneut.',
        invalid_input: 'Dieses Konto ist bereits gespeichert, oder Adresse oder Passwort sind ungültig. Wähle das gespeicherte Konto, um es erneut zu versuchen oder zu korrigieren.',
        vault_locked: 'Der Tresor hat sich gesperrt. Entsperre ihn erneut; es ging nichts verloren.',
        lifecycle_pending: 'Eine andere Kontoaktion läuft noch. Warte sie ab und versuche es dann erneut.',
        cancelled: 'Die Verbindung wurde abgebrochen. Versuche es erneut, wenn du bereit bist.',
      } as Record<string, string>,
      other: 'Die Verbindung ist fehlgeschlagen. Versuche es erneut oder korrigiere unten das App-Passwort.',
    },
  }
  let t = $derived(strings[lang])
  let provider = $state<Account['provider']>('mailbox_org')
  let login = $state('')
  let password = $state('')
  let correction = $state('')
  let busy = $state(false)
  let error = $state('')
  let job = $derived(status.job)
  let jobError = $derived(job?.progress.state === 'failed' && job.error ? (t.codes[job.error] ?? t.other) : '')
  let locked = $derived(disabled || busy || job?.progress.state === 'running')
  async function send(request: () => Promise<AccountStatus>, done: () => void = () => {}) {
    busy = true; error = ''
    try { onStatus(await request()); done() } catch { error = t.failed } finally { busy = false }
  }
  function add() {
    const entered = password; password = ''
    void send(() => connectAccount({ provider, login: login.trim(), password: entered }), () => { login = '' })
  }
  function correct(id: string) {
    const entered = correction; correction = ''
    void send(() => reconnectAccount(id, entered))
  }
</script>

<div class="connect-account">
  {#each status.interrupted as id (id)}
    <div class="connect-interrupted"><p role="alert">{t.interrupted}</p><button disabled={locked} onclick={() => send(() => finishInterrupted(id))}>{t.cleanUp}</button></div>
  {/each}
  <h3>{t.title}</h3>
  <p class="help">{t.readOnly}</p>
  <form onsubmit={(event) => { event.preventDefault(); if (login.trim() && password) add() }}>
    <label>{t.provider}<select bind:value={provider} disabled={locked}><option value="mailbox_org">mailbox.org</option><option value="gmail">Gmail</option></select></label>
    <p class="help">{t.hints[provider]}</p>
    <label>{t.login}<input type="email" bind:value={login} autocomplete="username" maxlength="320" required disabled={locked} /></label>
    <label>{t.password}<input type="password" bind:value={password} autocomplete="new-password" maxlength="1024" required disabled={locked} /></label>
    <button type="submit" disabled={locked || !login.trim() || !password}>{t.add}</button>
  </form>
  {#if job?.progress.state === 'running'}<p role="status">{t.connecting}</p>{/if}
  {#if jobError}<p role="alert">{jobError}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if account && (account.connection !== 'observed' || (job?.progress.state === 'failed' && job.account?.id === account.id))}
    <div class="connect-correction">
      <p>{account.login} · {account.connection === 'unverified' ? t.unverified : t.failedAccount}</p>
      <button disabled={locked} onclick={() => { const id = account.id; void send(() => reconnectAccount(id)) }}>{t.retry}</button>
      <form onsubmit={(event) => { event.preventDefault(); if (correction) correct(account.id) }}>
        <label>{t.newPassword}<input type="password" bind:value={correction} autocomplete="new-password" maxlength="1024" required disabled={locked} /></label>
        <button type="submit" disabled={locked || !correction}>{t.save}</button>
      </form>
    </div>
  {:else if account && job?.progress.state === 'complete' && job.account?.id === account.id}
    <p role="status">{t.connected}</p>
  {/if}
</div>

<style>
  .connect-account { margin-block: 1rem; max-width: 40rem; }
  form, label { display: grid; gap: .4rem; margin-block: .7rem; }
  .connect-correction, .connect-interrupted { padding: .8rem 1rem; border: 1px solid #a4b6bf; border-radius: 5px; margin-block: .8rem; }
</style>
