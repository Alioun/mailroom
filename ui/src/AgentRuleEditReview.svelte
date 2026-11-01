<script lang="ts">
  import ProofDiagnostics from './ProofDiagnostics.svelte'
  import { previewAgentEdits, confirmAgentEdits, type AgentEditReview, type SummaryBinding } from './api'
  import type { AgentProposal } from './agent-exchange'
  import { proofGateText } from './proof-diagnostics'
  let { account, summary, proposal, lang }: { account: string; summary: SummaryBinding; proposal: AgentProposal; lang: 'en' | 'de' } = $props()
  const copy = {
    en: {
      title: 'Proposed rule edits', detail: 'The agent proposed structured edits to your local rules. They are untrusted until you review the exact resulting documents and proof below. Nothing is saved until you confirm.',
      load: 'Review rule edits against saved local rules', patch: 'Change preset rule', addition: 'Add new rule', renamed: 'current ID',
      original: 'Current override document', resulting: 'Resulting override document', diff: 'Document changes', proof: 'Current and resulting proof',
      current: 'Current proof', after: 'Resulting proof', counts: 'Messages / filed / Inbox / copies', gate: 'Proof gate', pins: 'Pin results', omissions: 'Omissions', warnings: 'Warnings', folders: 'Folder changes',
      blocked: 'Acceptance is blocked by the resulting proof. Rejection is still available.', full: 'Rejection history is full. Nothing was removed.',
      confirm: 'I reviewed these exact documents and proof', accept: 'Save these edits to local rules', reject: 'Reject these exact edits',
      saved: 'Local rules saved. Run fresh proof over the saved rules before any export.', rejected: 'Edits rejected. The same edits are refused if proposed again.',
      failure: 'Review refused or saved state changed. Rejected, conflicting or invalid edits cannot be imported. Review a fresh summary and proposal.',
      hold: 'Saving changes local rules only. It never deploys rules, sends mail or acts on messages.',
    },
    de: {
      title: 'Vorgeschlagene Regeländerungen', detail: 'Der Agent hat strukturierte Änderungen an deinen lokalen Regeln vorgeschlagen. Sie bleiben unbestätigt, bis du die genauen resultierenden Dokumente und die Prüfung unten kontrollierst. Erst deine Bestätigung speichert etwas.',
      load: 'Regeländerungen mit gespeicherten lokalen Regeln prüfen', patch: 'Vorlagenregel ändern', addition: 'Neue Regel hinzufügen', renamed: 'aktuelle ID',
      original: 'Aktuelles Anpassungsdokument', resulting: 'Resultierendes Anpassungsdokument', diff: 'Änderungen am Dokument', proof: 'Aktuelle und resultierende Prüfung',
      current: 'Aktuelle Prüfung', after: 'Resultierende Prüfung', counts: 'Nachrichten / abgelegt / Posteingang / Kopien', gate: 'Prüfstatus', pins: 'Fixierte Testergebnisse', omissions: 'Auslassungen', warnings: 'Hinweise', folders: 'Ordneränderungen',
      blocked: 'Die resultierende Prüfung sperrt die Annahme. Ablehnen bleibt möglich.', full: 'Der Ablehnungsverlauf ist voll. Es wurde nichts gelöscht.',
      confirm: 'Ich habe genau diese Dokumente und diese Prüfung kontrolliert', accept: 'Diese Änderungen in lokalen Regeln speichern', reject: 'Genau diese Änderungen ablehnen',
      saved: 'Lokale Regeln gespeichert. Prüfe die gespeicherten Regeln vor jedem Export erneut.', rejected: 'Änderungen abgelehnt. Dieselben Änderungen werden bei einem neuen Vorschlag abgewiesen.',
      failure: 'Prüfung abgelehnt oder gespeicherter Stand geändert. Abgelehnte, widersprüchliche oder ungültige Änderungen lassen sich nicht übernehmen. Prüfe eine neue Zusammenfassung und einen neuen Vorschlag.',
      hold: 'Speichern ändert nur lokale Regeln. Es aktiviert keine Regeln, sendet keine Mail und ändert keine Nachrichten.',
    },
  }
  const t = $derived(copy[lang])
  let review = $state<AgentEditReview | null>(null)
  let confirmed = $state(false)
  let busy = $state(false)
  let message = $state('')
  function counts(proof: AgentEditReview['current']['proof']) {
    const number = new Intl.NumberFormat(lang === 'de' ? 'de-DE' : 'en-US')
    return [proof.counts.messages, proof.counts.filed, proof.counts.inbox, proof.counts.copies].map(value => number.format(value)).join(' / ')
  }
  async function load() {
    busy = true; review = null; confirmed = false; message = ''
    try { review = await previewAgentEdits(account, summary, proposal) } catch { message = t.failure }
    finally { busy = false }
  }
  async function decide(disposition: 'accept' | 'reject') {
    const current = review
    const receipt = disposition === 'accept' ? current?.accept_receipt : current?.reject_receipt
    if (!current || !confirmed || !receipt) return
    busy = true
    try { await confirmAgentEdits(account, summary, proposal, disposition, receipt); message = disposition === 'accept' ? t.saved : t.rejected; review = null }
    catch { message = t.failure; review = null }
    finally { busy = false; confirmed = false }
  }
