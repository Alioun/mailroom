<script lang="ts">
  import { onMount } from 'svelte'
  import { readLifecycle, previewForget, forgetAccount, replaceAccountPassword, rotateAccount, recoverAccount, type ForgetPreview, type LifecycleStatus } from './api'
  let { account, synthetic, lang, onChanged }: { account: string; synthetic: boolean; lang: 'en' | 'de'; onChanged: () => Promise<void> } = $props()
  const strings = {
    en: { title: 'Local account lifecycle', unavailable: 'Protected installation is unavailable. Production lifecycle actions are locked.', scope: 'Local changes only. No provider action or native prompt. Password replacement does not check connectivity. Rotation preserves saved identities and history, invalidates approvals and does not create a backup.', password: 'Replacement password', replace: 'Replace local account password', rotate: 'Rotate local encryption key', rotateConfirm: 'I confirm local key rotation and invalidation of outstanding approvals', preview: 'Preview exact local forgetting', confirm: 'I confirm forgetting this exact account and the displayed local resources', forget: 'Forget exactly these local resources', recovery: 'Refresh lifecycle recovery status', recover: 'Resume local lifecycle recovery', ready: 'No interrupted lifecycle operation.', error: 'Local lifecycle action failed. Preserve existing files and keys. Refresh recovery status and retry recovery. No success is claimed.', saved: 'Local lifecycle change saved. Account previews were cleared.', consequences: 'Saved headers, original content, rules, decisions, exports, undo and reconciliation history, and synthetic effect stores are removed. No remote undo, unsubscribe or account shutdown occurs. Shared addresses.yaml and unrelated accounts survive. Browser-managed downloads, exports and backups cannot be erased here. Coordination locks remain.', resources: 'Exact owned files', credentials: 'Credential references to remove', generation: 'Key generation', revision: 'Account revision', creating: 'Interrupted account creation', replacing: 'Interrupted password replacement', forgetting: 'Local deletion pending', rotating: 'Key rotation pending', forgotten: 'Account forgotten locally' },
    de: { title: 'Lokale Kontoverwaltung', unavailable: 'Geschützte Installation fehlt. Produktive Kontoaktionen sind gesperrt.', scope: 'Nur lokale Änderungen. Keine Anbieteraktion oder native Abfrage. Ein Passwortwechsel prüft die Verbindung nicht. Der Schlüsselwechsel erhält Identitäten und Verlauf, entwertet Freigaben und erstellt keine Sicherung.', password: 'Neues Passwort', replace: 'Lokales Kontopasswort ersetzen', rotate: 'Lokalen Verschlüsselungsschlüssel wechseln', rotateConfirm: 'Ich bestätige den lokalen Schlüsselwechsel und die Entwertung offener Freigaben', preview: 'Genaues lokales Vergessen prüfen', confirm: 'Ich bestätige das Vergessen genau dieses Kontos und der angezeigten lokalen Ressourcen', forget: 'Genau diese lokalen Ressourcen vergessen', recovery: 'Wiederherstellungsstatus aktualisieren', recover: 'Lokale Kontowiederherstellung fortsetzen', ready: 'Keine unterbrochene Kontoaktion.', error: 'Lokale Kontoaktion fehlgeschlagen. Vorhandene Dateien und Schlüssel erhalten. Status aktualisieren und die Wiederherstellung erneut versuchen. Kein Erfolg wird behauptet.', saved: 'Lokale Änderung gespeichert. Kontovorschauen wurden geleert.', consequences: 'Gespeicherte Kopfzeilen, Originalinhalte, Regeln, Entscheidungen, Exporte, Rückgängig- und Abgleichverlauf sowie synthetische Effektspeicher werden entfernt. Keine entfernte Rücknahme, Abmeldung oder Kontoschließung. Gemeinsame addresses.yaml und andere Konten bleiben erhalten. Browser-Downloads, Exporte und Sicherungen können hier nicht gelöscht werden. Koordinationssperren bleiben.', resources: 'Genaue eigene Dateien', credentials: 'Zu entfernende Zugangsdaten-Verweise', generation: 'Schlüsselgeneration', revision: 'Kontorevision', creating: 'Unterbrochene Kontoerstellung', replacing: 'Unterbrochener Passwortwechsel', forgetting: 'Lokale Löschung ausstehend', rotating: 'Schlüsselwechsel ausstehend', forgotten: 'Konto lokal vergessen' },
  }
  let t = $derived(strings[lang])
  let password = $state('')
  let preview = $state<ForgetPreview | null>(null)
  let status = $state<LifecycleStatus | null>(null)
  let confirmed = $state(false)
  let rotationConfirmed = $state(false)
  let busy = $state(false)
  let message = $state('')
  let error = $state('')
  function invalidate() { preview = null; confirmed = false; rotationConfirmed = false }
  async function act(action: () => Promise<void>) {
    busy = true; error = ''; message = ''
    try { await action() } catch { error = t.error; invalidate() } finally { busy = false }
  }
  async function changed() { invalidate(); password = ''; await onChanged(); status = await readLifecycle(); message = t.saved }
  async function replace() {
    const value = password; password = ''
    await act(async () => { await replaceAccountPassword(account, value); await changed() })
  }
  onMount(() => { if (synthetic) void act(async () => { status = await readLifecycle() }); return () => { password = '' } })
