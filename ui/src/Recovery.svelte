<script lang="ts">
  import { onMount } from 'svelte'
  import { recoveryStatus, previewCapture, capture, previewRestore, restore, resumeRestore, resumeLegacyRestore, type CaptureOffer, type RestoreOffer, type RecoveryStatus } from './recovery-api'
  import type { Account } from './api'
  let { account, accounts, synthetic, lang, onChanged }: { account: string; accounts: Account[]; synthetic: boolean; lang: 'en' | 'de'; onChanged: () => Promise<void> } = $props()
  const text = {
    en: { title: 'Encrypted local recovery', denied: 'Backup and recovery for real accounts are locked. The protected installation and recovery workflow still need verification.', scope: 'Synthetic demonstration only. The installed fake adapter holds a separate invented recovery key. No secret enters this browser. Explicitly select up to eight accounts. Authoritative ledger inclusion is optional and initially unchecked. Provider passwords and external mail effects are excluded. Other accounts survive. A differing current owner ledger rejects restoration.', preview: 'Preview exact capture', capture: 'Create this synthetic encrypted bundle', restorePreview: 'Authenticate and preview restore', restore: 'Restore these captured local accounts', confirm: 'I approve this exact synthetic recovery plan once', refresh: 'Refresh recovery status', pending: 'Interrupted local restore', resume: 'Resume authenticated local restore', captured: 'Capture boundary', resources: 'Exact captured inventory', keys: 'Decryption material inside encrypted bundle', excluded: 'Excluded resources', dependencies: 'Comparison dependencies', inside: 'captured', outside: 'not captured', choose: 'Accounts to capture', includeLedger: 'Include installed authoritative addresses.yaml', restoreLedger: 'Restore captured authoritative addresses.yaml if absent or identical', ledger: 'Authoritative ledger', included: 'included', excludedLedger: 'excluded', absent: 'target absent', identical: 'target bytes identical', preserve: 'preserve current target', ledgerEffects: 'Exact owner-authored bytes, including notes and unknown fields. Shared across accounts. Current differing bytes are never overwritten.', excludedAccounts: 'Accounts excluded from this capture', authenticated: 'Local bundle authenticated', required: 'Fresh provider evidence and reconciliation required. Historical completion cannot authorize retry. No automatic send, unsubscribe or Undo. Daily readiness remains locked.', saved: 'Local recovery action completed. Provider authority remains locked.', error: 'Recovery rejected. Preserve existing files, bundle and separate secret. Refresh the preview or resume interrupted restoration.', bundles: 'Synthetic captured bundle', empty: 'No synthetic captured bundle.' },
    de: { title: 'Verschlüsselte lokale Wiederherstellung', denied: 'Sicherung und Wiederherstellung echter Konten sind gesperrt. Die geschützte Installation und der Wiederherstellungsablauf müssen noch geprüft werden.', scope: 'Nur synthetische Demonstration. Der installierte Testadapter hält einen separaten erfundenen Wiederherstellungsschlüssel. Kein Geheimnis gelangt in diesen Browser. Bis zu acht Konten ausdrücklich auswählen. Der gemeinsame maßgebliche Ledger ist optional und anfangs nicht ausgewählt. Anbieterpasswörter und externe Mail-Effekte sind ausgeschlossen. Andere Konten bleiben erhalten. Ein abweichender aktueller Ledger verhindert die Wiederherstellung.', preview: 'Genauen Sicherungsumfang prüfen', capture: 'Diese synthetische Sicherung verschlüsselt erstellen', restorePreview: 'Authentifizieren und Wiederherstellung prüfen', restore: 'Diese erfassten Konten lokal wiederherstellen', confirm: 'Ich erteile diese genaue synthetische Freigabe einmalig', refresh: 'Wiederherstellungsstatus aktualisieren', pending: 'Lokale Wiederherstellung unterbrochen', resume: 'Authentifizierte lokale Wiederherstellung fortsetzen', captured: 'Erfassungszeitpunkt', resources: 'Genaues erfasstes Inventar', keys: 'Schlüsselmaterial in der verschlüsselten Sicherung', excluded: 'Ausgeschlossene Ressourcen', dependencies: 'Vergleichskonten', inside: 'erfasst', outside: 'nicht erfasst', choose: 'Zu erfassende Konten', includeLedger: 'Installierte maßgebliche addresses.yaml einschließen', restoreLedger: 'Erfasste maßgebliche addresses.yaml nur bei fehlendem oder identischem Ziel wiederherstellen', ledger: 'Maßgeblicher Ledger', included: 'eingeschlossen', excludedLedger: 'ausgeschlossen', absent: 'Ziel fehlt', identical: 'Zielbytes identisch', preserve: 'Aktuelles Ziel erhalten', ledgerEffects: 'Genaue eigene Dateibytes einschließlich Notizen und unbekannter Felder. Gemeinsam für alle Konten. Abweichende aktuelle Bytes werden nie überschrieben.', excludedAccounts: 'Von dieser Erfassung ausgeschlossene Konten', authenticated: 'Lokale Sicherung authentifiziert', required: 'Frische Anbieterbelege und Abgleich erforderlich. Historischer Abschluss erlaubt keine Wiederholung. Kein automatischer Versand, keine Abmeldung oder Rücknahme. Alltagsfreigabe bleibt gesperrt.', saved: 'Lokale Wiederherstellungsaktion abgeschlossen. Anbieterfreigabe bleibt gesperrt.', error: 'Wiederherstellung abgelehnt. Vorhandene Dateien, Sicherung und separates Geheimnis erhalten. Vorschau aktualisieren oder unterbrochene Wiederherstellung fortsetzen.', bundles: 'Synthetische Sicherung', empty: 'Keine synthetische Sicherung vorhanden.' },
  }
  let t = $derived(text[lang])
  let status = $state<RecoveryStatus | null>(null)
  let offer = $state<{ kind: 'capture'; value: CaptureOffer } | { kind: 'restore'; value: RestoreOffer } | null>(null)
  let bundle = $state('')
  let selectedAccounts = $state<string[]>([])
  let includeLedger = $state(false)
  let restoreLedger = $state(false)
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
  $effect(() => { void account; void selectedAccounts; void includeLedger; void restoreLedger; offer = null; approved = false })
  onMount(() => { if (synthetic) void act(refresh) })
