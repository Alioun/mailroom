<script lang="ts">
  import ProofDiagnostics from './ProofDiagnostics.svelte'
  import { openProposals, reviewProposals, confirmProposals, type ProposalGeneration, type ProposalReview, type ProposalSelection } from './api'
  import { proposalRationaleText, proposalReasonText, proposalStatusText } from './proposal-diagnostics'
  let { account, lang, disabled, onChanged }: { account: string; lang: 'en' | 'de'; disabled: boolean; onChanged: () => Promise<void> } = $props()
  const copy = {
    en: { title: 'Advanced · Filing proposals', detail: 'Review deterministic suggestions from saved local observations. Sender claims are unauthenticated. Approval saves local rules only. It never acts on mail or deploys rules.', load: 'Generate or reload filing proposals', choose: 'Select proposal', extend: 'Add this sender to the category sender list. Newsletters keep their bulk routing.', rejected: 'Sender proposal rejected. It stays unavailable after refresh.', changed: 'Earlier rejection retained.', unresolved: 'Unresolved observations', review: 'Review selected local changes', reject: 'Review exact rejections', confirm: 'I confirm this exact proposal batch', apply: 'Save reviewed local decision', original: 'Original rule documents', resulting: 'Resulting rule documents', selected: 'Selected changes', impact: 'Current and resulting proof', pins: 'Pin results', omissions: 'Omissions', warnings: 'Warnings', diff: 'Folder diff', sourceDiff: 'Rule document diff', empty: 'No supported filing changes. Unresolved observations need owner classification.', fresh: 'Local decision saved. Run fresh proof over the saved rules.', failure: 'Action rejected or saved state changed. Reload and review the exact candidates again.', dirty: 'Save or reload rule edits before using proposals.', uncertainty: 'Unauthenticated sender claim', history: 'Retained decision history', observations: 'Supporting local observations', counts: 'Messages / filed / Inbox / copies', gate: 'Proof gate', blocked: 'Acceptance blocked by proof. Rejection is still available.' },
    de: { title: 'Erweitert · Ablagevorschläge', detail: 'Prüfe deterministische Vorschläge aus lokal gespeicherten Belegen. Absenderangaben sind unbestätigt. Die Freigabe speichert nur lokale Regeln. Sie ändert keine Nachrichten und aktiviert keine Regeln.', load: 'Ablagevorschläge erzeugen oder neu laden', choose: 'Vorschlag auswählen', extend: 'Diesen Absender zur Absenderliste der Kategorie hinzufügen. Newsletter behalten ihre Sammelablage.', rejected: 'Absendervorschlag abgelehnt. Er bleibt nach dem Neuladen gesperrt.', changed: 'Frühere Ablehnung bleibt erhalten.', unresolved: 'Ungeklärte Beobachtungen', review: 'Ausgewählte lokale Änderungen prüfen', reject: 'Genaue Ablehnungen prüfen', confirm: 'Ich bestätige genau diese Vorschlagsauswahl', apply: 'Geprüfte lokale Entscheidung speichern', original: 'Ursprüngliche Regeldokumente', resulting: 'Resultierende Regeldokumente', selected: 'Ausgewählte Änderungen', impact: 'Aktuelle und resultierende Prüfung', pins: 'Fixierte Testergebnisse', omissions: 'Auslassungen', warnings: 'Hinweise', diff: 'Ordneränderungen', sourceDiff: 'Änderungen am Regeldokument', empty: 'Keine unterstützten Ablageänderungen. Ungeklärte Beobachtungen erfordern deine Zuordnung.', fresh: 'Lokale Entscheidung gespeichert. Gespeicherte Regeln neu prüfen.', failure: 'Aktion abgelehnt oder gespeicherter Stand geändert. Neu laden und genaue Vorschläge erneut prüfen.', dirty: 'Regeländerungen vor den Vorschlägen speichern oder neu laden.', uncertainty: 'Unbestätigte Absenderangabe', history: 'Aufbewahrte Entscheidungen', observations: 'Lokale Belege', counts: 'Nachrichten / abgelegt / Posteingang / Kopien', gate: 'Prüfstatus', blocked: 'Annahme durch Prüfung gesperrt. Ablehnung bleibt möglich.' },
  }
  let t = $derived(copy[lang])
  let diagnostic = $derived(proposalStatusText[lang])
  function counts(proof: ProposalGeneration['current']['proof']) {
    const { messages, filed, inbox, copies } = proof.counts
    const number = new Intl.NumberFormat(lang === 'de' ? 'de-DE' : 'en-US')
    return [messages, filed, inbox, copies].map(value => number.format(value)).join(' / ')
  }
  let generation = $state<ProposalGeneration | null>(null)
  let selected = $state<string[]>([])
  let review = $state<ProposalReview | null>(null)
  let confirmed = $state(false)
  let busy = $state(false)
  let message = $state('')
  $effect(() => { if (disabled) { generation = null; selected = []; review = null; confirmed = false } })
  function reset() { review = null; confirmed = false; message = '' }
  function choose(id: string, checked: boolean) { selected = checked ? [...selected, id] : selected.filter(value => value !== id); reset() }
  async function load() {
    busy = true; selected = []; reset()
    try { generation = await openProposals(account) } catch { generation = null; message = t.failure }
    finally { busy = false }
  }
  async function preview(disposition: ProposalReview['disposition']) {
    if (!generation) return
    busy = true; reset()
    const selection: ProposalSelection = { account, generation: generation.generation, candidates: generation.candidates.filter(c => selected.includes(c.id)).map(c => ({ id: c.id, change: c.change })) }
    try { review = await reviewProposals(account, selection, disposition) } catch { message = t.failure }
    finally { busy = false }
  }
  async function apply() {
    if (!review || !confirmed) return
    busy = true
    try { const next = await confirmProposals(account, review); await onChanged(); generation = next; selected = []; reset(); message = t.fresh }
    catch { reset(); message = t.failure }
    finally { busy = false }
  }
