<script lang="ts">
  import { onMount } from 'svelte'
  import { openSubscriptionReview, previewSubscriptionReview, confirmSubscriptionReview, recoverUnsubscribe, type SubscriptionReviewView, type SubscriptionReviewPreview, type SubscriptionReviewInput, type UnsubscribeMethod } from './api'
  let { lang }: { lang: 'en' | 'de' } = $props()
  const strings = {
    en: { title: 'Unsubscribe and mute', subscriptions: 'separate subscriptions', refresh: 'Refresh subscription facts', scope: 'Grouped by sender. Approve each account, receiving address and mailing list separately. Only checked subscriptions enter the exact confirmation.', limits: 'Synthetic only. Real provider DKIM, DNS/TLS transport, SMTP, OS confirmation and deployment are unavailable. No request, application launch, alias retirement or retroactive move/read occurs. External unsubscribe has no Undo.', personal: 'Personal muted folder segment', business: 'Business muted folder segment', choose: 'Include exact subscription', action: 'Exact action', review: 'Preview selected subscription actions', confirm: 'Confirm this exact synthetic action list', mail: 'Confirm the additional named synthetic mail send', execute: 'Apply exactly this synthetic list', remove: 'Preview removing unchanged owned mute', recover: 'Reconcile without retry', history: 'Token-safe history', error: 'Unsubscribe review rejected. Refresh the exact preview. Reconcile uncertainty without retrying. Edited mutes are preserved.', flags: 'Read flag evidence', auth: 'Authentication', newest: 'Newest dated Seen message, not read time', recommended: 'Meets initial-selection criteria', no: 'No', yes: 'Yes', unknown: 'Unknown', count: '12-month messages', read: 'Read share', proof: 'Compiled mute proof', selected: 'Eligible subscriptions are initially checked. Review each exact target and method before confirming.', one_click: 'Synthetic one-click POST', local_mail: 'Synthetic local mail with extra confirmation', browser_preparation: 'Browser copy card only', mail_app_preparation: 'Mail-app copy card only', retirement_preparation: 'Retirement preparation only', mute: 'Mute future messages, marked read', keep: 'Keep', remove_mute: 'Remove unchanged owned mute', status: 'Status', arrivals: 'Later-arrival evidence', prepared: 'Preparation only. Nothing launched.', acquired: 'Last complete census', age: 'Census age in seconds' },
    de: { title: 'Abmelden und stummschalten', subscriptions: 'getrennte Abonnements', refresh: 'Abonnementdaten aktualisieren', scope: 'Nach Absender gruppiert. Jedes Konto, jede Empfangsadresse und jede Mailingliste wird getrennt freigegeben. Nur angekreuzte Abonnements gelangen in die genaue Bestätigung.', limits: 'Nur synthetisch. Anbieter-DKIM, DNS/TLS-Transport, SMTP, Betriebssystembestätigung und Bereitstellung fehlen. Keine Anfrage, kein Programmstart, keine Aliasstilllegung oder rückwirkende Verschiebung/Lesemarkierung. Externe Abmeldung hat kein Rückgängig.', personal: 'Persönlicher Stummordner, ein Segment', business: 'Geschäftlicher Stummordner, ein Segment', choose: 'Genaues Abonnement einbeziehen', action: 'Genaue Aktion', review: 'Ausgewählte Abonnementaktionen prüfen', confirm: 'Genau diese synthetische Aktionsliste bestätigen', mail: 'Zusätzlichen benannten synthetischen Mailversand bestätigen', execute: 'Genau diese synthetische Liste anwenden', remove: 'Entfernung unveränderter eigener Stummregel prüfen', recover: 'Ohne Wiederholung abgleichen', history: 'Verlauf ohne Abmeldetoken', error: 'Abmeldeprüfung abgelehnt. Genaue Vorschau aktualisieren. Ungewissheit ohne Wiederholung abgleichen. Bearbeitete Stummregeln bleiben erhalten.', flags: 'Belege für Lesemarkierungen', auth: 'Authentifizierung', newest: 'Neuestes Datum einer als gelesen markierten Nachricht, keine Lesezeit', recommended: 'Erfüllt Kriterien der Anfangsauswahl', no: 'Nein', yes: 'Ja', unknown: 'Unbekannt', count: 'Nachrichten in 12 Monaten', read: 'Gelesener Anteil', proof: 'Kompilierter Stummregel-Nachweis', selected: 'Geeignete Abonnements sind vorausgewählt. Prüfe jedes genaue Ziel und die Methode vor der Bestätigung.', one_click: 'Synthetischer Ein-Klick-POST', local_mail: 'Synthetische lokale Mail mit Zusatzbestätigung', browser_preparation: 'Nur Browser-Kopierkarte', mail_app_preparation: 'Nur Mailprogramm-Kopierkarte', retirement_preparation: 'Nur Stilllegung vorbereiten', mute: 'Künftige Nachrichten stumm und gelesen', keep: 'Behalten', remove_mute: 'Unveränderte eigene Stummregel entfernen', status: 'Status', arrivals: 'Belege für spätere Eingänge', prepared: 'Nur Vorbereitung. Nichts geöffnet.', acquired: 'Letzte vollständige Erfassung', age: 'Alter der Erfassung in Sekunden' },
  }
  let t = $derived(strings[lang])
  let view = $state<SubscriptionReviewView | null>(null)
  let plan = $state<SubscriptionReviewPreview | null>(null)
  let reviewed = $state<SubscriptionReviewInput | null>(null)
  let choices = $state<Record<string, UnsubscribeMethod>>({})
  let included = $state<Record<string, boolean>>({})
  let personal = $state('Muted')
  let business = $state('Muted')
  let confirmed = $state(false)
  let mailConfirmed = $state(false)
  let busy = $state(false)
  let error = $state('')
  const groups = $derived.by(() => {
    const senders = new Map<string, Array<{ row: NonNullable<SubscriptionReviewView>['accounts'][number]['report']['rows'][number]; key: string }>>()
    for (const account of view?.accounts ?? []) for (const row of account.report.rows) { const rows = senders.get(row.sender) ?? []; rows.push({ row, key: `${row.account}:${row.id}` }); senders.set(row.sender, rows) }
    return [...senders].map(([sender, rows]) => ({
      sender,
      rows: rows.sort((a, b) => b.row.unread_volume - a.row.unread_volume || a.key.localeCompare(b.key)),
      unreadVolume: rows.reduce((total, { row }) => total + row.unread_volume, 0),
    })).sort((a, b) => b.unreadVolume - a.unreadVolume || a.sender.localeCompare(b.sender))
  })
  let asOf = Math.floor(Date.now() / 1000)
  function invalidate() { plan = null; reviewed = null; confirmed = false; mailConfirmed = false }
  async function act(action: () => Promise<void>) { busy = true; error = ''; try { await action() } catch { error = t.error; invalidate() } finally { busy = false } }
  async function refresh() { handoff = null; invalidate(); choices = {}; included = {}; asOf = Math.floor(Date.now() / 1000); view = await openSubscriptionReview(asOf); for (const account of view.accounts) for (const row of account.report.rows) { const key = `${row.account}:${row.id}`; included[key] = row.recommended; choices[key] = row.methods.find(m => ['one_click', 'browser_preparation', 'mail_app_preparation'].includes(m)) ?? 'keep' } }
  async function review(input: SubscriptionReviewInput) { invalidate(); plan = await previewSubscriptionReview(input); reviewed = input }
  function input(): SubscriptionReviewInput { return { as_of: asOf, selections: Object.entries(included).filter(([,v]) => v).map(([id]) => ({ account: id.split(':')[0], subscription: id.split(':')[1], method: choices[id] ?? 'keep' })), folders: { personal, business } } }
  export async function prepareHandoff(accountId: string, senders: string[], reportAsOf: number) {
    let prepared = false
    await act(async () => {
      invalidate(); choices = {}; included = {}; asOf = reportAsOf
      view = await openSubscriptionReview(asOf)
      for (const account of view.accounts) for (const row of account.report.rows) {
        const key = `${row.account}:${row.id}`
        included[key] = false
        choices[key] = 'keep'
      }
      handoff = { account: accountId, senders }; prepared = true
    })
    if (!prepared) throw new Error('Bulk handoff unavailable')
  }
  let handoff = $state<{ account: string; senders: string[] } | null>(null)
  onMount(() => { void act(refresh) })
