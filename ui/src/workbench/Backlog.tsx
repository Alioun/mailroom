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
  BacklogOptions,
  BacklogPreview,
  BacklogView,
  GmailPreview,
  GmailView,
  ProtocolPreview,
  ProtocolView,
} from "../api";
import { labelPreviewFor } from "../folder-provisioning-api";
import type { LabelPreview } from "../folder-provisioning-api";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  Notice,
  Stat,
  useCopy,
} from "./components";

export function Backlog({
  account,
  liveWrites,
  onGoSettings,
}: {
  account: Account;
  liveWrites: boolean;
  onGoSettings: () => void;
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

  const load = useCallback(async () => {
    setError("");
    const [p, g, l] = await Promise.allSettled([
      !isGmail ? openProtocolBacklog(account.id) : Promise.reject("gmail"),
      isGmail ? openGmailBacklog(account.id) : Promise.reject("mailbox"),
      openBacklog(account.id),
    ]);
    setProtocolView(p.status === "fulfilled" ? p.value : null);
    setGmailView(g.status === "fulfilled" ? g.value : null);
    setLegacyView(l.status === "fulfilled" ? l.value : null);
  }, [account.id, isGmail]);

  useEffect(() => {
    void load();
  }, [load]);

  const protocolReady = isGmail
    ? Boolean(gmailView?.installed)
    : Boolean(protocolView?.installed);
  const syntheticOnly = !protocolReady && Boolean(legacyView?.synthetic);

  async function runPreview() {
    setBusy(true);
    setError("");
    setMessage("");
    setConfirmed(false);
    setProtoPreview(null);
    setGmailPreview(null);
    setLegacyPreview(null);
    setLabelPreview(null);
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
        err instanceof Error
          ? err.message
          : t(
              "Could not preview backlog sorting. Ensure local rules are saved and valid, and the mailbox is reachable.",
              "Bestandsvorschau fehlgeschlagen. Prüfe, ob lokale Regeln gespeichert und gültig sind und das Postfach erreichbar ist.",
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
            "Gmail label sorting started. Refresh status to inspect progress and readback.",
            "Gmail-Label-Sortierung gestartet. Aktualisiere den Stand für Fortschritt und Rücklesung.",
          ),
        );
      } else if (protoPreview) {
        await confirmProtocolBacklog(account.id, protoPreview);
        setProtoPreview(null);
        setMessage(
          t(
            "IMAP MOVE backlog sorting started. Refresh status to inspect progress and undo eligibility.",
            "IMAP-MOVE-Bestandssortierung gestartet. Aktualisiere den Stand für Fortschritt und Rücknahme.",
          ),
        );
      } else if (legacyPreview) {
        setLegacyView(await startBacklog(account.id, legacyPreview));
        setLegacyPreview(null);
        setMessage(
          t("Synthetic backlog run started.", "Synthetischer Bestandslauf gestartet."),
        );
      }
      setConfirmed(false);
      setTimeout(() => void load(), 1200);
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : liveWrites
            ? t(
                "Backlog execution could not start. Refresh and try again.",
                "Bestandssortierung konnte nicht starten. Aktualisiere den Stand und versuche es erneut.",
              )
            : t(
                "Live provider writes are locked in Read-only mode. Enable Live writes in Settings first.",
                "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff in den Einstellungen.",
              ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function previewMissingLabels() {
    setBusy(true);
    setError("");
    try {
      setLabelPreview(await labelPreviewFor(account.id, options));
    } catch {
      setError(
        t(
          "Could not preview missing Gmail labels.",
          "Fehlende Gmail-Labels konnten nicht geprüft werden.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmMissingLabels() {
    if (!labelPreview) return;
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
      setMessage(
        t(
          "Gmail label creation started. Refresh and re-run backlog preview once complete.",
          "Erstellung der Gmail-Labels gestartet. Aktualisiere und prüfe den Bestand danach erneut.",
        ),
      );
      setTimeout(() => void load(), 1200);
    } catch {
      setError(
        t(
          "Could not create missing labels. Ensure Live writes are enabled in Settings.",
          "Fehlende Labels konnten nicht angelegt werden. Prüfe, ob Live-Schreibzugriff in den Einstellungen aktiviert ist.",
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

  const runs = isGmail
    ? (gmailView?.journal.runs ?? [])
    : (protocolView?.journal.runs ?? []);

  return (
    <div className="page-stack">
      {!liveWrites && !syntheticOnly && (
        <Notice>
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Previewing your backlog is always safe; executing moves, labels, or undo requires enabling Live writes in Settings.",
              "Der Nur-Lese-Modus ist aktiv. Die Vorschau ist immer sicher; zum Verschieben, Labeln oder Rückgängigmachen aktiviere Live-Schreibzugriff in den Einstellungen.",
            )}
          </span>
          <Action onClick={onGoSettings}>
            {t("Open Settings", "Einstellungen öffnen")}
          </Action>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          {message}
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {isGmail
                ? t("Sort existing Gmail backlog (labels)", "Gmail-Bestand sortieren (Labels)")
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
            label={t("Scope", "Bereich")}
            value={scope}
            onChange={(value) => {
              if (
                value === "inbox_unfiled" ||
                value === "inbox" ||
                value === "every_folder"
              )
                setScope(value);
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
          <CheckField checked={protectFlagged} onChange={setProtectFlagged}>
            {t("Keep flagged/starred messages", "Markierte Nachrichten schützen")}
          </CheckField>
          <CheckField
            checked={archiveUnmatched}
            onChange={setArchiveUnmatched}
          >
            <Archive size={14} />{" "}
            {t(
              "Archive old unmatched mail (Archive/<year>)",
              "Alte nicht zugeordnete Mails archivieren (Archive/<Jahr>)",
            )}
          </CheckField>
          <Action primary onClick={() => void runPreview()} disabled={busy}>
            {busy ? (
              <Busy>{t("Computing preview…", "Vorschau wird berechnet…")}</Busy>
            ) : (
              <>
                <FolderSync size={15} />
                {t("Preview backlog sort", "Bestandssortierung prüfen")}
              </>
            )}
          </Action>
        </div>

        {gmailPreview && gmailPreview.missing_labels.length > 0 && (
          <Notice>
            <span>
              {t(
                "Some target Gmail labels do not exist yet:",
                "Einige Ziel-Labels existieren in Gmail noch nicht:",
              )}{" "}
              {gmailPreview.missing_labels.map((m) => m.name).join(", ")}
            </span>
            <Action onClick={() => void previewMissingLabels()} disabled={busy}>
              {t("Preview missing labels", "Fehlende Labels prüfen")}
            </Action>
          </Notice>
        )}

        {labelPreview && (
          <div className="subpanel">
            <h3>{t("Create missing Gmail labels", "Fehlende Gmail-Labels anlegen")}</h3>
            <ul className="evidence-list">
              {labelPreview.operation.plan.entries.map((entry) => (
                <li key={entry.name.wire}>
                  <code>{entry.name.decoded}</code>
                </li>
              ))}
            </ul>
            <Action
              primary
              disabled={!liveWrites || busy}
              onClick={() => void confirmMissingLabels()}
            >
              {t("Create labels", "Labels anlegen")}
            </Action>
          </div>
        )}

        {activePlan && (
          <div className="subpanel">
            <h3>{t("Backlog sort preview", "Vorschau der Bestandssortierung")}</h3>
            <div className="stats compact">
              <Stat
                label={t("Planned moves/labels", "Geplante Änderungen")}
                value={activePlan.changes.length}
              />
              <Stat
                label={t("Unmatched (stay)", "Nicht zugeordnet")}
                value={activePlan.unmatched}
              />
              <Stat
                label={t("Held for review", "Zur Prüfung behalten")}
                value={activePlan.review}
              />
              <Stat
                label={t("Destinations", "Zielordner")}
                value={Object.keys(activePlan.groups).length}
              />
            </div>
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
                        <td className="numeric">{group.count}</td>
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
                  <Action
                    primary
                    disabled={
                      !confirmed || (!liveWrites && !syntheticOnly) || busy
                    }
                    onClick={() => void runConfirm()}
                  >
                    <Play size={15} />
                    {t("Start backlog sort", "Sortierung starten")}
                  </Action>
                  <Action
                    onClick={() => {
                      setProtoPreview(null);
                      setGmailPreview(null);
                      setLegacyPreview(null);
                    }}
                    disabled={busy}
                  >
                    {t("Cancel", "Abbrechen")}
                  </Action>
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
                  <th>{t("Actions", "Aktionen")}</th>
                </tr>
              </thead>
              <tbody>
                {runs.map((run) => {
                  const kind = isGmail ? "gmail" : "protocol";
                  return (
                    <tr key={run.id}>
                      <td>
                        <code>{run.id.slice(0, 12)}</code>
                      </td>
                      <td>
                        <span className="badge">{run.status}</span>
                      </td>
                      <td className="numeric">
                        {run.plan.filing.changes.length}
                      </td>
                      <td className="actions">
                        {run.status === "running" && (
                          <Action
                            disabled={busy}
                            onClick={async () => {
                              setBusy(true);
                              try {
                                await sortingPost(kind, account.id, {
                                  action: "cancel",
                                });
                                await load();
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            <Square size={14} />
                            {t("Stop", "Stoppen")}
                          </Action>
                        )}
                        {(run.status === "uncertain" ||
                          run.status === "stopped") && (
                          <Action
                            disabled={busy}
                            onClick={async () => {
                              setBusy(true);
                              try {
                                await sortingPost(kind, account.id, {
                                  action: "reconcile",
                                  run: run.id,
                                });
                                await load();
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            {t("Reconcile", "Abgleichen")}
                          </Action>
                        )}
                        {run.status === "stopped" && liveWrites && (
                          <Action
                            disabled={busy}
                            onClick={async () => {
                              setBusy(true);
                              try {
                                await sortingPost(kind, account.id, {
                                  action: "resume",
                                  run: run.id,
                                });
                                await load();
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            {t("Resume", "Fortsetzen")}
                          </Action>
                        )}
                        {(run.status === "complete" ||
                          run.status === "stopped") &&
                          !run.closed && (
                            <Action
                              disabled={busy || !liveWrites}
                              onClick={async () => {
                                setBusy(true);
                                setError("");
                                try {
                                  const undo = isGmail
                                    ? await previewGmailUndo(account.id, run.id)
                                    : await previewProtocolUndo(
                                        account.id,
                                        run.id,
                                      );
                                  if (isGmail) {
                                    await gmailAction(account.id, {
                                      action: "undo",
                                      run: run.id,
                                      receipt: undo.receipt,
                                      confirmed: true,
                                    });
                                  } else {
                                    await protocolAction(account.id, {
                                      action: "undo",
                                      run: run.id,
                                      receipt: undo.receipt,
                                      confirmed: true,
                                    });
                                  }
                                  setMessage(
                                    t(
                                      "Conditional undo started.",
                                      "Bedingte Rücknahme gestartet.",
                                    ),
                                  );
                                  setTimeout(() => void load(), 1200);
                                } catch {
                                  setError(
                                    t(
                                      "Undo could not start. Reconcile or inspect conflicts first.",
                                      "Rücknahme konnte nicht starten. Gleiche den Lauf zuerst ab.",
                                    ),
                                  );
                                } finally {
                                  setBusy(false);
                                }
                              }}
                            >
                              <Undo2 size={14} />
                              {t("Undo", "Rückgängig")}
                            </Action>
                          )}
                        {run.status === "complete" &&
                          !run.closed &&
                          liveWrites && (
                            <Action
                              disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                try {
                                  const close = await previewClose(
                                    kind,
                                    account.id,
                                    run.id,
                                  );
                                  await sortingPost(kind, account.id, {
                                    action: "close",
                                    run: run.id,
                                    receipt: close.receipt,
                                    confirmed: true,
                                  });
                                  await load();
                                } catch {
                                  setError(
                                    t(
                                      "Run cannot be closed yet.",
                                      "Lauf kann noch nicht abgeschlossen werden.",
                                    ),
                                  );
                                } finally {
                                  setBusy(false);
                                }
                              }}
                            >
                              {t("Close run", "Abschließen")}
                            </Action>
                          )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
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
                      <span className="badge">{run.status}</span>
                    </td>
                    <td className="actions">
                      {run.status === "running" && (
                        <Action
                          onClick={async () =>
                            setLegacyView(await stopBacklog(account.id, run.id))
                          }
                        >
                          {t("Stop", "Stoppen")}
                        </Action>
                      )}
                      {run.status === "stopped" && (
                        <Action
                          onClick={async () =>
                            setLegacyView(
                              await resumeBacklog(account.id, run.id),
                            )
                          }
                        >
                          {t("Resume", "Fortsetzen")}
                        </Action>
                      )}
                      {(run.status === "completed" ||
                        run.status === "stopped") && (
                        <Action
                          onClick={async () => {
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
                          }}
                        >
                          {t("Undo", "Rückgängig")}
                        </Action>
                      )}
                      {run.undo_requested && run.status === "uncertain" && (
                        <Action
                          onClick={async () =>
                            setLegacyView(
                              await resumeBacklogUndo(account.id, run.id),
                            )
                          }
                        >
                          {t("Resume undo", "Rücknahme fortsetzen")}
                        </Action>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Details>
      )}

      {!protocolReady && !syntheticOnly && (
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
    </div>
  );
}
