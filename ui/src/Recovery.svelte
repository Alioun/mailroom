<script lang="ts">
  import { onMount } from 'svelte'
  import { recoveryStatus, previewCapture, capture, previewRestore, restore, resumeRestore, type CaptureOffer, type RestoreOffer, type RecoveryStatus } from './recovery-api'
  let { account, synthetic, lang, onChanged }: { account: string; synthetic: boolean; lang: 'en' | 'de'; onChanged: () => Promise<void> } = $props()
  const text = {
    en: { title: 'Encrypted local recovery', denied: 'Production bundle creation and restoration are locked. Protected native launch and Keychain implementation are missing.', scope: 'Synthetic demonstration only. The installed fake adapter holds a separate invented recovery key. No secret enters this browser. One selected account per bundle. Provider passwords, external mail effects and shared addresses.yaml are excluded. Other accounts and current shared ledger edits survive.', preview: 'Preview exact capture', capture: 'Create this synthetic encrypted bundle', restorePreview: 'Authenticate and preview restore', restore: 'Restore this captured local account', confirm: 'I approve this exact synthetic recovery plan once', refresh: 'Refresh recovery status', pending: 'Interrupted local restore', resume: 'Resume authenticated local restore', captured: 'Capture boundary', resources: 'Exact captured inventory', keys: 'Decryption material inside encrypted bundle', excluded: 'Excluded resources', dependencies: 'Uncaptured comparison dependencies', authenticated: 'Local bundle authenticated', required: 'Fresh provider evidence and reconciliation required. Historical completion cannot authorize retry. No automatic send, unsubscribe or Undo. Daily readiness remains locked.', saved: 'Local recovery action completed. Provider authority remains locked.', error: 'Recovery rejected. Preserve existing files, bundle and separate secret. Refresh the preview or resume interrupted restoration.', bundles: 'Synthetic captured bundle', empty: 'No synthetic captured bundle.' },
    de: { title: 'Verschlüsselte lokale Wiederherstellung', denied: 'Produktive Sicherung und Wiederherstellung sind gesperrt. Geschützter nativer Start und Schlüsselbund-Implementierung fehlen.', scope: 'Nur synthetische Demonstration. Der installierte Testadapter hält einen separaten erfundenen Wiederherstellungsschlüssel. Kein Geheimnis gelangt in diesen Browser. Ein ausgewähltes Konto pro Sicherung. Anbieterpasswörter, externe Mail-Effekte und gemeinsame addresses.yaml sind ausgeschlossen. Andere Konten und aktuelle gemeinsame Ledger-Änderungen bleiben erhalten.', preview: 'Genauen Sicherungsumfang prüfen', capture: 'Diese synthetische Sicherung verschlüsselt erstellen', restorePreview: 'Authentifizieren und Wiederherstellung prüfen', restore: 'Dieses erfasste Konto lokal wiederherstellen', confirm: 'Ich erteile diese genaue synthetische Freigabe einmalig', refresh: 'Wiederherstellungsstatus aktualisieren', pending: 'Lokale Wiederherstellung unterbrochen', resume: 'Authentifizierte lokale Wiederherstellung fortsetzen', captured: 'Erfassungszeitpunkt', resources: 'Genaues erfasstes Inventar', keys: 'Schlüsselmaterial in der verschlüsselten Sicherung', excluded: 'Ausgeschlossene Ressourcen', dependencies: 'Nicht erfasste Vergleichskonten', authenticated: 'Lokale Sicherung authentifiziert', required: 'Frische Anbieterbelege und Abgleich erforderlich. Historischer Abschluss erlaubt keine Wiederholung. Kein automatischer Versand, keine Abmeldung oder Rücknahme. Alltagsfreigabe bleibt gesperrt.', saved: 'Lokale Wiederherstellungsaktion abgeschlossen. Anbieterfreigabe bleibt gesperrt.', error: 'Wiederherstellung abgelehnt. Vorhandene Dateien, Sicherung und separates Geheimnis erhalten. Vorschau aktualisieren oder unterbrochene Wiederherstellung fortsetzen.', bundles: 'Synthetische Sicherung', empty: 'Keine synthetische Sicherung vorhanden.' },
  }
  let t = $derived(text[lang])
  let status = $state<RecoveryStatus | null>(null)
  let offer = $state<{ kind: 'capture'; value: CaptureOffer } | { kind: 'restore'; value: RestoreOffer } | null>(null)
  let bundle = $state('')
  let busy = $state(false)
  let approved = $state(false)
  let message = $state('')
  let error = $state('')
  let manifest = $derived(offer?.kind === 'capture' ? offer.value.preview : offer?.value.preview.manifest)
  async function refresh() { status = await recoveryStatus(); bundle ||= status.bundles[0] ?? '' }
  async function act(action: () => Promise<void>) { busy = true; error = ''; message = ''; try { await action() } catch { error = t.error; offer = null; approved = false } finally { busy = false } }
  async function apply() {
    const selected = offer; offer = null; approved = false
    if (!selected) return
    await act(async () => { if (selected.kind === 'capture') bundle = (await capture(selected.value)).bundle; else { await restore(selected.value); await onChanged() }; await refresh(); message = t.saved })
  }
  $effect(() => { void account; offer = null; approved = false })
  onMount(() => { if (synthetic) void act(refresh) })
