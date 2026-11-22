import { ShieldAlert } from "lucide-react";
import type { ClosePreview, GmailUndo, ProtocolUndo } from "../../api";
import {
  Action,
  Busy,
  CheckField,
  Modal,
  Stat,
  StatusBadge,
  useCopy,
} from "../components";

export type PendingRunModal =
  | { kind: "undo"; runId: string; preview: ProtocolUndo | GmailUndo }
  | { kind: "resume"; runId: string; mode: "resume" | "resume_undo" }
  | { kind: "close"; runId: string; preview: ClosePreview };

export function BacklogRunModal({
  runModal,
  confirmed,
  busy,
  liveWrites,
  syntheticOnly,
  onConfirmedChange,
  onClose,
  onOpenWriteMode,
  onExecute,
}: {
  runModal: PendingRunModal | null;
  confirmed: boolean;
  busy: boolean;
  liveWrites: boolean;
  syntheticOnly: boolean;
  onConfirmedChange: (value: boolean) => void;
  onClose: () => void;
  onOpenWriteMode: () => void;
  onExecute: () => void;
}) {
  const t = useCopy();

  return (
    <Modal
      open={runModal !== null}
      onOpenChange={(open) => {
        if (!busy && !open) onClose();
      }}
      title={
        runModal?.kind === "undo"
          ? t("Confirm conditional undo", "Bedingte Rücknahme bestätigen")
          : runModal?.kind === "close"
            ? t("Close and compact backlog run", "Bestandslauf abschließen")
            : runModal?.mode === "resume_undo"
              ? t("Resume interrupted undo", "Unterbrochene Rücknahme fortsetzen")
              : t("Resume stopped backlog run", "Angehaltenen Lauf fortsetzen")
      }
    >
      {runModal && (
        <div className="page-stack">
          {runModal.kind === "undo" && (
            <>
              <div className="stats compact">
                <Stat
                  label={t("Reverse batches", "Rücknahme-Stapel")}
                  value={runModal.preview.reverse.length}
                />
                <Stat
                  label={t("Conflicts skipped", "Übersprungene Konflikte")}
                  value={runModal.preview.conflicts}
                />
              </div>
              <p className="help">{runModal.preview.limitations}</p>
              <CheckField checked={confirmed} onChange={onConfirmedChange}>
                {t(
                  "Move unchanged messages from this run back to their recorded source folders/labels.",
                  "Unveränderte Nachrichten dieses Laufs in ihre ursprünglichen Ordner/Labels zurückbewegen.",
                )}
              </CheckField>
            </>
          )}
          {runModal.kind === "resume" && (
            <>
              <p className="muted">
                {runModal.mode === "resume_undo"
                  ? t(
                      `Continue undoing remaining batches for run ${runModal.runId.slice(0, 12)}.`,
                      `Verbleibende Rücknahme-Stapel für Lauf ${runModal.runId.slice(0, 12)} fortsetzen.`,
                    )
                  : t(
                      `Continue sorting remaining batches for run ${runModal.runId.slice(0, 12)}.`,
                      `Verbleibende Sortier-Stapel für Lauf ${runModal.runId.slice(0, 12)} fortsetzen.`,
                    )}
              </p>
              <CheckField checked={confirmed} onChange={onConfirmedChange}>
                {t(
                  "I want to continue executing this journaled run on my mail account.",
                  "Ich möchte diesen protokollierten Lauf auf meinem Mailkonto fortsetzen.",
                )}
              </CheckField>
            </>
          )}
          {runModal.kind === "close" && (
            <>
              <p className="muted">
                {t(
                  "Closing compacts this run into a summary and ends its local undo journal.",
                  "Abschließen verdichtet diesen Lauf zu einer Zusammenfassung und beendet das lokale Rücknahmeprotokoll.",
                )}
              </p>
              <p className="help">
                {t("Reason", "Grund")}:{" "}
                <StatusBadge status={runModal.preview.reason} />
              </p>
              <CheckField checked={confirmed} onChange={onConfirmedChange}>
                {t(
                  "Close this run and remove its detailed undo journal.",
                  "Diesen Lauf abschließen und das detaillierte Rücknahmeprotokoll entfernen.",
                )}
              </CheckField>
            </>
          )}
          <div className="dialog-actions">
            <Action onClick={onClose} disabled={busy}>
              {t("Cancel", "Abbrechen")}
            </Action>
            {!liveWrites && !syntheticOnly && runModal.kind !== "close" ? (
              <Action tone="warning" onClick={onOpenWriteMode}>
                <ShieldAlert size={15} />
                {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
              </Action>
            ) : (
              <Action
                tone={runModal.kind === "close" ? "warning" : "primary"}
                disabled={!confirmed || busy}
                onClick={onExecute}
              >
                {busy ? (
                  <Busy>{t("Working…", "Wird ausgeführt…")}</Busy>
                ) : runModal.kind === "undo" ? (
                  t("Start undo", "Rücknahme starten")
                ) : runModal.kind === "close" ? (
                  t("Close run", "Lauf abschließen")
                ) : (
                  t("Continue", "Fortsetzen")
                )}
              </Action>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}
