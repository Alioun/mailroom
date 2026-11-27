import { useCallback, useContext, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  Plus,
  RefreshCw,
  Search,
  ShieldAlert,
} from "lucide-react";
import type { Account } from "../../api";
import { confirmLedger, openLedger, reviewLedger } from "../../ledger-api";
import type { LedgerEntry, LedgerReview, LedgerView } from "../../ledger-api";
import { formatNumber } from "../format";
import { describeError } from "../status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  LanguageContext,
  Modal,
  Notice,
  RequiresLocalRules,
  Stat,
  StatusBadge,
  useCopy,
} from "../components";

export function LedgerPanel({
  account,
  hasLocalRules,
  onGoRules,
}: {
  account: Account;
  hasLocalRules?: boolean;
  onGoRules?: () => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);
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
  const [errorField, setErrorField] = useState<
    "address" | "service" | "folder" | null
  >(null);
  const addressRef = useRef<HTMLInputElement>(null);
  const serviceRef = useRef<HTMLInputElement>(null);
  const folderRef = useRef<HTMLInputElement>(null);
  const [review, setReview] = useState<LedgerReview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    setError("");
    if (hasLocalRules === false) {
      setView(null);
      return;
    }
    try {
      const next = await openLedger(account.id);
      setView(next);
      setEnabled(next.snapshot.document.enabled);
      setEntries(next.snapshot.document.entries);
    } catch {
      setError(
        t(
          "Could not load the private address ledger. Refresh and try again.",
          "Das private Adressverzeichnis konnte nicht geladen werden. Aktualisiere und versuche es erneut.",
        ),
      );
    }
  }, [account.id, hasLocalRules]);

  useEffect(() => {
    void load();
  }, [load]);

  function addEntry() {
    setFormError("");
    setErrorField(null);
    const addr = newAddress.trim().toLowerCase();
    const svc = newService.trim();
    const folder = newFolder.trim();
    if (!addr || !addr.includes("@") || addr.startsWith("@") || addr.endsWith("@")) {
      setErrorField("address");
      setFormError(
        t(
          "Enter a valid service email address (e.g. shop@example.de).",
          "Gib eine gültige Dienst-E-Mail-Adresse ein (z. B. shop@example.de).",
        ),
      );
      addressRef.current?.focus();
      return;
    }
    if (!svc) {
      setErrorField("service");
      setFormError(
        t("Enter a service name.", "Gib einen Dienstnamen ein."),
      );
      serviceRef.current?.focus();
      return;
    }
    if (!folder) {
      setErrorField("folder");
      setFormError(
        t("Enter a target folder path.", "Gib einen Zielordner ein."),
      );
      folderRef.current?.focus();
      return;
    }
    if (entries.some((e) => e.address.toLowerCase() === addr)) {
      setErrorField("address");
      setFormError(
        t(
          "That address is already in the ledger.",
          "Diese Adresse ist bereits im Verzeichnis eingetragen.",
        ),
      );
      addressRef.current?.focus();
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
      {error && (
        <Notice error>
          <span>{error}</span>
          <Action size="compact" onClick={() => void load()} disabled={busy}>
            {t("Retry", "Erneut versuchen")}
          </Action>
        </Notice>
      )}
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
                value={formatNumber(entries.length, language)}
              />
              <Stat
                label={t("Discovery hints", "Entdeckungshinweise")}
                value={formatNumber(view.proposals.length, language)}
              />
              <Stat
                label={t("Leak alerts", "Leak-Hinweise")}
                value={formatNumber(view.leaks.length, language)}
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
                title={`${t("Observed address discovery hints", "Entdeckte Dienstadressen")} · ${formatNumber(view.proposals.length, language)}`}
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
                  ref={addressRef}
                  value={newAddress}
                  onChange={(e) => {
                    setNewAddress(e.target.value);
                    setFormError("");
                    setErrorField(null);
                  }}
                  aria-invalid={errorField === "address"}
                  aria-describedby={
                    formError && errorField === "address"
                      ? "ledger-form-error"
                      : undefined
                  }
                  placeholder="shop@example.de"
                />
              </label>
              <label className="field">
                {t("Service name", "Dienstname")}
                <input
                  ref={serviceRef}
                  value={newService}
                  onChange={(e) => {
                    setNewService(e.target.value);
                    setFormError("");
                    setErrorField(null);
                  }}
                  aria-invalid={errorField === "service"}
                  aria-describedby={
                    formError && errorField === "service"
                      ? "ledger-form-error"
                      : undefined
                  }
                  placeholder="Example Shop"
                />
              </label>
              <label className="field">
                {t("Target folder", "Zielordner")}
                <input
                  ref={folderRef}
                  value={newFolder}
                  onChange={(e) => {
                    setNewFolder(e.target.value);
                    setFormError("");
                    setErrorField(null);
                  }}
                  aria-invalid={errorField === "folder"}
                  aria-describedby={
                    formError && errorField === "folder"
                      ? "ledger-form-error"
                      : undefined
                  }
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
              {formError && (
                <div id="ledger-form-error">
                  <Notice error>{formError}</Notice>
                </div>
              )}
              <Action onClick={addEntry}>
                <Plus size={15} />
                {t("Add entry", "Eintrag hinzufügen")}
              </Action>
            </div>

            {entries.length > 0 && (
              <>
                <p className="help mt-sm">
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

            <div className="actions mt-md">
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
          <RequiresLocalRules onGoRules={onGoRules} />
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
                value={formatNumber(review.proof.proof.counts.filed, language)}
              />
              <Stat
                label={t("Kept in Inbox", "Im Posteingang")}
                value={formatNumber(review.proof.proof.counts.inbox, language)}
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

