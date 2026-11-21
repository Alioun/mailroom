import { useCallback, useContext, useEffect, useState } from "react";
import {
  BookUser,
  CheckCircle2,
  Download,
  ExternalLink,
  Forward,
  Plus,
  RefreshCw,
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
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  LanguageContext,
  Notice,
  Stat,
  useCopy,
} from "./components";

type Tab = "ledger" | "forwarding" | "winddown";

export function AddressesWinddown({ account }: { account: Account }) {
  const t = useCopy();
  const [tab, setTab] = useState<Tab>("ledger");

  return (
    <div className="page-stack">
      <div className="subtabs" role="tablist" aria-label={t("Sections", "Bereiche")}>
        <button
          role="tab"
          aria-selected={tab === "ledger"}
          className={`subtab ${tab === "ledger" ? "active" : ""}`}
          onClick={() => setTab("ledger")}
        >
          <BookUser size={15} />
          {t("Address ledger", "Adressverzeichnis")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "forwarding"}
          className={`subtab ${tab === "forwarding" ? "active" : ""}`}
          onClick={() => setTab("forwarding")}
        >
          <Forward size={15} />
          {t("Gmail forwarding", "Gmail-Weiterleitung")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "winddown"}
          className={`subtab ${tab === "winddown" ? "active" : ""}`}
          onClick={() => setTab("winddown")}
        >
          <TrendingDown size={15} />
          {t("Gmail wind-down", "Gmail-Abbau")}
        </button>
      </div>
      {tab === "ledger" && <LedgerPanel account={account} />}
      {tab === "forwarding" && <GmailForwardingPanel />}
      {tab === "winddown" && <WinddownPanel account={account} />}
    </div>
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
    const addr = newAddress.trim().toLowerCase();
    const svc = newService.trim();
    const folder = newFolder.trim();
    if (!addr.includes("@") || !svc || !folder) return;
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
    } catch {
      setError(
        t(
          "Ledger batch rejected. Check addresses, folder paths, and reload if needed.",
          "Adressverzeichnis-Änderung abgelehnt. Prüfe Adressen und Ordnerpfade und lade bei Bedarf neu.",
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
    } catch {
      setError(
        t(
          "Could not save address ledger. Reload and review again.",
          "Adressverzeichnis konnte nicht gespeichert werden. Lade neu und prüfe erneut.",
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
          {message}
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Private address policy & ledger", "Adressrichtlinie & Verzeichnis")}</h2>
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
                value={entries.length}
              />
              <Stat
                label={t("Discovery hints", "Entdeckungshinweise")}
                value={view.proposals.length}
              />
              <Stat
                label={t("Leak alerts", "Leak-Hinweise")}
                value={view.leaks.length}
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

            <div className="settings-form">
              <label className="field">
                {t("Service email address", "Dienst-E-Mail-Adresse")}
                <input
                  value={newAddress}
                  onChange={(e) => setNewAddress(e.target.value)}
                  placeholder="shop@example.de"
                />
              </label>
              <label className="field">
                {t("Service name", "Dienstname")}
                <input
                  value={newService}
                  onChange={(e) => setNewService(e.target.value)}
                  placeholder="Example Shop"
                />
              </label>
              <label className="field">
                {t("Target folder", "Zielordner")}
                <input
                  value={newFolder}
                  onChange={(e) => setNewFolder(e.target.value)}
                />
              </label>
              <Choice
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
              <Action onClick={addEntry}>
                <Plus size={15} />
                {t("Add entry", "Eintrag hinzufügen")}
              </Action>
            </div>

            {entries.length > 0 && (
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
                          <span className="badge">{entry.status}</span>
                        </td>
                        <td className="actions">
                          <Action
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
                            onClick={() => {
                              setEntries((prev) =>
                                prev.filter((_, i) => i !== idx),
                              );
                              setReview(null);
                            }}
                          >
                            {t("Remove", "Entfernen")}
                          </Action>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            <div className="actions">
              <Action primary onClick={() => void runPreview()} disabled={busy}>
                {busy ? (
                  <Busy>{t("Reviewing…", "Wird geprüft…")}</Busy>
                ) : (
                  t("Preview ledger changes", "Änderungen prüfen")
                )}
              </Action>
            </div>

            {review && (
              <div className="subpanel">
                <h3>{t("Ledger projection review", "Vorschau der Verzeichnis-Projektion")}</h3>
                <p className="muted">{review.effects}</p>
                <div className="stats compact">
                  <Stat
                    label={t("Would file", "Würden abgelegt")}
                    value={review.proof.proof.counts.filed}
                  />
                  <Stat
                    label={t("Kept in Inbox", "Im Posteingang")}
                    value={review.proof.proof.counts.inbox}
                  />
                  <Stat
                    label={t("Validation gate", "Prüfstatus")}
                    value={review.proof.proof.gate}
                  />
                </div>
                <CheckField checked={confirmed} onChange={setConfirmed}>
                  {t(
                    "Save this exact private address ledger locally.",
                    "Dieses genaue private Adressverzeichnis lokal speichern.",
                  )}
                </CheckField>
                <div className="actions">
                  <Action
                    primary
                    disabled={!confirmed || busy}
                    onClick={() => void runConfirm()}
                  >
                    {t("Save ledger", "Verzeichnis speichern")}
                  </Action>
                  <Action onClick={() => setReview(null)} disabled={busy}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                </div>
              </div>
            )}

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
      <Notice>{copy.boundary}</Notice>
      <ol className="guide-steps">
        {gmailRoutePlan.stages.map((stage, i) => (
          <li key={i}>
            <strong>{stage[language].title}</strong>
            <p className="muted">{stage[language].body}</p>
          </li>
        ))}
      </ol>
      <p className="help">{copy.next}</p>
    </section>
  );
}

function WinddownPanel({ account }: { account: Account }) {
  const t = useCopy();
  const [view, setView] = useState<WinddownView | null>(null);
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
      setView(await openWinddown(account.id, Math.floor(Date.now() / 1000)));
    } catch {
      setError(
        t(
          "Could not load wind-down report. Scan your mailbox and save local rules first.",
          "Abbau-Bericht konnte nicht geladen werden. Erfasse zuerst das Postfach und speichere lokale Regeln.",
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function toggleWindingDown() {
    if (!view) return;
    setBusy(true);
    setError("");
    try {
      const nextSettings = {
        ...view.report.settings,
        winding_down: !view.report.settings.winding_down,
      };
      const rev = await reviewWinddown(account.id, view.report, {
        kind: "settings",
        settings: nextSettings,
      });
      setReview(rev);
    } catch {
      setError(
        t(
          "Could not preview wind-down setting change.",
          "Einstellungsänderung konnte nicht geprüft werden.",
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
    } catch {
      setError(
        t("Could not update dismissal.", "Ausblendung konnte nicht gespeichert werden."),
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
    } catch {
      setError(t("Could not preview report export.", "Exportvorschau fehlgeschlagen."));
    } finally {
      setBusy(false);
    }
  }

  async function downloadExport() {
    if (!view || !exportPreview) return;
    setBusy(true);
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
    } finally {
      setBusy(false);
    }
  }

  const catalog = view?.report.sender_catalog;

  return (
    <div className="page-stack">
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
            <h2>{t("Account wind-down report", "Konto-Abbau-Bericht")}</h2>
            <p className="muted">
              {t(
                "Track which services and people still write to this account so you can update your email address systematically.",
                "Sieh, welche Dienste und Kontakte noch an dieses Konto schreiben, um deine Adresse Schritt für Schritt umzustellen.",
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
                onClick={() => void toggleWindingDown()}
                disabled={busy}
              >
                {view.report.settings.winding_down
                  ? t("Disable wind-down tracking", "Abbau-Tracking deaktivieren")
                  : t("Mark account as winding down", "Konto als abzubauen markieren")}
              </Action>
            )}
          </div>
        </div>

        {view && (
          <>
            <div className="stats compact">
              <Stat
                label={t("Verdict", "Status")}
                value={view.report.verdict}
              />
              <Stat
                label={t("Waiting senders", "Offene Absender")}
                value={view.report.counts.waiting ?? 0}
              />
              <Stat
                label={t("Moved", "Umgezogen")}
                value={view.report.counts.moved ?? 0}
              />
              {catalog && (
                <Stat
                  label={t("Curated sender catalog", "Kuratierter Katalog")}
                  value={`${catalog.rows} (${catalog.seed_complete ? t("Complete", "Vollständig") : t("Seed", "Entwurf")})`}
                />
              )}
            </div>

            {review && (
              <div className="subpanel">
                <p>{review.consequences}</p>
                <div className="actions">
                  <Action
                    primary
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      try {
                        await confirmWinddown(account.id, review);
                        setReview(null);
                        await load();
                      } finally {
                        setBusy(false);
                      }
                    }}
                  >
                    {t("Confirm setting change", "Änderung bestätigen")}
                  </Action>
                  <Action onClick={() => setReview(null)} disabled={busy}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                </div>
              </div>
            )}

            <div className="actions">
              <Action
                onClick={() => void runExport("local_file")}
                disabled={busy}
              >
                <Download size={15} />
                {t("Export full local report", "Lokalen Bericht exportieren")}
              </Action>
              <Action
                onClick={() => void runExport("agent_summary")}
                disabled={busy}
              >
                <Download size={15} />
                {t(
                  "Export masked summary",
                  "Maskierte Zusammenfassung exportieren",
                )}
              </Action>
            </div>

            {exportPreview && (
              <div className="subpanel">
                <pre tabIndex={0}>{exportPreview.text.slice(0, 1500)}</pre>
                <div className="actions">
                  <Action
                    primary
                    onClick={() => void downloadExport()}
                    disabled={busy}
                  >
                    <Download size={15} />
                    {t("Download", "Herunterladen")} ({exportPreview.filename})
                  </Action>
                  <Action onClick={() => setExportPreview(null)}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                </div>
              </div>
            )}

            {view.report.rows.length > 0 ? (
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
                    {view.report.rows.slice(0, 100).map((row) => (
                      <tr key={row.id}>
                        <td>
                          <strong>{row.sender ?? row.domain ?? row.id}</strong>
                        </td>
                        <td>{row.group}</td>
                        <td>
                          <span className="badge">{row.status}</span>
                        </td>
                        <td className="numeric">{row.recent}</td>
                        <td className="actions">
                          {row.settings_link && (
                            <a
                              className="button"
                              href={row.settings_link}
                              target="_blank"
                              rel="noreferrer"
                            >
                              <ExternalLink size={13} />
                              {t("Account settings", "Kontoeinstellungen")}
                            </a>
                          )}
                          <Action
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
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
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
    </div>
  );
}
