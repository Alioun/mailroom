export type BadgeTone = "success" | "warning" | "danger" | "info" | "subtle";

export type CopyFn = (en: string, de: string) => string;

export interface StatusMeta {
  label: string;
  tone: BadgeTone;
}

export function statusMeta(raw: string, t: CopyFn): StatusMeta {
  const key = raw.trim().toLowerCase();
  const table: Record<string, StatusMeta> = {
    complete: { label: t("Complete", "Abgeschlossen"), tone: "success" },
    completed: { label: t("Completed", "Abgeschlossen"), tone: "success" },
    observed: { label: t("Observed", "Bestätigt"), tone: "success" },
    verified: { label: t("Verified", "Geprüft"), tone: "success" },
    ready: { label: t("Ready", "Bereit"), tone: "success" },
    pass: { label: t("Pass", "Bestanden"), tone: "success" },
    passed: { label: t("Passed", "Bestanden"), tone: "success" },
    active: { label: t("Active", "Aktiv"), tone: "success" },
    matches: { label: t("Matched", "Übereinstimmend"), tone: "success" },
    matched: { label: t("Matched", "Übereinstimmend"), tone: "success" },
    moved: { label: t("Moved", "Umgezogen"), tone: "success" },
    undone: { label: t("Undone", "Rückgängig gemacht"), tone: "success" },
    kept: { label: t("Kept", "Behalten"), tone: "success" },
    muted: { label: t("Muted", "Stummgeschaltet"), tone: "success" },
    http_observed: { label: t("POST received", "POST erhalten"), tone: "success" },
    submitted: { label: t("Sent via SMTP", "Per SMTP gesendet"), tone: "success" },
    mail_handoff: { label: t("Sent (fixture)", "Gesendet (Test)"), tone: "success" },
    local_rules_saved: { label: t("Saved locally", "Lokal gespeichert"), tone: "success" },
    ok: { label: t("OK", "OK"), tone: "success" },
    clear: { label: t("Clear", "Frei"), tone: "success" },

    running: { label: t("Running", "Läuft"), tone: "info" },
    launching: { label: t("Starting T3", "T3 startet"), tone: "info" },
    launched: { label: t("Waiting for T3", "Wartet auf T3"), tone: "info" },
    connected: { label: t("T3 reading summary", "T3 liest Zusammenfassung"), tone: "info" },
    proposal_ready: { label: t("Proposal ready", "Vorschlag bereit"), tone: "info" },
    prepared: { label: t("Prepared", "Vorbereitet"), tone: "info" },
    intent: { label: t("In progress", "In Bearbeitung"), tone: "info" },
    pending: { label: t("Pending", "Ausstehend"), tone: "info" },
    handed_off: { label: t("Opened in Mail", "In Mail geöffnet"), tone: "info" },

    uncertain: { label: t("Uncertain — reconcile", "Unklar — abgleichen"), tone: "warning" },
    stopped: { label: t("Stopped", "Angehalten"), tone: "warning" },
    fresh_review: { label: t("Needs fresh review", "Neu prüfen"), tone: "warning" },
    undo_running: { label: t("Undo in progress", "Rücknahme läuft"), tone: "warning" },
    waiting: { label: t("Still writing here", "Schreibt noch hier"), tone: "warning" },
    elsewhere: { label: t("In another folder", "In anderem Ordner"), tone: "warning" },
    additional_folders: { label: t("Extra folders", "Zusätzliche Ordner"), tone: "warning" },
    uncheckable: { label: t("Uncheckable", "Nicht prüfbar"), tone: "warning" },
    warning: { label: t("Warning", "Warnung"), tone: "warning" },
    warn: { label: t("Warning", "Warnung"), tone: "warning" },
    not_ready: { label: t("Not ready yet", "Noch nicht bereit"), tone: "warning" },
    unverified: { label: t("Not yet verified", "Noch nicht geprüft"), tone: "warning" },
    partial: { label: t("Partial", "Teilweise"), tone: "warning" },
    stale: { label: t("Stale", "Veraltet"), tone: "warning" },
    review: { label: t("Needs review", "Prüfung nötig"), tone: "warning" },
    unknown: { label: t("Unknown", "Unbekannt"), tone: "warning" },
    quiet: { label: t("Quiet", "Ruhig"), tone: "warning" },
    unresolved: { label: t("Unresolved", "Ungeklärt"), tone: "warning" },
    bulk: { label: t("Bulk", "Massenmail"), tone: "warning" },
    creating: { label: t("Setup interrupted", "Einrichtung unterbrochen"), tone: "warning" },
    replacing: { label: t("Password update interrupted", "Passwortänderung unterbrochen"), tone: "warning" },
    rotating: { label: t("Key rotation interrupted", "Schlüsselrotation unterbrochen"), tone: "warning" },
    forgetting: { label: t("Removal interrupted", "Entfernen unterbrochen"), tone: "warning" },

    conflict: { label: t("Conflict", "Konflikt"), tone: "danger" },
    refused: { label: t("Refused", "Abgelehnt"), tone: "danger" },
    failed: { label: t("Failed", "Fehlgeschlagen"), tone: "danger" },
    blocked: { label: t("Blocked", "Blockiert"), tone: "danger" },
    fail: { label: t("Failed", "Fehlgeschlagen"), tone: "danger" },
    not_sent: { label: t("Not sent", "Nicht gesendet"), tone: "danger" },
    error: { label: t("Error", "Fehler"), tone: "danger" },
    edits_rejected: { label: t("Rejected", "Abgelehnt"), tone: "danger" },

    dismissed: { label: t("Dismissed", "Ausgeblendet"), tone: "subtle" },
    retired: { label: t("Retired", "Stillgelegt"), tone: "subtle" },
    mute_removed: { label: t("Mute removed", "Stummschaltung entfernt"), tone: "subtle" },
    closed: { label: t("Closed", "Abgeschlossen"), tone: "subtle" },
    not_winding_down: { label: t("Tracking off", "Tracking aus"), tone: "subtle" },
    cancelled: { label: t("Cancelled", "Abgebrochen"), tone: "subtle" },
    idle: { label: t("Idle", "Bereit"), tone: "subtle" },
    forgotten: { label: t("Removed", "Entfernt"), tone: "subtle" },
    armed: { label: t("Live writes armed", "Live-Schreibzugriff aktiv"), tone: "warning" },
    disarmed: { label: t("Read-only", "Nur-Lese"), tone: "success" },
    locked: { label: t("Vault locked", "Tresor gesperrt"), tone: "subtle" },
  };
  if (table[key]) return table[key];
  const fallback = raw
    .replace(/_/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase());
  return { label: fallback, tone: "subtle" };
}

