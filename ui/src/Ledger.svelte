<script lang="ts">
  import { openLedger, reviewLedger, confirmLedger, type LedgerView, type LedgerEntry, type LedgerReview } from './ledger-api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  let view = $state<LedgerView | null>(null)
  let entries = $state<LedgerEntry[]>([])
  let enabled = $state(false)
  let review = $state<LedgerReview | null>(null)
  let confirmed = $state(false)
  let busy = $state(false)
  let message = $state('')
  const strings = {
    en: { shared: 'This file and its policy switch are shared across installed accounts. Saving changes their common source. Each account reconciles before proof and actions; account roles and recipient authority remain separate.', editedTime: 'An edited first_seen is owner-supplied. Its historical meaning is unknown unless you document it.', advanced: 'Advanced · Private address ledger', detail: 'The installed private YAML file is authoritative. This synthetic adapter saves local bookkeeping only. Save a rule workspace first. Recipient hints grant no ownership, role, service or invoice approval. Provider routing remains unavailable.', load: 'Reload ledger and reconcile', enable: 'Enable ledger rule projection', proposals: 'Discovery proposals', saved: 'Durable saved entries', edit: 'Add to exact batch', service: 'Service chosen by you', folder: 'Folder leaf beneath the classified root', first: 'first_seen value', note: 'Private note', status: 'Filing status', active: 'Active', retired: 'Retired', domains: 'Approved sender organisational domains, comma separated', evidence: 'Explicit service-association approval evidence', unknown: 'Unknown', temporal: 'A new first_seen proposal is the time this review generated the proposal. It is not Message Date, first receipt or first census observation. Existing supplied values are preserved.', hints: 'Every header below is an untrusted discovery hint. To/Cc and pooled report recipients are not delivery authority. Incomplete census results may omit addresses.', batch: 'Exact batch', remove: 'Remove from batch', preview: 'Review exact ledger batch', confirm: 'I approve exactly this local ledger batch and its displayed effects', save: 'Save confirmed ledger batch', done: 'Authoritative file saved and projection reconciled. Reload rules and run fresh proof.', stale: 'Ledger action rejected or interrupted. Reload and reconcile from the authoritative file, then review again. It may have saved before interruption. Do not blindly repeat an old confirmation.', leaks: 'Sender-domain observations', signal: 'A different From organisational domain is a review signal, not proof of abuse. Missing or uncomparable evidence remains unknown. Refresh does not broaden approved associations.', proof: 'Resulting local proof', retirement: 'Retirement changes filing only. It neither disables an alias nor unsubscribes. Accounting copies remain independent.', loaded: 'Ledger loaded from the private file.', timestamp: 'Message Date', account: 'Source account', provenance: 'Delivery authority unknown', policy: 'Policy disabled initially. Enabling projection grants no provider write permission.', proofLimit: 'This local proof keeps unknown recipient provenance in Inbox. Synthetic engine tests are separate from provider validation.', servicePrivacy: 'Addresses, associations and notes remain private. They are excluded from agent summaries.' },
    de: { shared: 'Diese Datei und ihr Richtlinienschalter gelten gemeinsam für installierte Konten. Speichern ändert die gemeinsame Quelle. Jedes Konto gleicht sie vor Prüfungen und Aktionen ab; Kontorollen und Zustellberechtigung bleiben getrennt.', editedTime: 'Ein bearbeiteter first_seen-Wert stammt vom Eigentümer. Seine historische Bedeutung ist ohne Dokumentation unbekannt.', advanced: 'Erweitert · Privates Adressverzeichnis', detail: 'Die installierte private YAML-Datei ist maßgeblich. Dieser synthetische Adapter speichert nur lokale Ablagedaten. Speichere zuerst einen Regelarbeitsstand. Empfängerhinweise bestätigen weder Zugehörigkeit, Rolle, Dienst noch Rechnungsfreigabe. Anbieter-Routing ist nicht verfügbar.', load: 'Adressverzeichnis laden und abgleichen', enable: 'Regelprojektion des Adressverzeichnisses aktivieren', proposals: 'Entdeckungsvorschläge', saved: 'Dauerhaft gespeicherte Einträge', edit: 'Zum genauen Stapel hinzufügen', service: 'Von dir gewählter Dienst', folder: 'Ordnername unter dem zugeordneten Stamm', first: 'first_seen-Wert', note: 'Private Notiz', status: 'Ablagestatus', active: 'Aktiv', retired: 'Stillgelegt', domains: 'Freigegebene organisatorische Absenderdomains, durch Komma getrennt', evidence: 'Ausdrücklicher Freigabebeleg für die Dienstzuordnung', unknown: 'Unbekannt', temporal: 'Ein neuer first_seen-Vorschlag nennt den Zeitpunkt der Vorschlagserstellung. Er ist weder Nachrichtendatum, erster Empfang noch erste Erfassungsbeobachtung. Vorhandene Werte bleiben erhalten.', hints: 'Jede Kopfzeile unten ist ein ungeprüfter Entdeckungshinweis. An/Cc und zusammengefasste Empfänger sind keine Zustellberechtigung. Eine unvollständige Erfassung kann Adressen auslassen.', batch: 'Genauer Stapel', remove: 'Aus Stapel entfernen', preview: 'Genauen Adressstapel prüfen', confirm: 'Ich genehmige genau diesen lokalen Adressstapel und die angezeigten Auswirkungen', save: 'Bestätigten Adressstapel speichern', done: 'Maßgebliche Datei gespeichert und Projektion abgeglichen. Regeln neu laden und frisch prüfen.', stale: 'Adressaktion abgelehnt oder unterbrochen. Maßgebliche Datei neu laden und abgleichen, dann erneut prüfen. Die Datei könnte bereits gespeichert sein. Alte Bestätigungen nicht blind wiederholen.', leaks: 'Beobachtete Absenderdomains', signal: 'Eine andere organisatorische Von-Domain ist ein Prüfhinweis, kein Missbrauchsbeleg. Fehlende oder unvergleichbare Belege bleiben unbekannt. Neuladen erweitert keine Freigabe.', proof: 'Resultierende lokale Prüfung', retirement: 'Stilllegung ändert nur die Ablage. Sie deaktiviert keinen Alias und meldet kein Abonnement ab. Buchhaltungskopien bleiben unabhängig.', loaded: 'Adressverzeichnis aus privater Datei geladen.', timestamp: 'Nachrichtendatum', account: 'Quellkonto', provenance: 'Zustellberechtigung unbekannt', policy: 'Die Richtlinie ist anfangs deaktiviert. Aktivieren erteilt keine Schreibberechtigung beim Anbieter.', proofLimit: 'Die lokale Prüfung hält Nachrichten bei unbekannter Empfängerherkunft im Posteingang. Synthetische Engine-Tests sind keine Anbieterprüfung.', servicePrivacy: 'Adressen, Zuordnungen und Notizen bleiben privat. Sie erscheinen nicht in Agentenzusammenfassungen.' },
  }
  let t = $derived(strings[lang])
  function invalidate() { review = null; confirmed = false; message = '' }
  function apply(next: LedgerView) { view = next; enabled = next.snapshot.document.enabled; entries = []; invalidate() }
  async function load() {
    busy = true; invalidate()
    try { apply(await openLedger(account)); message = t.loaded } catch { message = t.stale } finally { busy = false }
  }
  function add(entry: LedgerEntry) { if (!entries.some(e => e.address === entry.address)) entries = [...entries, structuredClone($state.snapshot(entry))]; invalidate() }
  async function preview() {
    if (!view) return
    busy = true; invalidate()
    try { review = await reviewLedger(account, { revision: view.revision, content_version: view.snapshot.content_version, enabled, entries }) } catch { message = t.stale } finally { busy = false }
  }
  async function save() {
    if (!review || !confirmed) return
    busy = true
    try { apply(await confirmLedger(account, review)); message = t.done } catch { invalidate(); message = t.stale } finally { busy = false }
  }
