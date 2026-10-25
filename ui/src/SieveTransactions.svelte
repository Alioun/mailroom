<script lang="ts">
  import { onDestroy } from 'svelte'
  import { transactionStatus, transactionPreview, transactionCommand, reconcileTransaction, type TransactionView, type TransactionPreview } from './sieve-transactions-api'
  import { sieveStatus, exactReview } from './managesieve-api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { statuses: { running: 'Running', complete: 'Provider state observed', refused: 'Refused', uncertain: 'Uncertain', stopped: 'Stopped', conflict: 'Conflict', fresh_review: 'Fresh exact review required' }, outcomes: { intent: 'Intent persisted', acknowledged: 'Command acknowledged', observed: 'Readback observed', refused: 'Refused', uncertain: 'Uncertain', conflict: 'Conflict', preimage: 'Original state matches' }, effects: { upload: 'Replace existing script bytes', activate: 'Activate target', disable: 'Disable all scripts with empty SETACTIVE' }, ack: 'Acknowledgment received', yes: 'Yes', no: 'No', pending: 'Pending', title: 'Fixture-owned Sieve transactions', unavailable: 'Production and read-only installations cannot write. A separate synthetic installation owns this entire existing target script.', open: 'Open transaction history', preview: 'Preview exact replacement', confirm: 'Confirm exact synthetic transaction', approval: 'I reviewed the exact bytes, active selection and immediate effects.', cancel: 'Stop further commands', reconcile: 'Reconcile without writes', compensation: 'Preview conditional compensation', before: 'Before', after: 'After', private: 'Review exact private bytes', hide: 'Hide private bytes', running: 'Synthetic transaction running', error: 'Transaction rejected or interrupted. Reload history and reconcile uncertainty before a fresh review.', empty: 'No active script', limitation: 'CHECKSCRIPT checks syntax only. HAVESPACE reserves nothing. Conservative saved-account proof does not establish delivery or coexistence. Active replacement affects future mail immediately. Another editor can race the final read and write. Compensation cannot reverse forwards or processed mail. Empty SETACTIVE disables all scripts and requires this separate exact confirmation. No automatic retry or compensation.', fresh: 'Uncertainty, conflict or a matching preimage requires reconciliation and renewed exact review. Observed state does not prove attribution or historical non-occurrence.', steps: 'Validation, space check, upload and explicit activation as displayed', digest: 'Artifact SHA-256' },
    de: { statuses: { running: 'Läuft', complete: 'Serverzustand beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', stopped: 'Gestoppt', conflict: 'Konflikt', fresh_review: 'Erneute exakte Prüfung erforderlich' }, outcomes: { intent: 'Absicht gespeichert', acknowledged: 'Befehl bestätigt', observed: 'Rücklesen beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', conflict: 'Konflikt', preimage: 'Vorzustand stimmt überein' }, effects: { upload: 'Vorhandene Skriptbytes ersetzen', activate: 'Ziel aktivieren', disable: 'Alle Skripte mit leerem SETACTIVE deaktivieren' }, ack: 'Bestätigung erhalten', yes: 'Ja', no: 'Nein', pending: 'Ausstehend', title: 'Sieve-Transaktionen im Testbesitz', unavailable: 'Produktive und reine Leseinstallationen können nicht schreiben. Eine separate synthetische Installation besitzt dieses gesamte vorhandene Zielskript.', open: 'Transaktionsverlauf öffnen', preview: 'Exakten Ersatz prüfen', confirm: 'Exakte synthetische Transaktion bestätigen', approval: 'Ich habe die exakten Bytes, die aktive Auswahl und die sofortigen Folgen geprüft.', cancel: 'Weitere Befehle stoppen', reconcile: 'Ohne Schreibzugriff abgleichen', compensation: 'Bedingte Kompensation prüfen', before: 'Vorher', after: 'Nachher', private: 'Exakte private Bytes ansehen', hide: 'Private Bytes ausblenden', running: 'Synthetische Transaktion läuft', error: 'Transaktion abgelehnt oder unterbrochen. Verlauf neu laden und Ungewissheit vor einer neuen Prüfung abgleichen.', empty: 'Kein aktives Skript', limitation: 'CHECKSCRIPT prüft nur Syntax. HAVESPACE reserviert nichts. Der konservative Nachweis belegt weder Zustellung noch Koexistenz. Aktiver Ersatz wirkt sofort auf zukünftige Nachrichten. Andere Editoren können zwischen letztem Lesen und Schreiben ändern. Kompensation macht Weiterleitungen oder verarbeitete Nachrichten nicht rückgängig. Leeres SETACTIVE deaktiviert alle Skripte und erfordert diese separate exakte Bestätigung. Keine automatische Wiederholung oder Kompensation.', fresh: 'Ungewissheit, Konflikt oder übereinstimmender Vorzustand erfordern Abgleich und erneute exakte Prüfung. Beobachtung beweist weder Verursachung noch historisches Ausbleiben.', steps: 'Validierung, Platzprüfung, Upload und ausdrückliche Aktivierung wie angezeigt', digest: 'Artefakt SHA-256' },
  }
  let t = $derived(strings[lang])
  let view = $state<TransactionView | null>(null)
  let preview = $state<TransactionPreview | null>(null)
  let approved = $state(false)
  let show = $state(false)
  let busy = $state(false)
  let running = $state(false)
  let error = $state('')
  let alive = true
  let timer: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => { alive = false; clearTimeout(timer); preview = null; view = null })
  async function poll() {
    try {
      const status = await sieveStatus(account)
      if (!alive) return
      if (status.running) timer = setTimeout(() => void poll(), 200)
      else { view = await transactionStatus(account); running = false }
    } catch { if (alive) { error = t.error; running = false } }
  }
  async function act(action: 'open' | 'preview' | 'confirm' | 'cancel' | 'reconcile' | 'compensation', operation?: string) {
    busy = true; error = ''; show = false
    try {
      if (action === 'confirm' && preview && approved) {
        const p = preview; preview = null; approved = false
        await transactionCommand(account, p.transaction.compensates ? { action: 'compensation_confirm', operation: p.transaction.compensates, receipt: p.receipt, confirmed: true } : { action: 'confirm', receipt: p.receipt, confirmed: true })
        running = true; void poll()
      } else if (action === 'cancel') await transactionCommand(account, { action: 'cancel' })
      else if (action === 'reconcile' && operation) { preview = null; view = await reconcileTransaction(account, operation) }
      else if (action === 'preview' || action === 'compensation') { approved = false; preview = await transactionPreview(account, operation) }
      else if (action === 'open') { preview = null; approved = false; view = await transactionStatus(account) }
    } catch { error = t.error; preview = null; approved = false }
    finally { busy = false }
  }
