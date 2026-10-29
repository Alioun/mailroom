<script lang="ts">
  import { onMount } from 'svelte'
  import { invoiceDiagnosticText, invoiceStatusText } from './invoice-diagnostics'
  import { openInvoices, acquireInvoice, previewInvoice, sendInvoice, recoverInvoice, resolveInvoice, type InvoiceView, type InvoicePreview, type InvoiceRoute, type Resolution } from './api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { noCandidates: 'No invoice candidates in this report. Refresh after acquiring more mail or reviewing the rule workspace.', complete: 'Complete census', partial: 'Partial census', count: 'Invoice candidate count', unknown: 'Unknown', copyDetails: 'Copy proof details', technical: 'Technical proof details in English', title: 'Missed invoices', refresh: 'Refresh invoice report', business: 'Business candidates', unresolved: 'Unresolved role or evidence', preview: 'Preview exact invoice', target: 'Named target', confirm: 'Confirm this named synthetic send', send: 'Send exactly this invoice synthetically', recover: 'Reconcile synthetic acknowledgment', source: 'Original message and attachments', error: 'Invoice action rejected. Reload the report and exact preview. Uncertain sends are never retried automatically.', locked: 'Locked. Real sends and their records are denied until the owner authorizes live validation.', limits: 'The synthetic transport sends only inside the fixture. SMTP sends reach the account’s real server only after the OS confirmation names the exact action, and stay locked until live validation is authorized. Forwarding in Mail is not offered, because Mail cannot be held to the confirmed message and sender; forward it yourself if needed. Acceptance by the selected accounting tool remains unknown. Sent mail has no Undo.', proposal: 'Unapproved sender suggestion', predicted: 'Predicted copies', rules: 'Matching copy rules', acceptance: 'Accounting acceptance', empty: 'No current report rows. Save a rule workspace first.', stale: 'Report version', acquire: 'Read the original (read-only)', smtp: 'Preview sending by SMTP', confirmSmtp: 'Confirm this exact SMTP send', sendSmtp: 'Send exactly this invoice by SMTP', uncertain: 'Uncertain send. Check the Sent folder before recording what happened. Nothing is resent.', evidence: 'Sent-folder copies with the same Message-ID in the last census', noEvidence: 'No copy found in the last census. Refresh the census to see newer Sent mail.', checked: 'I checked the Sent folder', sent: 'Record as sent', notSent: 'Record as not sent', status: { intent: 'Recording intent', uncertain: 'Uncertain', completed: 'Sent', not_sent: 'Not sent', handed_off: 'Opened in Mail, sending unknown' }, resolved: { sent: 'recorded as sent by the owner', not_sent: 'recorded as not sent by the owner' } },
    de: { noCandidates: 'Dieser Bericht enthält keine Rechnungskandidaten. Aktualisiere ihn nach einer weiteren Erfassung oder nach der Prüfung des Regelwerks.', complete: 'Vollständige Erfassung', partial: 'Unvollständige Erfassung', count: 'Anzahl der Rechnungskandidaten', unknown: 'Unbekannt', copyDetails: 'Prüfdetails zur Kopie', technical: 'Technische Prüfdetails auf Englisch', title: 'Möglicherweise fehlende Rechnungen', refresh: 'Rechnungsbericht aktualisieren', business: 'Geschäftliche Kandidaten', unresolved: 'Ungeklärte Rolle oder Belege', preview: 'Genaue Rechnung prüfen', target: 'Benanntes Ziel', confirm: 'Benannten synthetischen Versand bestätigen', send: 'Genau diese Rechnung synthetisch senden', recover: 'Synthetische Empfangsbestätigung abgleichen', source: 'Originalnachricht und Anhänge', error: 'Rechnungsaktion abgelehnt. Bericht und genaue Vorschau neu laden. Ungewisser Versand wird nie automatisch wiederholt.', locked: 'Gesperrt. Echter Versand und seine Vermerke bleiben gesperrt, bis der Inhaber die Live-Prüfung freigibt.', limits: 'Der synthetische Versand sendet nur innerhalb der Testumgebung. SMTP-Versand erreicht den echten Server des Kontos erst, nachdem die Betriebssystembestätigung die genaue Aktion nennt, und bleibt bis zur Freigabe der Live-Prüfung gesperrt. Weiterleiten in Mail wird nicht angeboten, weil Mail nicht an die bestätigte Nachricht und den Absender gebunden werden kann; leite sie bei Bedarf selbst weiter. Die Annahme durch das gewählte Buchhaltungstool bleibt unbekannt. Gesendete Mail hat kein Rückgängig.', proposal: 'Unbestätigter Absendervorschlag', predicted: 'Vorhergesagte Kopien', rules: 'Passende Kopierregeln', acceptance: 'Buchhaltungsannahme', empty: 'Keine aktuellen Zeilen. Zuerst ein Regelwerk speichern.', stale: 'Berichtsversion', acquire: 'Originalnachricht lesen (nur lesend)', smtp: 'Versand per SMTP prüfen', confirmSmtp: 'Genau diesen SMTP-Versand bestätigen', sendSmtp: 'Genau diese Rechnung per SMTP senden', uncertain: 'Ungewisser Versand. Prüfe den Ordner Gesendet, bevor du vermerkst, was geschah. Nichts wird erneut gesendet.', evidence: 'Kopien mit derselben Message-ID im Ordner Gesendet der letzten Erfassung', noEvidence: 'Keine Kopie in der letzten Erfassung gefunden. Erfassung aktualisieren, um neuere gesendete Mail zu sehen.', checked: 'Ich habe den Ordner Gesendet geprüft', sent: 'Als gesendet vermerken', notSent: 'Als nicht gesendet vermerken', status: { intent: 'Absicht wird gespeichert', uncertain: 'Ungewiss', completed: 'Gesendet', not_sent: 'Nicht gesendet', handed_off: 'In Mail geöffnet, Versand unbekannt' }, resolved: { sent: 'vom Inhaber als gesendet vermerkt', not_sent: 'vom Inhaber als nicht gesendet vermerkt' } },
  }
  let t = $derived(strings[lang])
  let view = $state<InvoiceView | null>(null)
  let preview = $state<InvoicePreview | null>(null)
  let target = $state('')
  let confirmation = $state(false)
  let checked = $state<Record<string, boolean>>({})
  let busy = $state(false)
  let error = $state('')
  const mode = $derived(preview?.plan.mode)
  function invalidate() { preview = null; confirmation = false }
  async function act(action: () => Promise<void>) { busy = true; error = ''; try { await action() } catch (e) { error = e instanceof Error && e.message.startsWith('Denied:') ? t.locked : t.error; invalidate() } finally { busy = false } }
  async function refresh() { invalidate(); view = await openInvoices(account); if (!view.targets.includes(target)) target = view.targets[0] ?? '' }
  async function open(selection: string, route?: InvoiceRoute) { invalidate(); preview = await previewInvoice(account, selection, target, route) }
  async function resolve(send: string, outcome: Resolution) { view = await resolveInvoice(account, send, outcome); checked = {}; invalidate() }
  onMount(() => { void act(refresh) })
