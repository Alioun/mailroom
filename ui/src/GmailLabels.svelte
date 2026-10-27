<script lang="ts">
  import { readAccountStatus, sortingPost, type BacklogOptions } from './api'
  import { labelPreviewFor, type LabelOperation, type LabelPreview } from './folder-provisioning-api'
  let { account, lang, offer, options, history, limitations, disabled, onchange, onreload }: { account: string; lang: 'en' | 'de'; offer: boolean; options: BacklogOptions; history: LabelOperation[]; limitations: string; disabled: boolean; onchange: () => Promise<void>; onreload: () => Promise<void> } = $props()
  const text = {
    en: {
      title: 'Missing destination labels', preview: 'Preview exact label creation', confirm: 'I confirm creating exactly these labels, parents first, each read back before the next',
      create: 'Create labels', reconcile: 'Reconcile label observations only', journal: 'Label creation journal', existing: 'exists', missing: 'create', ancestor: 'parent', leaf: 'destination',
      error: 'Label action rejected. Reload and request a fresh preview. Existing evidence is retained.', wait: 'Creating labels. Each label is read back through LIST before the next.',
      status: { running: 'running', complete: 'complete', refused: 'refused', uncertain: 'uncertain', stopped: 'stopped', conflict: 'conflict', fresh_review: 'needs a fresh preview' },
      outcome: { intent: 'sent, no answer yet', acknowledged: 'acknowledged', observed: 'read back', refused: 'refused', uncertain: 'uncertain', conflict: 'conflict', absent: 'absent' },
    },
    de: {
      title: 'Fehlende Ziellabels', preview: 'Genaue Labelerstellung anzeigen', confirm: 'Ich bestätige, genau diese Labels zu erstellen, übergeordnete zuerst, jedes vor dem nächsten geprüft',
      create: 'Labels erstellen', reconcile: 'Nur Labelbeobachtungen abgleichen', journal: 'Journal der Labelerstellung', existing: 'vorhanden', missing: 'erstellen', ancestor: 'übergeordnet', leaf: 'Ziel',
      error: 'Labelaktion abgelehnt. Neu laden und frische Vorschau anfordern. Vorhandene Nachweise bleiben erhalten.', wait: 'Labels werden erstellt. Jedes Label wird vor dem nächsten über LIST geprüft.',
      status: { running: 'läuft', complete: 'abgeschlossen', refused: 'abgelehnt', uncertain: 'ungewiss', stopped: 'gestoppt', conflict: 'Konflikt', fresh_review: 'braucht eine frische Vorschau' },
      outcome: { intent: 'gesendet, noch keine Antwort', acknowledged: 'bestätigt', observed: 'geprüft', refused: 'abgelehnt', uncertain: 'ungewiss', conflict: 'Konflikt', absent: 'nicht vorhanden' },
    },
  }
  let t = $derived(text[lang])
  const limitationsDe = 'Nur synthetische Gmail-X-GM-EXT-1-Quelle über Loopback. Labels stammen aus den Zielen, die der Gmail-Sortiervorschau für dieselben Optionen fehlen; jedes fehlende übergeordnete Label ist ein eigener ausdrücklicher CREATE-Befehl, von oben nach unten, und jedes wird vor dem nächsten über LIST geprüft. Namen werden in der genauen Schreibweise und modifiziertem UTF-7 des Plans erstellt und nie normalisiert. Ein Label, das sich nur in Groß- und Kleinschreibung unterscheidet, ein reservierter Name oder eines im Systembereich wird abgelehnt, nie umbenannt oder ersatzweise verwendet. Mailroom benennt Labels nie um, löscht sie nie und erstellt sie nie über einen Labelbefehl. CREATE hat kein Compare-and-Swap: Ein anderer Client kann nach dem letzten Lesen ein Label erstellen, umbenennen oder entfernen, und Gmail kann übergeordnete Labels selbst anlegen oder eine andere Schreibweise speichern; Mailroom verzeichnet dann einen Konflikt und wiederholt nichts. Kein automatisches Wiederholen, Fortsetzen, Aufräumen oder Rückgängigmachen. Gmails Labelerstellung ist eine ungeprüfte Annahme der Testumgebung.'
  let preview = $state<LabelPreview | null>(null)
  let confirmed = $state(false)
  let busy = $state(false)
  let running = $state(false)
  let error = $state('')
  async function act(fn: () => Promise<void>) { busy = true; error = ''; try { await fn() } catch { error = t.error } finally { busy = false } }
  async function inspect() { preview = null; confirmed = false; preview = await labelPreviewFor(account, options) }
  async function create() {
    if (!preview || !confirmed) return
    const receipt = preview.receipt
    preview = null; confirmed = false
    await sortingPost('gmail', account, { action: 'label_confirm', options, receipt, confirmed: true })
    running = true
    try {
      for (let i = 0; i < 600; i++) {
        await new Promise(resolve => setTimeout(resolve, 250))
        if ((await readAccountStatus()).job?.progress.state !== 'running') break
      }
    } finally { running = false }
    await onchange()
  }
</script>
<section class="gmail-labels" aria-label={t.title}>
  <h4>{t.title}</h4>
  {#if offer}<button disabled={disabled || busy || running} onclick={() => act(inspect)}>{t.preview}</button>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if running}<p role="status">{t.wait}</p>{/if}
  {#if preview}
    {#each preview.operation.plan.entries as e}<p class="label-step">{e.name.decoded} · {e.name.wire} · {e.role === 'leaf' ? t.leaf : t.ancestor} · {e.existing ? t.existing : t.missing}</p>{/each}
    <label><input type="checkbox" bind:checked={confirmed} disabled={disabled || busy} />{t.confirm}</label>
    <button disabled={disabled || busy || !confirmed} onclick={() => act(create)}>{t.create}</button>
  {/if}
  {#if history.length}
    <h5>{t.journal}</h5>
    {#each history as op (op.id)}
      <article data-label-operation={op.id}><p class="label-state">{t.status[op.status]}</p>
        {#each op.steps as s}<p>{s.name.decoded} · {t.outcome[s.outcome]}</p>{/each}
        <button disabled={disabled || busy || running} onclick={() => act(async () => { await sortingPost('gmail', account, { action: 'label_reconcile', operation: op.id }); await onreload() })}>{t.reconcile}</button>
      </article>
    {/each}
  {/if}
  {#if offer || history.length}<p>{lang === 'de' ? limitationsDe : limitations}</p>{/if}
</section>
<style>
  .gmail-labels { margin-block: 1rem; padding-block: .5rem; border-block: 1px dashed #7775; }
  p, label { overflow-wrap: anywhere; }
  label { display: block; margin-block: .5rem; }
</style>