</script>
<section class="sieve-transactions" aria-label={t.title}>
  <h3>{t.title}</h3><p>{t.unavailable}</p>
  <div class="actions"><button disabled={!account || busy || running} onclick={() => act('open')}>{t.open}</button><button disabled={!view?.installed || view.recovery_hold || busy || running} onclick={() => act('preview')}>{t.preview}</button><button disabled={!running || busy} onclick={() => act('cancel')}>{t.cancel}</button></div>
  {#if error}<p role="alert">{error}</p>{/if}{#if running}<p role="status">{t.running}</p>{/if}
  {#if preview}
    <h4>{preview.transaction.compensates ? t.compensation : t.preview}</h4>
    <p>{preview.transaction.target} · {preview.transaction.ownership_epoch}</p>
    {#if 'gate' in preview.proof}<p>{lang === 'de' ? 'Nachweis' : 'Proof'} · {preview.proof.gate} · {preview.proof.semantics}</p><ul>{#each preview.proof.warnings as warning}<li>{warning}</li>{/each}{#each preview.proof.pins as pin}<li>{pin.message_id_hash} · {pin.status}</li>{/each}</ul><p>{preview.proof.runtime_cross_check}</p>{/if}<p>{t.digest} {preview.transaction.artifact}</p><p>{t.steps} · CHECKSCRIPT · HAVESPACE · {preview.effects.map(effect => t.effects[effect]).join(' · ')}</p>
    <button onclick={() => show = !show}>{show ? t.hide : t.private}</button>
    {#each [{ label: t.before, image: preview.transaction.before }, { label: t.after, image: preview.transaction.after }] as item}
      <h5>{item.label} · {item.image.names.find(n => n.active)?.name ?? t.empty}</h5>
      {#each item.image.bodies as body}<p>{body.name} · {body.bytes.length} bytes · SHA-256 {body.sha256}</p>{#if show}<pre>{exactReview(body)}</pre>{/if}{/each}
    {/each}
    <label><input type="checkbox" bind:checked={approved} />{t.approval}</label>
    <button disabled={!approved || busy || running} onclick={() => act('confirm')}>{t.confirm}</button>
  {/if}
  {#if view}{#each view.journal.history as transaction}
    <article><h4>{transaction.compensates ? t.compensation : t.preview} · {t.statuses[transaction.status]}</h4><p>{transaction.id}</p>
      <p>CHECKSCRIPT · {transaction.validation ? 'Ok' in transaction.validation ? t.outcomes.acknowledged : transaction.validation.Err : t.pending} · HAVESPACE · {transaction.space ? 'Ok' in transaction.space ? t.outcomes.acknowledged : transaction.space.Err : t.pending}</p>
      <ul>{#each transaction.steps as step}<li>{t.effects[step.effect]} · {t.outcomes[step.outcome]} · {t.ack} {step.acknowledgment ? t.yes : t.no} {step.failure ?? ''}</li>{/each}</ul>
      <button disabled={busy || running} onclick={() => act('reconcile', transaction.id)}>{t.reconcile}</button>
      {#if !transaction.compensates}<button disabled={busy || running || view.recovery_hold} onclick={() => act('compensation', transaction.id)}>{t.compensation}</button>{/if}
    </article>
  {/each}{/if}
  <p>{t.fresh}</p><p>{t.limitation}</p>
</section>
<style>
  section { margin-block: 1rem; padding: 1rem; border: 1px solid #879397; border-radius: .4rem; overflow-wrap: anywhere; }
  .actions { display: flex; flex-wrap: wrap; gap: .5rem; }
  article { border-top: 1px solid #879397; margin-top: 1rem; padding-top: .6rem; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 28rem; overflow: auto; }
  label { display: block; margin-block: 1rem; }
</style>
