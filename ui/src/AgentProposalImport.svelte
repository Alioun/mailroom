<script lang="ts">
  import AgentProposalReview from './AgentProposalReview.svelte'
  import AgentRuleEditReview from './AgentRuleEditReview.svelte'
  import { readAgentProposal, type AgentProposal } from './agent-exchange'
  import type { SummaryBinding } from './api'
  let { account, summary, binding, lang }: { account: string; summary: string; binding: SummaryBinding; lang: 'en' | 'de' } = $props()
  const copy = {
    en: { title: 'Open agent recommendations', detail: 'Choose a proposal JSON returned from the separate agent exchange. It must match this exact summary. The text stays in this browser view.', file: 'Proposal JSON file', loading: 'Checking the proposal against the summary…', failure: 'Proposal refused. Use a valid proposal for this exact summary, with at most 20 recommendations, at most 20 rule edits and a file no larger than 128 KiB.' },
    de: { title: 'Agentenvorschläge öffnen', detail: 'Wähle die JSON-Vorschlagsdatei aus dem getrennten Agentenaustausch. Sie muss genau zu dieser Zusammenfassung passen. Der Text bleibt in dieser Browseransicht.', file: 'JSON-Vorschlagsdatei', loading: 'Vorschlag wird mit der Zusammenfassung verglichen…', failure: 'Vorschlag abgelehnt. Verwende einen gültigen Vorschlag für genau diese Zusammenfassung, mit höchstens 20 Empfehlungen, höchstens 20 Regeländerungen und einer Datei bis 128 KiB.' },
  }
  const t = $derived(copy[lang])
  type Import = { kind: 'empty' } | { kind: 'loading' } | { kind: 'loaded'; proposal: AgentProposal; summary: string } | { kind: 'error' }
  let imported = $state<Import>({ kind: 'empty' })
  async function open(file: File | undefined) {
    imported = { kind: 'empty' }
    if (!file) return
    imported = { kind: 'loading' }
    const current = summary
    try {
      if (file.size > 131_072) throw new Error('Proposal too large')
      const proposal = await readAgentProposal(await file.text(), current)
      imported = current === summary ? { kind: 'loaded', proposal, summary: current } : { kind: 'error' }
    } catch { imported = { kind: 'error' } }
  }
</script>

<details class="agent-proposal-import">
  <summary>{t.title}</summary>
  <p>{t.detail}</p>
  <label>{t.file}<input type="file" accept=".json,application/json" disabled={imported.kind === 'loading'} onchange={event => void open(event.currentTarget.files?.[0])} /></label>
  {#if imported.kind === 'loading'}<p role="status">{t.loading}</p>{/if}
  {#if imported.kind === 'error'}<p role="alert">{t.failure}</p>{/if}
  {#if imported.kind === 'loaded' && imported.summary === summary}
    <AgentProposalReview proposal={imported.proposal} {lang} />
    {#if imported.proposal.rule_edits}<AgentRuleEditReview {account} summary={binding} proposal={imported.proposal} {lang} />{/if}
  {/if}
</details>
