import { useCallback, useEffect, useState } from "react";
import {
  Archive,
  CheckCircle2,
  FolderSync,
  Play,
  RefreshCw,
  ShieldAlert,
  Square,
  Undo2,
} from "lucide-react";
import {
  confirmProtocolBacklog,
  gmailAction,
  openBacklog,
  openGmailBacklog,
  openProtocolBacklog,
  previewBacklog,
  previewClose,
  previewGmailBacklog,
  previewGmailUndo,
  previewProtocolBacklog,
  previewProtocolUndo,
  protocolAction,
  resumeBacklog,
  resumeBacklogUndo,
  sortingPost,
  startBacklog,
  stopBacklog,
  undoBacklog,
  previewBacklogUndo,
} from "../api";
import type {
  Account,
  AccountStatus,
  BacklogOptions,
  BacklogPreview,
  BacklogView,
  ClosePreview,
  GmailPreview,
  GmailUndo,
  GmailView,
  ProtocolPreview,
  ProtocolUndo,
  ProtocolView,
} from "../api";
import { labelPreviewFor } from "../folder-provisioning-api";
import type { LabelPreview } from "../folder-provisioning-api";
import { describeError } from "./status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  Modal,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "./components";

type PendingRunModal =
  | { kind: "undo"; runId: string; preview: ProtocolUndo | GmailUndo }
  | { kind: "resume"; runId: string; mode: "resume" | "resume_undo" }
  | { kind: "close"; runId: string; preview: ClosePreview };

