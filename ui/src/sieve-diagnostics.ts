import { z } from 'zod'

export const sieveFailure = z.enum(['cancelled', 'deadline', 'connect', 'tls', 'unsupported', 'protocol', 'refused', 'credential_unavailable', 'quota', 'missing', 'referral', 'bye', 'eof', 'limit', 'unstable'])
export const sieveDiscoveryOutcome = z.enum(['running', 'complete', 'failed', 'cancelled', 'interrupted'])
type Failure = z.infer<typeof sieveFailure>
type DiscoveryOutcome = z.infer<typeof sieveDiscoveryOutcome>
type Language = 'en' | 'de'

const failureText = {
  en: {
    cancelled: 'Operation cancelled. Review recorded effects before starting another action.',
    deadline: 'Time limit reached. Check the recorded outcome before any further changes.',
    connect: 'Connection could not be established. Check connectivity and the server settings.',
    tls: 'Secure connection failed. Check the server identity and certificate trust.',
    unsupported: 'Required server behavior is unsupported. Check capabilities before a new exact review.',
    protocol: 'Server response could not be interpreted safely. Check observations before any further changes.',
    refused: 'The server refused the command. Review the script and recorded result.',
    credential_unavailable: 'The required credential was unavailable. Check it through the approved local account controls.',
    quota: 'The server reported a storage quota. Check available space before a new exact review.',
    missing: 'The requested script was missing. Refresh the script inventory before a new review.',
    referral: 'The server requested another destination. Mailroom did not follow it.',
    bye: 'The server closed the session. Check recorded effects before starting another action.',
    eof: 'The connection ended before a complete response. Check recorded effects before any further changes.',
    limit: 'An input or response exceeded a safety limit. Review the selected scripts before another action.',
    unstable: 'Server observations did not agree. Refresh evidence before a new exact review.',
  },
  de: {
    cancelled: 'Vorgang abgebrochen. Vor einer neuen Aktion die gespeicherten Änderungen prüfen.',
    deadline: 'Zeitgrenze erreicht. Vor weiteren Änderungen das gespeicherte Ergebnis prüfen.',
    connect: 'Verbindung konnte nicht hergestellt werden. Erreichbarkeit und Servereinstellungen prüfen.',
    tls: 'Sichere Verbindung fehlgeschlagen. Serveridentität und Zertifikatsvertrauen prüfen.',
    unsupported: 'Erforderliches Serververhalten wird nicht unterstützt. Vor einer neuen genauen Prüfung die Fähigkeiten prüfen.',
    protocol: 'Serverantwort konnte nicht sicher ausgewertet werden. Vor weiteren Änderungen die Beobachtungen prüfen.',
    refused: 'Der Server hat den Befehl abgelehnt. Skript und gespeichertes Ergebnis prüfen.',
    credential_unavailable: 'Die benötigten Zugangsdaten waren nicht verfügbar. Über die freigegebene lokale Kontoverwaltung prüfen.',
    quota: 'Der Server hat eine Speichergrenze gemeldet. Vor einer neuen genauen Prüfung den freien Speicher prüfen.',
    missing: 'Das angeforderte Skript fehlt. Vor einer neuen Prüfung die Skriptliste aktualisieren.',
    referral: 'Der Server hat ein anderes Ziel genannt. Mailroom ist diesem Verweis nicht gefolgt.',
    bye: 'Der Server hat die Sitzung beendet. Vor einer neuen Aktion die gespeicherten Änderungen prüfen.',
    eof: 'Verbindung vor einer vollständigen Antwort beendet. Vor weiteren Änderungen die gespeicherten Änderungen prüfen.',
    limit: 'Eine Eingabe oder Antwort überschreitet eine Sicherheitsgrenze. Vor einer neuen Aktion die ausgewählten Skripte prüfen.',
    unstable: 'Die Serverbeobachtungen stimmen nicht überein. Vor einer neuen genauen Prüfung die Belege aktualisieren.',
  },
} satisfies Record<Language, Record<Failure, string>>

const discoveryText = {
  en: { running: 'Discovery running', complete: 'Discovery completed', failed: 'Discovery failed', cancelled: 'Discovery cancelled', interrupted: 'Discovery interrupted' },
  de: { running: 'Erfassung läuft', complete: 'Erfassung abgeschlossen', failed: 'Erfassung fehlgeschlagen', cancelled: 'Erfassung abgebrochen', interrupted: 'Erfassung unterbrochen' },
} satisfies Record<Language, Record<DiscoveryOutcome, string>>

export function sieveFailureText({ failure, lang }: { failure: Failure; lang: Language }): string {
  return failureText[lang][failure]
}

export function sieveDiscoveryOutcomeText({ outcome, lang }: { outcome: DiscoveryOutcome; lang: Language }): string {
  return discoveryText[lang][outcome]
}