</script>

<details class="ledger"><summary>{t.advanced}</summary>
  <p>{t.detail}</p><p>{t.shared}</p><p>{t.servicePrivacy}</p><p>{t.policy}</p><p>{t.retirement}</p>
  <button onclick={load} disabled={busy}>{t.load}</button><p role="status">{message}</p>
  {#if view}
    <p>{view.census_state} · {view.snapshot.content_version}</p>
    <label><input type="checkbox" bind:checked={enabled} onchange={invalidate} disabled={busy} />{t.enable}</label>
    <h3>{t.saved} · {view.snapshot.document.entries.length}</h3>
    {#each view.snapshot.document.entries as entry (entry.address)}
      <article><p>{entry.address} · {entry.service} · {entry.folder} · {entry.status} · {entry.first_seen}</p><p>{entry.note}</p><button onclick={() => add(entry)} disabled={busy}>{t.edit} {entry.address}</button></article>
    {/each}
    <h3>{t.proposals} · {view.proposals.length}</h3><p>{t.hints}</p><p>{t.temporal}</p><p>{t.editedTime}</p>
    {#each view.proposals as proposal (proposal.address)}
      <details><summary>{proposal.address} · {t.unknown}</summary><p>{proposal.first_seen_proposal}</p>
        {#each proposal.evidence as evidence}<p>{t.account} {evidence.account} · {evidence.folder} · UIDVALIDITY {evidence.uidvalidity} · UID {evidence.uid} · {evidence.recipient_header} · {t.provenance} · {evidence.sender_domain ?? t.unknown} · {t.timestamp} {evidence.message_date ?? t.unknown}</p>{/each}
        <button disabled={busy} onclick={() => add({ address: proposal.address, service: '', folder: '', first_seen: proposal.first_seen_proposal, first_seen_meaning: proposal.first_seen_meaning, status: 'active', note: '', category: 'orders', invoice: false, association: null })}>{t.edit} {proposal.address}</button>
      </details>
    {/each}
    <h3>{t.batch} · {entries.length}</h3>
    {#each entries as entry (entry.address)}
      <fieldset disabled={busy} oninput={invalidate}><legend>{entry.address}</legend>
        <label>{t.service}<input bind:value={entry.service} autocomplete="off" /></label>
        <label>{t.folder}<input bind:value={entry.folder} autocomplete="off" /></label>
        <label>{t.first}<input bind:value={entry.first_seen} oninput={() => { entry.first_seen_meaning = 'Owner-supplied value in exact review. Historical meaning has not been independently established.'; invalidate() }} readonly={view.snapshot.document.entries.some(e => e.address === entry.address)} /></label>
        <label>{t.status}<select aria-label={t.status} bind:value={entry.status} onchange={invalidate}><option value="active">{t.active}</option><option value="retired">{t.retired}</option></select></label>
        <label>{t.note}<textarea bind:value={entry.note} autocomplete="off"></textarea></label>
        {#if entry.association}<p>{entry.association.sender_domains.join(', ')} · {entry.association.approved_evidence}</p>{:else}
          <label>{t.domains}<input data-domains={entry.address} autocomplete="off" /></label><label>{t.evidence}<input data-evidence={entry.address} autocomplete="off" /></label>
          <button onclick={(event) => { const fieldset = event.currentTarget.closest('fieldset'); const domains = fieldset?.querySelector('input[data-domains]'); const evidence = fieldset?.querySelector('input[data-evidence]'); if (domains instanceof HTMLInputElement && evidence instanceof HTMLInputElement && domains.value.trim() && evidence.value.trim()) { entry.association = { sender_domains: domains.value.split(',').map(v => v.trim()), approved_evidence: evidence.value }; invalidate() } }}>{lang === 'en' ? 'Include explicit association in review' : 'Ausdrückliche Zuordnung zur Prüfung hinzufügen'}</button>
        {/if}
        <button onclick={() => { entries = entries.filter(e => e.address !== entry.address); invalidate() }}>{t.remove}</button>
      </fieldset>
    {/each}
    <button onclick={preview} disabled={busy}>{t.preview}</button>
    {#if review}
      <div class="ledger-review"><h3>{t.batch}</h3><pre>{JSON.stringify(review.batch, null, 2)}</pre><p>{t.shared}</p><p>{t.retirement}</p><p>{t.proofLimit}</p>
        <details open><summary>{t.proof}</summary><p>{review.proof.proof.gate} · {review.proof.proof.artifact.identity}</p><pre>{JSON.stringify(review.proof.proof.counts, null, 2)}</pre>{#each review.proof.proof.warnings as warning}<p>{warning}</p>{/each}<pre>{review.resulting_overrides}</pre><pre>{review.proof.proof.artifact.executable}</pre></details>
        <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label><button onclick={save} disabled={busy || !confirmed}>{t.save}</button>
      </div>
    {/if}
    <h3>{t.leaks} · {view.leaks.length}</h3><p>{t.signal}</p><p>{t.unknown} · {view.unknown_sender_observations}</p>
    {#each view.leaks as leak}<p>{leak.address} · {leak.service} · {leak.approved.sender_domains.join(', ')} · {leak.observation.sender_domain ?? t.unknown} · {leak.observation.recipient_header} · UID {leak.observation.uid} · {leak.approved.approved_evidence}</p>{/each}
  {/if}
</details>

<style>
  .ledger { margin-block: 1.5rem; border: 1px solid var(--border, #777); padding: 1rem; }
  summary { cursor: pointer; }
  fieldset { min-inline-size: 0; display: grid; gap: .75rem; margin-block: 1rem; }
  label { display: grid; gap: .35rem; }
  label:has(input[type="checkbox"]) { display: flex; align-items: center; }
  input, textarea, select { box-sizing: border-box; min-width: 0; max-width: 100%; }
  legend { overflow-wrap: anywhere; max-width: 100%; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; }
  p { overflow-wrap: anywhere; }
</style>