export function Backlog({
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
  const isGmail = account.provider === "gmail";
  const [scope, setScope] =
    useState<BacklogOptions["scope"]>("inbox_unfiled");
  const [protectFlagged, setProtectFlagged] = useState(true);
  const [archiveUnmatched, setArchiveUnmatched] = useState(false);

  const [protocolView, setProtocolView] = useState<ProtocolView | null>(null);
  const [gmailView, setGmailView] = useState<GmailView | null>(null);
  const [legacyView, setLegacyView] = useState<BacklogView | null>(null);

  const [protoPreview, setProtoPreview] = useState<ProtocolPreview | null>(
    null,
  );
  const [gmailPreview, setGmailPreview] = useState<GmailPreview | null>(null);
  const [legacyPreview, setLegacyPreview] = useState<BacklogPreview | null>(
    null,
  );
  const [labelPreview, setLabelPreview] = useState<LabelPreview | null>(null);
  const [labelConfirmed, setLabelConfirmed] = useState(false);

  const [runModal, setRunModal] = useState<PendingRunModal | null>(null);
  const [runModalConfirmed, setRunModalConfirmed] = useState(false);

  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const options: BacklogOptions = {
    scope,
    protect_flagged: protectFlagged,
    selected: null,
    intent: archiveUnmatched ? "archive_unmatched" : "sort",
  };

  const jobRunning = liveJob?.progress.state === "running";

  const clearPreviews = () => {
    setProtoPreview(null);
    setGmailPreview(null);
    setLegacyPreview(null);
    setLabelPreview(null);
    setConfirmed(false);
    setLabelConfirmed(false);
  };

  const load = useCallback(async () => {
    const [p, g, l] = await Promise.allSettled([
      !isGmail ? openProtocolBacklog(account.id) : Promise.reject("gmail"),
      isGmail ? openGmailBacklog(account.id) : Promise.reject("mailbox"),
      openBacklog(account.id),
    ]);
    // Never null out existing views when a running background job temporarily holds the lock
    if (p.status === "fulfilled") setProtocolView(p.value);
    if (g.status === "fulfilled") setGmailView(g.value);
    if (l.status === "fulfilled") setLegacyView(l.value);
  }, [account.id, isGmail]);

  useEffect(() => {
    void load();
  }, [load]);

  const activeView = isGmail ? gmailView : protocolView;
  const runs = activeView?.journal.runs ?? [];
  const summaries = activeView?.journal.summaries ?? [];
  const closable = new Set(activeView?.closable ?? []);
  const deadlines = new Map(
    (activeView?.undo_deadlines ?? []).map((d) => [d.run, d]),
  );
  const hasRunningRun =
    jobRunning ||
    runs.some((r) => r.status === "running" || r.status === "undo_running");

  useEffect(() => {
    if (!hasRunningRun) return;
    const timer = setInterval(() => void load(), 1800);
    return () => clearInterval(timer);
  }, [hasRunningRun, load]);

  const protocolReady = isGmail
    ? Boolean(gmailView?.installed)
    : Boolean(protocolView?.installed);
  const syntheticOnly = !protocolReady && Boolean(legacyView?.synthetic);
  const backlogKind = isGmail ? "gmail" : "protocol";

  async function runPreview() {
    setBusy(true);
    setError("");
    setMessage("");
    clearPreviews();
    try {
      if (isGmail && gmailView?.installed) {
        setGmailPreview(await previewGmailBacklog(account.id, options));
      } else if (!isGmail && protocolView?.installed) {
        setProtoPreview(await previewProtocolBacklog(account.id, options));
      } else {
        setLegacyPreview(await previewBacklog(account.id, options));
      }
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview backlog sorting. Ensure local rules are saved and valid, and the mailbox is reachable.",
            "Bestandsvorschau fehlgeschlagen. Prüfe, ob lokale Regeln gespeichert und gültig sind und das Postfach erreichbar ist.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runConfirm() {
    if (!confirmed) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      if (gmailPreview) {
        await gmailAction(account.id, {
          action: "confirm",
          options: gmailPreview.operation.plan.filing.options,
          receipt: gmailPreview.receipt,
          confirmed: true,
        });
        setGmailPreview(null);
        setMessage(
          t(
            "Gmail label sorting started. Progress and readback refresh automatically.",
            "Gmail-Label-Sortierung gestartet. Fortschritt und Rücklesung aktualisieren sich automatisch.",
          ),
        );
      } else if (protoPreview) {
        await confirmProtocolBacklog(account.id, protoPreview);
        setProtoPreview(null);
        setMessage(
          t(
            "IMAP MOVE backlog sorting started. Progress and undo eligibility refresh automatically.",
            "IMAP-MOVE-Bestandssortierung gestartet. Fortschritt und Rücknahme aktualisieren sich automatisch.",
          ),
        );
      } else if (legacyPreview) {
        setLegacyView(await startBacklog(account.id, legacyPreview));
        setLegacyPreview(null);
        setMessage(
          t(
            "Synthetic backlog run started.",
            "Synthetischer Bestandslauf gestartet.",
          ),
        );
      }
      setConfirmed(false);
      setTimeout(() => void load(), 1000);
    } catch (err) {
      setError(
        describeError(
          err,
          liveWrites
            ? t(
                "Backlog execution could not start. Refresh and try again.",
                "Bestandssortierung konnte nicht starten. Aktualisiere den Stand und versuche es erneut.",
              )
            : t(
                "Live provider writes are locked in Read-only mode. Enable Live writes first.",
                "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff.",
              ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function previewMissingLabels() {
    setBusy(true);
    setError("");
    setLabelConfirmed(false);
    try {
      setLabelPreview(await labelPreviewFor(account.id, options));
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview missing Gmail labels.",
            "Fehlende Gmail-Labels konnten nicht geprüft werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmMissingLabels() {
    if (!labelPreview || !labelConfirmed) return;
    setBusy(true);
    setError("");
    try {
      await gmailAction(account.id, {
        action: "label_confirm",
        options,
        receipt: labelPreview.receipt,
        confirmed: true,
      });
      setLabelPreview(null);
      setLabelConfirmed(false);
      setMessage(
        t(
          "Gmail label creation started. Refresh and re-run backlog preview once complete.",
          "Erstellung der Gmail-Labels gestartet. Aktualisiere und prüfe den Bestand danach erneut.",
        ),
      );
      setTimeout(() => void load(), 1200);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not create missing labels. Ensure Live writes are enabled.",
            "Fehlende Labels konnten nicht angelegt werden. Prüfe, ob Live-Schreibzugriff aktiviert ist.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function openUndoModal(runId: string) {
    setBusy(true);
    setError("");
    setRunModalConfirmed(false);
    try {
      const undo = isGmail
        ? await previewGmailUndo(account.id, runId)
        : await previewProtocolUndo(account.id, runId);
      setRunModal({ kind: "undo", runId, preview: undo });
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview undo. Reconcile the run or check conflicts first.",
            "Rücknahmevorschau fehlgeschlagen. Gleiche den Lauf zuerst ab.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function openCloseModal(runId: string) {
    setBusy(true);
    setError("");
    setRunModalConfirmed(false);
    try {
      const close = await previewClose(backlogKind, account.id, runId);
      setRunModal({ kind: "close", runId, preview: close });
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Run cannot be closed yet.",
            "Lauf kann noch nicht abgeschlossen werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function executeRunModal() {
    if (!runModal || !runModalConfirmed) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      if (runModal.kind === "undo") {
        if (isGmail) {
          await gmailAction(account.id, {
            action: "undo",
            run: runModal.runId,
            receipt: runModal.preview.receipt,
            confirmed: true,
          });
        } else {
          await protocolAction(account.id, {
            action: "undo",
            run: runModal.runId,
            receipt: runModal.preview.receipt,
            confirmed: true,
          });
        }
        setMessage(
          t(
            "Conditional undo started. Progress updates automatically.",
            "Bedingte Rücknahme gestartet. Fortschritt aktualisiert sich automatisch.",
          ),
        );
      } else if (runModal.kind === "resume") {
        await sortingPost(backlogKind, account.id, {
          action: runModal.mode,
          run: runModal.runId,
        });
        setMessage(
          runModal.mode === "resume_undo"
            ? t("Undo resumed.", "Rücknahme fortgesetzt.")
            : t("Backlog run resumed.", "Bestandslauf fortgesetzt."),
        );
      } else if (runModal.kind === "close") {
        await sortingPost(backlogKind, account.id, {
          action: "close",
          run: runModal.runId,
          receipt: runModal.preview.receipt,
          confirmed: true,
        });
        setMessage(
          t(
            "Run closed and compacted into summary history.",
            "Lauf abgeschlossen und im Verlauf verdichtet.",
          ),
        );
      }
      setRunModal(null);
      setRunModalConfirmed(false);
      setTimeout(() => void load(), 1000);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Action could not execute. Refresh status and try again.",
            "Aktion konnte nicht ausgeführt werden. Aktualisiere den Stand.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function stopActiveRun() {
    setBusy(true);
    setError("");
    try {
      await sortingPost(backlogKind, account.id, { action: "cancel" });
      setMessage(
        t(
          "Stop requested. The current batch will halt cleanly.",
          "Stopp angefordert. Der aktuelle Stapel wird sauber angehalten.",
        ),
      );
      setTimeout(() => void load(), 1000);
    } catch (err) {
      setError(
        describeError(
          err,
          t("Could not stop the active run.", "Lauf konnte nicht gestoppt werden."),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const activePlan =
    gmailPreview?.operation.plan.filing ??
    protoPreview?.operation.plan.filing ??
    legacyPreview?.plan ??
    null;

  const excludedEntries = activePlan
    ? Object.entries(activePlan.excluded).filter(([, n]) => n > 0)
    : [];

  return (
    <div className="page-stack">
      {!liveWrites && !syntheticOnly && (
        <Notice tone="info">
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Previewing your backlog is always safe; executing moves, labels, resume, or undo requires enabling Live writes.",
              "Der Nur-Lese-Modus ist aktiv. Die Vorschau ist immer sicher; zum Verschieben, Labeln, Fortsetzen oder Rückgängigmachen aktiviere Live-Schreibzugriff.",
            )}
          </span>
          <Action size="compact" tone="warning" onClick={onOpenWriteMode}>
            {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
          </Action>
        </Notice>
      )}
      {hasRunningRun && (
        <Notice tone="info">
          <Busy>
            {t(
              "A backlog operation is currently running. Progress refreshes automatically.",
              "Eine Bestandssortierung läuft derzeit. Der Fortschritt wird automatisch aktualisiert.",
            )}
          </Busy>
          <Action
            size="compact"
            tone="warning"
            disabled={busy}
            onClick={() => void stopActiveRun()}
          >
            <Square size={14} />
            {t("Stop", "Stoppen")}
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
            <h2>
              {isGmail
                ? t(
                    "Sort existing Gmail backlog (labels)",
                    "Gmail-Bestand sortieren (Labels)",
                  )
                : t(
                    "Sort existing mailbox.org backlog (IMAP MOVE)",
                    "mailbox.org-Bestand sortieren (IMAP MOVE)",
                  )}
            </h2>
            <p className="muted">
              {t(
                "Journaled, batch-bounded sorting with flagged-message protection and conditional Undo. Sent, Drafts, Spam, and Trash stay untouched.",
                "Protokollierte Stapelsortierung mit Schutz markierter Nachrichten und bedingtem Rückgängigmachen. Gesendet, Entwürfe, Spam und Papierkorb bleiben unberührt.",
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
            label={t("Mode", "Modus")}
            value={archiveUnmatched ? "archive_unmatched" : "sort"}
            onChange={(value) => {
              setArchiveUnmatched(value === "archive_unmatched");
              clearPreviews();
            }}
            options={[
              {
                value: "sort",
                label: t(
                  "Sort matched mail by rules",
                  "Zugeordnete Mails nach Regeln sortieren",
                ),
              },
              {
                value: "archive_unmatched",
                label: t(
                  "Archive unmatched mail (>12 months) into Archive/<year>",
                  "Nicht zugeordnete Mails (>12 Monate) in Archive/<Jahr>",
                ),
              },
            ]}
          />
          <Choice
            label={t("Scope", "Bereich")}
            value={scope}
            onChange={(value) => {
              if (
                value === "inbox_unfiled" ||
                value === "inbox" ||
                value === "every_folder"
              ) {
                setScope(value);
                clearPreviews();
              }
            }}
            options={[
              {
                value: "inbox_unfiled",
                label: t("Inbox + unfiled mail", "Posteingang + unsortiert"),
              },
              {
                value: "inbox",
                label: t("Inbox only", "Nur Posteingang"),
              },
              {
                value: "every_folder",
                label: t(
                  "Every folder (advanced)",
                  "Alle Ordner (erweitert)",
                ),
              },
            ]}
          />
          <CheckField
            checked={protectFlagged}
            onChange={(next) => {
              setProtectFlagged(next);
              clearPreviews();
            }}
          >
            {t(
              "Keep flagged/starred messages",
              "Markierte Nachrichten schützen",
            )}
          </CheckField>
          <Action
            primary
            onClick={() => void runPreview()}
            disabled={busy || hasRunningRun}
          >
            {busy ? (
              <Busy>{t("Computing preview…", "Vorschau wird berechnet…")}</Busy>
            ) : (
              <>
                {archiveUnmatched ? (
                  <Archive size={15} />
                ) : (
                  <FolderSync size={15} />
                )}
                {archiveUnmatched
                  ? t("Preview archive", "Archivierung prüfen")
                  : t("Preview backlog sort", "Bestandssortierung prüfen")}
              </>
            )}
          </Action>
        </div>

        {gmailPreview && gmailPreview.missing_labels.length > 0 && (
          <Notice tone="warning">
            <div>
              <strong>
                {t(
                  "Some target Gmail labels do not exist yet:",
                  "Einige Ziel-Labels existieren in Gmail noch nicht:",
                )}
              </strong>
              <ul className="evidence-list" style={{ margin: "6px 0 0" }}>
                {gmailPreview.missing_labels.map((m) => (
                  <li key={m.name}>
                    <code>{m.name}</code> ({m.messages.toLocaleString()}{" "}
                    {t("messages", "Nachrichten")})
                    {m.blocked ? ` — ${m.blocked}` : ""}
                  </li>
                ))}
              </ul>
            </div>
            <Action
              size="compact"
              onClick={() => void previewMissingLabels()}
              disabled={busy}
            >
              {t("Preview missing labels", "Fehlende Labels prüfen")}
            </Action>
          </Notice>
        )}

        {labelPreview && (
          <div className="subpanel">
            <h3>
              {t(
                "Create missing Gmail labels",
                "Fehlende Gmail-Labels anlegen",
              )}
            </h3>
            <ul className="evidence-list">
              {labelPreview.operation.plan.entries.map((entry) => (
                <li key={entry.name.wire}>
                  <code>{entry.name.decoded}</code>
                </li>
              ))}
            </ul>
            <CheckField checked={labelConfirmed} onChange={setLabelConfirmed}>
              {t(
                "Create these exact Gmail labels on my account.",
                "Diese genauen Gmail-Labels in meinem Konto anlegen.",
              )}
            </CheckField>
            <div className="actions">
              <Action onClick={() => setLabelPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              {!liveWrites ? (
                <Action tone="warning" onClick={onOpenWriteMode}>
                  <ShieldAlert size={15} />
                  {t(
                    "Enable live writes to create",
                    "Live-Schreibzugriff aktivieren",
                  )}
                </Action>
              ) : (
                <Action
                  primary
                  disabled={!labelConfirmed || busy}
                  onClick={() => void confirmMissingLabels()}
                >
                  {t("Create labels", "Labels anlegen")}
                </Action>
              )}
            </div>
          </div>
        )}

        {activePlan && (
          <div className="subpanel">
            <h3>
              {t("Backlog sort preview", "Vorschau der Bestandssortierung")}
            </h3>
            <div className="stats compact">
              <Stat
                label={t("Planned moves/labels", "Geplante Änderungen")}
                value={activePlan.changes.length.toLocaleString()}
              />
              <Stat
                label={t("Unmatched (stay)", "Nicht zugeordnet")}
                value={activePlan.unmatched.toLocaleString()}
              />
              <Stat
                label={t("Held for review", "Zur Prüfung behalten")}
                value={activePlan.review.toLocaleString()}
              />
              <Stat
                label={t("Destinations", "Zielordner")}
                value={Object.keys(activePlan.groups).length.toLocaleString()}
              />
            </div>
            {(excludedEntries.length > 0 ||
              activePlan.stopped_folders.length > 0) && (
              <p className="help">
                {excludedEntries.length > 0 && (
                  <>
                    {t("Held back / excluded", "Ausgenommen")}:{" "}
                    {excludedEntries
                      .map(([k, v]) => `${k.replace(/_/g, " ")} (${v})`)
                      .join(", ")}
                    .{" "}
                  </>
                )}
                {activePlan.stopped_folders.length > 0 && (
                  <>
                    {t("Stopped folders skipped", "Übersprungene Ordner")}:{" "}
                    {activePlan.stopped_folders.join(", ")}.
                  </>
                )}
              </p>
            )}
            {Object.keys(activePlan.groups).length > 0 && (
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>{t("Destination", "Ziel")}</th>
                      <th className="numeric">{t("Messages", "Nachrichten")}</th>
                      <th>{t("Top senders", "Häufigste Absender")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {Object.entries(activePlan.groups).map(([dest, group]) => (
                      <tr key={dest}>
                        <th>{dest}</th>
                        <td className="numeric">
                          {group.count.toLocaleString()}
                        </td>
                        <td>
                          {group.top_senders
                            .slice(0, 3)
                            .map(([s, n]) => `${s} (${n})`)
                            .join(", ")}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {activePlan.changes.length > 0 ? (
              <>
                <CheckField checked={confirmed} onChange={setConfirmed}>
                  {t(
                    "I reviewed the planned destinations and counts above and want to run this journaled backlog sort.",
                    "Ich habe die geplanten Ziele und Zahlen oben geprüft und möchte diese protokollierte Bestandssortierung starten.",
                  )}
                </CheckField>
                <div className="actions">
                  <Action onClick={clearPreviews} disabled={busy}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                  {!liveWrites && !syntheticOnly ? (
                    <Action tone="warning" onClick={onOpenWriteMode}>
                      <ShieldAlert size={15} />
                      {t(
                        "Enable live writes to start",
                        "Live-Schreibzugriff aktivieren",
                      )}
                    </Action>
                  ) : (
                    <Action
                      primary
                      disabled={!confirmed || busy}
                      onClick={() => void runConfirm()}
                    >
                      <Play size={15} />
                      {t("Start backlog sort", "Sortierung starten")}
                    </Action>
                  )}
                </div>
              </>
            ) : (
              <p className="muted">
                {t(
                  "No messages need moving under the selected scope and saved rules.",
                  "Unter dem gewählten Bereich und den gespeicherten Regeln müssen keine Nachrichten verschoben werden.",
                )}
              </p>
            )}
          </div>
        )}
      </section>

      {runs.length > 0 && (
        <section className="panel">
          <h2>{t("Backlog runs and undo", "Bestandsläufe & Rücknahme")}</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Run", "Lauf")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th className="numeric">{t("Changes", "Änderungen")}</th>
                  <th>{t("Undo window", "Rücknahmefrist")}</th>
                  <th>{t("Actions", "Aktionen")}</th>
                </tr>
              </thead>
              <tbody>
                {runs.map((run) => {
                  const dl = deadlines.get(run.id);
                  const canClose = closable.has(run.id) && !run.closed;
                  const canResumeUndo =
                    run.status === "undo_running" ||
                    ((run.status === "uncertain" || run.status === "stopped") &&
                      Boolean(run.undo_receipt));
                  return (
                    <tr key={run.id}>
                      <td>
                        <code>{run.id.slice(0, 12)}</code>
                      </td>
                      <td>
                        <StatusBadge status={run.status} />
                      </td>
                      <td className="numeric">
                        {run.plan.filing.changes.length.toLocaleString()}
                      </td>
                      <td>
                        {run.closed
                          ? t("Closed", "Abgeschlossen")
                          : dl?.undo_until
                            ? new Date(dl.undo_until * 1000).toLocaleDateString()
                            : "—"}
                      </td>
                      <td>
                        <div className="cell-actions">
                          {(run.status === "running" ||
                            run.status === "undo_running") && (
                            <Action
                              size="compact"
                              tone="warning"
                              disabled={busy}
                              onClick={() => void stopActiveRun()}
                            >
                              <Square size={13} />
                              {t("Stop", "Stoppen")}
                            </Action>
                          )}
                          {(run.status === "uncertain" ||
                            run.status === "stopped" ||
                            run.status === "conflict") && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                setError("");
                                try {
                                  await sortingPost(backlogKind, account.id, {
                                    action: "reconcile",
                                    run: run.id,
                                  });
                                  setMessage(
                                    t(
                                      "Run reconciled against live server.",
                                      "Lauf mit dem Server abgeglichen.",
                                    ),
                                  );
                                  await load();
                                } catch (err) {
                                  setError(
                                    describeError(
                                      err,
                                      t(
                                        "Reconciliation failed. Check connectivity and retry.",
                                        "Abgleich fehlgeschlagen.",
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
                          {run.status === "stopped" && !run.undo_receipt && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={() => {
                                setRunModalConfirmed(false);
                                setRunModal({
                                  kind: "resume",
                                  runId: run.id,
                                  mode: "resume",
                                });
                              }}
                            >
                              {t("Resume…", "Fortsetzen…")}
                            </Action>
                          )}
                          {canResumeUndo && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={() => {
                                setRunModalConfirmed(false);
                                setRunModal({
                                  kind: "resume",
                                  runId: run.id,
                                  mode: "resume_undo",
                                });
                              }}
                            >
                              <Undo2 size={13} />
                              {t("Resume undo…", "Rücknahme fortsetzen…")}
                            </Action>
                          )}
                          {(run.status === "complete" ||
                            run.status === "stopped") &&
                            !run.closed &&
                            !run.undo_receipt && (
                              <Action
                                size="compact"
                                disabled={busy}
                                onClick={() => void openUndoModal(run.id)}
                              >
                                <Undo2 size={13} />
                                {t("Preview undo", "Rücknahme prüfen")}
                              </Action>
                            )}
                          {canClose && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={() => void openCloseModal(run.id)}
                            >
                              {t("Close run…", "Abschließen…")}
                            </Action>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {summaries.length > 0 && (
            <Details
              title={`${t("Closed & compacted run summaries", "Abgeschlossene Laufzusammenfassungen")} · ${summaries.length}`}
            >
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>{t("Run", "Lauf")}</th>
                      <th>{t("Status", "Status")}</th>
                      <th className="numeric">{t("Applied", "Angewendet")}</th>
                      <th className="numeric">{t("Restored", "Zurückgelegt")}</th>
                      <th>{t("Completed", "Abgeschlossen")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {summaries.map((s) => (
                      <tr key={s.id}>
                        <td>
                          <code>{s.id.slice(0, 12)}</code>
                        </td>
                        <td>
                          <StatusBadge status={s.status} />
                        </td>
                        <td className="numeric">{s.applied.toLocaleString()}</td>
                        <td className="numeric">
                          {s.restored.toLocaleString()}
                        </td>
                        <td>
                          {new Date(s.completed_at * 1000).toLocaleDateString()}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </Details>
          )}
        </section>
      )}

      {legacyView && legacyView.runs.length > 0 && (
        <Details
          title={`${t("Synthetic backlog runs", "Synthetische Bestandsläufe")} · ${legacyView.runs.length}`}
        >
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Run", "Lauf")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th>{t("Actions", "Aktionen")}</th>
                </tr>
              </thead>
              <tbody>
                {legacyView.runs.map((run) => (
                  <tr key={run.id}>
                    <td>
                      <code>{run.id.slice(0, 12)}</code>
                    </td>
                    <td>
                      <StatusBadge status={run.status} />
                    </td>
                    <td>
                      <div className="cell-actions">
                        {run.status === "running" && (
                          <Action
                            size="compact"
                            onClick={async () => {
                              try {
                                setLegacyView(
                                  await stopBacklog(account.id, run.id),
                                );
                              } catch (err) {
                                setError(
                                  describeError(
                                    err,
                                    t("Could not stop run.", "Stopp fehlgeschlagen."),
                                  ),
                                );
                              }
                            }}
                          >
                            {t("Stop", "Stoppen")}
                          </Action>
                        )}
                        {run.status === "stopped" && (
                          <Action
                            size="compact"
                            onClick={async () => {
                              try {
                                setLegacyView(
                                  await resumeBacklog(account.id, run.id),
                                );
                              } catch (err) {
                                setError(
                                  describeError(
                                    err,
                                    t("Could not resume run.", "Fortsetzen fehlgeschlagen."),
                                  ),
                                );
                              }
                            }}
                          >
                            {t("Resume", "Fortsetzen")}
                          </Action>
                        )}
                        {(run.status === "completed" ||
                          run.status === "stopped") && (
                          <Action
                            size="compact"
                            onClick={async () => {
                              try {
                                const undo = await previewBacklogUndo(
                                  account.id,
                                  run.id,
                                );
                                setLegacyView(
                                  await undoBacklog(
                                    account.id,
                                    run.id,
                                    undo.receipt,
                                  ),
                                );
                              } catch (err) {
                                setError(
                                  describeError(
                                    err,
                                    t("Undo failed.", "Rücknahme fehlgeschlagen."),
                                  ),
                                );
                              }
                            }}
                          >
                            {t("Undo", "Rückgängig")}
                          </Action>
                        )}
                        {run.undo_requested && run.status === "uncertain" && (
                          <Action
                            size="compact"
                            onClick={async () => {
                              try {
                                setLegacyView(
                                  await resumeBacklogUndo(account.id, run.id),
                                );
                              } catch (err) {
                                setError(
                                  describeError(
                                    err,
                                    t(
                                      "Could not resume undo.",
                                      "Rücknahme konnte nicht fortgesetzt werden.",
                                    ),
                                  ),
                                );
                              }
                            }}
                          >
                            {t("Resume undo", "Rücknahme fortsetzen")}
                          </Action>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Details>
      )}

      {!protocolReady && !syntheticOnly && !hasRunningRun && (
        <Empty
          title={t(
            "No backlog sorting source active",
            "Keine Bestandssortierung aktiv",
          )}
        >
          {t(
            "Save local rules and scan your mailbox to preview and run backlog sorting.",
            "Speichere lokale Regeln und erfasse dein Postfach, um die Bestandssortierung zu nutzen.",
          )}
        </Empty>
      )}

      <Modal
        open={runModal !== null}
        onOpenChange={(open) => {
          if (!busy && !open) {
            setRunModal(null);
            setRunModalConfirmed(false);
          }
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
                <CheckField
                  checked={runModalConfirmed}
                  onChange={setRunModalConfirmed}
                >
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
                <CheckField
                  checked={runModalConfirmed}
                  onChange={setRunModalConfirmed}
                >
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
                <CheckField
                  checked={runModalConfirmed}
                  onChange={setRunModalConfirmed}
                >
                  {t(
                    "Close this run and remove its detailed undo journal.",
                    "Diesen Lauf abschließen und das detaillierte Rücknahmeprotokoll entfernen.",
                  )}
                </CheckField>
              </>
            )}
            <div className="dialog-actions">
              <Action onClick={() => setRunModal(null)} disabled={busy}>
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
                  disabled={!runModalConfirmed || busy}
                  onClick={() => void executeRunModal()}
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
    </div>
  );
}