</script>
<details class="recovery">
  <summary>{t.title}</summary>
  {#if !synthetic}<p>{t.denied}</p>{:else}
    <p>{t.scope}</p><p>{t.required}</p>
    <button disabled={busy} onclick={() => act(refresh)}>{t.refresh}</button>
    {#each status?.pending ?? [] as id}<p>{t.pending} · {id}</p><button disabled={busy} onclick={() => act(async () => { await resumeRestore(id); await onChanged(); await refresh(); message = t.saved })}>{t.resume} · {id}</button>{/each}
    {#each status?.restored ?? [] as item}<div class="restored"><p>{item.account} · {t.captured} {new Date(item.captured_at * 1000).toISOString()}</p><p>{t.authenticated}</p><p>{t.resources}</p><ul>{#each item.resources as resource}<li>{resource.resource} · {resource.bytes} B · {resource.sha256}</li>{/each}</ul><p>{t.keys}</p><ul>{#each item.key_material as key}<li>{key.resource} · {key.bytes} B</li>{/each}</ul><p>{t.required}</p></div>{/each}
    <button disabled={busy || !account} onclick={() => act(async () => { approved = false; offer = { kind: 'capture', value: await previewCapture(account) } })}>{t.preview}</button>
    {#if status?.bundles.length}
      <label>{t.bundles}<select bind:value={bundle} disabled={busy} onchange={() => { offer = null; approved = false }}>{#each status.bundles as id}<option value={id}>{id}</option>{/each}</select></label>
      <button disabled={busy || !bundle} onclick={() => act(async () => { approved = false; offer = { kind: 'restore', value: await previewRestore(bundle) } })}>{t.restorePreview}</button>
    {:else}<p>{t.empty}</p>{/if}
    {#if offer && manifest}
      <div class="recovery-preview">
        <p>{manifest.account} · {manifest.provider} · {manifest.login}</p>
        <p>{t.captured} {new Date(manifest.captured_at * 1000).toISOString()} · v{manifest.state_schema} · {manifest.revision} · g{manifest.generation}</p>
        {#if offer.kind === 'restore'}<p>{t.authenticated}</p><p>{offer.value.preview.target}</p>{/if}
        <p>{t.resources}</p><ul>{#each manifest.resources as item}<li>{item.resource} · {item.bytes} B · {item.sha256}</li>{/each}</ul>
        <p>{t.keys}</p><ul>{#each manifest.key_material as key}<li>{key.resource} · {key.bytes} B</li>{/each}</ul>
        <p>{t.excluded}</p><ul>{#each manifest.excluded as item}<li>{item}</li>{/each}</ul>
        <p>{t.dependencies}</p><ul>{#each manifest.dependencies as item}<li>{item}</li>{/each}</ul>
        <p>{t.required}</p><pre>{offer.value.confirmation.display}</pre>
        <label><input type="checkbox" bind:checked={approved} disabled={busy} />{t.confirm}</label>
        <button disabled={busy || !approved} onclick={apply}>{offer.kind === 'capture' ? t.capture : t.restore}</button>
      </div>
    {/if}
  {/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</details>
<style>
  .recovery { margin-block: 1rem; overflow-wrap: anywhere; }
  .recovery-preview, .restored { border: 1px solid currentColor; padding: 1rem; margin-block: 1rem; }
  label { display: grid; gap: .4rem; margin-block: .7rem; }
  select { max-width: 100%; }
  input { justify-self: start; }
  button { margin: .3rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
