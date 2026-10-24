<script lang="ts">
  import { onMount } from 'svelte'
  import { ToggleGroup } from '@ark-ui/svelte/toggle-group'
  import { load, evaluate, proveAccount, type Sample, type Preview, type Pin } from './api'
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
  function reset() { proofState = { kind: 'initial' } }
  async function run() {
    if (!sample) return
    proofState = { kind: 'loading' }
    try {
      const common = { documents: { preset_yaml: preset, override_yaml: overrides }, as_of: Date.parse(`${asof}T23:59:59Z`) / 1000, pins, baseline, previous: sample.previous }
      const proof = account ? await proveAccount(account, common) : await evaluate({ ...sample, ...common, context: { ...sample.context, engine } })
      proofState = { kind: 'done', proof }
    } catch (error) { proofState = { kind: 'error', message: error instanceof Error ? error.message : 'Invalid response' } }
  }
  function pin(hash: string, expected: Pin['expected']) {
    if (!sample) return
    pins = [...pins.filter(p => p.message_id_hash !== hash), { account: account ?? sample.context.account, message_id_hash: hash, expected }]
    reset()
  }
  onMount(() => {
    void load().then(data => { sample = data.sample; preset = sample.documents.preset_yaml; overrides = sample.documents.override_yaml; pins = account ? [] : sample.pins }).catch(() => { proofState = { kind: 'error', message: 'Local rule sample unavailable' } })
  })
</script>

<section class="rules-proof" aria-label={t.title}>
  <h2>{t.title}</h2><p>{t.detail}</p><p class="source">{account ? t.local : t.sample}</p>
  <details><summary>{t.edit}</summary>
    <div class="rule-documents"><label>{t.preset}<textarea bind:value={preset} oninput={reset} spellcheck="false" rows="14"></textarea></label><label>{t.overrides}<textarea bind:value={overrides} oninput={reset} spellcheck="false" rows="14"></textarea></label></div>
  </details>
  <div class="census-actions">
    {#if !account}<label>{t.engine}<select aria-label={t.engine} bind:value={engine} onchange={reset}><option value="mailbox_sieve">mailbox.org Sieve</option><option value="gmail_backlog">Gmail backlog</option></select></label>{/if}
    <label>{t.compare}<select aria-label={t.compare} bind:value={baseline} onchange={reset}><option value="current">{t.current}</option>{#if !account}<option value="previous">{t.previous}</option>{/if}</select></label>
    <label>{t.asof}<input type="date" bind:value={asof} onchange={reset} /></label>
    <button class="primary" onclick={run} disabled={!sample || proofState.kind === 'loading'}>{proofState.kind === 'loading' ? t.running : account ? t.localRun : t.run}</button>
  </div>
  <h3>{t.pinned}</h3>
  <ul>{#each pins as p}<li>{p.message_id_hash.slice(0, 12)} · <label>{t.expected}<input value={p.expected.kind === 'inbox' ? 'INBOX' : p.expected.name} onchange={(event) => { const name = event.currentTarget.value; pins = pins.map(old => old === p ? { ...old, expected: name.toLowerCase() === 'inbox' ? { kind: 'inbox' } : { kind: 'folder', name } } : old); reset() }} /></label></li>{/each}</ul>
  <div aria-live="polite" aria-busy={proofState.kind === 'loading'}>
    {#if proofState.kind === 'error'}<p role="alert">{proofState.message}</p>
    {:else if proofState.kind === 'done'}
      <h3>{t.result}</h3><p class="proof-gate" role={proofState.proof.gate === 'blocked' ? 'alert' : 'status'}>{proofState.proof.gate === 'blocked' ? t.blocked : t.warn}</p>
      <dl class="counts"><div><dt>{t.messages}</dt><dd>{proofState.proof.counts.messages}</dd></div><div><dt>{t.filed}</dt><dd>{proofState.proof.counts.filed}</dd></div><div><dt>{t.inbox}</dt><dd>{proofState.proof.counts.inbox}</dd></div><div><dt>{t.copies}</dt><dd>{proofState.proof.counts.copies}</dd></div></dl>
      <p>{proofState.proof.counts.copied_messages} {t.copied} {proofState.proof.counts.review} {t.review}</p>
      <ToggleGroup.Root class="filters" bind:value={filters} deselectable={false} aria-label={t.result}><ToggleGroup.Item value="all">{t.all}</ToggleGroup.Item><ToggleGroup.Item value="inbox">{t.retained}</ToggleGroup.Item><ToggleGroup.Item value="copies">{t.withCopies}</ToggleGroup.Item></ToggleGroup.Root>
      {#each visible as d (d.id)}<article class="decision"><div><h3>{d.subject}</h3><p class="sender">{d.from}</p></div><div class="filing">{d.destination.kind === 'inbox' ? 'INBOX' : d.destination.name}</div><div class="reason">{d.matched_rule}<p>{d.review ? t.hold : d.mark_read ? t.seen : ''}</p></div><div class="copy-line">{#each d.copies as c}<span>{c.rule} → {c.recipient}</span>{/each}{#each d.copy_eligibility as c}<span>{t.eligibility} · {c.rule} · {c.status}</span>{/each}</div>{#if d.message_id_hash}<button onclick={() => { if (d.message_id_hash) pin(d.message_id_hash, d.destination) }}>{t.pin}</button>{/if}</article>{/each}
      <details open><summary>{t.diff} · {proofState.proof.diff.length}</summary>{#each proofState.proof.diff as d}<p>{d.id} · {d.before.join(', ')} → {d.after}</p>{/each}</details>
      <details><summary>{t.rules}</summary><p>{proofState.proof.months.join(' · ')}</p>{#each proofState.proof.rules as r}<p>{r.id} · {r.matched} / {r.candidates} {t.counts} {r.zero_match ? t.zero : ''} {r.fully_shadowed ? t.shadow : ''}</p><pre>{JSON.stringify(r.monthly, null, 2)}</pre>{/each}</details>
      <details><summary>{t.unmatched}</summary>{#each Object.entries(proofState.proof.unmatched_senders) as [sender, count]}<p>{sender} · {count}</p>{/each}</details>
      <details open><summary>{t.omissions}</summary>{#each proofState.proof.artifact.omissions as o}<p>{o.rule} · {o.reason}</p>{/each}</details>
      <details open><summary>{t.pinned}</summary>{#each proofState.proof.pins as p}<p>{p.message_id_hash.slice(0, 12)} · {t[p.status]}</p>{/each}</details>
      <details><summary>{t.warnings}</summary>{#each proofState.proof.warnings as w}<p>{w}</p>{/each}<p>{proofState.proof.runtime_cross_check}</p></details>
      <details><summary>{t.program}</summary><p class="artifact-id">{t.id} · {proofState.proof.artifact.identity}</p><p>{proofState.proof.semantics}</p><pre>{proofState.proof.artifact.executable}</pre><ul>{#each proofState.proof.artifact.trace as line}<li>{line}</li>{/each}</ul></details>
    {:else}<p>{t.empty}</p>{/if}
  </div>
</section>
