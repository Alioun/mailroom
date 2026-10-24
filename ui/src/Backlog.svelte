<script lang="ts">
  import { onMount } from 'svelte'
  import { openBacklog, previewBacklog, startBacklog, advanceBacklog, resumeBacklog, previewBacklogUndo, undoBacklog, resumeBacklogUndo, type BacklogOptions, type BacklogPreview, type BacklogView, type BacklogUndoPreview } from './api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { title: 'Backlog sorting', source: 'Simulation changes only the installed synthetic mailbox. Real-account execution is unavailable. Flags stay unchanged. No accounting copies are sent.', scope: 'Scope', inbox: 'Inbox only', unfiled: 'Inbox plus unfiled', every: 'Advanced: every folder', protect: 'Protect flagged messages', preview: 'Preview exact backlog changes', confirm: 'I confirm these exact synthetic mailbox changes', start: 'Sort synthetic backlog', reload: 'Reload saved backlog runs', resume: 'Resume saved synthetic run', undoPreview: 'Preview conditional undo', undoConfirm: 'I confirm this conditional undo', undo: 'Undo eligible synthetic changes', resumeUndo: 'Resume confirmed undo', changes: 'eligible changes', unmatched: 'unmatched', review: 'need review', exclusions: 'Exclusions', stopped: 'Folders the engine stops filling', detail: 'Exact local changes and identities', runs: 'Encrypted run journal', applied: 'applied', restored: 'restored', conflict: 'conflicts', eligible: 'eligible for undo', pending: 'Cancel behavior awaits the owner decision. Stop and undo are independent server operations. No Cancel action is wired.', error: 'Action rejected. Refresh the preview or recover the saved run. Existing files are retained.', conditional: 'Undo restores only effects still eligible at their recorded identity and destination. It preserves unrelated labels and current flags.', labels: 'Original labels', flags: 'Preserved flags', add: 'Add labels', remove: 'Remove labels', reports: 'Local run report', completed: 'completed', running: 'running', prepared: 'prepared', stoppedState: 'stopped', uncertain: 'uncertain', failed: 'failed' },
    de: { title: 'Bestand sortieren', source: 'Die Simulation ändert nur das installierte synthetische Postfach. Echte Konten können nicht ausgeführt werden. Flags bleiben erhalten. Keine Buchhaltungskopien werden gesendet.', scope: 'Umfang', inbox: 'Nur Posteingang', unfiled: 'Posteingang und unsortierte Nachrichten', every: 'Erweitert: alle Ordner', protect: 'Markierte Nachrichten schützen', preview: 'Genaue Bestandsänderungen anzeigen', confirm: 'Ich bestätige diese genauen synthetischen Postfachänderungen', start: 'Synthetischen Bestand sortieren', reload: 'Gespeicherte Sortierungen laden', resume: 'Gespeicherte synthetische Sortierung fortsetzen', undoPreview: 'Bedingtes Rückgängigmachen prüfen', undoConfirm: 'Ich bestätige dieses bedingte Rückgängigmachen', undo: 'Geeignete synthetische Änderungen rückgängig machen', resumeUndo: 'Bestätigtes Rückgängigmachen fortsetzen', changes: 'geeignete Änderungen', unmatched: 'ohne Zuordnung', review: 'zu prüfen', exclusions: 'Ausnahmen', stopped: 'Ordner, die die Engine nicht mehr füllt', detail: 'Genaue lokale Änderungen und Identitäten', runs: 'Verschlüsseltes Ablaufjournal', applied: 'angewendet', restored: 'wiederhergestellt', conflict: 'Konflikte', eligible: 'zum Rückgängigmachen geeignet', pending: 'Das Verhalten von Abbrechen wartet auf die Entscheidung des Inhabers. Anhalten und Rückgängigmachen sind getrennte Serveraktionen. Es gibt noch keine Aktion Abbrechen.', error: 'Aktion abgelehnt. Vorschau erneuern oder gespeicherte Sortierung fortsetzen. Vorhandene Dateien bleiben erhalten.', conditional: 'Rückgängigmachen stellt nur weiterhin geeignete Änderungen an der gespeicherten Identität und am Ziel wieder her. Andere Labels und aktuelle Flags bleiben erhalten.', labels: 'Ursprüngliche Labels', flags: 'Erhaltene Flags', add: 'Labels hinzufügen', remove: 'Labels entfernen', reports: 'Lokaler Ablaufbericht', completed: 'abgeschlossen', running: 'läuft', prepared: 'vorbereitet', stoppedState: 'angehalten', uncertain: 'ungewiss', failed: 'fehlgeschlagen' },
  }
  let t = $derived(strings[lang])
  let scope = $state<BacklogOptions['scope']>('inbox_unfiled')
  let protect = $state(true)
  let preview = $state<BacklogPreview | null>(null)
  let view = $state<BacklogView | null>(null)
  let confirmation = $state(false)
  let undoConfirmation = $state(false)
  let undoPreview = $state<BacklogUndoPreview | null>(null)
  let undoRun = $state('')
  let busy = $state(false)
  let error = $state('')
  let active = true
  function invalidate() { preview = null; confirmation = false; undoPreview = null; undoConfirmation = false }
  async function act(fn: () => Promise<void>) { busy = true; error = ''; try { await fn() } catch { error = t.error } finally { busy = false } }
  async function load() { view = await openBacklog(account) }
  async function plan(selected: string[] | null = null) { confirmation = false; undoPreview = null; preview = await previewBacklog(account, { scope, protect_flagged: protect, selected }) }
  async function select(destination: string, checked: boolean) {
    if (!preview) return
    const selected = Object.entries(preview.plan.groups).filter(([name, group]) => name === destination ? checked : group.selected).map(([name]) => name)
    await plan(selected)
  }
  async function drive(id: string) {
    while (active) {
      view = await advanceBacklog(account, id)
      const run = view.runs.find(r => r.id === id)
      if (!run || run.status !== 'running') break
      await new Promise(resolve => setTimeout(resolve, 80))
    }
  }
  async function start() { if (!preview || !confirmation) return; view = await startBacklog(account, preview); const id = view.runs.at(-1)?.id; invalidate(); if (id) await drive(id) }
  async function inspectUndo(id: string) { undoConfirmation = false; undoRun = id; undoPreview = await previewBacklogUndo(account, id) }
  async function compensate() { if (!undoPreview || !undoConfirmation) return; view = await undoBacklog(account, undoRun, undoPreview.receipt); undoPreview = null; undoConfirmation = false }
  onMount(() => { active = true; void act(load); return () => { active = false } })
