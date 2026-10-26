<script lang="ts">
  import { onMount } from 'svelte'
  import { readAccountStatus, openGmailBacklog, previewGmailBacklog, previewGmailUndo, gmailAction, type GmailView, type GmailPreview, type GmailUndo, type GmailOutcome, type BacklogOptions } from './api'
  let { account, lang, workspaceRevision = 0 }: { account: string; lang: 'en' | 'de'; workspaceRevision?: number } = $props()
  const text = {
    en: { title: 'Gmail backlog sorting', source: 'Synthetic Gmail label source. Real-account writes remain unavailable. Recipient evidence is installed separately. Messages stay in All Mail; only labels change.', preview: 'Preview exact label changes', confirm: 'I confirm these exact label changes: add the destination label, then remove only Inbox', start: 'Sort Gmail backlog', reload: 'Reload saved label runs', cancel: 'Cancel sorting', reconcile: 'Reconcile observations only', resume: 'Resume never-attempted remainder', undoPreview: 'Preview conditional undo', undoConfirm: 'I confirm restoring Inbox and removing only the labels this run added', undo: 'Undo eligible label changes', resumeUndo: 'Resume confirmed undo remainder', scope: 'Scope', inbox: 'Inbox only', unfiled: 'Inbox plus unlabelled mail', every: 'Advanced: every label', protect: 'Protect flagged messages', changes: 'eligible changes', conflicts: 'conflicts', report: 'Exact local identities and label evidence', journal: 'Encrypted label journal', error: 'Action rejected. Reload the saved journal and request a fresh preview. Existing evidence is retained.', wait: 'Worker active. Cancel stops new label steps and retains completed changes. A transmitted store may remain uncertain.', stopped: 'Stop is saved. Resume is explicit. Undo requires a separate preview and confirmation.', hold: 'Restored account is held. No source or approval is restored.', approval: 'Saved proof', observation: 'Fresh observation proof', empty: 'No label runs.', attempted: 'Attempted', never: 'Never attempted', completion: 'Command completion', readback: 'Readback saved', context: 'All Mail', step: { add_label: 'Add label', remove_inbox: 'Remove Inbox', restore_inbox: 'Restore Inbox', remove_label: 'Remove added label' }, outcome: { pending: 'never attempted', verified: 'verified', changed: 'applied, other changes seen', unapplied: 'not applied', uncertain: 'uncertain', conflict: 'conflict' } },
    de: { title: 'Gmail-Bestand sortieren', source: 'Synthetische Gmail-Labelquelle. Schreibzugriff auf echte Konten bleibt gesperrt. Empfängernachweise werden separat installiert. Nachrichten bleiben in „Alle Nachrichten“; nur Labels ändern sich.', preview: 'Genaue Labeländerungen anzeigen', confirm: 'Ich bestätige diese genauen Labeländerungen: Ziellabel hinzufügen, dann nur den Posteingang entfernen', start: 'Gmail-Bestand sortieren', reload: 'Gespeicherte Labelabläufe laden', cancel: 'Sortierung abbrechen', reconcile: 'Nur Beobachtungen abgleichen', resume: 'Nie versuchten Rest fortsetzen', undoPreview: 'Bedingtes Rückgängigmachen prüfen', undoConfirm: 'Ich bestätige: Posteingang wiederherstellen und nur die von diesem Ablauf hinzugefügten Labels entfernen', undo: 'Geeignete Labeländerungen rückgängig machen', resumeUndo: 'Bestätigten Rest des Rückgängigmachens fortsetzen', scope: 'Umfang', inbox: 'Nur Posteingang', unfiled: 'Posteingang und Nachrichten ohne Label', every: 'Erweitert: alle Labels', protect: 'Markierte Nachrichten schützen', changes: 'geeignete Änderungen', conflicts: 'Konflikte', report: 'Genaue lokale Identitäten und Labelnachweise', journal: 'Verschlüsseltes Labeljournal', error: 'Aktion abgelehnt. Journal neu laden und frische Vorschau anfordern. Vorhandene Nachweise bleiben erhalten.', wait: 'Ablauf aktiv. Abbrechen stoppt neue Labelschritte und behält abgeschlossene Änderungen. Ein übertragener Befehl kann ungewiss bleiben.', stopped: 'Stopp gespeichert. Fortsetzen erfolgt ausdrücklich. Rückgängigmachen benötigt eine eigene Vorschau und Bestätigung.', hold: 'Wiederhergestelltes Konto bleibt gesperrt. Quelle und Freigabe werden nicht wiederhergestellt.', approval: 'Gespeicherter Nachweis', observation: 'Nachweis aus frischer Beobachtung', empty: 'Keine Labelabläufe.', attempted: 'Versucht', never: 'Nie versucht', completion: 'Befehlsabschluss', readback: 'Prüfung gespeichert', context: 'Alle Nachrichten', step: { add_label: 'Label hinzufügen', remove_inbox: 'Posteingang entfernen', restore_inbox: 'Posteingang wiederherstellen', remove_label: 'Hinzugefügtes Label entfernen' }, outcome: { pending: 'nie versucht', verified: 'geprüft', changed: 'angewendet, weitere Änderungen gesehen', unapplied: 'nicht angewendet', uncertain: 'ungewiss', conflict: 'Konflikt' } },
  }
  let t = $derived(text[lang])
  let scope = $state<BacklogOptions['scope']>('inbox_unfiled')
  let protect = $state(true)
  let view = $state<GmailView | null>(null)
  let preview = $state<GmailPreview | null>(null)
  let undo = $state<GmailUndo | null>(null)
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
  async function load() { invalidate(); view = await openGmailBacklog(account) }
  async function plan(selected: string[] | null = null) { invalidate(); const revision = workspaceRevision; const result = await previewGmailBacklog(account, { scope, protect_flagged: protect, selected }); if (revision === workspaceRevision) preview = result }
  async function select(name: string, checked: boolean) {
    if (!preview) return
    const selected = Object.entries(preview.operation.plan.filing.groups).filter(([key, group]) => key === name ? checked : group.selected).map(([key]) => key)
    await plan(selected)
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
        if (status.job?.progress.state !== 'running') { view = await openGmailBacklog(account); break }
      }
    } finally { running = false }
  }
  async function start() {
    if (!preview || !confirmed) return
    const command = { action: 'confirm', options: preview.operation.plan.filing.options, receipt: preview.receipt, confirmed: true } satisfies Parameters<typeof gmailAction>[1]
    invalidate(); await gmailAction(account, command); await watch()
  }
  async function continuation(action: 'resume' | 'resume_undo', run: string) { invalidate(); await gmailAction(account, { action, run }); await watch() }
  async function cancel() {
    cancelling = true; error = ''; invalidate()
    try { await gmailAction(account, { action: 'cancel' }); running = false; await load() } catch { error = t.error } finally { cancelling = false }
  }
  async function inspectUndo(run: string) { invalidate(); const revision = workspaceRevision; const result = await previewGmailUndo(account, run); if (revision === workspaceRevision) undo = result }
  async function reverse() {
    if (!undo || !undoConfirmed) return
    const command = { action: 'undo', run: undo.run, receipt: undo.receipt, confirmed: true } satisfies Parameters<typeof gmailAction>[1]
    invalidate(); await gmailAction(account, command); await watch()
  }
  function tally(outcomes: Record<string, GmailOutcome> | undefined) {
    const counts = new Map<GmailOutcome, number>()
    for (const o of Object.values(outcomes ?? {})) counts.set(o, (counts.get(o) ?? 0) + 1)
    return [...counts].map(([o, n]) => `${t.outcome[o]} ${n}`).join(' · ')
  }
  onMount(() => { active = true; void act(load); return () => { active = false; running = false } })
