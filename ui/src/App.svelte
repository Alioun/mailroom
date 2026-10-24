<script lang="ts">
  import { onMount } from 'svelte'
  import { ToggleGroup } from '@ark-ui/svelte/toggle-group'
  import { load, evaluate, type Availability, type Sample, type Preview } from './api'
  import { strings, operationLabels } from './strings'
  let languages = $state(['en'])
  let lang: 'en' | 'de' = $derived(languages.includes('de') ? 'de' : 'en')
  let t = $derived(strings[lang])
  let sample = $state<Sample | null>(null)
  let availability = $state<Availability>([])
  let overrides = $state(true)
  let filters = $state(['all'])
  type State = { kind: 'initial' } | { kind: 'loading' } | { kind: 'done'; result: Preview } | { kind: 'error'; message: string }
  let previewState = $state<State>({ kind: 'initial' })
  let visible = $derived(previewState.kind === 'done' ? previewState.result.decisions.filter(d => filters.includes('inbox') ? d.destination.kind === 'inbox' : filters.includes('copies') ? d.copies.length > 0 : true) : [])
  $effect(() => { document.documentElement.lang = lang })
  async function initialize() {
    previewState = { kind: 'loading' }
    try {
      const data = await load()
      sample = data.sample
      availability = data.availability
      previewState = { kind: 'initial' }
    } catch (error) { previewState = { kind: 'error', message: error instanceof Error ? error.message : 'Invalid response' } }
  }
  async function run() {
    if (!sample) return
    previewState = { kind: 'loading' }
    try {
      const input = overrides ? sample : { ...sample, overrides: { ...sample.overrides, patches: [], additions: [] } }
      previewState = { kind: 'done', result: await evaluate(input) }
    } catch (error) { previewState = { kind: 'error', message: error instanceof Error ? error.message : 'Invalid response' } }
  }
  function reason(value: string) {
    if (value === 'preset') return t.preset
    if (value === 'preset patched by override') return t.patched
    if (value === 'added by override') return t.added
    return t.unmatched
  }
  onMount(() => { void initialize() })
</script>

<div class="app-shell">
  <header>
    <a href="/" class="wordmark">mailroom<span aria-hidden="true">.</span></a>
    <ToggleGroup.Root class="language" bind:value={languages} deselectable={false} aria-label={t.language}>
      <ToggleGroup.Item value="en">English</ToggleGroup.Item><ToggleGroup.Item value="de">Deutsch</ToggleGroup.Item>
    </ToggleGroup.Root>
  </header>
  <main>
    <div class="intro"><h1>{t.title}</h1><p>{t.intro}</p><div class="status-line"><span class="source">{t.synthetic}</span><span>{t.locked}</span></div></div>
    <section class="sample" aria-labelledby="sample-heading">
      <div><h2 id="sample-heading">{t.sample}</h2><p>{t.sampleDetail}</p></div>
      <div class="sample-controls">
        <label><input type="checkbox" bind:checked={overrides} disabled={previewState.kind === 'loading'} onchange={() => previewState = { kind: 'initial' }} />{t.overrides}</label>
        <p class="help">{t.overrideDetail}</p>
        <button class="primary" onclick={run} disabled={!sample || previewState.kind === 'loading'}>{previewState.kind === 'loading' ? t.running : t.run}<span aria-hidden="true"> →</span></button>
      </div>
    </section>
    <section class="results" aria-labelledby="results-heading" aria-busy={previewState.kind === 'loading'}>
      <h2 id="results-heading">{t.results}</h2>
      <div aria-live="polite">
        {#if previewState.kind === 'error'}
          <div class="error" role="alert"><h3>{t.error}</h3><p>{previewState.message}</p><button onclick={initialize}>{t.retry}</button></div>
        {:else if previewState.kind === 'loading'}<p class="empty">{t.running}</p>
        {:else if previewState.kind === 'initial'}<div class="empty"><h3>{t.initial}</h3><p>{t.initialDetail}</p></div>
        {:else}
          <dl class="counts">
            <div><dt>{t.messages}</dt><dd>{previewState.result.counts.messages}</dd></div><div><dt>{t.filed}</dt><dd>{previewState.result.counts.filed}</dd></div>
            <div><dt>{t.inbox}</dt><dd>{previewState.result.counts.inbox}</dd></div><div><dt>{t.copies}</dt><dd>{previewState.result.counts.copies}</dd></div>
          </dl>
          <p class="help">{previewState.result.counts.copied_messages} {t.copiedMessages}. {previewState.result.semantics}</p>
          <ToggleGroup.Root class="filters" bind:value={filters} deselectable={false} aria-label={t.results}>
            <ToggleGroup.Item value="all">{t.all}</ToggleGroup.Item><ToggleGroup.Item value="inbox">{t.retained}</ToggleGroup.Item><ToggleGroup.Item value="copies">{t.withCopies}</ToggleGroup.Item>
          </ToggleGroup.Root>
          <div class="decisions">
            <div class="table-heading" aria-hidden="true"><span>{t.message}</span><span>{t.destination}</span><span>{t.reason}</span></div>
            {#each visible as decision (decision.id)}
              <article class="decision">
                <div><h3>{decision.subject || t.subjectMissing}</h3><p class="sender">{decision.from}</p></div>
                <div class="filing"><span class:retained={decision.destination.kind === 'inbox'}>{decision.destination.kind === 'inbox' ? t.inbox : decision.destination.name}</span></div>
                <div class="reason"><strong>{decision.matched_rule ?? t.unmatched}</strong><p>{reason(decision.reason)}</p></div>
                <div class="copy-line">{#each decision.copies as copy}<span>{t.copy} → {copy.recipient} <small>({copy.rule})</small></span>{:else}<span>{t.noCopy}</span>{/each}</div>
              </article>
            {:else}<p class="empty">{t.empty}</p>{/each}
          </div>
          <details><summary>{t.program}</summary><h3>{t.sieve}</h3><pre>{previewState.result.sieve}</pre><h3>{t.trace}</h3><ul>{#each previewState.result.trace as line}<li>{line}</li>{/each}</ul></details>
        {/if}
      </div>
    </section>
    <section class="limitations" aria-labelledby="limits-heading"><h2 id="limits-heading">{t.limits}</h2><p>{t.limitsDetail}</p><p>{t.subset}</p><p>{t.omissions}</p>
      {#if sample}<details><summary>{t.fixtures}</summary><pre>{JSON.stringify(sample, null, 2)}</pre></details>{/if}
    </section>
    <section class="operations" aria-labelledby="operations-heading"><h2 id="operations-heading">{t.boundary}</h2><p>{t.boundaryDetail}</p><p>{t.unavailableDetail}</p>
      <ul>{#each availability.filter(a => !a.allowed) as action}<li><span>{operationLabels[lang][action.operation]}</span><span class="availability">{action.effect === 'account_write' ? t.denied : t.unavailable}</span></li>{/each}</ul>
    </section>
  </main>
  <footer><span>mailroom</span><span>{t.footer}</span></footer>
</div>
