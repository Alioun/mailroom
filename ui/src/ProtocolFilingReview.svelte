<script lang="ts">
  import { isCopyProtocolRun, type ProtocolPreview } from './api'
  import { archiveCopy } from './archive-copy'
  let { preview, lang, available, disabled, onconfirm }: { preview: ProtocolPreview; lang: 'en' | 'de'; available: boolean; disabled: boolean; onconfirm: () => Promise<void> } = $props()
  const text = {
    en: { title: 'Review filing changes', copy: 'File with COPY and source removal', copyStep: 'Copy these messages to their reviewed destinations and verify the new copies.', flagStep: 'Mark only unchanged source messages as Deleted, using their observed versions.', expungeStep: 'Remove only those verified source UIDs with UID EXPUNGE.', preserve: 'Other messages and their flags stay outside this action. Cancel stops new batches. Undo requires a separate review.', hold: 'COPY filing is unavailable in this build. This review cannot start it.', moveConfirm: 'I confirm these exact UID MOVE changes', copyConfirm: 'I confirm these exact copies and source removals', start: 'File this reviewed list', empty: 'No eligible filing changes remain.', from: 'From', to: 'To', messages: 'Message UIDs', flags: 'Preserved message flags', none: 'None', detail: 'Exact local review record', account: 'Account', receipt: 'Confirmation receipt', saved: 'Saved proof', observed: 'Fresh observation proof' },
    de: { title: 'Ablageänderungen prüfen', copy: 'Mit COPY ablegen und Quellen entfernen', copyStep: 'Diese Nachrichten in die geprüften Zielordner kopieren und die neuen Kopien prüfen.', flagStep: 'Nur unveränderte Quellnachrichten anhand ihrer beobachteten Versionen als Deleted markieren.', expungeStep: 'Nur diese geprüften Quell-UIDs mit UID EXPUNGE entfernen.', preserve: 'Andere Nachrichten und ihre Flags bleiben außerhalb dieser Aktion. Abbrechen stoppt neue Stapel. Rückgängigmachen braucht eine eigene Prüfung.', hold: 'COPY-Ablage ist in dieser Version nicht verfügbar. Diese Prüfung kann sie nicht starten.', moveConfirm: 'Ich bestätige diese genauen UID-MOVE-Änderungen', copyConfirm: 'Ich bestätige diese genauen Kopien und Quellentfernungen', start: 'Geprüfte Liste ablegen', empty: 'Keine geeigneten Ablageänderungen bleiben übrig.', from: 'Aus', to: 'Nach', messages: 'Nachrichten-UIDs', flags: 'Erhaltene Nachrichtenflags', none: 'Keine', detail: 'Genauer lokaler Prüfbeleg', account: 'Konto', receipt: 'Bestätigungsbeleg', saved: 'Gespeicherter Nachweis', observed: 'Nachweis aus frischer Beobachtung' },
  }
  let t = $derived(text[lang])
  let copy = $derived(isCopyProtocolRun(preview.operation))
  let archive = $derived(preview.operation.plan.filing.options.intent === 'archive_unmatched')
  let archiveText = $derived(archiveCopy(lang))
  let empty = $derived(preview.operation.batches.length === 0 || preview.operation.plan.filing.changes.length === 0)
  let confirmed = $state(false)
  async function apply() { if (confirmed && available && !disabled && !empty) await onconfirm() }
</script>
<div class="filing-review">
  <h4>{archive ? archiveText.title : t.title}</h4>
  {#if copy}
    <p>{t.copy}</p>
    <ol><li>{t.copyStep}</li><li>{t.flagStep}</li><li>{t.expungeStep}</li></ol>
    <p>{t.preserve}</p>
    {#if !available}<p role="status">{t.hold}</p>{/if}
  {/if}
  <ul class="destinations">
    {#each preview.operation.batches as batch}
      <li><p>{t.from} {batch.intent.source.name.decoded} · {t.to} {batch.intent.destination.name.decoded}</p><p>{t.messages} · {Object.keys(batch.intent.messages).join(', ')}</p></li>
    {/each}
  </ul>
  <details><summary>{t.detail}</summary>
    <dl><dt>{t.account}</dt><dd>{preview.operation.plan.binding.account}</dd><dt>{t.receipt}</dt><dd>{preview.receipt}</dd><dt>{t.saved}</dt><dd>{preview.operation.plan.saved_proof}</dd><dt>{t.observed}</dt><dd>{preview.operation.plan.execution_proof}</dd></dl>
    {#each preview.operation.batches as batch}<p>{t.from} {batch.intent.source.name.decoded} / UIDVALIDITY {batch.intent.source.uidvalidity} · {t.to} {batch.intent.destination.name.decoded} / UIDVALIDITY {batch.intent.destination.uidvalidity}</p>{/each}
    {#each preview.operation.plan.filing.changes as change}<p>{change.source.folder} / {change.source.uidvalidity} / {change.source.uid} → {change.destination} · {change.sender} · {t.flags} · {change.flags.join(', ') || t.none}</p>{/each}
    <pre>{preview.operation.plan.filing.artifact.executable}</pre>
  </details>
  {#if empty}<p>{t.empty}</p>{:else}
    <fieldset disabled={disabled || !available}>
      <label><input type="checkbox" bind:checked={confirmed} />{copy ? t.copyConfirm : archive ? archiveText.confirm : t.moveConfirm}</label>
      <button disabled={disabled || !available || !confirmed} onclick={apply}>{archive ? archiveText.start : t.start}</button>
    </fieldset>
  {/if}
</div>
<style>
  .filing-review { margin-block: 1.5rem; max-width: 72ch; }
  fieldset { border: 0; padding: 0; margin-block: 1rem; min-width: 0; }
  label { display: block; margin-block: 1rem; }
  .destinations li { margin-block: 1rem; }
  dl { display: grid; grid-template-columns: minmax(5rem, auto) minmax(0, 1fr); gap: .5rem 1rem; }
  dd { margin: 0; }
  p, dt, dd, label, li { overflow-wrap: anywhere; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