export function vaultProblemText(
  problem: "unsafe_file" | "damaged" | "unsupported_version" | null,
  t: CopyFn,
): string {
  switch (problem) {
    case "unsafe_file":
      return t(
        "The vault file permissions or ownership are unsafe (or it is a symlink). Mailroom refused to read it. Fix permissions to owner-only (chmod 600) on the vault file and reload.",
        "Die Dateirechte oder der Eigentümer des Tresors sind unsicher (oder es ist ein Symlink). Mailroom hat den Zugriff verweigert. Setze die Rechte auf nur Eigentümer (chmod 600) und lade neu.",
      );
    case "damaged":
      return t(
        "The vault file is damaged or truncated. Mailroom left it unchanged. Restore from a backup or move the damaged file aside before creating a fresh vault.",
        "Die Tresordatei ist beschädigt oder unvollständig. Mailroom hat sie unverändert gelassen. Stelle eine Sicherung wieder her oder verschiebe die beschädigte Datei.",
      );
    case "unsupported_version":
      return t(
        "This vault file was created by a newer version of Mailroom. Update Mailroom before opening it.",
        "Diese Tresordatei stammt von einer neueren Mailroom-Version. Aktualisiere Mailroom vor dem Öffnen.",
      );
    default:
      return t(
        "The local vault could not be opened. Mailroom left all files unchanged.",
        "Der lokale Tresor konnte nicht geöffnet werden. Mailroom hat alle Dateien unverändert gelassen.",
      );
  }
}

