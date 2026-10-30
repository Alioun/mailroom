<script lang="ts">
  import type { AgentProposal } from './agent-exchange'
  let { proposal, lang }: { proposal: AgentProposal; lang: 'en' | 'de' } = $props()
  const copy = {
    en: { title: 'Agent recommendations', detail: 'These recommendations match the exact summary above. This match is not owner approval or verified account evidence. Review the text before editing local rules. Run fresh proof after any edit.', reason: 'Supporting observations and limits', change: 'Suggested local change', digest: 'Summary SHA-256', hold: 'Opening this file saves no rules and performs no mail action.' },
    de: { title: 'Agentenvorschläge', detail: 'Diese Vorschläge passen genau zur Zusammenfassung oben. Das ist keine Freigabe und kein geprüfter Kontobeleg. Prüfe den Text vor Änderungen an lokalen Regeln. Starte nach jeder Änderung eine neue Prüfung.', reason: 'Beobachtungen und Grenzen', change: 'Vorgeschlagene lokale Änderung', digest: 'SHA-256 der Zusammenfassung', hold: 'Das Öffnen dieser Datei speichert keine Regeln und führt keine Mailaktion aus.' },
  }
  const t = $derived(copy[lang])
</script>

<section class="agent-proposal-review" aria-label={t.title}>
  <h5>{t.title}</h5>
  <p>{t.detail}</p>
  <p>{t.hold}</p>
  <details><summary>{t.digest}</summary><code>{proposal.summary_sha256}</code></details>
  {#each proposal.recommendations as recommendation, index (index)}
    <article>
      <h6>{recommendation.title}</h6>
      <p>{t.reason}</p><pre>{recommendation.reason}</pre>
      <p>{t.change}</p><pre>{recommendation.suggested_change}</pre>
    </article>
  {/each}
</section>

<style>
  article { border-block-start: 1px solid var(--border, #aaa); margin-block-start: 1rem; padding-block-start: 1rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 20rem; overflow-y: auto; }
  code { overflow-wrap: anywhere; }
</style>
