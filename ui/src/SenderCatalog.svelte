<script lang="ts">
  import { senderCategory, type WinddownReport } from './winddown-api'
  let { catalog, lang }: { catalog: NonNullable<WinddownReport['sender_catalog']>; lang: 'en' | 'de' } = $props()
  const copy = {
    en: {
      title: 'Bundled sender catalog', release: 'Release', rows: 'Rows with recorded sender reviews',
      incomplete: 'The first sender list is incomplete. It requires at least 20 reviewed rows per category, covering Germany, Austria, Switzerland and international senders.',
      complete: 'The recorded first-list counts and country coverage are present. Installation does not independently prove the recorded reviews.',
      category: 'Category', count: 'Rows', missing: 'Categories below 20 rows', countries: 'Missing country coverage', international: 'International',
      stale: 'Rows last checked at least two years ago', future: 'Rows checked after the report date', services: 'Explicit transactional-service reviews', unusable: 'Service reviews unavailable for current classification',
      limit: 'Category membership alone does not establish a transactional service. Stale or future reviews leave that classification unresolved. Catalog data authenticates no sender and authorizes no mail action.',
      money: 'Money', contracts: 'Contracts', orders: 'Orders', travel: 'Travel', social: 'Social', newsletters: 'Newsletters',
    },
    de: {
      title: 'Mitgelieferte Absenderdaten', release: 'Version', rows: 'Zeilen mit dokumentierter Absenderprüfung',
      incomplete: 'Die erste Absenderliste ist unvollständig. Sie benötigt mindestens 20 geprüfte Zeilen je Kategorie sowie Abdeckung für Deutschland, Österreich, die Schweiz und internationale Absender.',
      complete: 'Die dokumentierten Mindestzahlen und Länder sind vorhanden. Die Installation bestätigt die dokumentierten Prüfungen nicht unabhängig.',
      category: 'Kategorie', count: 'Zeilen', missing: 'Kategorien mit weniger als 20 Zeilen', countries: 'Fehlende Länderabdeckung', international: 'International',
      stale: 'Zuletzt vor mindestens zwei Jahren geprüfte Zeilen', future: 'Nach dem Berichtsdatum geprüfte Zeilen', services: 'Ausdrückliche Prüfungen als Transaktionsdienst', unusable: 'Für die aktuelle Zuordnung nicht nutzbare Dienstprüfungen',
      limit: 'Kategoriezugehörigkeit allein belegt keinen Transaktionsdienst. Veraltete oder zukünftige Prüfungen lassen diese Zuordnung ungeklärt. Absenderdaten authentifizieren keinen Absender und erlauben keine Nachrichtenaktion.',
      money: 'Finanzen', contracts: 'Verträge', orders: 'Bestellungen', travel: 'Reisen', social: 'Soziale Netzwerke', newsletters: 'Newsletter',
    },
  }
  let t = $derived(copy[lang])
  function number(value: number) { return new Intl.NumberFormat(lang === 'de' ? 'de-DE' : 'en-US').format(value) }
</script>

<details class="sender-catalog">
  <summary>{t.title}</summary>
  <p>{t.release}: {catalog.release} · {catalog.license}</p>
  <p>{catalog.seed_complete ? t.complete : t.incomplete}</p>
  <p>{t.rows}: {number(catalog.rows)}</p>
  <table><thead><tr><th scope="col">{t.category}</th><th scope="col">{t.count}</th></tr></thead><tbody>
    {#each senderCategory.options as category (category)}<tr><th scope="row">{t[category]}</th><td>{number(catalog.categories[category])}</td></tr>{/each}
  </tbody></table>
  {#if catalog.missing_categories.length}<p>{t.missing}: {catalog.missing_categories.map(category => t[category]).join(', ')}</p>{/if}
  {#if catalog.missing_countries.length}<p>{t.countries}: {catalog.missing_countries.map(country => country === '*' ? t.international : country).join(', ')}</p>{/if}
  <p>{t.stale}: {number(catalog.stale_rows)} · {t.future}: {number(catalog.future_rows)}</p>
  <p>{t.services}: {number(catalog.service_reviews)} · {t.unusable}: {number(catalog.unusable_service_reviews)}</p>
  <p>{t.limit}</p>
</details>

<style>
  .sender-catalog { margin-block: 20px; overflow-wrap: anywhere; }
  table { border-collapse: collapse; }
  th, td { padding: 5px 15px 5px 0; text-align: start; }
  td { font-variant-numeric: tabular-nums; }
</style>
