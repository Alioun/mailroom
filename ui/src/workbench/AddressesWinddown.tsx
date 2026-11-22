import { useCallback, useContext, useEffect, useMemo, useState } from "react";
import {
  BookUser,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  Download,
  ExternalLink,
  Forward,
  Plus,
  RefreshCw,
  Search,
  ShieldAlert,
  TrendingDown,
} from "lucide-react";
import type { Account } from "../api";
import { gmailRoutePlan, gmailRouteText } from "../gmail-route";
import { confirmLedger, openLedger, reviewLedger } from "../ledger-api";
import type { LedgerEntry, LedgerReview, LedgerView } from "../ledger-api";
import {
  confirmWinddown,
  exportWinddown,
  openWinddown,
  previewWinddownExport,
  reviewWinddown,
} from "../winddown-api";
import type {
  WinddownExport,
  WinddownReview,
  WinddownView,
} from "../winddown-api";
import {
  describeError,
  winddownBlockerText,
  winddownGroupText,
} from "./status-labels";
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
  SubTabs,
  useCopy,
} from "./components";
import type { SubTabItem } from "./components";

type Tab = "ledger" | "forwarding" | "winddown";

export function AddressesWinddown({ account }: { account: Account }) {
  const t = useCopy();
  const [tab, setTab] = useState<Tab>("ledger");

  const items: SubTabItem<Tab>[] = [
    {
      value: "ledger",
      label: t("Address ledger", "Adressverzeichnis"),
      icon: <BookUser size={15} />,
      content: <LedgerPanel account={account} />,
      keepMounted: true,
    },
    {
      value: "forwarding",
      label: t("Gmail forwarding", "Gmail-Weiterleitung"),
      icon: <Forward size={15} />,
      content: <GmailForwardingPanel />,
      keepMounted: true,
    },
    {
      value: "winddown",
      label: t("Gmail wind-down", "Gmail-Abbau"),
      icon: <TrendingDown size={15} />,
      content: <WinddownPanel account={account} />,
      keepMounted: true,
    },
  ];

  return (
    <SubTabs
      value={tab}
      onChange={setTab}
      label={t("Addresses and wind-down sections", "Adress- und Abbaubereiche")}
      items={items}
    />
  );
}