</script>
<section class="unsubscribe" aria-label={t.title}>
  <h3>{t.title}</h3><p class="help">{t.scope}</p><p class="help">{t.limits}</p>
  <button disabled={busy} onclick={() => act(refresh)}>{t.refresh}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if view}
    {#each view.accounts as account (account.account)}
      <p>{account.account} · {account.report.complete ? 'Complete census' : 'Partial census'} · {account.report.version} · {account.report.revision}</p>
      <p>{t.acquired} · {account.report.last_completed_at === null ? t.unknown : new Date(account.report.last_completed_at * 1000).toISOString()} · {t.age} · {account.report.census_age_seconds ?? t.unknown}</p>
      {#each account.report.warnings as warning}<p>{warning}</p>{/each}
    {/each}
    {#if handoff}<p class="handoff" role="status">{lang === 'en' ? 'Prepared from wind-down. Matching subscriptions are highlighted. Nothing is selected or approved.' : 'Aus dem Stilllegungsbericht vorbereitet. Passende Abonnements sind markiert. Nichts ist ausgewählt oder genehmigt.'}</p>{/if}
    <p>{handoff ? t.scope : t.selected}</p>
    <label>{t.personal}<input type="text" bind:value={personal} disabled={busy} oninput={invalidate} maxlength="160" /></label>
    <label>{t.business}<input type="text" bind:value={business} disabled={busy} oninput={invalidate} maxlength="160" /></label>
    {#each groups as { sender, rows } (sender)}
      <details class="sender-group" data-sender={sender} open={rows.some(({ row }) => row.recommended)}><summary>{sender} · {rows.length} {t.subscriptions}</summary>
      {#each rows as { row, key } (key)}
      <article class:handoff-match={handoff?.account === row.account && handoff.senders.includes(row.sender)} data-subscription={row.id} data-account={row.account}>
        <p>{row.sender}</p><p>{row.account} · {row.recipient || t.unknown} · {row.list_id ?? 'From'} · {row.list_evidence} · {row.class} · {row.role}</p>
        <p>{t.count} · {row.count_12_months} · {t.read} · {row.read_share === null ? t.unknown : `${Math.round(row.read_share * 100)}%`} · unread {row.unread_volume}</p>
        <p>{t.newest} · {row.newest_dated_seen_message === null ? t.unknown : new Date(row.newest_dated_seen_message * 1000).toISOString().slice(0, 10)}</p>
        <p>{t.flags} · {row.flags_known ? t.yes : t.unknown} · {t.recommended} · {row.recommended ? t.yes : t.no}</p>
        <p>{t.auth} · {row.authentication} · {row.recipient_evidence}</p><p>{t.arrivals} · {row.straggler}</p>
        {#if view.synthetic}
          <label><input type="checkbox" checked={included[key] ?? false} disabled={busy} onchange={(e) => { included[key] = e.currentTarget.checked; invalidate() }} />{t.choose}</label>
          <label>{t.action}<select value={choices[key] ?? 'keep'} disabled={busy} onchange={(e) => { const method = row.methods.find(m => m === e.currentTarget.value); if (method) choices[key] = method; invalidate() }}>{#each row.methods as method}<option value={method}>{t[method]}</option>{/each}</select></label>
        {/if}
      </article>
      {/each}</details>
    {/each}
    {#if view.synthetic}<button disabled={busy || !Object.values(included).some(Boolean)} onclick={() => act(async () => review(input()))}>{t.review}</button>{/if}
    {#each view.accounts as account (account.account)}
    {#each Object.entries(account.mutes) as [id, mute] (id)}<p>{mute.rule.id}</p><button disabled={busy} onclick={() => act(async () => review({ as_of: asOf, selections: [{ account: account.account, subscription: id, method: 'remove_mute' }], folders: { personal, business } }))}>{t.remove} · {account.account}</button>{/each}
    {/each}
    {#if plan}
      <div class="confirmation" data-unsubscribe-preview>
        <p>{plan.confirmation_action}</p>
        <pre data-exact-confirmation>{plan.confirmation.display}</pre>
        {#each plan.previews as accountPlan (accountPlan.account)}
        <p>{accountPlan.scope}</p>
        {#each accountPlan.actions as action}
          <p>{action.account} · {action.sender} · {action.recipient} · {action.list_id ?? 'From'}</p><p>{t[action.method]} · {action.host ?? ''} · {action.role} · {action.destination ?? ''}</p>
          <p>{action.effect}</p><p>{action.source} · {action.header_digest} · {action.payload_digest}</p>
          {#if action.copy_card}<div data-copy-card><p>{t.prepared}</p><pre>{action.copy_card.uri ?? ''}{'\n'}{action.copy_card.to ?? ''}{'\n'}{action.copy_card.from ?? ''}{'\n'}{action.copy_card.subject ?? ''}{'\n'}{action.copy_card.body ?? ''}</pre></div>{/if}
        {/each}
        {#if accountPlan.proof}<details open><summary>{t.proof}</summary><p>{accountPlan.proof.gate} · {accountPlan.proof.counts.filed} filed · {accountPlan.proof.counts.copies} independent copies</p>{#each accountPlan.proof.warnings as warning}<p>{warning}</p>{/each}{#each accountPlan.proof.pins as pin}<p>{pin.status} · {pin.message_id_hash}</p>{/each}<pre>{accountPlan.proof.artifact.executable}</pre></details>{/if}
        {/each}
        <label><input type="checkbox" bind:checked={confirmed} disabled={busy} />{t.confirm}</label>
        {#if plan.local_mail_confirmation}<p>{plan.local_mail_confirmation}</p><label><input type="checkbox" bind:checked={mailConfirmed} disabled={busy} />{t.mail}</label>{/if}
        <button disabled={busy || !confirmed || (!!plan.local_mail_confirmation && !mailConfirmed) || plan.previews.some(p => p.proof?.gate === 'blocked')} onclick={() => act(async () => { if (!plan || !reviewed) return; view = await confirmSubscriptionReview(reviewed, plan); invalidate(); included = {} })}>{t.execute}</button>
      </div>
    {/if}
    <h4>{t.history}</h4>
    {#each view.accounts as account (account.account)}
    <p>{account.account}</p>
    {#each account.history as entry (entry.id)}<p data-unsubscribe-status>{entry.sender_key} · {entry.method} · {entry.status} · {entry.host ?? ''} · {entry.http_status ?? ''} · success {entry.success} · {new Date(entry.time * 1000).toISOString()}</p>{#if entry.status === 'uncertain' || entry.status === 'intent'}<button disabled={busy} onclick={() => act(async () => { await recoverUnsubscribe(account.account, entry.id, asOf); await refresh() })}>{t.recover}</button>{/if}{/each}
    {/each}
  {/if}
</section>
<style>
  .unsubscribe { margin-block: 2rem; padding-top: 1rem; border-top: 1px solid #7775; }
  .sender-group { margin-block: .8rem; }
  summary { cursor: pointer; overflow-wrap: anywhere; }
  .handoff-match { background: #eaf0f3; }
  article { border-bottom: 1px solid #7775; padding-block: .75rem; }
  p { overflow-wrap: anywhere; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 24rem; overflow: auto; }
  button { margin: .4rem .4rem .4rem 0; }
  label { display: block; margin-block: .7rem; }
  select, input[type=text] { max-width: 100%; }
</style>
