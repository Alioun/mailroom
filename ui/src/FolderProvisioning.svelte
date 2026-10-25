<script lang="ts">
  import { onDestroy } from 'svelte'
  import { folderCommand, folderStatus, folderPreview, folderReconcile, type FolderView, type FolderPreview } from './folder-provisioning-api'
  import { localFetch } from './api'
  import { z } from 'zod'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { title: 'Synthetic IMAP folder provisioning', empty: 'The saved artifact emits no filing destinations. No CREATE is authorized by this review.', record: 'Record the empty folder plan', open: 'Open folder history', preview: 'Review exact folders', confirm: 'Create these synthetic folders', approval: 'I reviewed every exact decoded and wire name and the possible server effects.', cancel: 'Stop further folder commands', reconcile: 'Reconcile folders without creating', existing: 'Existing', missing: 'Missing, CREATE planned', ancestor: 'Ancestor', leaf: 'Filing leaf', decoded: 'Exact folder', wire: 'IMAP wire name', private: 'Show private folder names', hide: 'Hide private folder names', ack: 'Acknowledged', readback: 'Readback', error: 'Folder operation rejected or interrupted. Open history and reconcile before renewed exact review.', running: 'Synthetic folder operation running', proof: 'Saved-workspace proof', synthetic: 'Explicit synthetic recipient evidence installed. This fixture fact grants no provider trust.', conservative: 'Recipient evidence is unknown. Filing remains conservative.', held: 'Restored account remains read-only.', unavailable: 'Production and read-only installations deny folder creation. A separately installed synthetic create source is required.', limitations: 'Emitted filing destinations need not be reachable under saved-account recipient authority. Creating visible folders proves neither delivery nor attribution. The supported mapping uses an empty personal prefix and slash hierarchy. Another editor can remove a parent after the final check; the server may recreate parents or subscribe folders implicitly. No automatic retry, continuation, cleanup, rule deployment or native confirmation.', fresh: 'Uncertainty or absence requires explicit reconciliation and renewed exact approval. Reconciliation never creates folders.', yes: 'Yes', no: 'No', none: 'Not observed', statuses: { running: 'Running', complete: 'Visible postconditions observed', refused: 'Refused', uncertain: 'Uncertain', stopped: 'Stopped', conflict: 'Conflict', fresh_review: 'Fresh exact review required' }, outcomes: { intent: 'Intent persisted', acknowledged: 'Command acknowledged', observed: 'Visible state observed', refused: 'Refused', uncertain: 'Uncertain', conflict: 'Conflict', absent: 'Absent now, history unknown' } },
    de: { title: 'Synthetische IMAP-Ordneranlage', empty: 'Das gespeicherte Artefakt enthält keine Ablageziele. Diese Prüfung erlaubt keinen CREATE-Befehl.', record: 'Leeren Ordnerplan festhalten', open: 'Ordnerverlauf öffnen', preview: 'Exakte Ordner prüfen', confirm: 'Diese synthetischen Ordner anlegen', approval: 'Ich habe alle exakten Ordnernamen, Übertragungsnamen und möglichen Serverfolgen geprüft.', cancel: 'Weitere Ordnerbefehle stoppen', reconcile: 'Ordner ohne Anlage abgleichen', existing: 'Vorhanden', missing: 'Fehlt, CREATE geplant', ancestor: 'Übergeordneter Ordner', leaf: 'Ablageziel', decoded: 'Exakter Ordner', wire: 'IMAP-Übertragungsname', private: 'Private Ordnernamen anzeigen', hide: 'Private Ordnernamen ausblenden', ack: 'Bestätigt', readback: 'Rücklesen', error: 'Ordneranlage abgelehnt oder unterbrochen. Verlauf öffnen und vor erneuter exakter Prüfung abgleichen.', running: 'Synthetische Ordneranlage läuft', proof: 'Nachweis des gespeicherten Arbeitsstands', synthetic: 'Explizite synthetische Empfängerbelege installiert. Dieser Testbeleg schafft kein Anbietervertrauen.', conservative: 'Empfängerbelege sind unbekannt. Die Ablage bleibt konservativ.', held: 'Wiederhergestelltes Konto bleibt schreibgeschützt.', unavailable: 'Produktive und reine Leseinstallationen verweigern die Ordneranlage. Eine separat installierte synthetische Anlagequelle ist erforderlich.', limitations: 'Ausgegebene Ablageziele müssen mit der konservativen Empfängerberechtigung nicht erreichbar sein. Sichtbare Ordner beweisen weder Zustellung noch Verursachung. Unterstützt werden ein leeres persönliches Präfix und Schrägstriche. Andere Editoren können Elternordner nach der letzten Prüfung löschen. Der Server kann sie neu anlegen oder Ordner automatisch abonnieren. Keine automatische Wiederholung, Fortsetzung, Bereinigung, Regelaktivierung oder native Bestätigung.', fresh: 'Ungewissheit oder Abwesenheit erfordern ausdrücklichen Abgleich und erneute exakte Freigabe. Der Abgleich legt keine Ordner an.', yes: 'Ja', no: 'Nein', none: 'Nicht beobachtet', statuses: { running: 'Läuft', complete: 'Sichtbare Nachbedingungen beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', stopped: 'Gestoppt', conflict: 'Konflikt', fresh_review: 'Erneute exakte Prüfung erforderlich' }, outcomes: { intent: 'Absicht gespeichert', acknowledged: 'Befehl bestätigt', observed: 'Sichtbarer Zustand beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', conflict: 'Konflikt', absent: 'Jetzt abwesend, Verlauf ungewiss' } },
  }
  let t = $derived(strings[lang])
  let view = $state<FolderView | null>(null)
  let preview = $state<FolderPreview | null>(null)
  let approved = $state(false)
  let show = $state(false)
  let busy = $state(false)
  let running = $state(false)
  let error = $state('')
  let alive = true
  let timer: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => { alive = false; clearTimeout(timer); preview = null; view = null })
  async function poll() {
    try {
      const response = await localFetch('/api/accounts/status')
      const status = z.object({ job: z.object({ progress: z.object({ state: z.string() }) }).nullable() }).parse(await response.json())
      if (!alive) return
      if (status.job?.progress.state === 'running') timer = setTimeout(() => void poll(), 200)
      else { view = await folderStatus(account); running = false }
    } catch { if (alive) { error = t.error; running = false } }
  }
  async function act(action: 'open' | 'preview' | 'confirm' | 'cancel' | 'reconcile', operation?: string) {
    busy = true; error = ''; show = false
    try {
      if (action === 'confirm' && preview && approved) {
        const p = preview; preview = null; approved = false
        await folderCommand(account, { action: 'confirm', receipt: p.receipt, confirmed: true })
        running = true; void poll()
      } else if (action === 'cancel') await folderCommand(account, { action: 'cancel' })
      else if (action === 'reconcile' && operation) { preview = null; approved = false; view = await folderReconcile(account, operation) }
      else if (action === 'preview') { approved = false; preview = await folderPreview(account) }
      else if (action === 'open') { preview = null; approved = false; view = await folderStatus(account) }
    } catch { error = t.error; preview = null; approved = false }
    finally { busy = false }
  }
