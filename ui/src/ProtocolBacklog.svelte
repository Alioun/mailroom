<script lang="ts">
  import { onMount } from 'svelte'
  import ArchiveOffer from './ArchiveOffer.svelte'
  import CopyPhaseEvidence from './CopyPhaseEvidence.svelte'
  import { archiveCopy, archiveReason, archiveOfferAvailable } from './archive-copy'
  import { readAccountStatus, openProtocolBacklog, previewProtocolBacklog, previewProtocolUndo, protocolAction, type ProtocolView, type ProtocolPreview, type ProtocolUndo, type BacklogOptions } from './api'
  import UndoRetention from './UndoRetention.svelte'
  import CloseRun from './CloseRun.svelte'
  let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()
  const text = {
    en: { copyHold: 'COPY history is preserved. Execution, reconciliation and Undo are unavailable in this build.', title: 'Backlog sorting', source: 'Synthetic protocol mailbox. Real-account writes remain unavailable. Recipient evidence is installed separately.', preview: 'Preview exact protocol changes', confirm: 'I confirm these exact UID MOVE changes', start: 'Sort protocol backlog', reload: 'Reload saved protocol runs', cancel: 'Cancel sorting', reconcile: 'Reconcile observations only', resume: 'Resume never-attempted remainder', undoPreview: 'Preview conditional undo', undoConfirm: 'I confirm these exact reverse UID MOVE changes', undo: 'Undo eligible protocol changes', resumeUndo: 'Resume confirmed reverse remainder', scope: 'Scope', inbox: 'Inbox only', unfiled: 'Inbox plus unfiled', every: 'Advanced: every folder', protect: 'Protect flagged messages', changes: 'eligible changes', conflicts: 'conflicts', report: 'Exact local identities and protocol evidence', journal: 'Encrypted protocol journal', error: 'Action rejected. Reload the saved journal and request a fresh preview. Existing evidence is retained.', wait: 'Worker active. Cancel stops new batches and retains completed changes. A transmitted command may remain uncertain.', stopped: 'Stop is saved. Resume is explicit. Undo requires a separate preview and confirmation.', mapping: 'Validated COPYUID mapping', completion: 'Command completion', readback: 'Identity readback saved', attempted: 'Attempted', pending: 'Never attempted', hold: 'Restored account is held. No source or approval is restored.', approval: 'Saved proof', observation: 'Fresh observation proof', empty: 'No protocol runs.' },
    de: { copyHold: 'COPY-Verlauf bleibt erhalten. Ausführung, Abgleich und Rückgängigmachen sind in dieser Version nicht verfügbar.', title: 'Bestand sortieren', source: 'Synthetisches Protokollpostfach. Schreibzugriff auf echte Konten bleibt gesperrt. Empfängernachweise werden separat installiert.', preview: 'Genaue Protokolländerungen anzeigen', confirm: 'Ich bestätige diese genauen UID-MOVE-Änderungen', start: 'Protokollbestand sortieren', reload: 'Gespeicherte Protokollabläufe laden', cancel: 'Sortierung abbrechen', reconcile: 'Nur Beobachtungen abgleichen', resume: 'Nie versuchten Rest fortsetzen', undoPreview: 'Bedingtes Rückgängigmachen prüfen', undoConfirm: 'Ich bestätige diese genauen umgekehrten UID-MOVE-Änderungen', undo: 'Geeignete Protokolländerungen rückgängig machen', resumeUndo: 'Bestätigten umgekehrten Rest fortsetzen', scope: 'Umfang', inbox: 'Nur Posteingang', unfiled: 'Posteingang und unsortierte Nachrichten', every: 'Erweitert: alle Ordner', protect: 'Markierte Nachrichten schützen', changes: 'geeignete Änderungen', conflicts: 'Konflikte', report: 'Genaue lokale Identitäten und Protokollnachweise', journal: 'Verschlüsseltes Protokolljournal', error: 'Aktion abgelehnt. Journal neu laden und frische Vorschau anfordern. Vorhandene Nachweise bleiben erhalten.', wait: 'Ablauf aktiv. Abbrechen stoppt neue Stapel und behält abgeschlossene Änderungen. Ein übertragener Befehl kann ungewiss bleiben.', stopped: 'Stopp gespeichert. Fortsetzen erfolgt ausdrücklich. Rückgängigmachen benötigt eine eigene Vorschau und Bestätigung.', mapping: 'Geprüfte COPYUID-Zuordnung', completion: 'Befehlsabschluss', readback: 'Identitätsprüfung gespeichert', attempted: 'Versucht', pending: 'Nie versucht', hold: 'Wiederhergestelltes Konto bleibt gesperrt. Quelle und Freigabe werden nicht wiederhergestellt.', approval: 'Gespeicherter Nachweis', observation: 'Nachweis aus frischer Beobachtung', empty: 'Keine Protokollabläufe.' },
  }
  let t = $derived(text[lang])
  function copyHeld(run: ProtocolView['journal']['runs'][number]) { return run.batches.some(batch => batch.copy_journal !== null) }
  let archiveText = $derived(archiveCopy(lang))
  let scope = $state<BacklogOptions['scope']>('inbox_unfiled')
  let protect = $state(true)
  let view = $state<ProtocolView | null>(null)
  let preview = $state<ProtocolPreview | null>(null)
  let undo = $state<ProtocolUndo | null>(null)
  let confirmed = $state(false)
  let undoConfirmed = $state(false)
  let busy = $state(false)
  let running = $state(false)
  let savedCount = $state(0)
  let cancelling = $state(false)
  let error = $state('')
  let active = true
  function invalidate() { preview = null; undo = null; confirmed = false; undoConfirmed = false }
  $effect(() => { workspaceRevision; invalidate() })
  async function act(fn: () => Promise<void>) { busy = true; error = ''; try { await fn() } catch { error = t.error } finally { busy = false } }
  async function load() { invalidate(); view = await openProtocolBacklog(account) }
  async function plan(selected: string[] | null = null, intent: NonNullable<BacklogOptions['intent']> = 'sort') { invalidate(); const revision = workspaceRevision; const result = await previewProtocolBacklog(account, { scope: intent === 'archive_unmatched' ? 'inbox' : scope, protect_flagged: protect, selected, intent }); if (revision === workspaceRevision) preview = result }
  async function select(name: string, checked: boolean) {
    if (!preview) return
    const selected = Object.entries(preview.operation.plan.filing.groups).filter(([key, group]) => key === name ? checked : group.selected).map(([key]) => key)
    await plan(selected, preview.operation.plan.filing.options.intent)
  }
  async function watch() {
    running = true
    try {
      const deadline = Date.now() + 150000
      while (active && running) {
        if (Date.now() > deadline) throw new Error(t.error)
        await new Promise(resolve => setTimeout(resolve, 250))
        const status = await readAccountStatus()
        savedCount = status.job?.progress.saved ?? 0
        if (status.job?.progress.state !== 'running') { view = await openProtocolBacklog(account); break }
      }
    } finally { running = false }
  }
  async function start() {
    if (!preview || !confirmed) return
    const command = { action: 'confirm', options: preview.operation.plan.filing.options, receipt: preview.receipt, confirmed: true } satisfies Parameters<typeof protocolAction>[1]
    invalidate(); await protocolAction(account, command); await watch()
  }
  async function continuation(action: 'resume' | 'resume_undo', run: string) { invalidate(); await protocolAction(account, { action, run }); await watch() }
  async function cancel() {
    cancelling = true; error = ''; invalidate()
    try { await protocolAction(account, { action: 'cancel' }); running = false; await load() } catch { error = t.error } finally { cancelling = false }
  }
  async function inspectUndo(run: string) { invalidate(); const revision = workspaceRevision; const result = await previewProtocolUndo(account, run); if (revision === workspaceRevision) undo = result }
  async function reverse() {
    if (!undo || !undoConfirmed) return
    const command = { action: 'undo', run: undo.run, receipt: undo.receipt, confirmed: true } satisfies Parameters<typeof protocolAction>[1]
    invalidate(); await protocolAction(account, command); await watch()
  }
  onMount(() => { active = true; void act(load); return () => { active = false; running = false } })
