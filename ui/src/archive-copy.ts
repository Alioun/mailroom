import type { BacklogPreview } from './api'

const text = {
  en: { confirm: 'I confirm archiving only this exact list of old unmatched Inbox mail.', start: 'Archive this reviewed list', title: 'Separate old unmatched-mail archive', archive_review_hold: 'Recipient review hold. Keep in Inbox.', archive_matched: 'An existing rule applies. Excluded from unmatched-mail archiving.', archive_unknown_date: 'Arrival date is unavailable or invalid.', archive_recent_or_future: 'Arrival date is within twelve calendar months or in the future.', archive_destination_unavailable: 'A unique Archive, Archiv or Gmail All Mail destination is unavailable.' },
  de: { confirm: 'Ich bestätige die Archivierung genau dieser Liste alter unzugeordneter Posteingangsnachrichten.', start: 'Geprüfte Liste archivieren', title: 'Separate Archivierung alter unzugeordneter Nachrichten', archive_review_hold: 'Empfänger muss geprüft werden. Im Posteingang belassen.', archive_matched: 'Eine vorhandene Regel greift. Von der Archivierung unzugeordneter Nachrichten ausgeschlossen.', archive_unknown_date: 'Eingangsdatum fehlt oder ist ungültig.', archive_recent_or_future: 'Eingangsdatum liegt innerhalb von zwölf Kalendermonaten oder in der Zukunft.', archive_destination_unavailable: 'Ein eindeutiges Ziel Archive, Archiv oder Gmail All Mail fehlt.' },
}
export function archiveCopy(lang: 'en' | 'de') { return text[lang] }
export function archiveReason(lang: 'en' | 'de', reason: string) {
  const messages = text[lang]
  return Object.entries(messages).find(([key]) => key === reason)?.[1] ?? reason
}

export function archiveOfferAvailable(completed: boolean, plan: BacklogPreview['plan'] | undefined): boolean {
  return completed || !!plan && plan.options.intent !== 'archive_unmatched' && plan.proof_gate !== 'blocked' && plan.changes.length === 0
}