</script>
<section class="backlog" aria-label={t.title}>
  <h3>{t.title}</h3>
  <p>{t.source}</p>
  <label>{t.scope}<select bind:value={scope} disabled={busy} onchange={invalidate}><option value="inbox_unfiled">{t.unfiled}</option><option value="inbox">{t.inbox}</option><option value="every_folder">{t.every}</option></select></label>
  <label><input type="checkbox" bind:checked={protect} disabled={busy} onchange={invalidate} />{t.protect}</label>
  <div class="actions"><button disabled={busy} onclick={() => act(() => plan())}>{t.preview}</button><button disabled={busy} onclick={() => act(load)}>{t.reload}</button></div>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if preview}
    <p>{preview.plan.changes.length} {t.changes} · {preview.plan.unmatched} {t.unmatched} · {preview.plan.review} {t.review}</p>
    <p class="help">{preview.plan.source} · {preview.plan.artifact.identity}</p>
    {#each Object.entries(preview.plan.groups) as [destination, group] (destination)}
      <div class="destination"><label><input type="checkbox" checked={group.selected} disabled={busy} onchange={(event) => { const checked = event.currentTarget.checked; void act(() => select(destination, checked)) }} />{destination} · {group.count}</label><p>{group.top_senders.map(([sender, count]) => `${sender} (${count})`).join(', ')}</p></div>
    {/each}
    <h4>{t.exclusions}</h4>{#each Object.entries(preview.plan.excluded) as [reason, count]}<p>{reason} · {count}</p>{/each}
    <h4>{t.stopped}</h4><p>{preview.plan.stopped_folders.join(', ') || '0'}</p>
    {#each preview.plan.diagnostics as diagnostic}<p class="help">{diagnostic}</p>{/each}
    <details><summary>{t.detail}</summary>{#each preview.plan.changes as change}<p>{change.source.folder} / {change.source.uidvalidity} / {change.source.uid} → {change.destination} · {change.sender} · {t.flags}: {change.flags.join(', ')} · {change.operation.kind}</p><p>Message-ID hash {change.source.message_id_hash} · Gmail ID {change.source.gmail_id ?? '?'}</p><p>{t.labels}: {change.original_labels.join(', ')}</p>{#if change.operation.kind === 'gmail'}<p>{t.add}: {change.operation.add.join(', ')} · {t.remove}: {change.operation.remove.join(', ')}</p>{/if}{/each}</details>
    {#if preview.execution_available}<div class="confirmation"><label><input type="checkbox" bind:checked={confirmation} disabled={busy} />{t.confirm}</label><button class="primary" disabled={busy || !confirmation} onclick={() => act(start)}>{t.start}</button></div>{/if}
  {/if}
  <h4>{t.runs}</h4>
  {#each view?.runs ?? [] as run (run.id)}
    <article data-run={run.id}>
      <p class="run-state">{run.status === 'stopped' ? t.stoppedState : t[run.status]} · {run.outcomes.filter(o => o.state === 'applied').length} / {run.outcomes.length} {t.applied} · {run.undo.filter(u => u.state === 'restored').length} {t.restored} · {run.outcomes.filter(o => o.state === 'conflict').length + run.undo.filter(u => u.state === 'conflict').length} {t.conflict}</p>
      {#if !run.undo_requested && ['prepared', 'running', 'stopped', 'uncertain'].includes(run.status)}<button disabled={busy} onclick={() => act(async () => { view = await resumeBacklog(account, run.id); await drive(run.id) })}>{t.resume}</button>{/if}
      {#if !run.undo_requested}<button disabled={busy} onclick={() => act(() => inspectUndo(run.id))}>{t.undoPreview}</button>{/if}
      {#if run.undo.some(u => u.state === 'intent')}<button disabled={busy} onclick={() => act(async () => { view = await resumeBacklogUndo(account, run.id) })}>{t.resumeUndo}</button>{/if}
      <details><summary>{t.reports}</summary><p>{run.id}</p>{#each run.plan.changes as change, i}<p>{change.source.folder} / {change.source.uid} → {change.destination} · {run.outcomes[i].state} · {run.undo[i].state}</p>{#if run.outcomes[i].state === 'applied'}<p>COPYUID / Gmail identity · {run.outcomes[i].observation.destination.folder} / {run.outcomes[i].observation.destination.uidvalidity} / {run.outcomes[i].observation.destination.uid}</p>{/if}{#if run.outcomes[i].state === 'conflict'}<p>{run.outcomes[i].reason}</p>{/if}{#if run.undo[i].state === 'conflict'}<p>{run.undo[i].reason}</p>{/if}{/each}</details>
    </article>
  {/each}
  {#if undoPreview}<div class="confirmation"><p>{t.conditional}</p><p>{undoPreview.eligible.length} {t.eligible} · {Object.keys(undoPreview.conflicts).length} {t.conflict}</p>{#each Object.entries(undoPreview.conflicts) as [index, reason]}<p>{index} · {reason}</p>{/each}<label><input type="checkbox" bind:checked={undoConfirmation} disabled={busy} />{t.undoConfirm}</label><button disabled={busy || !undoConfirmation} onclick={() => act(compensate)}>{t.undo}</button></div>{/if}
  <p class="help">{t.pending}</p>
</section>
<style>
  .backlog { margin-block: 2rem; border-top: 1px solid #7775; padding-top: 1rem; }
  .actions, .confirmation { display: flex; flex-wrap: wrap; gap: .75rem; margin-block: 1rem; }
  .destination, article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  .backlog p { overflow-wrap: anywhere; }
  .backlog label { display: block; margin-block: .5rem; }
</style>
