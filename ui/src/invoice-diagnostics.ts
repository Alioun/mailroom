import { z } from 'zod'

const message = z.enum([
  'content_unknown', 'document_content_types', 'unsupported_format', 'link_only',
  'receipt_body', 'invoice_subject', 'recipient_held', 'bulk_excludes_keyword_copy',
  'handoff_unknown', 'independent_evidence_required', 'keyword_mime_required', 'workspace_required',
])
export const invoiceDiagnostic = z.discriminatedUnion('kind', [
  z.object({ kind: message }),
  z.object({ kind: z.literal('filename_hint'), names: z.array(z.string()) }),
  z.object({ kind: z.literal('rule_omitted'), rule: z.string(), detail: z.string() }),
])
export const invoiceRole = z.enum(['business', 'unresolved'])
export const invoiceHandoff = z.enum([
  'owner_recorded_sent', 'synthetic_handoff', 'smtp_accepted', 'mail_app_handoff',
  'uncertain_synthetic_submission', 'uncertain_submission', 'real_handoff_unknown',
])
type Language = 'en' | 'de'
type Message = z.infer<typeof message>
type Diagnostic = z.infer<typeof invoiceDiagnostic>

const text = {
  en: {
    content_unknown: 'The attachment structure and message body have not been read.',
    document_content_types: 'An attachment declares a PDF or XML content type. This does not establish that the accounting tool accepts it.',
    unsupported_format: 'The observed attachments have no supported PDF or XML content type. The selected accounting tool may reject this submission.',
    link_only: 'The message contains a link to the invoice, without an enclosed invoice document.',
    receipt_body: 'Receipt content is in the message body. Check whether the selected accounting tool processes message bodies. Acceptance is unknown.',
    invoice_subject: 'The subject contains an invoice or receipt keyword.',
    recipient_held: 'The recipient role or delivery evidence is unresolved. Keep this message in Inbox without an automatic accounting copy.',
    bulk_excludes_keyword_copy: 'Mailing-list evidence excludes an automatic copy based on invoice keywords.',
    handoff_unknown: 'A predicted copy or downloaded Sieve file does not establish delivery. Actual handoff and accounting acceptance remain unknown.',
    independent_evidence_required: 'Target approval, sender restrictions, encryption, delivery provenance and MIME support need separate verification.',
    keyword_mime_required: 'Keyword copies require Sieve MIME support and a PDF or XML part. Predictions need an acquired original. Headers alone cannot establish missing attachments, link-only content or ZUGFeRD.',
    workspace_required: 'Save a rule workspace to determine recipient roles and predicted accounting copies.',
  },
  de: {
    content_unknown: 'Anhangsstruktur und Nachrichteninhalt wurden noch nicht gelesen.',
    document_content_types: 'Ein Anhang deklariert PDF oder XML als Inhaltstyp. Das belegt keine Annahme durch das Buchhaltungstool.',
    unsupported_format: 'Die beobachteten Anhänge deklarieren keinen unterstützten PDF- oder XML-Inhaltstyp. Das gewählte Buchhaltungstool könnte die Übermittlung ablehnen.',
    link_only: 'Die Nachricht enthält einen Link zur Rechnung, aber kein beigefügtes Rechnungsdokument.',
    receipt_body: 'Der Beleg steht im Nachrichteninhalt. Prüfe, ob das gewählte Buchhaltungstool Nachrichteninhalte verarbeitet. Die Annahme ist unbekannt.',
    invoice_subject: 'Der Betreff enthält ein Schlüsselwort für eine Rechnung oder einen Beleg.',
    recipient_held: 'Empfängerrolle oder Zustellbelege sind ungeklärt. Die Nachricht bleibt ohne automatische Buchhaltungskopie im Posteingang.',
    bulk_excludes_keyword_copy: 'Belege für eine Mailingliste schließen eine automatische Kopie anhand von Rechnungsschlüsselwörtern aus.',
    handoff_unknown: 'Eine vorhergesagte Kopie oder heruntergeladene Sieve-Datei belegt keine Zustellung. Tatsächliche Übergabe und Buchhaltungsannahme bleiben unbekannt.',
    independent_evidence_required: 'Zielfreigabe, Absendervorgaben, Verschlüsselung, Zustellherkunft und MIME-Unterstützung müssen gesondert geprüft werden.',
    keyword_mime_required: 'Schlüsselwortkopien benötigen Sieve-MIME-Unterstützung und einen PDF- oder XML-Teil. Vorhersagen benötigen das erfasste Original. Kopfzeilen allein belegen weder fehlende Anhänge noch reine Linknachrichten oder ZUGFeRD.',
    workspace_required: 'Speichere ein Regelwerk, um Empfängerrollen und vorhergesagte Buchhaltungskopien zu ermitteln.',
  },
} satisfies Record<Language, Record<Message, string>>

export function invoiceDiagnosticText(diagnostic: Diagnostic, lang: Language): string {
  if (diagnostic.kind === 'filename_hint') {
    return lang === 'de'
      ? `Nur Dateinamen sind bekannt. PDF, XML oder ZUGFeRD sind nicht geprüft: ${diagnostic.names.join(', ')}`
      : `Only filenames are known. PDF, XML or ZUGFeRD content is unverified: ${diagnostic.names.join(', ')}`
  }
  if (diagnostic.kind === 'rule_omitted') {
    return lang === 'de'
      ? `Regel ${diagnostic.rule} ist in dieser Ausgabe ausgelassen. Prüfe die technischen Details, bevor du diese Aktion erwartest.`
      : `Rule ${diagnostic.rule} is omitted from this executable. Check the technical details before expecting this action.`
  }
  return text[lang][diagnostic.kind]
}

export const invoiceStatusText = {
  en: {
    role: { business: 'Business', unresolved: 'Recipient role unresolved' },
    handoff: {
      owner_recorded_sent: 'Recorded as sent by the owner',
      synthetic_handoff: 'Synthetic transport acknowledged the fixture message',
      smtp_accepted: 'SMTP server accepted the message. Accounting acceptance is unknown.',
      mail_app_handoff: 'Opened in Mail. Sending is unknown.',
      uncertain_synthetic_submission: 'Synthetic submission is uncertain. Reconcile before another action.',
      uncertain_submission: 'Sending is uncertain. Check Sent mail before recording an outcome.',
      real_handoff_unknown: 'Actual handoff is unknown',
    },
  },
  de: {
    role: { business: 'Geschäftlich', unresolved: 'Empfängerrolle ungeklärt' },
    handoff: {
      owner_recorded_sent: 'Vom Inhaber als gesendet vermerkt',
      synthetic_handoff: 'Der synthetische Versand hat die Testnachricht bestätigt',
      smtp_accepted: 'Der SMTP-Server hat die Nachricht angenommen. Die Buchhaltungsannahme ist unbekannt.',
      mail_app_handoff: 'In Mail geöffnet. Versand unbekannt.',
      uncertain_synthetic_submission: 'Synthetische Übermittlung ungewiss. Vor einer weiteren Aktion abgleichen.',
      uncertain_submission: 'Versand ungewiss. Prüfe gesendete Nachrichten, bevor du das Ergebnis vermerkst.',
      real_handoff_unknown: 'Tatsächliche Übergabe unbekannt',
    },
  },
} satisfies Record<Language, {
  role: Record<z.infer<typeof invoiceRole>, string>
  handoff: Record<z.infer<typeof invoiceHandoff>, string>
}>