</script>
<details class="lifecycle">
  <summary>{t.title}</summary>
  {#if !synthetic}<p>{t.unavailable}</p>{:else}
    <p>{t.scope}</p>
    <button disabled={busy} onclick={() => act(async () => { status = await readLifecycle() })}>{t.recovery}</button>
    {#if status}
      {#if status.recovery.length === 0}<p>{t.ready}</p>{/if}
      {#each status.recovery as item (item.account)}
        <p>{item.account} · {t[item.state]}</p>
        {#if item.state !== 'forgotten'}<button disabled={busy} onclick={() => act(async () => { await recoverAccount(item.account); await changed() })}>{t.recover} · {item.account}</button>{/if}
      {/each}
    {/if}
    {#if account}
      <p>{account}</p>
      <form onsubmit={(event) => { event.preventDefault(); void replace() }}>
        <label>{t.password}<input type="password" bind:value={password} autocomplete="off" maxlength="1024" required disabled={busy} /></label>
        <button type="submit" disabled={busy || !password}>{t.replace}</button>
      </form>
      <label><input type="checkbox" bind:checked={rotationConfirmed} disabled={busy} />{t.rotateConfirm}</label>
      <button disabled={busy || !rotationConfirmed} onclick={() => act(async () => { await rotateAccount(account); await changed() })}>{t.rotate}</button>
      <button disabled={busy} onclick={() => act(async () => { invalidate(); preview = await previewForget(account) })}>{t.preview}</button>
      {#if preview}
        <div class="lifecycle-review">
          <p>{preview.account} · {t.revision} {preview.revision} · {t.generation} {preview.generation}</p>
          <p>{preview.consequences[lang]}</p><p>{t.resources}</p>
          <ul>{#each preview.resources as resource (resource.path)}<li>{resource.path} · {resource.bytes} B · {resource.digest}</li>{/each}</ul>
          <p>{t.credentials}</p><ul>{#each preview.credential_references as reference}<li>{reference}</li>{/each}</ul>
          <p>{preview.receipt}</p>
          <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label>
          <button disabled={busy || !confirmed} onclick={() => act(async () => { if (preview) { await forgetAccount(account, preview); await changed() } })}>{t.forget}</button>
        </div>
      {/if}
    {/if}
  {/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</details>
<style>
  .lifecycle { margin-block: 1rem; }
  .lifecycle-review { padding: 1rem; border: 1px solid currentColor; overflow-wrap: anywhere; }
  form, label { display: grid; gap: .4rem; margin-block: .7rem; }
  input[type="checkbox"] { justify-self: start; }
  button { margin: .3rem; }
</style>
