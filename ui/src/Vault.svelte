<script lang="ts">
  import { createVault, unlockVault, VaultRefusal, type AccountStatus } from './api'
  let { status, lang, onOpened }: { status: AccountStatus; lang: 'en' | 'de'; onOpened: (next: AccountStatus) => void } = $props()
  const strings = {
    en: {
      createTitle: 'Create your Mailroom password',
      createDetail: 'Mailroom keeps your mail app passwords and census keys in an encrypted vault on this computer. The master password opens it after each start of Mailroom.',
      loss: 'If you forget the master password, nobody can open the vault: your saved accounts and census are lost and you would add the accounts again. Keep the password in your password manager or another safe place.',
      understood: 'I understand that a forgotten master password cannot be recovered',
      password: 'Master password', repeat: 'Repeat master password', minimum: 'At least 12 characters.',
      create: 'Create vault',
      unlockTitle: 'Unlock Mailroom',
      unlockDetail: 'Mailroom started again, so the vault is locked. Enter the master password to open your accounts.',
      unlock: 'Unlock',
      working: 'Opening the vault…',
      unavailableTitle: 'The vault cannot be opened',
      unsafe_file: 'The vault file has unsafe permissions or links, so Mailroom refused it and left it unchanged. Only your user may read ~/.config/mailroom-web/vault.mrv, and it must be a regular file. Fix that, then start Mailroom again.',
      damaged: 'The vault file is damaged, so Mailroom left it unchanged. Restore ~/.config/mailroom-web/vault.mrv from a backup, then start Mailroom again.',
      unsupported_version: 'The vault file comes from a different Mailroom version, so this version left it unchanged. Start the Mailroom version that created it.',
      limits: 'While the vault is open, programs running under your user account on this computer can use the local Mailroom session. Other websites cannot.',
      tooShort: 'Use at least 12 characters.', differ: 'The two passwords differ. Type the same password twice.',
      codes: {
        unlock_failed: 'That master password did not open the vault. Check it and try again.',
        weak_password: 'Use at least 12 characters, without control characters.',
        mismatch: 'The two passwords differ. Type the same password twice.',
        busy: 'Another unlock is still running. Wait a moment, then try again.',
        exists: 'A vault already exists. Unlock it with its master password.',
        absent: 'No vault exists yet. Create one first.',
        unavailable: 'The vault file is unsafe, damaged or unreadable. Mailroom left it unchanged.',
        storage: 'Mailroom could not save the vault. Check free disk space, then try again.',
        failed: 'Mailroom did not answer. Check that it is still running, reload this page and try again.',
      } as Record<string, string>,
    },
    de: {
      createTitle: 'Mailroom-Passwort festlegen',
      createDetail: 'Mailroom speichert die App-Passwörter deiner Postfächer und die Schlüssel der Erfassung in einem verschlüsselten Tresor auf diesem Computer. Das Hauptpasswort öffnet ihn nach jedem Start von Mailroom.',
      loss: 'Wenn du das Hauptpasswort vergisst, kann niemand den Tresor öffnen: gespeicherte Konten und Erfassungen sind dann verloren, und du müsstest die Konten neu hinzufügen. Bewahre das Passwort im Passwortmanager oder an einem anderen sicheren Ort auf.',
      understood: 'Ich habe verstanden, dass ein vergessenes Hauptpasswort nicht wiederhergestellt werden kann',
      password: 'Hauptpasswort', repeat: 'Hauptpasswort wiederholen', minimum: 'Mindestens 12 Zeichen.',
      create: 'Tresor anlegen',
      unlockTitle: 'Mailroom entsperren',
      unlockDetail: 'Mailroom wurde neu gestartet, daher ist der Tresor gesperrt. Gib das Hauptpasswort ein, um deine Konten zu öffnen.',
      unlock: 'Entsperren',
      working: 'Tresor wird geöffnet…',
      unavailableTitle: 'Der Tresor lässt sich nicht öffnen',
      unsafe_file: 'Die Tresordatei hat unsichere Berechtigungen oder Verknüpfungen. Mailroom hat sie abgelehnt und unverändert gelassen. Nur dein Benutzer darf ~/.config/mailroom-web/vault.mrv lesen, und es muss eine normale Datei sein. Behebe das und starte Mailroom erneut.',
      damaged: 'Die Tresordatei ist beschädigt. Mailroom hat sie unverändert gelassen. Stelle ~/.config/mailroom-web/vault.mrv aus einer Sicherung wieder her und starte Mailroom erneut.',
      unsupported_version: 'Die Tresordatei stammt aus einer anderen Mailroom-Version. Diese Version hat sie unverändert gelassen. Starte die Mailroom-Version, die sie angelegt hat.',
      limits: 'Solange der Tresor offen ist, können Programme, die unter deinem Benutzerkonto auf diesem Computer laufen, die lokale Mailroom-Sitzung nutzen. Andere Websites können das nicht.',
      tooShort: 'Verwende mindestens 12 Zeichen.', differ: 'Die beiden Passwörter unterscheiden sich. Gib zweimal dasselbe Passwort ein.',
      codes: {
        unlock_failed: 'Mit diesem Hauptpasswort ließ sich der Tresor nicht öffnen. Prüfe es und versuche es erneut.',
        weak_password: 'Verwende mindestens 12 Zeichen ohne Steuerzeichen.',
        mismatch: 'Die beiden Passwörter unterscheiden sich. Gib zweimal dasselbe Passwort ein.',
        busy: 'Eine andere Entsperrung läuft noch. Warte kurz und versuche es dann erneut.',
        exists: 'Es gibt bereits einen Tresor. Entsperre ihn mit seinem Hauptpasswort.',
        absent: 'Es gibt noch keinen Tresor. Lege zuerst einen an.',
        unavailable: 'Die Tresordatei ist unsicher, beschädigt oder nicht lesbar. Mailroom hat sie unverändert gelassen.',
        storage: 'Mailroom konnte den Tresor nicht speichern. Prüfe den freien Speicherplatz und versuche es erneut.',
        failed: 'Mailroom hat nicht geantwortet. Prüfe, ob es noch läuft, lade die Seite neu und versuche es erneut.',
      } as Record<string, string>,
    },
  }
  let t = $derived(strings[lang])
  let password = $state('')
  let confirmation = $state('')
  let understood = $state(false)
  let busy = $state(false)
  let error = $state('')
  let length = $derived([...password].length)
  async function submit(open: (password: string, confirmation: string) => Promise<AccountStatus>) {
    const entered = password, repeated = confirmation
    password = ''; confirmation = ''
    busy = true; error = ''
    try { onOpened(await open(entered, repeated)) }
    catch (refusal) { error = t.codes[refusal instanceof VaultRefusal ? refusal.code : 'failed'] ?? t.codes.failed }
    finally { busy = false }
  }
