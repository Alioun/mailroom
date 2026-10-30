<script lang="ts">
  import { gmailRoutePlan, gmailRouteText, type RouteLanguage } from './gmail-route'
  let { lang }: { lang: RouteLanguage } = $props()
  const t = $derived(gmailRoutePlan[lang])
  const download = $derived(`data:text/plain;charset=utf-8,${encodeURIComponent(gmailRouteText(lang))}`)
</script>

<details class="gmail-route">
  <summary>{t.title}</summary>
  <p class="route-summary">{t.summary}</p>
  <p>{t.boundary}</p>
  <p class="help">{t.private}</p>
  <ol>
    {#each gmailRoutePlan.stages as stage (stage.id)}
      <li>
        <h4>{stage[lang].title}</h4>
        <p>{stage[lang].body}</p>
      </li>
    {/each}
  </ol>
  <p>{t.next}</p>
  <p><a href={download} download="gmail-route-preparation.txt">{t.download}</a></p>
  <p class="help">{t.wizard} <code>./scripts/prepare-gmail-route.sh --plan</code></p>
  <details>
    <summary>{t.sources}</summary>
    <ul>{#each gmailRoutePlan.sources as source (source.url)}<li><a href={source.url} target="_blank" rel="noreferrer noopener">{source.label}</a></li>{/each}</ul>
  </details>
</details>

<style>
  .gmail-route { margin-block: 1rem; overflow-wrap: anywhere; }
  .route-summary { color: var(--ink); font-weight: 600; }
  ol { padding-inline-start: 1.5rem; max-width: 76ch; }
  li { padding-inline-start: .35rem; margin-block: 1.25rem; }
  h4 { margin: 0; font-size: 1rem; }
  code { white-space: normal; }
  a[download] { display: inline-block; padding-block: .65rem; }
</style>
