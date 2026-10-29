import { z } from 'zod'

export const proposalRationale = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('subject_majority'), matching: z.number().int().nonnegative(), total: z.number().int().positive() })
    .refine(value => value.matching <= value.total && value.matching * 2 > value.total),
  z.object({ kind: z.literal('mapped_folder') }),
])
export const proposalReason = z.enum([
  'no_unique_sender', 'mixed_organisation', 'owner_mapping_retained',
  'preset_mapping_retained', 'no_unique_category', 'category_conflict',
  'configuration_conflict', 'candidate_limit', 'compilation_failed',
])
type Reason = z.infer<typeof proposalReason>
type Rationale = z.infer<typeof proposalRationale>
type Language = 'en' | 'de'

const reasons = {
  en: {
    no_unique_sender: 'The From header has no single valid sender address. No filing proposal can use it.',
    mixed_organisation: 'Organisational evidence is insufficient or mixed. Sending infrastructure does not establish the identity of the sender.',
    owner_mapping_retained: 'An existing owner rule already files this sender. It is retained, with no replacement proposed.',
    preset_mapping_retained: 'An existing preset mapping or owner patch is retained. No overwrite is proposed.',
    no_unique_category: 'No single preset category or approved folder mapping is supported. Classify this sender before adding a rule.',
    category_conflict: 'The preset sender category conflicts with the observed category. No change is proposed.',
    configuration_conflict: 'The proposal conflicts with existing owner rules or cannot compile. The existing configuration is retained.',
    candidate_limit: 'The review has reached its proposal limit. No additional proposal was generated.',
    compilation_failed: 'The proposal cannot compile with the current owner rules. The existing configuration is retained.',
  },
  de: {
    no_unique_sender: 'Die From-Kopfzeile enthält keine eindeutige gültige Absenderadresse. Sie kann keinen Ablagevorschlag begründen.',
    mixed_organisation: 'Belege für einen Organisationsabsender fehlen oder sind uneinheitlich. Versandtechnik belegt keine Absenderidentität.',
    owner_mapping_retained: 'Eine vorhandene eigene Regel legt Nachrichten dieses Absenders bereits ab. Sie bleibt erhalten. Kein Ersatz vorgeschlagen.',
    preset_mapping_retained: 'Eine vorhandene Absenderzuordnung der Voreinstellung oder eine eigene Anpassung bleibt erhalten. Kein Überschreiben vorgeschlagen.',
    no_unique_category: 'Keine eindeutige Kategorie der Voreinstellung oder freigegebene Ordnerzuordnung ist belegt. Ordne diesen Absender zu, bevor eine Regel ergänzt wird.',
    category_conflict: 'Die Absenderkategorie der Voreinstellung widerspricht der beobachteten Kategorie. Keine Änderung vorgeschlagen.',
    configuration_conflict: 'Der Vorschlag widerspricht vorhandenen eigenen Regeln oder lässt sich nicht kompilieren. Die vorhandene Konfiguration bleibt erhalten.',
    candidate_limit: 'Die Höchstzahl an Vorschlägen für diese Prüfung ist erreicht. Kein weiterer Vorschlag erzeugt.',
    compilation_failed: 'Der Vorschlag lässt sich mit den aktuellen eigenen Regeln nicht kompilieren. Die vorhandene Konfiguration bleibt erhalten.',
  },
} satisfies Record<Language, Record<Reason, string>>

export function proposalReasonText(reason: Reason, lang: Language): string {
  return reasons[lang][reason]
}

export function proposalRationaleText(rationale: Rationale, lang: Language): string {
  switch (rationale.kind) {
    case 'subject_majority': {
      const number = new Intl.NumberFormat(lang === 'de' ? 'de-DE' : 'en-US')
      const matching = number.format(rationale.matching)
      const total = number.format(rationale.total)
      return lang === 'de'
        ? `Der Betreff passt bei ${matching} von ${total} ${rationale.total === 1 ? 'Nachricht' : 'Nachrichten'} zu dieser Kategorie der Voreinstellung. Das ist eine strikte Mehrheit.`
        : `Subject evidence supports this preset category for ${matching} of ${total} ${rationale.total === 1 ? 'message' : 'messages'}, a strict majority.`
    }
    case 'mapped_folder':
      return lang === 'de'
        ? 'Der beobachtete Ablageordner ist dieser Kategorie durch die Voreinstellung oder vorhandene eigene Regeln zugeordnet.'
        : 'The observed filing folder maps to this category through the preset or existing owner rules.'
    default: {
      const exhaustive: never = rationale
      return exhaustive
    }
  }
}

export const proposalStatusText = {
  en: {
    unknownSender: 'Missing or ambiguous From address',
    gate: { blocked: 'Blocked by proof', warnings: 'Proof completed with warnings' },
    disposition: { accept: 'Accept local changes', reject: 'Reject exact proposals' },
    effects: {
      accept: 'Save exactly the selected local filing rules and decisions. No export, ledger-file change, provider operation or mail action. Run fresh proof after saving.',
      reject: 'Record only the selected exact rejections. Rule documents remain unchanged.',
    },
    exactEffects: 'Original confirmation text',
    originalDiagnostics: 'Original technical proof diagnostics below remain in English.',
  },
  de: {
    unknownSender: 'Fehlende oder mehrdeutige From-Adresse',
    gate: { blocked: 'Durch Prüfung gesperrt', warnings: 'Prüfung mit Hinweisen abgeschlossen' },
    disposition: { accept: 'Lokale Änderungen annehmen', reject: 'Genaue Vorschläge ablehnen' },
    effects: {
      accept: 'Speichert genau die ausgewählten lokalen Ablageregeln und Entscheidungen. Kein Export, keine Änderung der Adressdatei und keine Aktion beim Anbieter oder an Nachrichten. Gespeicherte Regeln anschließend neu prüfen.',
      reject: 'Speichert nur die ausgewählten genauen Ablehnungen. Die Regeldokumente bleiben unverändert.',
    },
    exactEffects: 'Originaltext der Bestätigung',
    originalDiagnostics: 'Die folgenden ursprünglichen technischen Prüfhinweise bleiben auf Englisch.',
  },
}