</script>
<section class="folder-provisioning" aria-label={t.title}>
  <h3>{t.title}</h3><p>{t.unavailable}</p>
  <div class="actions"><button disabled={!account || busy || running} onclick={() => act('open')}>{t.open}</button><button disabled={!view?.installed || view.recovery_hold || busy || running} onclick={() => act('preview')}>{t.preview}</button><button disabled={!running || busy} onclick={() => act('cancel')}>{t.cancel}</button></div>
  {#if error}<p role="alert">{error}</p>{/if}{#if running}<p role="status">{t.running}</p>{/if}
  {#if view?.recovery_hold}<p>{t.held}</p>{/if}
  {#if view || preview}<button onclick={() => show = !show}>{show ? t.hide : t.private}</button>{/if}
  {#if preview}
    <h4>{t.preview}</h4><p>{preview.proof.recipient_evidence === 'unknown' ? t.conservative : t.synthetic}</p><p>{preview.proof.recipient_evidence} · {preview.proof.capabilities.join(', ')}</p>{#if preview.operation.plan.entries.length === 0}<p>{t.empty}</p>{/if}<p>{t.proof} · {preview.proof.gate} · {preview.proof.semantics}</p>
    <ul>{#each preview.proof.warnings as warning}<li>{warning}</li>{/each}{#each preview.proof.pins as pin}<li>{pin.message_id_hash} · {pin.status}</li>{/each}</ul><p>{preview.proof.runtime_cross_check}</p><p>SHA-256 {preview.operation.artifact}</p>
    <ol>{#each preview.operation.plan.entries as entry}<li><p>{entry.role === 'leaf' ? t.leaf : t.ancestor} · {entry.existing ? t.existing : t.missing}</p>{#if show}<p>{t.decoded} <code>{entry.name.decoded}</code></p><p>{t.wire} <code>{entry.name.wire}</code></p>{#if entry.existing}<p>{entry.existing.attributes.join(' ')}</p>{/if}{/if}</li>{/each}</ol>
    <label><input type="checkbox" bind:checked={approved} />{t.approval}</label><button disabled={!show || !approved || busy || running} onclick={() => act('confirm')}>{preview.operation.plan.entries.length ? t.confirm : t.record}</button>
  {/if}
  {#if view}{#each view.journal.history as operation}<article><h4>{t.statuses[operation.status]}</h4><p>{operation.id}</p><ol>{#each operation.steps as step}<li>{#if show}<code>{step.name.decoded}</code> · <code>{step.name.wire}</code> · {/if}{t.outcomes[step.outcome]} · {t.ack} {step.acknowledgment ? t.yes : t.no} · {t.readback} {step.observed ? t.yes : t.none} {step.failure ?? ''}</li>{/each}</ol><button disabled={busy || running || view.recovery_hold} onclick={() => act('reconcile', operation.id)}>{t.reconcile}</button></article>{/each}{/if}
  <p>{t.fresh}</p><p>{t.limitations}</p>
</section>
<style>
  section { margin-block: 1rem; padding: 1rem; border: 1px solid #879397; border-radius: .4rem; overflow-wrap: anywhere; }
  .actions { display: flex; flex-wrap: wrap; gap: .5rem; }
  article { border-top: 1px solid #879397; margin-top: 1rem; padding-top: .6rem; }
  label { display: block; margin-block: 1rem; }
  code { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