export function sieveRefusalText(reason: string, t: CopyFn): string {
  const map: Record<string, string> = {
    placement_unchosen: t(
      "Choose a coexistence strategy (Block or Include) and save it before previewing deployment.",
      "Wähle zuerst eine Koexistenz-Strategie (Block oder Include) und speichere sie vor der Vorschau.",
    ),
    invalid_placement: t(
      "The selected placement script name is invalid or conflicts with a reserved name. Choose a simple script name such as 'mailroom'.",
      "Der gewählte Skriptname ist ungültig oder reserviert. Wähle einen einfachen Namen wie ‚mailroom‘.",
    ),
    host_missing: t(
      "No Open-Xchange host script was found on the server. Run ManageSieve discovery first, or verify Open-Xchange rules exist in mailbox.org.",
      "Kein Open-Xchange-Skript auf dem Server gefunden. Starte zuerst die ManageSieve-Prüfung oder prüfe die Filter in mailbox.org.",
    ),
    host_inactive: t(
      "The Open-Xchange script exists on the server, but another script is currently active. Activate Open-Xchange in mailbox.org or inspect existing scripts.",
      "Das Open-Xchange-Skript existiert, ist aber nicht aktiv. Aktiviere es in mailbox.org oder prüfe die vorhandenen Skripte.",
    ),
    unparseable: t(
      "Existing server Sieve rules could not be parsed safely. Mailroom will not overwrite rules it cannot understand.",
      "Bestehende Sieve-Regeln auf dem Server konnten nicht sicher geparst werden. Mailroom überschreibt keine unbekannten Regeln.",
    ),
    marker_damaged: t(
      "The existing Mailroom block markers inside Open-Xchange are incomplete or damaged. Inspect the observed script before changing it.",
      "Die Mailroom-Markierungen im Open-Xchange-Skript sind beschädigt. Prüfe das gelesene Skript zuerst.",
    ),
    owned_name_foreign: t(
      "A script with the chosen Mailroom name already exists on the server and was not created by Mailroom. Pick a different Include script name.",
      "Ein Skript mit dem gewählten Namen existiert bereits auf dem Server und stammt nicht von Mailroom. Wähle einen anderen Skriptnamen.",
    ),
    unsafe_rule: t(
      "A rule in your workspace cannot be compiled safely for this server's Sieve extensions. Review Validation details in Local rules.",
      "Eine Regel im Arbeitsstand lässt sich mit den Sieve-Erweiterungen dieses Servers nicht sicher kompilieren. Prüfe die Prüfdetails unter Lokale Regeln.",
    ),
    mailroom_unreachable: t(
      "Existing Open-Xchange rules stop processing every message before Mailroom's rules could run.",
      "Bestehende Open-Xchange-Regeln stoppen alle Nachrichten, bevor die Mailroom-Regeln erreicht werden.",
    ),
    mailroom_content_invalid: t(
      "The compiled Mailroom Sieve script failed validation or exceeded the server's script size limit.",
      "Das kompilierte Mailroom-Sieve-Skript hat die Prüfung nicht bestanden oder überschreitet das Größenlimit des Servers.",
    ),
    proof_blocked: t(
      "A pinned expectation or validation check is failing in Local rules. Fix blocked pins in Local rules & proof first.",
      "Ein gesetzter Pin oder eine Prüfregel ist blockiert. Behebe blockierte Pins zuerst unter Lokale Regeln & Prüfung.",
    ),
    nothing_to_remove: t(
      "No Mailroom Sieve block or script is currently installed on the server.",
      "Auf dem Server ist derzeit kein Mailroom-Sieve-Block oder -Skript installiert.",
    ),
  };
  return map[reason] ?? reason.replace(/_/g, " ");
}

