import { z } from 'zod'

const notice = z.enum([
  'delivery_tag_limits', 'synthetic_recipient_limits', 'attachment_and_duplicate_limits',
  'folder_topology_unverified', 'partial_census', 'invalid_dates',
  'rules_without_selections', 'shadowed_rules', 'stale_pins',
])
export const proofGate = z.enum(['blocked', 'warnings'])
const gates = {
  en: { blocked: 'Blocked by proof', warnings: 'Proof completed with warnings' },
  de: { blocked: 'Durch Prüfung gesperrt', warnings: 'Prüfung mit Hinweisen abgeschlossen' },
}
export function proofGateText(value: z.infer<typeof proofGate>, lang: 'en' | 'de'): string {
  return gates[lang][value]
}
export const proofDiagnostics = z.object({ notices: z.array(notice), runtime_cross_check: z.literal('not_run') }).strict()
export type ProofDiagnostics = z.infer<typeof proofDiagnostics>

const notices = {
  en: {
    delivery_tag_limits: 'Delivery tags identify a matching ingress address. They do not authenticate Gmail origin. In this proof, Sieve starts with empty accumulated flags. Observed IMAP flags are separate, and permanent keyword storage is unverified. A verified envelope route may add its keyword while a recipient review hold keeps the message in Inbox without changing read status or making accounting copies.',
    synthetic_recipient_limits: 'Recipient safety accepts exactly one verified synthetic delivery recipient. Multiple recipients stay in Inbox for review, including recipients with the same role. Real provider provenance remains unknown.',
    attachment_and_duplicate_limits: 'A header census proves neither attachment presence nor accounting acceptance. Duplicate state starts empty for each proof and has no historical provider cache.',
    folder_topology_unverified: 'Folder paths use a logical slash separator. The actual provider separator, existing folders and coexistence with existing rules remain unverified.',
    partial_census: 'The census is partial. Counts cover saved records only.',
    invalid_dates: 'Missing, invalid or future Date values are excluded from 13-month counts.',
    rules_without_selections: 'Some enabled rules select no messages in the 13-month window.',
    shadowed_rules: 'Earlier compiled rules fully shadow some rules.',
    stale_pins: 'Some pinned census checks are stale.',
  },
  de: {
    delivery_tag_limits: 'Zustellmarkierungen kennzeichnen eine passende Eingangsadresse. Sie authentifizieren keine Gmail-Herkunft. In dieser Prüfung beginnt Sieve ohne angesammelte Markierungen. Beobachtete IMAP-Markierungen sind davon getrennt; die dauerhafte Speicherung von Schlagwörtern ist ungeprüft. Eine geprüfte Umschlagroute darf ihr Schlagwort ergänzen, während die Empfängerprüfung den Posteingang und Lesestatus unverändert lässt und keine Buchhaltungskopie erstellt.',
    synthetic_recipient_limits: 'Die Empfängersicherheit akzeptiert genau einen geprüften synthetischen Zustellempfänger. Mehrere Empfänger bleiben zur Prüfung im Posteingang, auch bei gleicher Rolle. Die Herkunft realer Anbieterbelege bleibt unbekannt.',
    attachment_and_duplicate_limits: 'Eine Kopfzeilenerfassung belegt weder vorhandene Anhänge noch die Annahme durch die Buchhaltung. Der Duplikatstand beginnt für jede Prüfung leer und enthält keine früheren Duplikatinformationen des Anbieters.',
    folder_topology_unverified: 'Ordnerpfade verwenden einen logischen Schrägstrich als Trennzeichen. Das tatsächliche Anbietertrennzeichen, vorhandene Ordner und das Zusammenspiel mit bestehenden Regeln sind ungeprüft.',
    partial_census: 'Die Erfassung ist unvollständig. Die Zahlen umfassen nur gespeicherte Datensätze.',
    invalid_dates: 'Fehlende, ungültige oder zukünftige Date-Werte sind von den Zahlen über 13 Monate ausgeschlossen.',
    rules_without_selections: 'Einige aktive Regeln wählen im Zeitraum von 13 Monaten keine Nachrichten aus.',
    shadowed_rules: 'Frühere kompilierte Regeln verdecken einige Regeln vollständig.',
    stale_pins: 'Einige fixierte Erfassungsprüfungen sind veraltet.',
  },
} satisfies Record<'en' | 'de', Record<z.infer<typeof notice>, string>>

export function proofNoticeText(value: z.infer<typeof notice>, lang: 'en' | 'de'): string {
  return notices[lang][value]
}

const runtime = {
  en: 'This proof does not run an independent Sieve check. Run scripts/verify-sieve.sh on synthetic artifacts. That check does not establish provider behavior.',
  de: 'Diese Prüfung führt keine unabhängige Sieve-Prüfung aus. scripts/verify-sieve.sh prüft synthetische Artefakte. Das belegt kein Anbieterverhalten.',
}
export function proofRuntimeText(value: ProofDiagnostics['runtime_cross_check'], lang: 'en' | 'de'): string {
  switch (value) {
    case 'not_run': return runtime[lang]
  }
}
