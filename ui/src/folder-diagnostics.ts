import { z } from 'zod'

export const folderFailure = z.enum(['cancelled', 'deadline', 'connect', 'tls', 'authentication', 'unsupported', 'unsupported_name', 'protocol', 'no', 'bad', 'bye', 'eof', 'quota', 'unknown_code', 'limit', 'unstable'])
type Failure = z.infer<typeof folderFailure>
type Language = 'en' | 'de'

const failureText = {
  en: {
    cancelled: 'Operation cancelled. Reconcile observations before another review.',
    deadline: 'Time limit reached. Reconcile observations before another review.',
    connect: 'Connection could not be established. Check connectivity and the server settings.',
    tls: 'Secure connection failed. Check the server identity and certificate trust.',
    authentication: 'The server did not accept authentication. Check access through the local account controls.',
    unsupported: 'Required folder or label behavior is unsupported. Review the server capabilities and folder layout.',
    unsupported_name: 'The exact folder or label name is unsupported. Review its spelling and the supported name format.',
    protocol: 'Server response could not be interpreted safely. Reconcile observations before another review.',
    no: 'The server refused the command. Review permissions, limits and recorded observations.',
    bad: 'The server rejected the command format. Review server compatibility and recorded observations.',
    bye: 'The server closed the session. Reconcile observations before another review.',
    eof: 'The connection ended before a complete response. Reconcile observations before another review.',
    quota: 'The server reported a capacity limit. Check available space and folder or label limits.',
    unknown_code: 'The server returned an unsupported response code. Reconcile observations before another review.',
    limit: 'An input or response exceeded a safety limit. Review the selected destinations and server inventory.',
    unstable: 'Server observations did not agree. Refresh observations before another review.',
  },
  de: {
    cancelled: 'Vorgang abgebrochen. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    deadline: 'Zeitgrenze erreicht. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    connect: 'Verbindung konnte nicht hergestellt werden. Erreichbarkeit und Servereinstellungen prüfen.',
    tls: 'Sichere Verbindung fehlgeschlagen. Serveridentität und Zertifikatsvertrauen prüfen.',
    authentication: 'Der Server hat die Anmeldung nicht akzeptiert. Zugang über die lokale Kontoverwaltung prüfen.',
    unsupported: 'Erforderliches Ordner- oder Labelverhalten wird nicht unterstützt. Serverfähigkeiten und Ordnerstruktur prüfen.',
    unsupported_name: 'Der exakte Ordner- oder Labelname wird nicht unterstützt. Schreibweise und unterstütztes Namensformat prüfen.',
    protocol: 'Serverantwort konnte nicht sicher ausgewertet werden. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    no: 'Der Server hat den Befehl abgelehnt. Berechtigungen, Grenzen und gespeicherte Beobachtungen prüfen.',
    bad: 'Der Server hat das Befehlsformat abgelehnt. Serverkompatibilität und gespeicherte Beobachtungen prüfen.',
    bye: 'Der Server hat die Sitzung beendet. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    eof: 'Verbindung vor einer vollständigen Antwort beendet. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    quota: 'Der Server hat eine Kapazitätsgrenze gemeldet. Freien Speicher und Ordner- oder Labelgrenzen prüfen.',
    unknown_code: 'Der Server hat einen nicht unterstützten Antwortcode gesendet. Vor einer neuen Prüfung die Beobachtungen abgleichen.',
    limit: 'Eine Eingabe oder Antwort überschreitet eine Sicherheitsgrenze. Ausgewählte Ziele und Serverbestand prüfen.',
    unstable: 'Die Serverbeobachtungen stimmen nicht überein. Vor einer neuen Prüfung die Beobachtungen aktualisieren.',
  },
} satisfies Record<Language, Record<Failure, string>>

export function folderFailureText({ failure, lang }: { failure: Failure; lang: Language }): string {
  return failureText[lang][failure]
}