</script>
<section class="gmail-backlog" aria-label={t.title}>
  <h3>{t.title}</h3><p>{t.source}</p>
  {#if view?.recovery_hold}<p role="status">{t.hold}</p>{/if}
  <label>{t.scope}<select bind:value={scope} disabled={busy || running} onchange={invalidate}><option value="inbox_unfiled">{t.unfiled}</option><option value="inbox">{t.inbox}</option><option value="every_folder">{t.every}</option></select></label>
  <label><input type="checkbox" bind:checked={protect} disabled={busy || running} onchange={invalidate} />{t.protect}</label>
  <div class="actions"><button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(() => plan())}>{t.preview}</button><button disabled={busy || running} onclick={() => act(load)}>{t.reload}</button><button disabled={cancelling || !view?.installed || view?.recovery_hold} onclick={cancel}>{t.cancel}</button></div>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if running || cancelling}<p role="status">{t.wait} · {savedCount}</p>{/if}
  {#if preview}
    <p class="change-count">{preview.operation.plan.filing.changes.length} {t.changes}</p>
    <p>{preview.operation.plan.filing.artifact.identity}</p>
    <p>{t.approval} · {preview.operation.plan.saved_proof}</p><p>{t.observation} · {preview.operation.plan.execution_proof}</p>
    {#each Object.entries(preview.operation.plan.filing.groups) as [name, group]}<label><input type="checkbox" checked={group.selected} disabled={busy} onchange={event => { const checked = event.currentTarget.checked; void act(() => select(name, checked)) }} />{name} · {group.count}</label>{/each}
    {#each Object.entries(preview.operation.plan.filing.excluded) as [reason, count]}<p>{reason} · {count}</p>{/each}
    {#each preview.operation.plan.filing.diagnostics as diagnostic}<p>{diagnostic}</p>{/each}
    <details><summary>{t.report}</summary>{#each preview.operation.batches as b}{#each b.steps as s}<p class="planned-step">{t.context} / {b.intent.context.uidvalidity} / {s.uids.join(', ')} · {t.step[s.kind]} · {b.intent.destination.decoded}</p>{/each}{/each}</details>
    <label><input type="checkbox" bind:checked={confirmed} disabled={busy || running} />{t.confirm}</label><button disabled={busy || running || !confirmed} onclick={() => act(start)}>{t.start}</button>
  {/if}
  <h4>{t.journal}</h4>
  {#each view?.journal.runs ?? [] as run (run.id)}
    <article data-gmail-run={run.id}><p class="run-state">{run.status}</p>{#if run.stop_requested}<p>{t.stopped}</p>{/if}
      {#if run.undo_receipt}<p>{run.undo_conflicts} {t.conflicts}</p>{/if}
      <button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(async () => { invalidate(); await gmailAction(account, { action: 'reconcile', run: run.id }); await load() })}>{t.reconcile}</button>
      {#if !run.undo_receipt && !['complete', 'undone'].includes(run.status)}<button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(() => continuation('resume', run.id))}>{t.resume}</button>{/if}
      {#if !run.undo_receipt}<button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(() => inspectUndo(run.id))}>{t.undoPreview}</button>{:else if run.status !== 'undone'}<button disabled={busy || running || !view?.installed || view?.recovery_hold} onclick={() => act(() => continuation('resume_undo', run.id))}>{t.resumeUndo}</button>{/if}
      <details><summary>{t.report}</summary><p>{run.id}</p><p>{run.plan.filing.artifact.identity}</p>
        {#each [['forward', run.batches], ['reverse', run.reverse.map(r => r.batch)]] as const as [direction, batches]}{#each batches as b, n}{#each b.steps as s, i}<p class="step" data-step={s.kind}>{t.step[s.kind]} · {b.intent.destination.decoded} · {s.uids.join(', ')}</p><p>{s.attempted ? t.attempted : t.never} · {t.completion} · {s.completion?.kind === 'tagged' ? s.completion.status : s.completion?.kind ?? '?'}{#if s.readback} · {t.readback}{/if}</p><p class="outcome">{tally(view?.outcomes[run.id]?.[direction][n]?.[i])}</p>{/each}{/each}{/each}
      </details>
    </article>
  {:else}<p>{t.empty}</p>{/each}
  {#if undo}<div class="confirmation"><p>{undo.reverse.reduce((n, r) => n + Object.keys(r.batch.intent.messages).length, 0)} {t.changes} · {undo.conflicts} {t.conflicts}</p>{#each undo.reverse as r}{#each r.batch.steps as s}<p>{t.step[s.kind]} · {r.batch.intent.destination.decoded} · {s.uids.join(', ')}</p>{/each}{/each}<label><input type="checkbox" bind:checked={undoConfirmed} disabled={busy || running} />{t.undoConfirm}</label><button disabled={busy || running || !undoConfirmed || undo.reverse.length === 0} onclick={() => act(reverse)}>{t.undo}</button></div>{/if}
  <p>{view?.limitations ?? preview?.limitations}</p>
</section>
<style>
  .gmail-backlog { margin-block: 2rem; border-top: 1px solid #7775; padding-top: 1rem; }
  .actions, .confirmation { display: flex; flex-wrap: wrap; gap: .75rem; margin-block: 1rem; }
  article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  p { overflow-wrap: anywhere; }
  label { display: block; margin-block: .5rem; overflow-wrap: anywhere; }
</style>