</script>

<section class="agent-rule-edits" aria-label={t.title}>
  <h5>{t.title}</h5>
  <p>{t.detail}</p>
  <p>{t.hold}</p>
  <button disabled={busy} onclick={load}>{t.load}</button>
  <p role="status" aria-live="polite">{message}</p>
  {#if review}
    <ul class="edits">
      {#each review.edits as edit (edit.digest)}
        <li data-edit={edit.digest}>{edit.kind === 'patch' ? t.patch : t.addition} · <code>{edit.requested}</code>{#if edit.requested !== edit.target} → {t.renamed} <code>{edit.target}</code>{/if}</li>
      {/each}
    </ul>
    <div class="side-by-side">
      <figure><figcaption>{t.original}</figcaption><pre>{review.original_documents.override_yaml}</pre></figure>
      <figure><figcaption>{t.resulting}</figcaption><pre>{review.resulting_documents.override_yaml}</pre></figure>
    </div>
    <details open><summary>{t.diff}</summary><pre>{review.document_diff}</pre></details>
    <h6>{t.proof}</h6>
    <div class="side-by-side proof">
      {#each [[t.current, review.current], [t.after, review.resulting]] as const as [label, side] (label)}
        <figure>
          <figcaption>{label}</figcaption>
          <p>{t.counts} · {counts(side.proof)}</p>
          <p>{t.gate} · {proofGateText(side.proof.gate, lang)}</p>
          <details><summary>{t.pins} · {side.proof.pins.length}</summary>{#each side.proof.pins as pin}<p>{pin.message_id_hash} · {pin.status}</p>{/each}</details>
          <details><summary>{t.omissions} · {side.proof.artifact.omissions.length}</summary>{#each side.proof.artifact.omissions as omission}<p>{omission.rule} · {omission.reason}</p>{/each}</details>
          <details><summary>{t.warnings}</summary><ProofDiagnostics diagnostics={side.proof.diagnostics} {lang} /></details>
        </figure>
      {/each}
    </div>
    <details open><summary>{t.folders}</summary><pre>{JSON.stringify(review.resulting.proof.diff, null, 2)}</pre></details>
    {#if !review.accept_receipt}<p role="alert">{t.blocked}</p>{/if}
    {#if !review.reject_receipt}<p role="alert">{t.full}</p>{/if}
    <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label>
    <button class="primary" disabled={busy || !confirmed || !review.accept_receipt} onclick={() => decide('accept')}>{t.accept}</button>
    <button disabled={busy || !confirmed || !review.reject_receipt} onclick={() => decide('reject')}>{t.reject}</button>
  {/if}
</section>

<style>
  .side-by-side { display: grid; grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr)); gap: 1rem; }
  figure { margin: 0; min-width: 0; }
  figcaption { font-weight: 600; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 24rem; overflow-y: auto; }
  p, li, label { overflow-wrap: anywhere; }
  button { margin-block: .5rem; }
</style>
