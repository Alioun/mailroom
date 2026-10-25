<script lang="ts">
  import { onDestroy } from 'svelte'
  import { openSieve, startSieve, cancelSieve, sieveStatus, exactReview, type SieveView } from './managesieve-api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { title: 'ManageSieve observations', unavailable: 'Production ManageSieve installation is missing. Production access and deployment remain locked.', open: 'Open local Sieve evidence', start: 'Discover installed test server', cancel: 'Cancel Sieve discovery', running: 'Sieve discovery running', error: 'Sieve action rejected. Existing observations were retained.', current: 'Current in installed test context', historical: 'Historical observations', none: 'No successful baseline captured', partial: 'Latest attempt and partial observations', completeNames: 'Complete returned script names', unknownNames: 'Script listing unavailable', empty: 'Complete listing returned zero scripts', noActive: 'No active script returned', active: 'Active', caps: 'Authenticated Sieve capabilities', private: 'Review exact private bytes', hide: 'Hide private bytes', time: 'Observed at', age: 'Age in seconds', limits: 'Limits', warning: 'Matching reads do not form a server transaction or exclude change-and-revert. Include dependencies are uncaptured. Advertisements do not change compiler authority.', restored: 'Recovery hold. Refresh and resumption remain blocked.' },
    de: { title: 'ManageSieve-Beobachtungen', unavailable: 'Produktive ManageSieve-Installation fehlt. Produktiver Zugriff und Bereitstellung bleiben gesperrt.', open: 'Lokale Sieve-Belege öffnen', start: 'Installierten Testserver erfassen', cancel: 'Sieve-Erfassung abbrechen', running: 'Sieve-Erfassung läuft', error: 'Sieve-Aktion abgelehnt. Vorhandene Beobachtungen bleiben erhalten.', current: 'Aktuell im installierten Testkontext', historical: 'Historische Beobachtungen', none: 'Keine erfolgreiche Bestandsaufnahme vorhanden', partial: 'Letzter Versuch und Teilbeobachtungen', completeNames: 'Vollständig zurückgegebene Skriptnamen', unknownNames: 'Skriptliste nicht verfügbar', empty: 'Vollständige Liste enthält keine Skripte', noActive: 'Kein aktives Skript zurückgegeben', active: 'Aktiv', caps: 'Authentifizierte Sieve-Fähigkeiten', private: 'Exakte private Bytes ansehen', hide: 'Private Bytes ausblenden', time: 'Beobachtet am', age: 'Alter in Sekunden', limits: 'Grenzen', warning: 'Übereinstimmende Lesevorgänge bilden keine Servertransaktion und schließen zwischenzeitliche Änderungen nicht aus. Include-Abhängigkeiten fehlen. Anzeigen ändern keine Compilerbefugnis.', restored: 'Wiederherstellungssperre. Aktualisierung und Fortsetzung bleiben gesperrt.' },
  }
  let t = $derived(strings[lang])
  let view = $state<SieveView | null>(null)
  let running = $state(false)
  let busy = $state(false)
  let error = $state('')
  let review = $state(false)
  let active = true
  let timer: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => { active = false; clearTimeout(timer); view = null })
  async function poll() {
    try {
      const status = await sieveStatus(account)
      if (!active) return
      if (status.running) timer = setTimeout(() => void poll(), 200)
      else {
        const next = await openSieve(account)
        if (active) { view = next; running = false }
      }
    } catch { if (active) { error = t.error; running = false } }
  }
  async function act(action: 'open' | 'start' | 'cancel') {
    busy = true; error = ''; review = false
    try {
      if (action === 'open') view = await openSieve(account)
      if (action === 'start') { await startSieve(account); running = true; void poll() }
      if (action === 'cancel') await cancelSieve(account)
    } catch (e) { error = e instanceof Error && e.message === 'unavailable' ? t.unavailable : t.error }
    finally { busy = false }
  }
  function time(value: number) { return new Date(value * 1000).toLocaleString(lang) }
</script>
<section aria-label={t.title} class="sieve">
  <h3>{t.title}</h3>
  <p>{t.unavailable}</p>
  <div class="actions">
    <button disabled={!account || busy || running} onclick={() => act('open')}>{t.open}</button>
    <button disabled={!view?.installed || view.recovery_hold || busy || running} onclick={() => act('start')}>{t.start}</button>
    <button disabled={!running || busy} onclick={() => act('cancel')}>{t.cancel}</button>
  </div>
  {#if running}<p role="status">{t.running}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if view}
    {#if view.recovery_hold}<p>{t.restored}</p>{/if}
    {#if view.facts.latest && !running}
      {@const latest = view.facts.latest}
      <h4>{t.partial}</h4><p>{latest.outcome} · {latest.id} · {time(latest.started_at)}{latest.failure ? ` · ${latest.failure}` : ''}</p>
      {#if latest.capabilities}<p>{t.caps}</p><ul>{#each Object.entries(latest.capabilities) as [name, value]}<li>{name} {value}</li>{/each}</ul>{/if}
      {#if latest.names}<p>{t.completeNames}</p>{#if !latest.names.length}<p>{t.empty}</p>{/if}<ul>{#each latest.names as name}<li>{name.name} {name.active ? t.active : ''}</li>{/each}</ul>{:else}<p>{t.unknownNames}</p>{/if}
      {#if latest.bodies.length}<button onclick={() => review = !review}>{review ? t.hide : t.private}</button>{#if review}{#each latest.bodies as body}<p>{body.name} · SHA-256 {body.sha256}</p><pre>{exactReview(body)}</pre>{/each}{/if}<p>{latest.bodies.length} / {latest.names?.filter(n => n.active || n.name === 'Open-Xchange').length ?? 0}</p>{/if}
    {/if}
    {#if view.facts.last_successful}
      {@const baseline = view.facts.last_successful}
      <h4>{view.current && !running ? t.current : t.historical}</h4>
      <p>{t.time} {time(baseline.observed_at)} · {t.age} {Math.max(0, Math.floor(Date.now() / 1000) - baseline.observed_at)}</p>
      <p>{baseline.context.profile} · {baseline.context.source_epoch} · TLS {baseline.context.tls_identity} · {baseline.context.adapter}</p>
      <p>{t.caps}</p><ul>{#each Object.entries(baseline.capabilities) as [name, value]}<li>{name} {value}</li>{/each}</ul>
      <p>{t.completeNames}</p><ul>{#each baseline.names as name}<li>{name.name} {name.active ? t.active : ''}</li>{/each}</ul>
      {#if !baseline.names.some(n => n.active)}<p>{t.noActive}</p>{/if}
      <button onclick={() => review = !review}>{review ? t.hide : t.private}</button>
      {#each baseline.bodies as body}<p>{body.name} · {body.bytes.length} bytes · SHA-256 {body.sha256}</p>{#if review}<pre>{exactReview(body)}</pre>{/if}{/each}
    {:else}<p>{t.none}</p>{/if}
    <p>{t.limits} · {view.limits.script_bytes} bytes/script · {view.limits.attempt_bytes} bytes/attempt · {view.limits.names} names</p>
  {/if}
  <p>{t.warning}</p>
</section>
<style>
  .sieve { margin-block: 1rem; padding: 1rem; border: 1px solid #879397; border-radius: .4rem; overflow-wrap: anywhere; }
  .actions { display: flex; flex-wrap: wrap; gap: .5rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 28rem; overflow: auto; }
</style>
