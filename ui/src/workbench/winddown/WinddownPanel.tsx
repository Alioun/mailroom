import { useCallback, useContext, useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  Download,
  ExternalLink,
  RefreshCw,
  Search,
} from "lucide-react";
import type { Account } from "../../api";
import {
  confirmWinddown,
  exportWinddown,
  openWinddown,
  previewWinddownExport,
  reviewWinddown,
} from "../../winddown-api";
import type {
  WinddownExport,
  WinddownReview,
  WinddownView,
} from "../../winddown-api";
import { formatNumber } from "../format";
import {
  describeError,
  winddownBlockerText,
  winddownGroupText,
} from "../status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  LanguageContext,
  Modal,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "../components";

export function WinddownPanel({ account }: { account: Account }) {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const [view, setView] = useState<WinddownView | null>(null);
  const [comparisons, setComparisons] = useState<Record<string, boolean>>({});
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const [review, setReview] = useState<WinddownReview | null>(null);
  const [exportPreview, setExportPreview] = useState<WinddownExport | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    setError("");
    try {
      const next = await openWinddown(account.id, Math.floor(Date.now() / 1000));
      setView(next);
      const selected: Record<string, boolean> = {};
      for (const acc of next.available) {
        selected[acc.account] = Boolean(
          next.report.settings.comparisons[acc.account],
        );
      }
      setComparisons(selected);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not load wind-down report. Scan your mailbox and save local rules first.",
            "Abbau-Bericht konnte nicht geladen werden. Erfasse zuerst das Postfach und speichere lokale Regeln.",
          ),
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  function buildComparisonMap() {
    const map: Record<string, string[]> = {};
    for (const acc of view?.available ?? []) {
      if (comparisons[acc.account]) {
        map[acc.account] = acc.owned_addresses;
      }
    }
    return map;
  }

  async function previewSettingsChange(nextWindingDown: boolean) {
    if (!view) return;
    setBusy(true);
    setError("");
    try {
      const nextSettings = {
        winding_down: nextWindingDown,
        comparisons: buildComparisonMap(),
      };
      const rev = await reviewWinddown(account.id, view.report, {
        kind: "settings",
        settings: nextSettings,
      });
      setReview(rev);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview wind-down setting change.",
            "Einstellungsänderung konnte nicht geprüft werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function toggleDismiss(rowId: string, dismissed: boolean) {
    if (!view) return;
    setBusy(true);
    setError("");
    try {
      const rev = await reviewWinddown(account.id, view.report, {
        kind: "dismiss",
        rows: [rowId],
        dismissed,
      });
      await confirmWinddown(account.id, rev);
      await load();
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not update dismissal.",
            "Ausblendung konnte nicht gespeichert werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runExport(disclosure: "local_file" | "agent_summary") {
    if (!view) return;
    setBusy(true);
    setError("");
    try {
      const prev = await previewWinddownExport(
        account.id,
        view.report,
        disclosure,
      );
      setExportPreview(prev);
    } catch (err) {
      setError(
        describeError(
          err,
          t("Could not preview report export.", "Exportvorschau fehlgeschlagen."),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function downloadExport() {
    if (!view || !exportPreview) return;
    setBusy(true);
    setError("");
    try {
      const done = await exportWinddown(account.id, view.report, exportPreview);
      const blob = new Blob([done.text], { type: "text/markdown;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = done.filename;
      a.click();
      URL.revokeObjectURL(url);
      setExportPreview(null);
      setMessage(t("Wind-down report exported.", "Abbau-Bericht exportiert."));
    } catch (err) {
      setError(
        describeError(
          err,
          t("Could not export wind-down report.", "Export fehlgeschlagen."),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const catalog = view?.report.sender_catalog;
  const filteredRows = useMemo(() => {
    const rows = view?.report.rows ?? [];
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) =>
      `${r.sender ?? ""} ${r.domain ?? ""} ${r.group} ${r.status}`
        .toLowerCase()
        .includes(q),
    );
  }, [view, query]);

  const perPage = 25;
  const pages = Math.max(1, Math.ceil(filteredRows.length / perPage));
  const currentPage = Math.min(page, pages - 1);
  const visibleRows = filteredRows.slice(
    currentPage * perPage,
    (currentPage + 1) * perPage,
  );

  const waitingCount = view?.report.counts.waiting ?? 0;
  const movedCount = view?.report.counts.moved ?? 0;
  const trackedTotal = waitingCount + movedCount;
  const movedPct =
    trackedTotal > 0 ? Math.round((movedCount / trackedTotal) * 100) : 0;

  return (
    <div className="page-stack">
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
            <h2>{t("Account wind-down report", "Konto-Abbau-Bericht")}</h2>
            <p className="muted">
              {t(
                "Track which services and people still write to this account and verify when they start arriving at your destination account.",
                "Sieh, welche Dienste und Kontakte noch an dieses Konto schreiben, und prüfe, wann sie im Zielkonto ankommen.",
              )}
            </p>
          </div>
          <div className="actions">
            <Action onClick={() => void load()} disabled={busy}>
              <RefreshCw size={15} />
              {t("Refresh", "Aktualisieren")}
            </Action>
            {view && (
              <Action
                primary
                onClick={() =>
                  void previewSettingsChange(!view.report.settings.winding_down)
                }
                disabled={busy}
              >
                {view.report.settings.winding_down
                  ? t(
                      "Disable wind-down tracking",
                      "Abbau-Tracking deaktivieren",
                    )
                  : t(
                      "Mark account as winding down",
                      "Konto als abzubauen markieren",
                    )}
              </Action>
            )}
          </div>
        </div>

        {view && (
          <>
            <div
              className="actions align-center mb-sm"
            >
              <strong>{t("Readiness verdict", "Status")}:</strong>
              <StatusBadge status={view.report.verdict} />
            </div>

            {view.report.blockers.length > 0 && (
              <Notice tone="warning">
                <div>
                  <strong>
                    {t(
                      "Blockers preventing ready verdict:",
                      "Gründe, warum der Abbau noch nicht abgeschlossen ist:",
                    )}
                  </strong>
                  <ul className="evidence-list list-compact">
                    {view.report.blockers.map((b) => (
                      <li key={b}>{winddownBlockerText(b, t)}</li>
                    ))}
                  </ul>
                </div>
              </Notice>
            )}

            {trackedTotal > 0 && (
              <div className="readiness-bar">
                <div className="actions actions-between">
                  <span className="help">
                    {t(
                      `${movedCount} of ${trackedTotal} active senders moved (${movedPct}%)`,
                      `${movedCount} von ${trackedTotal} aktiven Absendern umgezogen (${movedPct} %)`,
                    )}
                  </span>
                </div>
                <div className="readiness-track">
                  <div
                    className="readiness-fill"
                    style={{ width: `${movedPct}%` }}
                  />
                </div>
              </div>
            )}

            <div className="stats compact">
              <Stat
                label={t("Still writing here", "Schreiben noch hier")}
                value={formatNumber(waitingCount, language)}
              />
              <Stat
                label={t("Moved", "Umgezogen")}
                value={formatNumber(movedCount, language)}
              />
              <Stat
                label={t("Quiet / dismissed", "Ruhig / ausgeblendet")}
                value={formatNumber(
                  (view.report.counts.quiet ?? 0) +
                    (view.report.counts.dismissed ?? 0),
                  language,
                )}
              />
              {catalog && (
                <Stat
                  label={t("Curated sender catalog", "Kuratierter Katalog")}
                  value={`${formatNumber(catalog.rows, language)} (${catalog.seed_complete ? t("Complete", "Vollständig") : t("Seed", "Entwurf")})`}
                />
              )}
            </div>

            {view.available.length > 0 && (
              <div className="subpanel">
                <h3>
                  {t(
                    "Destination comparison accounts",
                    "Zielkonten für den Abbau-Vergleich",
                  )}
                </h3>
                <p className="help">
                  {t(
                    "Select the account(s) you are migrating to (for example, your mailbox.org account) so Mailroom can detect when senders move over.",
                    "Wähle die Konten, zu denen du umziehst (z. B. dein mailbox.org-Konto), damit Mailroom erkennt, wann Absender umgezogen sind.",
                  )}
                </p>
                <div className="domain-options">
                  {view.available.map((acc) => (
                    <CheckField
                      key={acc.account}
                      checked={Boolean(comparisons[acc.account])}
                      onChange={(checked) =>
                        setComparisons((prev) => ({
                          ...prev,
                          [acc.account]: checked,
                        }))
                      }
                    >
                      {acc.login}
                    </CheckField>
                  ))}
                </div>
                <div className="actions">
                  <Action
                    disabled={busy}
                    onClick={() =>
                      void previewSettingsChange(
                        view.report.settings.winding_down,
                      )
                    }
                  >
                    {t(
                      "Save comparison accounts",
                      "Vergleichskonten speichern",
                    )}
                  </Action>
                </div>
              </div>
            )}

            <div className="toolbar">
              <label className="search">
                <Search size={16} />
                <input
                  type="search"
                  aria-label={t("Search wind-down senders", "Absender suchen")}
                  placeholder={t(
                    "Filter sender, domain, or status…",
                    "Absender, Domain oder Status filtern…",
                  )}
                  value={query}
                  onChange={(e) => {
                    setQuery(e.target.value);
                    setPage(0);
                  }}
                />
              </label>
              <Action
                size="compact"
                onClick={() => void runExport("local_file")}
                disabled={busy}
              >
                <Download size={14} />
                {t("Export full report", "Vollständigen Bericht exportieren")}
              </Action>
              <Action
                size="compact"
                onClick={() => void runExport("agent_summary")}
                disabled={busy}
              >
                <Download size={14} />
                {t(
                  "Export masked summary",
                  "Maskierte Zusammenfassung",
                )}
              </Action>
            </div>

            {exportPreview && (
              <div className="subpanel">
                <pre tabIndex={0}>{exportPreview.text.slice(0, 1500)}</pre>
                <div className="actions">
                  <Action onClick={() => setExportPreview(null)}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                  <Action
                    primary
                    onClick={() => void downloadExport()}
                    disabled={busy}
                  >
                    <Download size={15} />
                    {t("Download", "Herunterladen")} ({exportPreview.filename})
                  </Action>
                </div>
              </div>
            )}

            {filteredRows.length > 0 ? (
              <>
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("Sender / Domain", "Absender / Domain")}</th>
                        <th>{t("Group", "Gruppe")}</th>
                        <th>{t("Status", "Status")}</th>
                        <th className="numeric">{t("Recent", "Aktuell")}</th>
                        <th>{t("Actions", "Aktionen")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {visibleRows.map((row) => (
                        <tr key={row.id}>
                          <td>
                            <strong>
                              {row.sender ?? row.domain ?? row.id}
                            </strong>
                          </td>
                          <td>{winddownGroupText(row.group, t)}</td>
                          <td>
                            <StatusBadge status={row.status} />
                          </td>
                          <td className="numeric">
                            {formatNumber(row.recent, language)}
                          </td>
                          <td>
                            <div className="cell-actions">
                              {row.settings_link && (
                                <a
                                  className="button compact"
                                  href={row.settings_link}
                                  target="_blank"
                                  rel="noreferrer"
                                >
                                  <ExternalLink size={13} />
                                  {t("Account settings", "Kontoeinstellungen")}
                                </a>
                              )}
                              <Action
                                size="compact"
                                disabled={busy}
                                onClick={() =>
                                  void toggleDismiss(
                                    row.id,
                                    row.status !== "dismissed",
                                  )
                                }
                              >
                                {row.status === "dismissed"
                                  ? t("Restore", "Wiederherstellen")
                                  : t("Dismiss", "Ausblenden")}
                              </Action>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {filteredRows.length > perPage && (
                  <div className="pagination">
                    <span className="muted">
                      {currentPage * perPage + 1}–
                      {Math.min(
                        (currentPage + 1) * perPage,
                        filteredRows.length,
                      )}{" "}
                      / {filteredRows.length}
                    </span>
                    <div>
                      <Action
                        size="compact"
                        aria-label={t("Previous page", "Vorherige Seite")}
                        disabled={currentPage === 0}
                        onClick={() => setPage(currentPage - 1)}
                      >
                        <ChevronLeft size={16} />
                      </Action>
                      <span>
                        {currentPage + 1} / {pages}
                      </span>
                      <Action
                        size="compact"
                        aria-label={t("Next page", "Nächste Seite")}
                        disabled={currentPage + 1 >= pages}
                        onClick={() => setPage(currentPage + 1)}
                      >
                        <ChevronRight size={16} />
                      </Action>
                    </div>
                  </div>
                )}
              </>
            ) : (
              <Empty
                title={t(
                  "No wind-down senders listed",
                  "Keine Abbau-Absender gefunden",
                )}
              />
            )}
          </>
        )}
      </section>

      <Modal
        open={review !== null}
        onOpenChange={(open) => {
          if (!busy && !open) setReview(null);
        }}
        title={t(
          "Confirm wind-down settings",
          "Abbau-Einstellungen bestätigen",
        )}
      >
        {review && (
          <div className="page-stack">
            <p className="muted">{review.consequences}</p>
            <div className="dialog-actions">
              <Action onClick={() => setReview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                primary
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setError("");
                  try {
                    await confirmWinddown(account.id, review);
                    setReview(null);
                    setMessage(
                      t(
                        "Wind-down settings updated.",
                        "Abbau-Einstellungen gespeichert.",
                      ),
                    );
                    await load();
                  } catch (err) {
                    setError(
                      describeError(
                        err,
                        t(
                          "Could not save wind-down settings.",
                          "Einstellungen konnten nicht gespeichert werden.",
                        ),
                      ),
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {t("Confirm change", "Änderung bestätigen")}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