</script>
<section class="invoices" aria-label={t.title}>
  <h3>{t.title} <span class="source" aria-label={t.count}>{view?.report.count ?? '?'}</span></h3>
  <button disabled={busy} onclick={() => act(refresh)}>{t.refresh}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if view}
    <p>{t.business} · {view.report.eligible_business} · {t.unresolved} · {view.report.unresolved}</p>
    <p class="help">{t.stale} · {view.report.version} · {view.report.semantics} · {view.report.complete ? t.complete : t.partial}</p>
    <label>{t.target}<select bind:value={target} disabled={busy} onchange={invalidate}>{#each view.targets as name}<option value={name}>{name}</option>{/each}</select></label>
    {#each view.report.warnings as warning}
      <p class="help">{invoiceDiagnosticText(warning, lang)}</p>
      {#if warning.kind === 'rule_omitted'}<details><summary>{t.technical}</summary><pre lang="en">{warning.detail}</pre></details>{/if}
    {/each}
    {#if !view.report.rows.length}<p>{view.report.warnings.some(warning => warning.kind === 'workspace_required') ? t.empty : t.noCandidates}</p>{/if}
    {#each view.report.rows as row (row.id)}
      <article data-invoice={row.id}>
        <h4>{row.subject}</h4><p>{row.sender} · {invoiceStatusText[lang].role[row.role]} · {invoiceStatusText[lang].handoff[row.handoff]}</p>
        <p>{t.predicted} · {row.predicted_targets.join(', ') || '0'} · {t.rules} · {row.matching_rules.join(', ') || '0'}</p>
        {#each row.copy_eligibility as eligibility}<details><summary>{t.copyDetails} · {eligibility.rule}</summary><p lang="en">{eligibility.status} · {eligibility.reason}</p></details>{/each}
        <p>{t.acceptance} · {t.unknown}</p>
        {#each row.reasons as reason}<p>{invoiceDiagnosticText(reason, lang)}</p>{/each}
        {#if row.sender_proposal}<p>{t.proposal} · {row.sender_proposal}</p>{/if}
        {#if row.send_available}
          {#if view.synthetic}<button disabled={busy || !target} onclick={() => act(() => open(row.id))}>{t.preview}</button>{/if}
          <button disabled={busy} onclick={() => act(async () => { view = await acquireInvoice(account, row.id); invalidate() })}>{t.acquire}</button>
          <button disabled={busy || !target} onclick={() => act(() => open(row.id, 'smtp'))}>{t.smtp}</button>
        {/if}
      </article>
    {/each}
    {#if preview}
      <div class="confirmation" data-invoice-preview>
        <p>{preview.plan.target_name} · {preview.plan.target}</p><p>{preview.plan.sending_address}</p><p>{preview.plan.source.folder} / {preview.plan.source.uidvalidity} / {preview.plan.source.uid}</p>
        <p lang="en">{preview.plan.confirmation_action}</p>
        {#if preview.confirmation}<pre data-exact-confirmation>{preview.confirmation.display}</pre>{/if}<p class="help">{preview.plan.content_digest}</p>
        <h4>{t.source}</h4><pre data-invoice-source>{preview.plan.source.original}</pre>
        <label><input type="checkbox" bind:checked={confirmation} disabled={busy} />{mode === 'smtp' ? t.confirmSmtp : t.confirm}</label>
        <button disabled={busy || !confirmation} onclick={() => act(async () => { if (!preview) return; view = await sendInvoice(account, preview); invalidate() })}>{mode === 'smtp' ? t.sendSmtp : t.send}</button>
      </div>
    {/if}
    {#each view.sends as send (send.id)}
      <p data-invoice-status>{t.status[send.status]}{send.resolved ? ` · ${t.resolved[send.resolved]}` : ''} · {send.plan.selection} · {send.plan.target_name}</p>
      {#if send.plan.mode === 'synthetic_transport' && send.status !== 'completed'}<button disabled={busy} onclick={() => act(async () => { view = await recoverInvoice(account, send.id); invalidate() })}>{t.recover}</button>{/if}
      {#if send.status === 'uncertain' && send.plan.mode !== 'synthetic_transport'}
        <div class="uncertain" data-invoice-uncertain={send.id}>
          <p>{t.uncertain}</p>
          <p>{t.evidence}</p>
          {#each view.sent_evidence[send.id] ?? [] as copy}<p>{copy}</p>{:else}<p>{t.noEvidence}</p>{/each}
          <label><input type="checkbox" bind:checked={checked[send.id]} disabled={busy} />{t.checked}</label>
          <button disabled={busy || !checked[send.id]} onclick={() => act(() => resolve(send.id, 'sent'))}>{t.sent}</button>
          <button disabled={busy || !checked[send.id]} onclick={() => act(() => resolve(send.id, 'not_sent'))}>{t.notSent}</button>
        </div>
      {/if}
    {/each}
  {/if}
  <p class="help">{t.limits}</p>
</section>
<style>
  .invoices { margin-block: 2rem; padding-top: 1rem; border-top: 1px solid #7775; }
  article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  .uncertain { border-left: 3px solid #b5651d; padding-left: .75rem; }
  p { overflow-wrap: anywhere; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 24rem; overflow: auto; }
  button { margin: .4rem .4rem .4rem 0; }
  label { display: block; margin-block: .7rem; }
</style>
