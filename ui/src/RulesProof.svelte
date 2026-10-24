<script lang="ts">
  import { onMount } from 'svelte'
  import { ToggleGroup } from '@ark-ui/svelte/toggle-group'
  import { load, evaluate, openWorkspace, saveWorkspace, previewWorkspace, downloadSieve, type WorkspaceView, type ProofPreview, type Sample, type Preview, type Pin } from './api'
  let { lang, account = null }: { lang: 'en' | 'de'; account?: string | null } = $props()
  let sample = $state<Sample | null>(null)
  let preset = $state('')
  let overrides = $state('')
  let engine = $state<Sample['context']['engine']>('mailbox_sieve')
  let baseline = $state<Sample['baseline']>('current')
  let filters = $state(['all'])
  let pins = $state<Pin[]>([])
  type State = { kind: 'initial' } | { kind: 'loading' } | { kind: 'done'; proof: Preview } | { kind: 'error'; message: string }
  let proofState = $state<State>({ kind: 'initial' })
  let visible = $derived(proofState.kind === 'done' ? proofState.proof.decisions.filter(d => filters.includes('inbox') ? d.destination.kind === 'inbox' : filters.includes('copies') ? d.copies.length > 0 : true) : [])
  const copy = {
    en: { title: 'Rules and proof', detail: 'Review YAML rules and their effect on saved headers. One filing destination per message. Unknown recipient roles retain Inbox for review.', sample: 'Synthetic fixtures only. No account is read.', local: 'Saved local census. Message details and private targets stay in this browser view. Provider recipient provenance is unknown, so role safety holds messages in Inbox.', preset: 'Preset YAML', overrides: 'Override YAML', run: 'Run sample preview', localRun: 'Prove saved census', running: 'Evaluating…', engine: 'Proof engine', current: 'Current folders', previous: 'Previous sample configuration', compare: 'Compare with', result: 'Proof results', messages: 'Messages', filed: 'Filed', inbox: 'Retained in Inbox', copies: 'Copy actions', copied: 'messages receive copies.', review: 'need recipient review.', all: 'All messages', retained: 'Inbox only', withCopies: 'With copies', hold: 'Recipient review. No copies or flag changes.', seen: 'Would mark read', pin: 'Pin expected folder', pinned: 'Census pins', diff: 'Folder diff', rules: '13-month per-rule counts', unmatched: 'Inbox sender groups', omissions: 'Engine omissions', warnings: 'Diagnostics', program: 'Compiled artifact', blocked: 'Failing pins block this proof.', warn: 'Proof completed with warnings. Live writes remain locked.', empty: 'Run the proof to inspect decisions.', edit: 'Rule documents', eligibility: 'Copy eligibility', pass: 'pass', fail: 'fail', stale: 'stale', zero: 'zero selected', shadow: 'fully shadowed', id: 'Artifact identity', counts: 'selected / candidates', expected: 'Expected folder', asof: 'As-of date in UTC' },
    de: { title: 'Regeln und Prüfung', detail: 'Prüfe YAML-Regeln und ihre Wirkung auf gespeicherte Kopfzeilen. Ein Ablageziel je Nachricht. Unbekannte Empfängerrollen bleiben zur Prüfung im Posteingang.', sample: 'Nur synthetische Beispiele. Kein Kontozugriff.', local: 'Lokal gespeicherte Erfassung. Nachrichtendetails und vertrauliche Ziele bleiben in dieser Ansicht. Die Herkunft der Anbieter-Empfängerbelege ist unbekannt. Nachrichten bleiben deshalb im Posteingang.', preset: 'Voreinstellung als YAML', overrides: 'Anpassung als YAML', run: 'Beispielvorschau starten', localRun: 'Gespeicherte Erfassung prüfen', running: 'Wird ausgewertet…', engine: 'Prüfengine', current: 'Aktuelle Ordner', previous: 'Vorherige Beispielkonfiguration', compare: 'Vergleichen mit', result: 'Prüfergebnis', messages: 'Nachrichten', filed: 'Abgelegt', inbox: 'Im Posteingang behalten', copies: 'Kopieraktionen', copied: 'Nachrichten erhalten Kopien.', review: 'brauchen Empfängerprüfung.', all: 'Alle Nachrichten', retained: 'Nur Posteingang', withCopies: 'Mit Kopien', hold: 'Empfängerprüfung. Keine Kopien oder Markierungen.', seen: 'Würde als gelesen markieren', pin: 'Erwarteten Ordner fixieren', pinned: 'Fixierte Erfassungsnachrichten', diff: 'Ordneränderungen', rules: 'Regelzahlen über 13 Monate', unmatched: 'Absender im Posteingang', omissions: 'Ausgelassene Regeln', warnings: 'Hinweise', program: 'Kompiliertes Artefakt', blocked: 'Fehlgeschlagene fixierte Tests sperren diese Prüfung.', warn: 'Prüfung mit Warnungen abgeschlossen. Kontoänderungen bleiben gesperrt.', empty: 'Starte die Prüfung für Ablageentscheidungen.', edit: 'Regeldokumente', eligibility: 'Kopierberechtigung', pass: 'bestanden', fail: 'fehlgeschlagen', stale: 'veraltet', zero: 'keine ausgewählt', shadow: 'vollständig verdeckt', id: 'Artefaktidentität', counts: 'ausgewählt / Kandidaten', expected: 'Erwarteter Ordner', asof: 'Stichtag in UTC' },
  }
  let t = $derived(copy[lang])
  let asof = $state('2026-10-24')
  let workspace = $state<WorkspaceView | null>(null)
  let savedAsOf = $state<number | null>(null)
  let savedInput = $state('')
  let receipt = $state<ProofPreview | null>(null)
  let confirmed = $state(false)
  let workspaceMessage = $state('')
  let workspaceBusy = $state(false)
  function input() { return { documents: { preset_yaml: preset, override_yaml: overrides }, as_of: savedAsOf !== null && new Date(savedAsOf * 1000).toISOString().slice(0, 10) === asof ? savedAsOf : Date.parse(`${asof}T23:59:59Z`) / 1000, pins, baseline } }
  let dirty = $derived(JSON.stringify(input()) !== savedInput)
  function applyWorkspace(value: WorkspaceView) {
    workspace = value
    engine = value.engine
    if (value.workspace) {
      const saved = value.workspace.input
      preset = saved.documents.preset_yaml; overrides = saved.documents.override_yaml; pins = saved.pins; baseline = saved.baseline
      savedAsOf = saved.as_of
      asof = new Date(saved.as_of * 1000).toISOString().slice(0, 10)
      savedInput = JSON.stringify(input())
    } else { savedInput = '' }
    reset()
  }
  async function reloadWorkspace() {
    if (!account) return
    workspaceBusy = true
    try { applyWorkspace(await openWorkspace(account)); workspaceMessage = lang === 'en' ? 'Saved workspace loaded.' : 'Gespeicherter Arbeitsstand geladen.' }
    catch { workspaceMessage = lang === 'en' ? 'Workspace unavailable. Retry loading.' : 'Arbeitsstand nicht verfügbar. Erneut laden.' }
    finally { workspaceBusy = false }
  }
  async function save() {
    if (!account || !workspace) return
    workspaceBusy = true; reset()
    try { applyWorkspace(await saveWorkspace(account, workspace.revision, input())); workspaceMessage = lang === 'en' ? 'Rules and pins saved encrypted.' : 'Regeln und Fixierungen verschlüsselt gespeichert.' }
    catch { workspaceMessage = lang === 'en' ? 'Save rejected. Invalid input or saved state changed. Reload before saving again.' : 'Speichern abgelehnt. Ungültige Eingabe oder gespeicherter Stand geändert. Erneut laden.' }
    finally { workspaceBusy = false }
  }
  async function download() {
    if (!account || !receipt || !confirmed || dirty) return
    workspaceBusy = true
    try {
      const result = await downloadSieve(account, receipt.receipt)
      const url = URL.createObjectURL(new Blob([result.sieve], { type: 'application/sieve;charset=utf-8' }))
      const link = document.createElement('a'); link.href = url; link.download = result.filename; link.click(); URL.revokeObjectURL(url)
      applyWorkspace(result.workspace)
      workspaceMessage = lang === 'en' ? 'Local download prepared and export baseline recorded. Provider application is unverified.' : 'Lokaler Download vorbereitet und Exportvergleich gespeichert. Anwendung beim Anbieter ist ungeprüft.'
    } catch { reset(); workspaceMessage = lang === 'en' ? 'Download rejected. Saved state changed or the gate blocks. Reload and review a fresh proof.' : 'Download abgelehnt. Stand geändert oder Prüfung gesperrt. Erneut laden und neu prüfen.' }
    finally { workspaceBusy = false }
  }
  let frozen = $derived(!sample || workspaceBusy || proofState.kind === 'loading' || (account !== null && !workspace))
  async function compare(mode: Sample['baseline']) {
    if (dirty || frozen) return
    baseline = mode; reset()
    await save()
    if (!dirty) await run()
  }
  function reset() { proofState = { kind: 'initial' }; receipt = null; confirmed = false }
  async function run() {
    if (!sample) return
    receipt = null; confirmed = false
    proofState = { kind: 'loading' }
    try {
      const common = { ...input(), previous: sample.previous }
      let proof: Preview
      if (account) {
        if (dirty) throw new Error(lang === 'en' ? 'Save rules and pins before proof.' : 'Regeln und Fixierungen vor der Prüfung speichern.')
        receipt = await previewWorkspace(account)
        proof = receipt.proof
      } else { proof = await evaluate({ ...sample, ...common, context: { ...sample.context, engine } }) }
      proofState = { kind: 'done', proof }
    } catch (error) { proofState = { kind: 'error', message: error instanceof Error ? error.message : 'Invalid response' } }
  }
  function pin(hash: string, expected: Pin['expected']) {
    if (!sample) return
    pins = [...pins.filter(p => p.message_id_hash !== hash), { account: account ?? sample.context.account, message_id_hash: hash, expected }]
    reset()
  }
  onMount(() => {
    void load().then(data => { sample = data.sample; preset = sample.documents.preset_yaml; overrides = sample.documents.override_yaml; pins = account ? [] : sample.pins; if (account) void reloadWorkspace() }).catch(() => { proofState = { kind: 'error', message: 'Local rule sample unavailable' } })
  })
