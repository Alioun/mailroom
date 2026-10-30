<script lang="ts">
  import { proofNoticeText, proofRuntimeText, type ProofDiagnostics } from './proof-diagnostics'
  let { diagnostics, lang }: { diagnostics: ProofDiagnostics; lang: 'en' | 'de' } = $props()
</script>

<ul class="proof-notices">
  {#each diagnostics.notices as notice}<li>{proofNoticeText(notice, lang)}</li>{/each}
</ul>
<p>{proofRuntimeText(diagnostics.runtime_cross_check, lang)}</p>

<style>
  .proof-notices { padding-inline-start: 1.25rem; }
  li { margin-block: .6rem; }
  li, p { overflow-wrap: anywhere; }
</style>