</script>

<div class="vault">
  {#if status.vault === 'absent'}
    <h3>{t.createTitle}</h3>
    <p>{t.createDetail}</p>
    <p class="vault-warning" role="note">{t.loss}</p>
    <form onsubmit={(event) => { event.preventDefault(); if (length >= 12 && password === confirmation && understood) void submit(createVault) }}>
      <label>{t.password}<input type="password" bind:value={password} autocomplete="new-password" minlength="12" maxlength="1024" required disabled={busy} aria-describedby="vault-minimum" /></label>
      <p id="vault-minimum" class="help">{t.minimum}</p>
      <label>{t.repeat}<input type="password" bind:value={confirmation} autocomplete="new-password" maxlength="1024" required disabled={busy} /></label>
      {#if password && length < 12}<p class="help">{t.tooShort}</p>{:else if confirmation && password !== confirmation}<p class="help">{t.differ}</p>{/if}
      <label class="vault-check"><input type="checkbox" bind:checked={understood} disabled={busy} />{t.understood}</label>
      <button type="submit" disabled={busy || length < 12 || password !== confirmation || !understood}>{t.create}</button>
    </form>
  {:else if status.vault === 'locked'}
    <h3>{t.unlockTitle}</h3>
    <p>{t.unlockDetail}</p>
    <form onsubmit={(event) => { event.preventDefault(); if (password) void submit(unlockVault) }}>
      <label>{t.password}<input type="password" bind:value={password} autocomplete="current-password" maxlength="1024" required disabled={busy} /></label>
      <button type="submit" disabled={busy || !password}>{t.unlock}</button>
    </form>
  {:else if status.vault === 'unavailable'}
    <h3>{t.unavailableTitle}</h3>
    <p role="alert">{t[status.vault_problem ?? 'damaged']}</p>
  {/if}
  {#if busy}<p role="status">{t.working}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <p class="help">{t.limits}</p>
</div>

<style>
  .vault { margin-block: 1rem; max-width: 40rem; }
  .vault-warning { padding: .8rem 1rem; border-left: 4px solid #a8641d; background: #fbf3e8; }
  form, label { display: grid; gap: .4rem; margin-block: .7rem; }
  .vault-check { display: flex; gap: .6rem; align-items: start; }
</style>