</script>

<section class="rules-proof" aria-label={t.title}>
  <h2>{t.title}</h2><p>{t.detail}</p><p class="source">{account ? t.local : t.sample}</p>
  {#if account}
    <div class="workspace-controls">
      {#if !workspace}
        <p role="status">{lang === 'en' ? 'Saved workspace not loaded.' : 'Gespeicherter Arbeitsstand nicht geladen.'}</p>
      {:else}
        <p role="status">{dirty ? (lang === 'en' ? 'Unsaved rules or pins. Save before proof or download.' : 'Ungespeicherte Regeln oder Fixierungen. Vor Prüfung oder Download speichern.') : (lang === 'en' ? `Saved workspace revision ${workspace.workspace?.revision}. A census refresh may require reloading.` : `Gespeicherte Revision ${workspace.workspace?.revision}. Nach neuer Erfassung erneut laden.`)}</p>
      {/if}
      <button onclick={reloadWorkspace} disabled={workspaceBusy || proofState.kind === 'loading'}>{lang === 'en' ? 'Reload saved rules and pins' : 'Gespeicherte Regeln und Fixierungen laden'}</button>
      <button onclick={save} disabled={frozen || !workspace || !dirty}>{lang === 'en' ? 'Save rules and pins encrypted' : 'Regeln und Fixierungen verschlüsselt speichern'}</button>
      <p aria-live="polite">{workspaceMessage}</p>
      {#if workspace?.workspace && workspace.exports[workspace.exports.length - 1]?.usable}
        <button onclick={() => compare(baseline === 'current' ? 'previous' : 'current')} disabled={dirty || frozen}>{baseline === 'current' ? (lang === 'en' ? 'Compare with last recorded export' : 'Mit letztem Export vergleichen') : (lang === 'en' ? 'Compare with current folders' : 'Mit aktuellen Ordnern vergleichen')}</button>
      {/if}
      {#if workspace?.exports.length}<p>{lang === 'en' ? 'Last recorded local export' : 'Letzter gespeicherter lokaler Export'} · {workspace.exports[workspace.exports.length - 1]?.identity}</p><p>{workspace.exports[workspace.exports.length - 1]?.provenance}</p>{#if !workspace.exports[workspace.exports.length - 1]?.usable}<p role="status">{lang === 'en' ? 'Recorded export is obsolete or belongs to another engine. Use current folders.' : 'Gespeicherter Export ist veraltet oder gehört zu einer anderen Engine. Aktuelle Ordner verwenden.'}</p>{/if}{:else}<p>{lang === 'en' ? 'No recorded export baseline.' : 'Kein gespeicherter Exportvergleich.'}</p>{/if}
    </div>
  {/if}
  <details><summary>{t.edit}</summary>
    <div class="rule-documents"><label>{t.preset}<textarea bind:value={preset} oninput={reset} spellcheck="false" rows="14" disabled={frozen}></textarea></label><label>{t.overrides}<textarea bind:value={overrides} oninput={reset} spellcheck="false" rows="14" disabled={frozen}></textarea></label></div>
  </details>
  <div class="census-actions">
    {#if !account}<label>{t.engine}<select aria-label={t.engine} bind:value={engine} onchange={reset}><option value="mailbox_sieve">mailbox.org Sieve</option><option value="gmail_backlog">Gmail backlog</option></select></label>{/if}
    <label>{t.compare}<select aria-label={t.compare} bind:value={baseline} disabled={frozen} onchange={reset}><option value="current">{t.current}</option>{#if !account}<option value="previous">{t.previous}</option>{:else}<option value="previous" disabled={!workspace?.exports[workspace.exports.length - 1]?.usable}>{lang === 'en' ? 'Last recorded local export' : 'Letzter gespeicherter lokaler Export'}</option>{/if}</select></label>
    <label>{t.asof}<input type="date" bind:value={asof} onchange={reset} disabled={frozen} /></label>
    <button class="primary" onclick={run} disabled={!sample || proofState.kind === 'loading' || (account !== null && (dirty || workspaceBusy))}>{proofState.kind === 'loading' ? t.running : account ? t.localRun : t.run}</button>
  </div>
  <h3>{t.pinned}</h3>
  <ul>{#each pins as p}<li>{p.message_id_hash.slice(0, 12)} · <label>{t.expected}<input disabled={frozen} value={p.expected.kind === 'inbox' ? 'INBOX' : p.expected.name} oninput={(event) => { const name = event.currentTarget.value; pins = pins.map(old => old === p ? { ...old, expected: name.toLowerCase() === 'inbox' ? { kind: 'inbox' } : { kind: 'folder', name } } : old); reset() }} /></label></li>{/each}</ul>
  <div aria-live="polite" aria-busy={proofState.kind === 'loading'}>
    {#if proofState.kind === 'error'}<p role="alert">{proofState.message}</p>
    {:else if proofState.kind === 'done'}
      <h3>{t.result}</h3><p class="proof-gate" role={proofState.proof.gate === 'blocked' ? 'alert' : 'status'}>{proofState.proof.gate === 'blocked' ? t.blocked : t.warn}</p>
      {#if account && receipt}<p>{lang === 'en' ? 'Proof as of (UTC)' : 'Prüfstichtag (UTC)'} · {new Date(receipt.as_of * 1000).toISOString()}</p>{#if receipt.previous && baseline === 'previous'}<p>{lang === 'en' ? 'Compared with recorded artifact' : 'Vergleich mit gespeichertem Artefakt'} · {receipt.previous.identity}</p>{/if}{/if}
      <dl class="counts"><div><dt>{t.messages}</dt><dd>{proofState.proof.counts.messages}</dd></div><div><dt>{t.filed}</dt><dd>{proofState.proof.counts.filed}</dd></div><div><dt>{t.inbox}</dt><dd>{proofState.proof.counts.inbox}</dd></div><div><dt>{t.copies}</dt><dd>{proofState.proof.counts.copies}</dd></div></dl>
      <p>{proofState.proof.counts.copied_messages} {t.copied} {proofState.proof.counts.review} {t.review}</p>
      <ToggleGroup.Root class="filters" bind:value={filters} deselectable={false} aria-label={t.result}><ToggleGroup.Item value="all">{t.all}</ToggleGroup.Item><ToggleGroup.Item value="inbox">{t.retained}</ToggleGroup.Item><ToggleGroup.Item value="copies">{t.withCopies}</ToggleGroup.Item></ToggleGroup.Root>
      {#each visible as d (d.id)}<article class="decision"><div><h3>{d.subject}</h3><p class="sender">{d.from}</p></div><div class="filing">{d.destination.kind === 'inbox' ? 'INBOX' : d.destination.name}</div><div class="reason">{d.matched_rule}<p>{d.review ? t.hold : d.mark_read ? t.seen : ''}</p></div><div class="copy-line">{#each d.copies as c}<span>{c.rule} → {c.recipient}</span>{/each}{#each d.copy_eligibility as c}<span>{t.eligibility} · {c.rule} · {c.status}</span>{/each}</div>{#if d.message_id_hash}<button disabled={frozen} onclick={() => { if (d.message_id_hash) pin(d.message_id_hash, d.destination) }}>{t.pin}</button>{/if}</article>{/each}
      <details open><summary>{t.diff} · {proofState.proof.diff.length}</summary>{#each proofState.proof.diff as d}<p>{d.id} · {d.before.join(', ')} → {d.after}</p>{/each}</details>
      <details><summary>{t.rules}</summary><p>{proofState.proof.months.join(' · ')}</p>{#each proofState.proof.rules as r}<p>{r.id} · {r.matched} / {r.candidates} {t.counts} {r.zero_match ? t.zero : ''} {r.fully_shadowed ? t.shadow : ''}</p><pre>{JSON.stringify(r.monthly, null, 2)}</pre>{/each}</details>
      <details><summary>{t.unmatched}</summary>{#each Object.entries(proofState.proof.unmatched_senders) as [sender, count]}<p>{sender} · {count}</p>{/each}</details>
      <details open><summary>{t.omissions}</summary>{#each proofState.proof.artifact.omissions as o}<p>{o.rule} · {o.reason}</p>{/each}</details>
      <details open><summary>{t.pinned}</summary>{#each proofState.proof.pins as p}<p>{p.message_id_hash.slice(0, 12)} · {t[p.status]}</p>{/each}</details>
      <details open={account !== null}><summary>{t.warnings}</summary>{#each proofState.proof.warnings as w}<p>{w}</p>{/each}<p>{proofState.proof.runtime_cross_check}</p></details>
      <details open={account !== null}><summary>{t.program}</summary><p class="artifact-id">{t.id} · {proofState.proof.artifact.identity}</p><p>{proofState.proof.semantics}</p><pre>{proofState.proof.artifact.executable}</pre><ul>{#each proofState.proof.artifact.trace as line}<li>{line}</li>{/each}</ul></details>
      {#if account && receipt?.download_available}
        <div class="local-download">
          <p>{lang === 'en' ? 'Review this proof and its warnings before downloading these exact Sieve bytes. Live account changes remain locked.' : 'Prüfe dieses Ergebnis und die Hinweise vor dem Download genau dieser Sieve-Datei. Kontoänderungen bleiben gesperrt.'}</p>
          <label><input type="checkbox" bind:checked={confirmed} disabled={frozen || dirty} />{lang === 'en' ? 'I confirm this proof for a local Sieve download' : 'Ich bestätige diese Prüfung für einen lokalen Sieve-Download'}</label>
          <button onclick={download} disabled={!confirmed || workspaceBusy || dirty}>{lang === 'en' ? 'Download confirmed Sieve' : 'Bestätigte Sieve-Datei herunterladen'}</button>
        </div>
      {/if}
    {:else}<p>{t.empty}</p>{/if}
  </div>
</section>
