<script lang="ts">
  import { onMount } from 'svelte'
  import { ToggleGroup } from '@ark-ui/svelte/toggle-group'
  import Census from './Census.svelte'
  import Accounts from './Accounts.svelte'
  import RulesProof from './RulesProof.svelte'
  import { load, type Availability } from './api'
  import { strings, operationLabels } from './strings'
  let languages = $state(['en'])
  let lang: 'en' | 'de' = $derived(languages.includes('de') ? 'de' : 'en')
  let t = $derived(strings[lang])
  let availability = $state<Availability>([])
  let availabilityError = $state(false)
  $effect(() => { document.documentElement.lang = lang })
  onMount(() => { void load().then(data => { availability = data.availability }).catch(() => { availabilityError = true }) })
</script>

<div class="app-shell">
  <header>
    <a href="/" class="wordmark">mailroom<span aria-hidden="true">.</span></a>
    <ToggleGroup.Root class="language" bind:value={languages} deselectable={false} aria-label={t.language}>
      <ToggleGroup.Item value="en">English</ToggleGroup.Item><ToggleGroup.Item value="de">Deutsch</ToggleGroup.Item>
    </ToggleGroup.Root>
  </header>
  <main>
    <div class="intro"><h1>{t.title}</h1><p>{t.intro}</p><div class="status-line"><span class="source">{lang === 'en' ? 'Local interface' : 'Lokale Oberfläche'}</span><span>{t.locked}</span></div></div>
    <Accounts {lang} />
    <Census {lang} />
    <RulesProof {lang} />
    <section class="operations" aria-labelledby="operations-heading"><h2 id="operations-heading">{t.boundary}</h2><p>{t.boundaryDetail}</p><p>{t.unavailableDetail}</p>
      {#if availabilityError}<p role="alert">{lang === 'en' ? 'Could not load account availability. Reload the page.' : 'Kontoaktionen konnten nicht geladen werden. Lade die Seite neu.'}</p>{/if}
      <ul>{#each availability.filter(a => !a.allowed) as action}<li><span>{operationLabels[lang][action.operation]}</span><span class="availability">{action.effect === 'account_write' ? t.denied : t.unavailable}</span></li>{/each}</ul>
    </section>
  </main>
  <footer><span>mailroom</span><span>{t.footer}</span></footer>
</div>
