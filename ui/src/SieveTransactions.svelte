<script lang="ts">
  import { onDestroy } from 'svelte'
  import { RadioGroup } from '@ark-ui/svelte/radio-group'
  import { transactionStatus, transactionPreview, transactionCommand, reconcileTransaction, recordPlacement, type TransactionView, type TransactionPreview, type Placement } from './sieve-transactions-api'
  import { sieveStatus, exactReview } from './managesieve-api'
  let { account, lang }: { account: string; lang: 'en' | 'de' } = $props()
  const strings = {
    en: { statuses: { running: 'Running', complete: 'Provider state observed', refused: 'Refused', uncertain: 'Uncertain', stopped: 'Stopped', conflict: 'Conflict', fresh_review: 'Fresh exact review required' }, outcomes: { intent: 'Intent persisted', acknowledged: 'Command acknowledged', observed: 'Readback observed', refused: 'Refused', uncertain: 'Uncertain', conflict: 'Conflict', preimage: 'Original state matches' }, effects: { upload: 'Write the host script with existing rules unchanged', upload_owned: "Write Mailroom's own included script", activate: 'Activate target', disable: 'Disable all scripts with empty SETACTIVE' }, purposes: { deploy: 'Deploy after existing rules', removal: 'Remove Mailroom content', compensation: 'Conditional compensation' }, reasons: { placement_unchosen: 'No placement is recorded. Composed deploys stay refused until the owner records one.', invalid_placement: 'The recorded placement is not valid for this host script.', host_missing: 'The host script does not exist.', host_inactive: 'The host script is not the active script. Activating it would also switch existing rules on or off.', unparseable: 'Existing content does not parse. Nothing is rewritten.', marker_damaged: 'A Mailroom marker is damaged or appears in existing content.', owned_name_foreign: 'A script with the chosen include name exists and is not Mailroom-owned.', unsafe_rule: 'An existing rule cannot be placed before Mailroom safely.', mailroom_unreachable: 'An existing rule always stops processing, so Mailroom rules would never run.', mailroom_content_invalid: 'Mailroom content cannot be composed.', proof_blocked: 'The composed proof is blocked.', nothing_to_remove: 'No Mailroom content is present.' }, ack: 'Acknowledgment received', yes: 'Yes', no: 'No', pending: 'Pending', title: 'Sieve rules alongside existing rules', unavailable: 'Production and read-only installations cannot write. Existing rules run first and stay byte-identical. Mailroom writes only its own marked content after them.', placement: 'Placement of Mailroom rules', none: 'No placement recorded', block: 'Marked block inside the host script', include: 'Own script with one include line', script: 'Mailroom script name', record: 'Record placement choice', clear: 'Clear placement choice', open: 'Open transaction history', preview: 'Preview composed deploy', removal: 'Preview removal of Mailroom content', confirm: 'Confirm exact synthetic transaction', approval: 'I reviewed the exact bytes, active selection, existing-rule precedence and immediate effects.', cancel: 'Stop further commands', reconcile: 'Reconcile without writes', compensation: 'Preview conditional compensation', before: 'Before', after: 'After', private: 'Review exact private bytes', hide: 'Hide private bytes', running: 'Synthetic transaction running', error: 'Transaction rejected or interrupted. Reload history and reconcile uncertainty before a fresh review.', refused: 'Refused before any write', empty: 'No active script', existing: 'Existing rules, in order', precedence: 'Existing rule takes precedence', undetermined: 'Precedence undetermined', mailroom: 'Mailroom', stops: 'can stop processing', continues: 'never stops processing', counts: 'Census messages: Mailroom decides {m}, existing rules decide {e} ({c} with another destination), undetermined {u}, accounting copies stopped {s}', limitation: 'CHECKSCRIPT checks syntax only. HAVESPACE reserves nothing. Conservative saved-account proof does not establish delivery or coexistence. OX editor behavior is unverified. Active replacement affects future mail immediately. Another editor can race the final read and write. Compensation cannot reverse forwards or processed mail. Removal leaves Mailroom\'s own included script in place. No automatic retry or compensation.', fresh: 'Uncertainty, conflict or a matching preimage requires reconciliation and renewed exact review. Observed state does not prove attribution or historical non-occurrence.', steps: 'Validation, space check and writes as displayed', digest: 'Artifact SHA-256' },
    de: { statuses: { running: 'Läuft', complete: 'Serverzustand beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', stopped: 'Gestoppt', conflict: 'Konflikt', fresh_review: 'Erneute exakte Prüfung erforderlich' }, outcomes: { intent: 'Absicht gespeichert', acknowledged: 'Befehl bestätigt', observed: 'Rücklesen beobachtet', refused: 'Abgelehnt', uncertain: 'Ungewiss', conflict: 'Konflikt', preimage: 'Vorzustand stimmt überein' }, effects: { upload: 'Hauptskript schreiben, vorhandene Regeln unverändert', upload_owned: 'Eigenes eingebundenes Mailroom-Skript schreiben', activate: 'Ziel aktivieren', disable: 'Alle Skripte mit leerem SETACTIVE deaktivieren' }, purposes: { deploy: 'Nach vorhandenen Regeln einsetzen', removal: 'Mailroom-Inhalt entfernen', compensation: 'Bedingte Kompensation' }, reasons: { placement_unchosen: 'Keine Platzierung festgelegt. Zusammengesetzte Einsätze bleiben abgelehnt, bis der Inhaber eine festlegt.', invalid_placement: 'Die festgelegte Platzierung passt nicht zu diesem Hauptskript.', host_missing: 'Das Hauptskript existiert nicht.', host_inactive: 'Das Hauptskript ist nicht aktiv. Eine Aktivierung würde auch vorhandene Regeln ein- oder ausschalten.', unparseable: 'Vorhandener Inhalt ist nicht lesbar. Nichts wird umgeschrieben.', marker_damaged: 'Eine Mailroom-Markierung ist beschädigt oder steht in vorhandenem Inhalt.', owned_name_foreign: 'Ein Skript mit dem gewählten Namen existiert und gehört nicht Mailroom.', unsafe_rule: 'Eine vorhandene Regel lässt sich nicht sicher vor Mailroom einordnen.', mailroom_unreachable: 'Eine vorhandene Regel beendet die Verarbeitung immer, Mailroom-Regeln liefen nie.', mailroom_content_invalid: 'Mailroom-Inhalt lässt sich nicht zusammensetzen.', proof_blocked: 'Der zusammengesetzte Nachweis ist blockiert.', nothing_to_remove: 'Kein Mailroom-Inhalt vorhanden.' }, ack: 'Bestätigung erhalten', yes: 'Ja', no: 'Nein', pending: 'Ausstehend', title: 'Sieve-Regeln neben vorhandenen Regeln', unavailable: 'Produktive und reine Leseinstallationen können nicht schreiben. Vorhandene Regeln laufen zuerst und bleiben byte-genau erhalten. Mailroom schreibt nur eigenen markierten Inhalt dahinter.', placement: 'Platzierung der Mailroom-Regeln', none: 'Keine Platzierung festgelegt', block: 'Markierter Block im Hauptskript', include: 'Eigenes Skript mit einer include-Zeile', script: 'Name des Mailroom-Skripts', record: 'Platzierung festlegen', clear: 'Platzierung zurücksetzen', open: 'Transaktionsverlauf öffnen', preview: 'Zusammengesetzten Einsatz prüfen', removal: 'Entfernen des Mailroom-Inhalts prüfen', confirm: 'Exakte synthetische Transaktion bestätigen', approval: 'Ich habe die exakten Bytes, die aktive Auswahl, den Vorrang vorhandener Regeln und die sofortigen Folgen geprüft.', cancel: 'Weitere Befehle stoppen', reconcile: 'Ohne Schreibzugriff abgleichen', compensation: 'Bedingte Kompensation prüfen', before: 'Vorher', after: 'Nachher', private: 'Exakte private Bytes ansehen', hide: 'Private Bytes ausblenden', running: 'Synthetische Transaktion läuft', error: 'Transaktion abgelehnt oder unterbrochen. Verlauf neu laden und Ungewissheit vor einer neuen Prüfung abgleichen.', refused: 'Vor jedem Schreiben abgelehnt', empty: 'Kein aktives Skript', existing: 'Vorhandene Regeln in Reihenfolge', precedence: 'Vorhandene Regel hat Vorrang', undetermined: 'Vorrang unbestimmt', mailroom: 'Mailroom', stops: 'kann die Verarbeitung beenden', continues: 'beendet die Verarbeitung nie', counts: 'Zensusnachrichten: Mailroom entscheidet {m}, vorhandene Regeln {e} ({c} mit anderem Ziel), unbestimmt {u}, gestoppte Buchhaltungskopien {s}', limitation: 'CHECKSCRIPT prüft nur Syntax. HAVESPACE reserviert nichts. Der konservative Nachweis belegt weder Zustellung noch Koexistenz. Das Verhalten des OX-Editors ist ungeprüft. Aktiver Ersatz wirkt sofort auf zukünftige Nachrichten. Andere Editoren können zwischen letztem Lesen und Schreiben ändern. Kompensation macht Weiterleitungen oder verarbeitete Nachrichten nicht rückgängig. Entfernen lässt das eigene eingebundene Mailroom-Skript bestehen. Keine automatische Wiederholung oder Kompensation.', fresh: 'Ungewissheit, Konflikt oder übereinstimmender Vorzustand erfordern Abgleich und erneute exakte Prüfung. Beobachtung beweist weder Verursachung noch historisches Ausbleiben.', steps: 'Validierung, Platzprüfung und Schreibvorgänge wie angezeigt', digest: 'Artefakt SHA-256' },
  }
  let t = $derived(strings[lang])
  let view = $state<TransactionView | null>(null)
  let preview = $state<TransactionPreview | null>(null)
  let choice = $state<string | null>(null)
  let script = $state('')
  let approved = $state(false)
  let show = $state(false)
  let busy = $state(false)
  let running = $state(false)
  let error = $state('')
  let alive = true
  let timer: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => { alive = false; clearTimeout(timer); preview = null; view = null })
  const label = (p: Placement | null | undefined) => !p ? t.none : p.kind === 'block' ? t.block : `${t.include} · ${p.script}`
  const where = (d: { kind: string; name?: string } | null) => !d ? '?' : d.kind === 'inbox' ? 'INBOX' : d.kind === 'folder' ? d.name : d.kind
  const counted = (c: { mailroom: number; existing_wins: number; conflicting: number; undetermined: number; suppressed_copies: number }) => t.counts.replace('{m}', String(c.mailroom)).replace('{e}', String(c.existing_wins)).replace('{c}', String(c.conflicting)).replace('{u}', String(c.undetermined)).replace('{s}', String(c.suppressed_copies))
  async function poll() {
    try {
      const status = await sieveStatus(account)
      if (!alive) return
      if (status.running) timer = setTimeout(() => void poll(), 200)
      else { view = await transactionStatus(account); running = false }
    } catch { if (alive) { error = t.error; running = false } }
  }
  async function act(action: 'open' | 'preview' | 'removal' | 'confirm' | 'cancel' | 'reconcile' | 'compensation' | 'record' | 'clear', operation?: string) {
    busy = true; error = ''; show = false
    try {
      if (action === 'confirm' && preview?.outcome === 'ready' && approved) {
        const p = preview; preview = null; approved = false
        const tx = p.transaction
        await transactionCommand(account, tx.compensates ? { action: 'compensation_confirm', operation: tx.compensates, receipt: p.receipt, confirmed: true } : tx.composition?.purpose === 'removal' ? { action: 'removal_confirm', receipt: p.receipt, confirmed: true } : { action: 'confirm', receipt: p.receipt, confirmed: true })
        running = true; void poll()
      } else if (action === 'cancel') await transactionCommand(account, { action: 'cancel' })
      else if (action === 'reconcile' && operation) { preview = null; view = await reconcileTransaction(account, operation) }
      else if (action === 'preview' || action === 'removal' || action === 'compensation') { approved = false; preview = await transactionPreview(account, action === 'preview' ? 'deploy' : action === 'removal' ? 'removal' : { compensates: operation ?? '' }) }
      else if (action === 'record' || action === 'clear') { preview = null; approved = false; view = await recordPlacement(account, action === 'clear' || !choice ? null : choice === 'block' ? { kind: 'block' } : { kind: 'include', script }) }
      else if (action === 'open') { preview = null; approved = false; view = await transactionStatus(account) }
    } catch { error = t.error; preview = null; approved = false }
    finally { busy = false }
  }