function LedgerPanel({ account }: { account: Account }) {
  const t = useCopy();
  const [view, setView] = useState<LedgerView | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [entries, setEntries] = useState<LedgerEntry[]>([]);
  const [newAddress, setNewAddress] = useState("");
  const [newService, setNewService] = useState("");
  const [newFolder, setNewFolder] = useState("Privat/Verträge");
  const [newCategory, setNewCategory] =
    useState<LedgerEntry["category"]>("contracts");
  const [newInvoice, setNewInvoice] = useState(false);
  const [formError, setFormError] = useState("");
  const [review, setReview] = useState<LedgerReview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    setError("");
    try {
      const next = await openLedger(account.id);
      setView(next);
      setEnabled(next.snapshot.document.enabled);
      setEntries(next.snapshot.document.entries);
    } catch {
      setError(
        t(
          "Save local rules first before managing the private address ledger.",
          "Speichere zuerst lokale Regeln, um das private Adressverzeichnis zu öffnen.",
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  function addEntry() {
    setFormError("");
    const addr = newAddress.trim().toLowerCase();
    const svc = newService.trim();
    const folder = newFolder.trim();
    if (!addr || !addr.includes("@") || addr.startsWith("@") || addr.endsWith("@")) {
      setFormError(
        t(
          "Enter a valid service email address (e.g. shop@example.de).",
          "Gib eine gültige Dienst-E-Mail-Adresse ein (z. B. shop@example.de).",
        ),
      );
      return;
    }
    if (!svc) {
      setFormError(
        t("Enter a service name.", "Gib einen Dienstnamen ein."),
      );
      return;
    }
    if (!folder) {
      setFormError(
        t("Enter a target folder path.", "Gib einen Zielordner ein."),
      );
      return;
    }
    if (entries.some((e) => e.address.toLowerCase() === addr)) {
      setFormError(
        t(
          "That address is already in the ledger.",
          "Diese Adresse ist bereits im Verzeichnis eingetragen.",
        ),
      );
      return;
    }
    const today = new Date().toISOString().slice(0, 10);
    setEntries((prev) => [
      ...prev,
      {
        address: addr,
        service: svc,
        folder,
        first_seen: today,
        status: "active",
        note: "",
        category: newCategory,
        invoice: newInvoice,
        association: null,
      },
    ]);
    setNewAddress("");
    setNewService("");
    setReview(null);
  }

  async function runPreview() {
    if (!view) return;
    setBusy(true);
    setError("");
    setConfirmed(false);
    try {
      setReview(
        await reviewLedger(account.id, {
          content_version: view.snapshot.content_version,
          revision: view.revision,
          enabled,
          entries,
        }),
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Ledger batch rejected. Check addresses, folder paths, and reload if needed.",
            "Adressverzeichnis-Änderung abgelehnt. Prüfe Adressen und Ordnerpfade und lade bei Bedarf neu.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runConfirm() {
    if (!review || !confirmed) return;
    setBusy(true);
    setError("");
    try {
      const next = await confirmLedger(account.id, review);
      setView(next);
      setEnabled(next.snapshot.document.enabled);
      setEntries(next.snapshot.document.entries);
      setReview(null);
      setConfirmed(false);
      setMessage(
        t(
          "Private address ledger saved and projected into local rules.",
          "Privates Adressverzeichnis gespeichert und in lokale Regeln projiziert.",
        ),
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not save address ledger. Reload and review again.",
            "Adressverzeichnis konnte nicht gespeichert werden. Lade neu und prüfe erneut.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

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
            <h2>
              {t(
                "Private address policy & ledger",
                "Adressrichtlinie & Verzeichnis",
              )}
            </h2>
            <p className="muted">
              {t(
                "Track one dedicated email address per service, detect leaks when unexpected senders write to a service address, and project routing rules automatically.",
                "Verwalte eine eigene E-Mail-Adresse pro Dienst, erkenne Leaks bei unerwarteten Absendern und erzeuge passende Sortierregeln automatisch.",
              )}
            </p>
          </div>
          <Action onClick={() => void load()} disabled={busy}>
            <RefreshCw size={15} />
            {t("Refresh", "Aktualisieren")}
          </Action>
        </div>

        {view ? (
          <>
            <div className="stats compact">
              <Stat
                label={t("Recorded addresses", "Erfasste Adressen")}
                value={entries.length.toLocaleString()}
              />
              <Stat
                label={t("Discovery hints", "Entdeckungshinweise")}
                value={view.proposals.length.toLocaleString()}
              />
              <Stat
                label={t("Leak alerts", "Leak-Hinweise")}
                value={view.leaks.length.toLocaleString()}
              />
            </div>

            <CheckField
              checked={enabled}
              onChange={(val) => {
                setEnabled(val);
                setReview(null);
              }}
            >
              {t(
                "Project ledger entries into local routing rules",
                "Verzeichniseinträge in lokale Sortierregeln übernehmen",
              )}
            </CheckField>

            {view.proposals.length > 0 && (
              <Details
                title={`${t("Observed address discovery hints", "Entdeckte Dienstadressen")} · ${view.proposals.length}`}
              >
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("Address", "Adresse")}</th>
                        <th>{t("Observed senders", "Beobachtete Absender")}</th>
                        <th>{t("Action", "Aktion")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {view.proposals.slice(0, 20).map((p, i) => (
                        <tr key={`${p.address}-${i}`}>
                          <td>
                            <code>{p.address}</code>
                          </td>
                          <td>
                            {p.service ?? p.role} ({p.evidence.length})
                          </td>
                          <td>
                            <div className="cell-actions">
                              <Action
                                size="compact"
                                onClick={() => {
                                  setNewAddress(p.address);
                                  setNewService(
                                    p.service ??
                                      p.address.split("@")[0] ??
                                      "Service",
                                  );
                                  setFormError("");
                                }}
                              >
                                {t("Pre-fill", "Übernehmen")}
                              </Action>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </Details>
            )}

            <div className="settings-form">
              <label className="field">
                {t("Service email address", "Dienst-E-Mail-Adresse")}
                <input
                  value={newAddress}
                  onChange={(e) => {
                    setNewAddress(e.target.value);
                    setFormError("");
                  }}
                  placeholder="shop@example.de"
                />
              </label>
              <label className="field">
                {t("Service name", "Dienstname")}
                <input
                  value={newService}
                  onChange={(e) => {
                    setNewService(e.target.value);
                    setFormError("");
                  }}
                  placeholder="Example Shop"
                />
              </label>
              <label className="field">
                {t("Target folder", "Zielordner")}
                <input
                  value={newFolder}
                  onChange={(e) => {
                    setNewFolder(e.target.value);
                    setFormError("");
                  }}
                />
              </label>
              <Choice
                fieldLabel={t("Category", "Kategorie")}
                label={t("Category", "Kategorie")}
                value={newCategory}
                onChange={(v) => setNewCategory(v as LedgerEntry["category"])}
                options={[
                  { value: "money", label: t("Money", "Finanzen") },
                  { value: "contracts", label: t("Contracts", "Verträge") },
                  { value: "orders", label: t("Orders", "Bestellungen") },
                  { value: "travel", label: t("Travel", "Reisen") },
                  { value: "social", label: t("Social", "Soziale Netzwerke") },
                  { value: "newsletters", label: "Newsletter" },
                ]}
              />
              <CheckField checked={newInvoice} onChange={setNewInvoice}>
                {t("Invoice address", "Rechnungsadresse")}
              </CheckField>
              {formError && <Notice error>{formError}</Notice>}
              <Action onClick={addEntry}>
                <Plus size={15} />
                {t("Add entry", "Eintrag hinzufügen")}
              </Action>
            </div>

            {entries.length > 0 && (
              <>
                <p className="help" style={{ marginTop: 12 }}>
                  {t(
                    "Note: Retiring an address marks it retired in your local ledger only — it does not disable aliases at your mail provider.",
                    "Hinweis: Stilllegen markiert eine Adresse nur lokal im Verzeichnis — Aliase beim Anbieter werden nicht automatisch deaktiviert.",
                  )}
                </p>
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("Address", "Adresse")}</th>
                        <th>{t("Service", "Dienst")}</th>
                        <th>{t("Folder", "Ordner")}</th>
                        <th>{t("Status", "Status")}</th>
                        <th>{t("Actions", "Aktionen")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {entries.map((entry, idx) => (
                        <tr key={`${entry.address}-${idx}`}>
                          <td>
                            <code>{entry.address}</code>
                          </td>
                          <td>{entry.service}</td>
                          <td>{entry.folder}</td>
                          <td>
                            <StatusBadge status={entry.status} />
                          </td>
                          <td>
                            <div className="cell-actions">
                              <Action
                                size="compact"
                                onClick={() => {
                                  setEntries((prev) =>
                                    prev.map((e, i) =>
                                      i === idx
                                        ? {
                                            ...e,
                                            status:
                                              e.status === "active"
                                                ? "retired"
                                                : "active",
                                          }
                                        : e,
                                    ),
                                  );
                                  setReview(null);
                                }}
                              >
                                {entry.status === "active"
                                  ? t("Retire", "Stilllegen")
                                  : t("Reactivate", "Reaktivieren")}
                              </Action>
                              <Action
                                size="compact"
                                tone="danger-outline"
                                onClick={() => {
                                  setEntries((prev) =>
                                    prev.filter((_, i) => i !== idx),
                                  );
                                  setReview(null);
                                }}
                              >
                                {t("Remove", "Entfernen")}
                              </Action>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}

            <div className="actions" style={{ marginTop: 16 }}>
              <Action primary onClick={() => void runPreview()} disabled={busy}>
                {busy ? (
                  <Busy>{t("Reviewing…", "Wird geprüft…")}</Busy>
                ) : (
                  t("Preview ledger changes", "Änderungen prüfen")
                )}
              </Action>
            </div>

            {view.leaks.length > 0 && (
              <Details
                title={`${t("Observed address leaks", "Beobachtete Adress-Leaks")} · ${view.leaks.length}`}
              >
                <ul className="evidence-list">
                  {view.leaks.map((leak, i) => (
                    <li key={i}>
                      <code>{leak.address}</code> ({leak.service}) ·{" "}
                      {leak.observation.sender_domain ?? "?"} · {leak.signal}
                    </li>
                  ))}
                </ul>
              </Details>
            )}
          </>
        ) : (
          <Empty
            title={t(
              "Address ledger waiting for local rules",
              "Adressverzeichnis wartet auf lokale Regeln",
            )}
          />
        )}
      </section>

      <Modal
        open={review !== null}
        onOpenChange={(open) => {
          if (!busy && !open) setReview(null);
        }}
        title={t(
          "Confirm address ledger changes",
          "Änderungen am Adressverzeichnis bestätigen",
        )}
        wide
      >
        {review && (
          <div className="page-stack">
            <p className="muted">{review.effects}</p>
            <div className="stats compact">
              <Stat
                label={t("Would file", "Würden abgelegt")}
                value={review.proof.proof.counts.filed.toLocaleString()}
              />
              <Stat
                label={t("Kept in Inbox", "Im Posteingang")}
                value={review.proof.proof.counts.inbox.toLocaleString()}
              />
              <Stat
                label={t("Validation", "Prüfung")}
                value={review.proof.proof.gate}
              />
            </div>
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "Save this exact private address ledger locally and update local rule projection.",
                "Dieses genaue private Adressverzeichnis lokal speichern und die lokale Regelprojektion aktualisieren.",
              )}
            </CheckField>
            <div className="dialog-actions">
              <Action onClick={() => setReview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                primary
                disabled={!confirmed || busy}
                onClick={() => void runConfirm()}
              >
                {busy ? (
                  <Busy>{t("Saving…", "Wird gespeichert…")}</Busy>
                ) : (
                  t("Save ledger", "Verzeichnis speichern")
                )}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}

function GmailForwardingPanel() {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const copy = gmailRoutePlan[language];

  function downloadPlan() {
    const text = gmailRouteText(language);
    const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `gmail-forwarding-plan-${language}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  return (
    <section className="panel">
      <div className="section-heading">
        <div>
          <h2>{copy.title}</h2>
          <p className="muted">{copy.summary}</p>
        </div>
        <Action primary onClick={downloadPlan}>
          <Download size={15} />
          {t("Download plan", "Plan herunterladen")}
        </Action>
      </div>
      <Notice tone="warning">
        <ShieldAlert size={16} />
        <span>{copy.boundary}</span>
      </Notice>
      <p className="help">{copy.private}</p>
      <ol className="guide-steps">
        {gmailRoutePlan.stages.map((stage, i) => (
          <li key={i}>
            <div>
              <strong>{stage[language].title}</strong>
              <p className="muted">{stage[language].body}</p>
            </div>
          </li>
        ))}
      </ol>
      <p className="help">{copy.next}</p>
      {gmailRoutePlan.sources.length > 0 && (
        <Details title={t("Official references", "Offizielle Quellen")}>
          <ul className="evidence-list">
            {gmailRoutePlan.sources.map((src) => (
              <li key={src.url}>
                <a href={src.url} target="_blank" rel="noreferrer">
                  {src.label} <ExternalLink size={12} />
                </a>
              </li>
            ))}
          </ul>
        </Details>
      )}
    </section>
  );
}

function WinddownPanel({ account }: { account: Account }) {
  const t = useCopy();
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
              className="actions"
              style={{ alignItems: "center", marginBottom: 12 }}
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
                  <ul className="evidence-list" style={{ margin: "6px 0 0" }}>
                    {view.report.blockers.map((b) => (
                      <li key={b}>{winddownBlockerText(b, t)}</li>
                    ))}
                  </ul>
                </div>
              </Notice>
            )}

            {trackedTotal > 0 && (
              <div className="readiness-bar">
                <div className="actions" style={{ justifyContent: "space-between" }}>
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
                value={waitingCount.toLocaleString()}
              />
              <Stat
                label={t("Moved", "Umgezogen")}
                value={movedCount.toLocaleString()}
              />
              <Stat
                label={t("Quiet / dismissed", "Ruhig / ausgeblendet")}
                value={(
                  (view.report.counts.quiet ?? 0) +
                  (view.report.counts.dismissed ?? 0)
                ).toLocaleString()}
              />
              {catalog && (
                <Stat
                  label={t("Curated sender catalog", "Kuratierter Katalog")}
                  value={`${catalog.rows} (${catalog.seed_complete ? t("Complete", "Vollständig") : t("Seed", "Entwurf")})`}
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
                            {row.recent.toLocaleString()}
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
