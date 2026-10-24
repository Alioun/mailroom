import type { Operation } from './api'
export const strings = {
  en: {
    title: 'Preview the filing decisions', intro: 'Review rules against synthetic or saved local headers before any account changes.',
    locked: 'Live writes locked', language: 'Language', footer: 'Local interface · Account writes locked',
    boundary: 'Account operations', boundaryDetail: 'Availability comes from the Rust authorization policy. Direct HTTP requests pass through the same policy.',
    denied: 'Denied', unavailable: 'Unavailable', unavailableDetail: 'Owner-run connection and header acquisition are read-only. Every account write is denied.',
  },
  de: {
    title: 'Ablageentscheidungen prüfen', intro: 'Prüfe Regeln mit synthetischen oder lokal gespeicherten Kopfzeilen, bevor ein Konto geändert wird.',
    locked: 'Kontoänderungen gesperrt', language: 'Sprache', footer: 'Lokale Oberfläche · Kontoänderungen gesperrt',
    boundary: 'Kontoaktionen', boundaryDetail: 'Die Verfügbarkeit kommt aus der Rust-Autorisierung. Direkte HTTP-Anfragen durchlaufen dieselbe Prüfung.',
    denied: 'Gesperrt', unavailable: 'Nicht verfügbar', unavailableDetail: 'Kontoverbindung und Kopfzeilenerfassung durch den Inhaber sind schreibgeschützt. Alle Kontoänderungen sind gesperrt.',
  },
}
export const operationLabels: Record<'en' | 'de', Record<Operation, string>> = {
  en: { preview: 'Synthetic preview', synthetic_census: 'Synthetic census', connect: 'Connect account', census: 'Header census', upload_rules: 'Upload rules', activate_rules: 'Activate rules', create_folder: 'Create folders', move_messages: 'Move messages', change_flags: 'Change flags', change_forwarding: 'Change forwarding', send_message: 'Send messages', unsubscribe: 'Unsubscribe' },
  de: { preview: 'Synthetische Vorschau', synthetic_census: 'Synthetische Bestandsaufnahme', connect: 'Konto verbinden', census: 'Kopfzeilen erfassen', upload_rules: 'Regeln hochladen', activate_rules: 'Regeln aktivieren', create_folder: 'Ordner erstellen', move_messages: 'Nachrichten verschieben', change_flags: 'Markierungen ändern', change_forwarding: 'Weiterleitung ändern', send_message: 'Nachrichten senden', unsubscribe: 'Abmelden' },
}