</script>
<section class="sieve-transactions" aria-label={t.title}>
  <h3>{t.title}</h3><p>{t.unavailable}</p>
  <div class="actions"><button disabled={!account || busy || running} onclick={() => act('open')}>{t.open}</button><button disabled={!view?.installed || view.recovery_hold || busy || running} onclick={() => act('preview')}>{t.preview}</button><button disabled={!view?.installed || view.recovery_hold || busy || running} onclick={() => act('removal')}>{t.removal}</button><button disabled={!running || busy} onclick={() => act('cancel')}>{t.cancel}</button></div>
  {#if view}
    <fieldset class="placement"><legend>{t.placement}</legend><p>{label(view.placement)}</p>
      <RadioGroup.Root bind:value={choice} aria-label={t.placement}>
        <RadioGroup.Item value="block"><RadioGroup.ItemControl /><RadioGroup.ItemText>{t.block}</RadioGroup.ItemText><RadioGroup.ItemHiddenInput /></RadioGroup.Item>
        <RadioGroup.Item value="include"><RadioGroup.ItemControl /><RadioGroup.ItemText>{t.include}</RadioGroup.ItemText><RadioGroup.ItemHiddenInput /></RadioGroup.Item>
      </RadioGroup.Root>
      {#if choice === 'include'}<label>{t.script} <input bind:value={script} autocomplete="off" spellcheck="false" /></label>{/if}
      <div class="actions"><button disabled={!choice || (choice === 'include' && !script) || busy || running || view.recovery_hold} onclick={() => act('record')}>{t.record}</button><button disabled={!view.placement || busy || running || view.recovery_hold} onclick={() => act('clear')}>{t.clear}</button></div>
    </fieldset>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}{#if running}<p role="status">{t.running}</p>{/if}
  {#if preview?.outcome === 'refused'}
    <div class="refused" role="alert"><h4>{t.refused}</h4><p>{t.reasons[preview.refusal.reason]}</p>
      {#if preview.refusal.script}<p>{preview.refusal.script}{#if preview.refusal.rule} · {preview.refusal.rule}{/if}{#if preview.refusal.line} · {lang === 'de' ? 'Zeile' : 'line'} {preview.refusal.line}{/if}{#if preview.refusal.detail} · {preview.refusal.detail}{/if}</p>{/if}
    </div>
  {/if}
  {#if preview?.precedence}
    {@const p = preview.precedence}
    <div class="precedence"><h4>{t.existing} · {p.host} · {label(p.placement)}</h4>
      <ol>{#each p.foreign_rules as rule}<li>{[rule.name, rule.may_stop ? t.stops : t.continues, ...rule.dispositions.map(where)].join(' · ')}</li>{/each}<li>{t.mailroom}</li></ol>
      <p>{counted(p.counts)}</p>
      <ul>{#each p.decisions as d}<li>{d.status === 'existing_rule_wins' ? t.precedence : t.undetermined} · {d.rule} · {where(d.existing)} · {t.mailroom} {where(d.mailroom)}</li>{/each}</ul>
      <p>{p.gate}</p><ul>{#each p.warnings as warning}<li>{warning}</li>{/each}{#each p.pins as pin}<li>{pin.message_id_hash} · {pin.status}</li>{/each}</ul>
    </div>
  {/if}
  {#if preview?.outcome === 'ready'}
    {@const tx = preview.transaction}
    <h4>{tx.compensates ? t.compensation : tx.composition ? t.purposes[tx.composition.purpose] : t.preview}</h4>
    <p>{tx.target} · {tx.ownership_epoch}</p>
    {#if 'gate' in preview.proof}<p>{lang === 'de' ? 'Nachweis' : 'Proof'} · {preview.proof.gate} · {preview.proof.composed_gate} · {preview.proof.semantics}</p><p>{preview.proof.runtime_cross_check}</p>{/if}<p>{t.digest} {tx.artifact}</p><p>{t.steps} · CHECKSCRIPT · HAVESPACE · {preview.effects.map(effect => t.effects[effect]).join(' · ')}</p>
    <button onclick={() => show = !show}>{show ? t.hide : t.private}</button>
    {#each [{ label: t.before, image: tx.before }, { label: t.after, image: tx.after }] as item}
      <h5>{item.label} · {item.image.names.find(n => n.active)?.name ?? t.empty}</h5>
      {#each item.image.bodies as body}<p>{body.name} · {body.bytes.length} bytes · SHA-256 {body.sha256}</p>{#if show}<pre>{exactReview(body)}</pre>{/if}{/each}
    {/each}
    <label><input type="checkbox" bind:checked={approved} />{t.approval}</label>
    <button disabled={!approved || busy || running} onclick={() => act('confirm')}>{t.confirm}</button>
  {/if}
  {#if view}{#each view.journal.history as transaction}
    <article><h4>{transaction.compensates ? t.compensation : transaction.composition ? t.purposes[transaction.composition.purpose] : t.preview} · {t.statuses[transaction.status]}</h4><p>{transaction.id}</p>
      {#if transaction.composition}<p>{[...(transaction.composition.placement ? [label(transaction.composition.placement)] : []), ...transaction.composition.existing_rules].join(' · ')}</p>{#if transaction.composition.precedence}<p>{counted(transaction.composition.precedence)}</p>{/if}{/if}
      <p>CHECKSCRIPT · {transaction.validation ? 'Ok' in transaction.validation ? t.outcomes.acknowledged : transaction.validation.Err : t.pending} · HAVESPACE · {transaction.space ? 'Ok' in transaction.space ? t.outcomes.acknowledged : transaction.space.Err : t.pending}</p>
      <ul>{#each transaction.steps as step}<li>{t.effects[step.effect]} · {t.outcomes[step.outcome]} · {t.ack} {step.acknowledgment ? t.yes : t.no} {step.failure ?? ''}</li>{/each}</ul>
      <button disabled={busy || running} onclick={() => act('reconcile', transaction.id)}>{t.reconcile}</button>
      {#if !transaction.compensates && transaction.composition?.purpose !== 'removal'}<button disabled={busy || running || view.recovery_hold} onclick={() => act('compensation', transaction.id)}>{t.compensation}</button>{/if}
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
  fieldset { margin-block: 1rem; border: 1px solid #879397; border-radius: .4rem; }
  .refused { border-inline-start: .25rem solid #a4442f; padding-inline-start: .75rem; }
  :global(.placement [data-part='item']) { display: flex; align-items: center; gap: .5rem; margin-block: .4rem; cursor: pointer; }
  :global(.placement [data-part='item-control']) { inline-size: 1rem; block-size: 1rem; border: 1px solid #4a5559; border-radius: 50%; }
  :global(.placement [data-part='item-control'][data-state='checked']) { background: #2f5d6b; box-shadow: inset 0 0 0 .2rem #fff; }
</style>
