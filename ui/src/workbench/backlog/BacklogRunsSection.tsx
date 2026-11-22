import { Square, Undo2 } from "lucide-react";
import {
  previewBacklogUndo,
  resumeBacklog,
  resumeBacklogUndo,
  sortingPost,
  stopBacklog,
  undoBacklog,
} from "../../api";
import type {
  Account,
  BacklogView,
  GmailView,
  ProtocolView,
  RunSummary,
  UndoDeadline,
} from "../../api";
import { describeError } from "../status-labels";
import { Action, Details, StatusBadge, useCopy } from "../components";
import type { PendingRunModal } from "./BacklogRunModal";

type SortingRun =
  | ProtocolView["journal"]["runs"][number]
  | GmailView["journal"]["runs"][number];

export function BacklogRunsSection({
  account,
  backlogKind,
  runs,
  summaries,
  deadlines,
  closable,
  legacyView,
  busy,
  onBusy,
  onError,
  onMessage,
  onReload,
  onStopActiveRun,
  onOpenUndoModal,
  onOpenCloseModal,
  onOpenResumeModal,
  onLegacyView,
}: {
  account: Account;
  backlogKind: "protocol" | "gmail";
  runs: SortingRun[];
  summaries: RunSummary[];
  deadlines: Map<string, UndoDeadline>;
  closable: Set<string>;
  legacyView: BacklogView | null;
  busy: boolean;
  onBusy: (value: boolean) => void;
  onError: (value: string) => void;
  onMessage: (value: string) => void;
  onReload: () => Promise<void>;
  onStopActiveRun: () => void;
  onOpenUndoModal: (runId: string) => void;
  onOpenCloseModal: (runId: string) => void;
  onOpenResumeModal: (modal: PendingRunModal) => void;
  onLegacyView: (view: BacklogView) => void;
}) {
  const t = useCopy();

  return (
    <>
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
                              onClick={onStopActiveRun}
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
                                onBusy(true);
                                onError("");
                                try {
                                  await sortingPost(backlogKind, account.id, {
                                    action: "reconcile",
                                    run: run.id,
                                  });
                                  onMessage(
                                    t(
                                      "Run reconciled against live server.",
                                      "Lauf mit dem Server abgeglichen.",
                                    ),
                                  );
                                  await onReload();
                                } catch (err) {
                                  onError(
                                    describeError(
                                      err,
                                      t(
                                        "Reconciliation failed. Check connectivity and retry.",
                                        "Abgleich fehlgeschlagen.",
                                      ),
                                    ),
                                  );
                                } finally {
                                  onBusy(false);
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
                              onClick={() =>
                                onOpenResumeModal({
                                  kind: "resume",
                                  runId: run.id,
                                  mode: "resume",
                                })
                              }
                            >
                              {t("Resume…", "Fortsetzen…")}
                            </Action>
                          )}
                          {canResumeUndo && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={() =>
                                onOpenResumeModal({
                                  kind: "resume",
                                  runId: run.id,
                                  mode: "resume_undo",
                                })
                              }
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
                                onClick={() => onOpenUndoModal(run.id)}
                              >
                                <Undo2 size={13} />
                                {t("Preview undo", "Rücknahme prüfen")}
                              </Action>
                            )}
                          {canClose && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={() => onOpenCloseModal(run.id)}
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
                                onLegacyView(
                                  await stopBacklog(account.id, run.id),
                                );
                              } catch (err) {
                                onError(
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
                                onLegacyView(
                                  await resumeBacklog(account.id, run.id),
                                );
                              } catch (err) {
                                onError(
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
                                onLegacyView(
                                  await undoBacklog(
                                    account.id,
                                    run.id,
                                    undo.receipt,
                                  ),
                                );
                              } catch (err) {
                                onError(
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
                                onLegacyView(
                                  await resumeBacklogUndo(account.id, run.id),
                                );
                              } catch (err) {
                                onError(
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
    </>
  );
}