</script>

<details class="filing-proposals">
  <summary>{t.title}</summary>
  <p>{t.detail}</p>
  <button onclick={load} disabled={disabled || busy}>{t.load}</button>
  {#if disabled}<p>{t.dirty}</p>{/if}
  <p role="status" aria-live="polite">{message}</p>
  {#if generation}
    {#if !generation.candidates.length}<p>{t.empty}</p>{/if}
    {#each generation.candidates as candidate (candidate.id)}
      <article class="filing-candidate" data-candidate={candidate.id}>
        <label><input type="checkbox" checked={selected.includes(candidate.id)} onchange={event => choose(candidate.id, event.currentTarget.checked)} disabled={busy || disabled || candidate.rejected} />{t.choose} · {candidate.sender_claim}</label>
        <p>{t.uncertainty}</p><p>{proposalRationaleText(candidate.rationale, lang)}</p>
        {#if candidate.rejected}<p>{t.rejected}</p>{:else if candidate.prior_rejections}<p>{t.changed}</p>{/if}
        {#if candidate.change.kind === 'extend_sender'}<p>{t.extend}</p>{/if}
        <pre>{JSON.stringify(candidate.change, null, 2)}</pre>
        <details><summary>{t.observations} · {candidate.observations.length}</summary>{#each candidate.observations as observation}<p>{observation.message} · {observation.subject} · {observation.folders.join(', ')}</p>{/each}</details>
        <p>{t.counts} · {counts(generation.current.proof)} → {counts(candidate.impact.proof)}</p>
        <p>{t.gate} · {diagnostic.gate[candidate.impact.proof.gate]}</p>
        <details><summary>{t.impact}</summary><pre>{JSON.stringify({ current: generation.current.proof, resulting: candidate.impact.proof }, null, 2)}</pre></details>
      </article>
    {/each}
    <button onclick={() => preview('accept')} disabled={disabled || busy || !selected.length}>{t.review}</button>
    <button onclick={() => preview('reject')} disabled={disabled || busy || !selected.length}>{t.reject}</button>
    <details><summary>{t.unresolved} · {generation.unresolved.length}</summary>{#each generation.unresolved as observation}<p>{observation.sender_claim ?? diagnostic.unknownSender} · {observation.messages} · {proposalReasonText(observation.reason, lang)}</p>{/each}</details>
    <details><summary>{t.history} · {generation.decisions.history.length}</summary>{#each generation.decisions.history as decision}<p>{decision.target} · {diagnostic.disposition[decision.disposition]} · {decision.candidate}</p>{/each}</details>
  {/if}
  {#if review}
    <section class="proposal-review" aria-label={t.selected}>
      <h3>{t.selected} · {diagnostic.disposition[review.disposition]}</h3><pre>{JSON.stringify(review.changes, null, 2)}</pre>
      <p>{diagnostic.effects[review.disposition]}</p>
      <details><summary>{diagnostic.exactEffects}</summary><p>{review.effects}</p></details>
      <details open><summary>{t.original}</summary><pre>{review.original_documents.preset_yaml}</pre><pre>{review.original_documents.override_yaml}</pre></details>
      <details open><summary>{t.resulting}</summary><pre>{review.resulting_documents.preset_yaml}</pre><pre>{review.resulting_documents.override_yaml}</pre></details>
      <details open><summary>{t.sourceDiff}</summary><pre>{review.document_diff}</pre></details>
      <h3>{t.impact}</h3>
      <p>{t.counts} · {counts(review.current.proof)} → {counts(review.resulting.proof)}</p>
      <details open><summary>{t.diff}</summary><pre>{JSON.stringify(review.resulting.proof.diff, null, 2)}</pre></details>
      <details open><summary>{t.pins}</summary><pre>{JSON.stringify(review.resulting.proof.pins, null, 2)}</pre></details>
      <details open><summary>{t.omissions}</summary><p>{diagnostic.originalDiagnostics}</p>{#each review.resulting.proof.artifact.omissions as omission}<p>{omission.rule} · {omission.reason}</p>{/each}</details>
      <details open><summary>{t.warnings}</summary><ProofDiagnostics diagnostics={review.resulting.proof.diagnostics} {lang} /></details>
      <details><summary>{t.impact}</summary><pre>{JSON.stringify({ current: review.current.proof, resulting: review.resulting.proof }, null, 2)}</pre></details>
      {#if !review.acceptance_available}<p role="alert">{t.blocked}</p>{/if}
      <label><input type="checkbox" bind:checked={confirmed} disabled={disabled || busy || !review.acceptance_available} />{t.confirm}</label>
      <button onclick={apply} disabled={disabled || busy || !confirmed || !review.acceptance_available}>{t.apply}</button>
    </section>
  {/if}
</details>

<style>
  .filing-proposals { margin-block: 1.5rem; }
  .filing-candidate { border-block-start: 1px solid var(--border, #aaa); padding-block: 1rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 24rem; overflow-y: auto; }
  p, label { overflow-wrap: anywhere; }
  button { margin-block: .5rem; }
</style>