</script>
<details class="recovery">
  <summary>{t.title}</summary>
  {#if !synthetic}<p>{t.denied}</p>{:else}
    <p>{t.scope}</p><p>{t.required}</p>
    <button disabled={busy} onclick={() => act(refresh)}>{t.refresh}</button>
    {#if status?.selected_pending.length}<div class="selected-pending"><p>{t.pending} · {status.selected_pending.join(', ')}</p><button disabled={busy} onclick={() => act(async () => { await resumeRestore(); await onChanged(); await refresh(); message = t.saved })}>{t.resume}</button></div>{/if}
    {#each status?.pending ?? [] as id}<p>{t.pending} · {id}</p><button disabled={busy} onclick={() => act(async () => { await resumeLegacyRestore(id); await onChanged(); await refresh(); message = t.saved })}>{t.resume} · {id}</button>{/each}
    {#each status?.restored ?? [] as item}<div class="restored"><p>{item.account} · {t.captured} {new Date(item.captured_at * 1000).toISOString()}</p><p>{t.authenticated}</p><p>{t.resources}</p><ul>{#each item.resources as resource}<li>{resource.resource} · {resource.bytes} B · {resource.sha256}</li>{/each}</ul><p>{t.keys}</p><ul>{#each item.key_material as key}<li>{key.resource} · {key.bytes} B</li>{/each}</ul><p>{t.required}</p></div>{/each}
    <fieldset disabled={busy}><legend>{t.choose}</legend>{#each accounts as a (a.id)}<label><input type="checkbox" value={a.id} bind:group={selectedAccounts} />{a.login} · {a.provider} · {a.id}</label>{/each}</fieldset>
    <label><input class="ledger-inclusion" type="checkbox" bind:checked={includeLedger} disabled={busy} />{t.includeLedger}</label>
    <p>{t.ledgerEffects}</p>
    <button disabled={busy || selectedAccounts.length === 0 || selectedAccounts.length > 8} onclick={() => act(async () => { approved = false; offer = { kind: 'capture', value: await previewCapture(selectedAccounts, includeLedger) } })}>{t.preview}</button>
    {#if status?.bundles.length}
      <label>{t.bundles}<select bind:value={bundle} disabled={busy} onchange={() => { offer = null; approved = false }}>{#each status.bundles as id}<option value={id}>{id}</option>{/each}</select></label>
      <label><input class="ledger-restoration" type="checkbox" bind:checked={restoreLedger} disabled={busy} />{t.restoreLedger}</label>
      <button disabled={busy || !bundle} onclick={() => act(async () => { approved = false; offer = { kind: 'restore', value: await previewRestore(bundle, restoreLedger) } })}>{t.restorePreview}</button>
    {:else}<p>{t.empty}</p>{/if}
    {#if offer && manifest}
      <div class="recovery-preview">
        <p>{t.captured} {new Date(manifest.captured_at * 1000).toISOString()}</p>
        {#if offer.kind === 'restore'}<p>{t.authenticated}</p><p>{offer.value.preview.target}</p><p>{t.ledger} · {offer.value.preview.ledger_target.kind === 'absent' ? t.absent : offer.value.preview.ledger_target.kind === 'identical' ? t.identical : t.preserve}{#if offer.value.preview.ledger_target.kind !== 'absent' && offer.value.preview.ledger_target.version} · {offer.value.preview.ledger_target.version}{/if}</p>{/if}
        {#each manifest.accounts as a (a.account)}<div class="captured-account">
          <p>{a.account} · {a.provider} · {a.login} · v{a.state_schema} · {a.revision} · g{a.generation}</p>
          <p>{t.resources}</p><ul>{#each a.resources as item}<li>{item.resource} · {item.bytes} B · {item.sha256}</li>{/each}</ul>
          <p>{t.keys}</p><ul>{#each a.key_material as key}<li>{key.resource} · {key.bytes} B</li>{/each}</ul>
          <p>{t.dependencies}</p><ul>{#each a.dependencies as item}<li>{item} · {manifest.accounts.some(peer => peer.account === item) ? t.inside : t.outside}</li>{/each}</ul>
        </div>{/each}
        <p>{t.ledger} · {manifest.ledger.kind === 'included' ? t.included : t.excludedLedger}</p>
        {#if manifest.ledger.kind === 'included'}<p>{manifest.ledger.inventory.resource} · {manifest.ledger.inventory.bytes} B · {manifest.ledger.inventory.sha256}</p><p>v{manifest.ledger.schema} · {manifest.ledger.entries} · {manifest.ledger.semantics}</p>{:else}<p>{manifest.ledger.source_version ?? ''}</p>{/if}
        <p>{t.ledgerEffects}</p>
        <p>{t.excludedAccounts}</p><ul>{#each manifest.excluded_accounts as id}<li>{id}</li>{/each}</ul>
        <p>{t.excluded}</p><ul><li>provider_passwords</li><li>external_mailboxes_and_transports</li><li>uncaptured_changes</li></ul>
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