export function winddownBlockerText(blocker: string, t: CopyFn): string {
  const map: Record<string, string> = {
    not_winding_down: t(
      "Wind-down tracking is currently turned off for this account.",
      "Das Abbau-Tracking ist für dieses Konto derzeit deaktiviert.",
    ),
    comparison_accounts_absent: t(
      "Select at least one destination account (e.g. your mailbox.org account) below so Mailroom can verify which senders have moved.",
      "Wähle unten mindestens ein Zielkonto (z. B. dein mailbox.org-Konto), damit Mailroom prüfen kann, welche Absender bereits umgezogen sind.",
    ),
    waiting_senders: t(
      "Some important senders or people are still writing to this account.",
      "Einige wichtige Absender oder Kontakte schreiben noch an dieses Konto.",
    ),
    unresolved_senders: t(
      "Some recent senders could not be classified yet and need review.",
      "Einige aktuelle Absender konnten noch nicht eingeordnet werden.",
    ),
    incomplete_census: t(
      "The saved scan for this account or a comparison account is incomplete. Run a full scan first.",
      "Die gespeicherte Erfassung für dieses oder ein Vergleichskonto ist unvollständig. Führe zuerst eine vollständige Erfassung aus.",
    ),
    stale_census: t(
      "Saved scan data is stale. Refresh scans on both accounts for a current verdict.",
      "Die gespeicherten Erfassungsdaten sind veraltet. Aktualisiere die Erfassung beider Konten.",
    ),
    missing_own_addresses: t(
      "Confirm your own email addresses in Settings for both accounts so delivery can be matched accurately.",
      "Bestätige deine eigenen E-Mail-Adressen in den Einstellungen beider Konten.",
    ),
  };
  return (
    map[blocker] ??
    blocker.replace(/_/g, " ").replace(/^\w/, (c) => c.toUpperCase())
  );
}

export function winddownGroupText(group: string, t: CopyFn): string {
  const map: Record<string, string> = {
    people: t("Person", "Kontakt"),
    service: t("Service", "Dienst"),
    bulk: t("Bulk", "Massenmail"),
    unresolved: t("Unresolved", "Ungeklärt"),
  };
  return map[group] ?? group;
}

export function invoiceRoleText(role: string, t: CopyFn): StatusMeta {
  const map: Record<string, StatusMeta> = {
    business: { label: t("Business", "Geschäftlich"), tone: "success" },
    personal: { label: t("Personal", "Privat"), tone: "subtle" },
    mixed: { label: t("Mixed", "Gemischt"), tone: "warning" },
    unknown: { label: t("Unknown role", "Unbekannt"), tone: "warning" },
    unknown_or_mixed: { label: t("Unknown / mixed", "Unbekannt / gemischt"), tone: "warning" },
  };
  return map[role] ?? statusMeta(role, t);
}

export function localizedError(err: unknown, t: CopyFn, fallback: string): string {
  const code =
    typeof err === "object" && err !== null && "code" in err && typeof err.code === "string"
      ? err.code
      : undefined;
  const map: Record<string, string> = {
    no_local_rules: t(
      "Create and save local rules on Rules & deploy before previewing a backlog sort.",
      "Erstelle und speichere zuerst lokale Regeln unter Regeln & Bereitstellung, bevor du den Bestand prüfst.",
    ),
    scan_incomplete: t(
      "Finish scanning this mailbox before previewing a backlog sort.",
      "Schließe zuerst die Postfacherfassung ab, bevor du den Bestand prüfst.",
    ),
    pending_unsubscribe: t(
      "Reconcile pending unsubscribe actions before starting a backlog sort.",
      "Kläre zuerst ausstehende Abmeldungen, bevor du den Bestand sortierst.",
    ),
    pending_invoice: t(
      "Resolve pending invoice sends before starting a backlog sort.",
      "Kläre zuerst ausstehende Rechnungsversände, bevor du den Bestand sortierst.",
    ),
    job_running: t(
      "Wait for the active account operation to finish first.",
      "Warte zuerst, bis die laufende Kontoaktion abgeschlossen ist.",
    ),
    recovery_pending: t(
      "Run a fresh scan to reconcile this account after recovery.",
      "Starte nach der Wiederherstellung zuerst eine neue Erfassung.",
    ),
    backlog_rejected: t(
      "Backlog preview could not run. Save local rules first or refresh this view.",
      "Die Bestandsvorschau konnte nicht ausgeführt werden. Speichere zuerst lokale Regeln oder aktualisiere die Ansicht.",
    ),
  };
  if (code && map[code]) return map[code];
  return describeError(err, fallback);
}

export function describeError(err: unknown, fallback: string): string {
  if (err instanceof Error && err.message && err.message !== "rejected" && err.message !== "unavailable") {
    return err.message;
  }
  return fallback;
}
