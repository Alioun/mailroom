<script lang="ts">
  import { onMount } from 'svelte'
  import { openInvoices, previewInvoice, sendInvoice, recoverInvoice, prepareInvoiceMailApp, type InvoiceView, type InvoicePreview } from './api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { title: 'Missed invoices', refresh: 'Refresh invoice report', business: 'Business candidates', unresolved: 'Unresolved role or evidence', preview: 'Preview exact invoice', target: 'Named target', confirm: 'Confirm this named synthetic send', send: 'Send exactly this invoice synthetically', recover: 'Reconcile synthetic acknowledgment', fallback: 'Prepare mail-app forwarding', source: 'Original message and attachments', error: 'Invoice action rejected. Reload the report and exact preview. Uncertain sends are never retried automatically.', limits: 'Synthetic confirmation only. Native OS dialog, SMTP and native forward-original integration are unavailable. Qonto acceptance remains unknown. Completed sends have no Undo.', proposal: 'Unapproved sender suggestion', predicted: 'Predicted copies', rules: 'Matching copy rules', acceptance: 'Accounting acceptance', empty: 'No current report rows. Save a rule workspace first.', stale: 'Report version', preparation: 'Preparation only. Nothing launched.' },
    de: { title: 'Möglicherweise fehlende Rechnungen', refresh: 'Rechnungsbericht aktualisieren', business: 'Geschäftliche Kandidaten', unresolved: 'Ungeklärte Rolle oder Belege', preview: 'Genaue Rechnung prüfen', target: 'Benanntes Ziel', confirm: 'Benannten synthetischen Versand bestätigen', send: 'Genau diese Rechnung synthetisch senden', recover: 'Synthetische Empfangsbestätigung abgleichen', fallback: 'Weiterleitung im Mailprogramm vorbereiten', source: 'Originalnachricht und Anhänge', error: 'Rechnungsaktion abgelehnt. Bericht und genaue Vorschau neu laden. Ungewisser Versand wird nie automatisch wiederholt.', limits: 'Nur synthetische Bestätigung. Betriebssystemdialog, SMTP und native Originalweiterleitung fehlen. Qonto-Annahme bleibt unbekannt. Abgeschlossener Versand hat kein Rückgängig.', proposal: 'Unbestätigter Absendervorschlag', predicted: 'Vorhergesagte Kopien', rules: 'Passende Kopierregeln', acceptance: 'Buchhaltungsannahme', empty: 'Keine aktuellen Zeilen. Zuerst ein Regelwerk speichern.', stale: 'Berichtsversion', preparation: 'Nur Vorbereitung. Nichts geöffnet.' },
  }
  let t = $derived(strings[lang])
  let view = $state<InvoiceView | null>(null)
  let preview = $state<InvoicePreview | null>(null)
  let target = $state('')
  let confirmation = $state(false)
  let busy = $state(false)
  let error = $state('')
  let fallback = $state('')
  function invalidate() { preview = null; confirmation = false; fallback = '' }
  async function act(action: () => Promise<void>) { busy = true; error = ''; try { await action() } catch { error = t.error; invalidate() } finally { busy = false } }
  async function refresh() { invalidate(); view = await openInvoices(account); if (!view.targets.includes(target)) target = view.targets[0] ?? '' }
  onMount(() => { void act(refresh) })
</script>
<section class="invoices" aria-label={t.title}>
  <h3>{t.title} <span class="source" aria-label="Invoice account count">{view?.report.count ?? '?'}</span></h3>
  <button disabled={busy} onclick={() => act(refresh)}>{t.refresh}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if view}
    <p>{t.business} · {view.report.eligible_business} · {t.unresolved} · {view.report.unresolved}</p>
    <p class="help">{t.stale} · {view.report.version} · {view.report.semantics} · {view.report.complete ? 'complete census' : 'partial census'}</p>
    <label>{t.target}<select bind:value={target} disabled={busy} onchange={invalidate}>{#each view.targets as name}<option value={name}>{name}</option>{/each}</select></label>
    {#each view.report.warnings as warning}<p class="help">{warning}</p>{/each}
    {#if !view.report.rows.length}<p>{t.empty}</p>{/if}
    {#each view.report.rows as row (row.id)}
      <article data-invoice={row.id}>
        <h4>{row.subject}</h4><p>{row.sender} · {row.role} · {row.handoff}</p>
        <p>{t.predicted} · {row.predicted_targets.join(', ') || '0'} · {t.rules} · {row.matching_rules.join(', ') || '0'}</p>
        {#each row.copy_eligibility as eligibility}<p>{eligibility.rule} · {eligibility.status} · {eligibility.reason}</p>{/each}
        <p>{t.acceptance} · {row.accounting_acceptance}</p>
        {#each row.reasons as reason}<p>{reason}</p>{/each}
        {#if row.sender_proposal}<p>{t.proposal} · {row.sender_proposal}</p>{/if}
        {#if row.send_available}<button disabled={busy || !target} onclick={() => act(async () => { invalidate(); preview = await previewInvoice(account, row.id, target) })}>{t.preview}</button><button disabled={busy || !target} onclick={() => act(async () => { invalidate(); const prepared = await prepareInvoiceMailApp(account, row.id, target); preview = prepared.preview; fallback = prepared.limitation })}>{t.fallback}</button>{/if}
      </article>
    {/each}
    {#if preview}
      <div class="confirmation" data-invoice-preview>
        <p>{preview.plan.target_name} · {preview.plan.target}</p><p>{preview.plan.sending_address}</p><p>{preview.plan.source.folder} / {preview.plan.source.uidvalidity} / {preview.plan.source.uid}</p>
        <p>{preview.plan.confirmation_action}</p><p class="help">{preview.plan.content_digest}</p>
        <h4>{t.source}</h4><pre>{preview.plan.source.original}</pre>
        {#if fallback}<p>{t.preparation} {fallback}</p>{:else}<label><input type="checkbox" bind:checked={confirmation} disabled={busy} />{t.confirm}</label><button disabled={busy || !confirmation} onclick={() => act(async () => { if (!preview) return; view = await sendInvoice(account, preview); invalidate() })}>{t.send}</button>{/if}
      </div>
    {/if}
    {#each view.sends as send (send.id)}<p data-invoice-status>{send.status} · {send.plan.selection} · {send.plan.target_name}</p>{#if send.status !== 'completed'}<button disabled={busy} onclick={() => act(async () => { view = await recoverInvoice(account, send.id); invalidate() })}>{t.recover}</button>{/if}{/each}
  {/if}
  <p class="help">{t.limits}</p>
</section>
<style>
  .invoices { margin-block: 2rem; padding-top: 1rem; border-top: 1px solid #7775; }
  article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  p { overflow-wrap: anywhere; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 24rem; overflow: auto; }
  button { margin: .4rem .4rem .4rem 0; }
  label { display: block; margin-block: .7rem; }
</style>