</script>
<section class="protocol-backlog" aria-label={t.title}>
  <h3>{t.title}</h3><p>{t.source}</p>
  {#if view?.recovery_hold}<p role="status">{t.hold}</p>{/if}
  <label>{t.scope}<select bind:value={scope} disabled={busy || running} onchange={invalidate}><option value="inbox_unfiled">{t.unfiled}</option><option value="inbox">{t.inbox}</option><option value="every_folder">{t.every}</option></select></label>
  <label><input type="checkbox" bind:checked={protect} disabled={busy || running} onchange={invalidate} />{t.protect}</label>
  <div class="actions"><button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(() => plan())}>{t.preview}</button><button disabled={busy || running} onclick={() => act(load)}>{t.reload}</button><button disabled={cancelling || !view?.installed || view?.recovery_hold} onclick={cancel}>{t.cancel}</button></div>
  {#if archiveOfferAvailable(!!view?.journal.runs.some(run => run.status === 'complete'), preview?.operation.plan.filing)}<ArchiveOffer {account} {lang} disabled={busy || running || !view?.installed || !!view?.recovery_hold} folders={true} onpreview={() => act(() => plan(null, 'archive_unmatched'))} />{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if running || cancelling}<p role="status">{t.wait} · {savedCount}</p>{/if}
  {#if preview}
    {#if preview.operation.plan.filing.options.intent === 'archive_unmatched'}<h4>{archiveText.title}</h4>{/if}
    <p>{preview.operation.plan.filing.changes.length} {t.changes}</p>
    <p>{preview.operation.plan.filing.artifact.identity}</p>
    <p>{t.approval} · {preview.operation.plan.saved_proof}</p><p>{t.observation} · {preview.operation.plan.execution_proof}</p>
    {#each Object.entries(preview.operation.plan.filing.groups) as [name, group]}<label><input type="checkbox" checked={group.selected} disabled={busy} onchange={event => { const checked = event.currentTarget.checked; void act(() => select(name, checked)) }} />{name} · {group.count}</label>{/each}
    {#each Object.entries(preview.operation.plan.filing.excluded) as [reason, count]}<p>{archiveReason(lang, reason)} · {count}</p>{/each}
    {#each preview.operation.plan.filing.diagnostics as diagnostic}<p>{diagnostic}</p>{/each}
    <details><summary>{t.report}</summary><pre>{preview.operation.plan.filing.artifact.executable}</pre>{#each preview.operation.batches as b}<p>{b.intent.source.name.decoded} / {b.intent.source.uidvalidity} / {Object.keys(b.intent.messages).join(', ')} → {b.intent.destination.name.decoded} / {b.intent.destination.uidvalidity}</p>{/each}</details>
    <label><input type="checkbox" bind:checked={confirmed} disabled={busy || running} />{preview.operation.plan.filing.options.intent === 'archive_unmatched' ? archiveText.confirm : t.confirm}</label><button disabled={busy || running || !confirmed} onclick={() => act(start)}>{preview.operation.plan.filing.options.intent === 'archive_unmatched' ? archiveText.start : t.start}</button>
  {/if}
  <h4>{t.journal}</h4>
  {#each view?.journal.runs ?? [] as run (run.id)}
    <article data-protocol-run={run.id}>{#if run.plan.filing.options.intent === 'archive_unmatched'}<p>{archiveText.title}</p>{/if}<p class="run-state">{run.status}</p><UndoRetention {lang} days={view?.undo_window_days ?? 90} deadline={view?.undo_deadlines.find(d => d.run === run.id)} undone={run.status === 'undone'} />{#if run.stop_requested}<p>{t.stopped}</p>{/if}
      {#if copyHeld(run)}<p role="status">{t.copyHold}</p>{/if}
      {#if run.undo_receipt}<p>{run.undo_conflicts} {t.conflicts}</p>{/if}
      <CloseRun kind="protocol" {account} {run} {lang} closable={view?.closable.includes(run.id) ?? false} disabled={busy || running || copyHeld(run) || !view?.installed || !!view?.recovery_hold} onchange={() => act(load)} />
      {#if !run.closed}<button disabled={busy || running || copyHeld(run) || !view?.installed || view?.recovery_hold} onclick={() => act(async () => { invalidate(); await protocolAction(account, { action: 'reconcile', run: run.id }); await load() })}>{t.reconcile}</button>{/if}
      {#if !run.closed && !run.undo_receipt && !['complete', 'undone'].includes(run.status)}<button disabled={busy || running || copyHeld(run) || !view?.installed || view?.recovery_hold} onclick={() => act(() => continuation('resume', run.id))}>{t.resume}</button>{/if}
      {#if run.closed}{:else if !run.undo_receipt}<button disabled={busy || running || copyHeld(run) || !view?.installed || view?.recovery_hold || view?.undo_deadlines.find(d => d.run === run.id)?.undo_open === false} onclick={() => act(() => inspectUndo(run.id))}>{t.undoPreview}</button>{:else if run.status !== 'undone'}<button disabled={busy || running || copyHeld(run) || !view?.installed || view?.recovery_hold} onclick={() => act(() => continuation('resume_undo', run.id))}>{t.resumeUndo}</button>{/if}
      <details><summary>{t.report}</summary><p>{run.id}</p><p>{run.plan.filing.artifact.identity}</p>{#each [...run.batches, ...run.reverse.map(r => r.evidence)] as e}<p>{e.intent.source.name.decoded} / {e.intent.source.uidvalidity} / {Object.keys(e.intent.messages).join(', ')} → {e.intent.destination.name.decoded}</p><p>{e.attempted ? t.attempted : t.pending} · {t.completion} · {e.completion?.kind === 'tagged' ? e.completion.status : e.completion?.kind ?? '?'}</p>{#if e.mapping.kind === 'validated'}<p>{t.mapping} · {JSON.stringify(e.mapping.mapping.pairs)}</p>{/if}{#if e.readback}<p>{t.readback}</p>{/if}{#if e.copy_journal}<CopyPhaseEvidence evidence={e.copy_journal} {lang} />{/if}{/each}</details>
    </article>
  {:else}<p>{t.empty}</p>{/each}
  <UndoRetention {lang} days={view?.undo_window_days ?? 90} summaries={view?.journal.summaries} />
  {#if undo}<div class="confirmation"><p>{undo.reverse.reduce((n, r) => n + Object.keys(r.evidence.intent.messages).length, 0)} {t.changes} · {undo.conflicts} {t.conflicts}</p>{#each undo.reverse as r}<p>{r.evidence.intent.source.name.decoded} / {Object.keys(r.evidence.intent.messages).join(', ')} → {r.evidence.intent.destination.name.decoded}</p>{/each}<label><input type="checkbox" bind:checked={undoConfirmed} disabled={busy || running} />{t.undoConfirm}</label><button disabled={busy || running || !undoConfirmed || undo.reverse.length === 0} onclick={() => act(reverse)}>{t.undo}</button></div>{/if}
  <p>{view?.limitations ?? preview?.limitations}</p>
</section>
<style>
  .protocol-backlog { margin-block: 2rem; border-top: 1px solid #7775; padding-top: 1rem; }
  .actions, .confirmation { display: flex; flex-wrap: wrap; gap: .75rem; margin-block: 1rem; }
  article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  p { overflow-wrap: anywhere; }
  label { display: block; margin-block: .5rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
