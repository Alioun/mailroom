import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, FolderPlus, RefreshCw, ShieldAlert } from "lucide-react";
import type { Account, AccountStatus } from "../../api";
import {
  folderCommand,
  folderPreview,
  folderReconcile,
  folderStatus,
} from "../../folder-provisioning-api";
import type { FolderPreview, FolderView } from "../../folder-provisioning-api";
import { describeError } from "../status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  Notice,
  StatusBadge,
  useCopy,
} from "../components";

export function FoldersPanel({
  account,
  liveWrites,
  liveJob,
  onOpenWriteMode,
}: {
  account: Account;
  liveWrites: boolean;
  liveJob: AccountStatus["job"];
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<FolderView | null>(null);
  const [preview, setPreview] = useState<FolderPreview | null>(null);
  const [intent, setIntent] = useState<"sort" | "archive_unmatched">("sort");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const jobRunning = liveJob?.progress.state === "running";

  const load = useCallback(async () => {
    setError("");
    try {
      const next = await folderStatus(account.id);
      setView(next);
      setUnavailable(false);
    } catch {
      if (!jobRunning) setUnavailable((prev) => (view ? prev : true));
    }
  }, [account.id, jobRunning]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!jobRunning) return;
    const timer = setInterval(() => void load(), 2000);
    return () => clearInterval(timer);
  }, [jobRunning, load]);

  async function runPreview() {
    setBusy(true);
    setError("");
    setMessage("");
    setConfirmed(false);
    try {
      setPreview(await folderPreview(account.id, intent));
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview folders. Make sure local rules are saved and pass validation, and the server is reachable.",
            "Ordnervorschau fehlgeschlagen. Prüfe, ob lokale Regeln gespeichert und gültig sind und der Server erreichbar ist.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmCreate() {
    if (!preview || !confirmed) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await folderCommand(account.id, {
        action: "confirm",
        receipt: preview.receipt,
        confirmed: true,
        intent,
      });
      setPreview(null);
      setConfirmed(false);
      setMessage(
        t(
          "Folder creation started. Status refreshes automatically as folders are verified.",
          "Ordnererstellung gestartet. Der Status aktualisiert sich automatisch nach der Prüfung.",
        ),
      );
      setTimeout(() => void load(), 1200);
    } catch (err) {
      setError(
        liveWrites
          ? describeError(
              err,
              t(
                "Folder creation was refused or interrupted. Refresh status before retrying.",
                "Ordnererstellung wurde abgelehnt oder unterbrochen. Aktualisiere den Stand vor einem neuen Versuch.",
              ),
            )
          : t(
              "Live provider writes are locked in Read-only mode. Enable Live writes first.",
              "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  if (unavailable && !view) {
    return (
      <Empty
        title={t(
          "Folder provisioning unavailable",
          "Ordneranlage nicht verfügbar",
        )}
      >
        {t(
          "Connect a mailbox.org vault account to preview and create IMAP folders.",
          "Verbinde ein mailbox.org-Tresorkonto, um IMAP-Ordner vorzuschauen und anzulegen.",
        )}
      </Empty>
    );
  }

  const missingEntries =
    preview?.operation.plan.entries.filter((e) => e.existing === null) ?? [];

  return (
    <div className="page-stack">
      {!liveWrites && (
        <Notice tone="info">
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. You can preview missing folders now; creating them requires enabling Live writes.",
              "Der Nur-Lese-Modus ist aktiv. Du kannst fehlende Ordner sofort prüfen; zum Anlegen aktiviere Live-Schreibzugriff.",
            )}
          </span>
          <Action size="compact" tone="warning" onClick={onOpenWriteMode}>
            {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
          </Action>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          <span>{message}</span>
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("IMAP folder provisioning", "IMAP-Ordner anlegen")}</h2>
            <p className="muted">
              {t(
                "Compare folders required by your saved rules against live IMAP mailboxes and create only what is missing.",
                "Vergleiche die Zielordner deiner gespeicherten Regeln mit dem IMAP-Postfach und lege nur fehlende Ordner an.",
              )}
            </p>
          </div>
          <Action onClick={() => void load()} disabled={busy}>
            <RefreshCw size={15} />
            {t("Refresh", "Aktualisieren")}
          </Action>
        </div>
        <div className="toolbar">
          <Choice
            label={t("Folder intent", "Ordnerzweck")}
            value={intent}
            onChange={(value) => {
              if (value === "sort" || value === "archive_unmatched") {
                setIntent(value);
                setPreview(null);
              }
            }}
            options={[
              {
                value: "sort",
                label: t("Rule target folders", "Zielordner der Regeln"),
              },
              {
                value: "archive_unmatched",
                label: t(
                  "Archive/<year> unmatched folders",
                  "Archive/<Jahr> für nicht zugeordnete Mail",
                ),
              },
            ]}
          />
          <Action
            primary
            onClick={() => void runPreview()}
            disabled={busy || jobRunning}
          >
            {busy ? (
              <Busy>{t("Inspecting IMAP…", "IMAP wird geprüft…")}</Busy>
            ) : (
              <>
                <FolderPlus size={15} />
                {t("Preview missing folders", "Fehlende Ordner prüfen")}
              </>
            )}
          </Action>
        </div>
        {preview && (
          <div className="subpanel">
            <h3>
              {t("Folder creation plan", "Plan zur Ordnererstellung")} ·{" "}
              {missingEntries.length} {t("missing", "fehlend")}
            </h3>
            {missingEntries.length === 0 ? (
              <p className="muted">
                {t(
                  "All target folders already exist on the server.",
                  "Alle Zielordner existieren bereits auf dem Server.",
                )}
              </p>
            ) : (
              <>
                <ul className="evidence-list">
                  {preview.operation.plan.entries.map((entry) => (
                    <li key={entry.name.wire}>
                      <code>{entry.name.decoded}</code> ·{" "}
                      {entry.existing
                        ? t("Already exists", "Bereits vorhanden")
                        : entry.role === "ancestor"
                          ? t("Will create parent", "Überordner wird angelegt")
                          : t("Will create target", "Zielordner wird angelegt")}
                    </li>
                  ))}
                </ul>
                <CheckField checked={confirmed} onChange={setConfirmed}>
                  {t(
                    "I reviewed the exact folder list above and want to create these folders on the mail server.",
                    "Ich habe die genaue Ordnerliste oben geprüft und möchte diese Ordner auf dem Mailserver anlegen.",
                  )}
                </CheckField>
                <div className="actions">
                  <Action onClick={() => setPreview(null)} disabled={busy}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                  {!liveWrites ? (
                    <Action tone="warning" onClick={onOpenWriteMode}>
                      <ShieldAlert size={15} />
                      {t("Enable live writes to create", "Live-Schreibzugriff aktivieren")}
                    </Action>
                  ) : (
                    <Action
                      primary
                      disabled={!confirmed || busy}
                      onClick={() => void confirmCreate()}
                    >
                      <FolderPlus size={15} />
                      {t("Create folders", "Ordner anlegen")}
                    </Action>
                  )}
                </div>
              </>
            )}
          </div>
        )}
      </section>
      {view && view.journal.history.length > 0 && (
        <section className="panel">
          <h2>{t("Folder operation history", "Verlauf der Ordneranlage")}</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Operation", "Vorgang")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th className="numeric">{t("Folders", "Ordner")}</th>
                  <th>{t("Action", "Aktion")}</th>
                </tr>
              </thead>
              <tbody>
                {view.journal.history.map((op) => (
                  <tr key={op.id}>
                    <td>
                      <code>{op.id.slice(0, 12)}</code>
                    </td>
                    <td>
                      <StatusBadge status={op.status} />
                    </td>
                    <td className="numeric">{op.plan.entries.length}</td>
                    <td>
                      <div className="cell-actions">
                        {(op.status === "uncertain" ||
                          op.status === "stopped") && (
                          <Action
                            size="compact"
                            disabled={busy}
                            onClick={async () => {
                              setBusy(true);
                              setError("");
                              try {
                                setView(
                                  await folderReconcile(account.id, op.id),
                                );
                                setMessage(
                                  t(
                                    "Folder operation reconciled against live server.",
                                    "Ordnervorgang mit dem Server abgeglichen.",
                                  ),
                                );
                              } catch (err) {
                                setError(
                                  describeError(
                                    err,
                                    t(
                                      "Reconciliation failed. Check connectivity and try again.",
                                      "Abgleich fehlgeschlagen. Prüfe die Verbindung und versuche es erneut.",
                                    ),
                                  ),
                                );
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            {t("Reconcile", "Abgleichen")}
                          </Action>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  );
}

