import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  Download,
  FileText,
  RefreshCw,
  Send,
  ShieldAlert,
} from "lucide-react";
import {
  acquireInvoice,
  openInvoices,
  previewInvoice,
  recoverInvoice,
  resolveInvoice,
  sendInvoice,
} from "../api";
import type { Account, InvoicePreview, InvoiceView } from "../api";
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
  useCopy,
} from "./components";

export function Invoices({
  account,
  liveWrites,
  onGoSettings,
}: {
  account: Account;
  liveWrites: boolean;
  onGoSettings: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<InvoiceView | null>(null);
  const [target, setTarget] = useState("");
  const [preview, setPreview] = useState<InvoicePreview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    setError("");
    try {
      const next = await openInvoices(account.id);
      setView(next);
      setTarget((cur) =>
        next.targets.includes(cur) ? cur : (next.targets[0] ?? ""),
      );
    } catch {
      setError(
        t(
          "Could not load invoice report. Scan your mailbox and save local rules first.",
          "Rechnungsbericht konnte nicht geladen werden. Erfasse zuerst das Postfach und speichere lokale Regeln.",
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function acquire(selection: string) {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      setView(await acquireInvoice(account.id, selection));
      setMessage(
        t(
          "Original message fetched read-only. Attachments and MIME evidence updated.",
          "Originalnachricht schreibgeschützt abgerufen. Anhänge und MIME-Nachweise wurden aktualisiert.",
        ),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Could not read original message from IMAP.",
              "Originalnachricht konnte nicht per IMAP gelesen werden.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function openPreview(selection: string) {
    if (!target) {
      setError(
        t(
          "Define an accounting target (e.g. Qonto) in Local rules before sending invoices.",
          "Definiere zuerst ein Buchhaltungsziel (z. B. Qonto) in Lokale Regeln.",
        ),
      );
      return;
    }
    setBusy(true);
    setError("");
    setConfirmed(false);
    try {
      const mode = view?.synthetic ? undefined : "smtp";
      setPreview(await previewInvoice(account.id, selection, target, mode));
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Could not preview invoice send-on. Acquire the original message and check Live writes in Settings.",
              "Versandvorschau fehlgeschlagen. Rufe zuerst das Original ab und prüfe Live-Schreibzugriff in den Einstellungen.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmSend() {
    if (!preview || !confirmed) return;
    setBusy(true);
    setError("");
    try {
      setView(await sendInvoice(account.id, preview));
      setPreview(null);
      setConfirmed(false);
      setMessage(
        t(
          "Invoice handoff submitted. Accounting-tool acceptance remains separate from SMTP handoff.",
          "Rechnung weitergeleitet. Die Annahme im Buchhaltungstool ist unabhängig von der SMTP-Übergabe.",
        ),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Invoice send-on failed or was refused. Check the send history before retrying.",
              "Rechnungsversand fehlgeschlagen oder abgelehnt. Prüfe den Verlauf vor einem neuen Versuch.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="page-stack">
      {!liveWrites && !view?.synthetic && (
        <Notice>
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Reading missed invoices and fetching originals work now; sending invoices via SMTP requires enabling Live writes in Settings.",
              "Der Nur-Lese-Modus ist aktiv. Das Prüfen fehlender Rechnungen und Abrufen von Originalen funktioniert sofort; zum Senden per SMTP aktiviere Live-Schreibzugriff in den Einstellungen.",
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
            <h2>{t("Missed-invoice review & send-on", "Rechnungsprüfung & Weiterleitung")}</h2>
            <p className="muted">
              {t(
                "Inspect business invoice candidates, verify attachments read-only, and send approved originals to your accounting target (such as Qonto) without duplicate copies.",
                "Prüfe geschäftliche Rechnungskandidaten, verifiziere Anhänge nur lesend und leite freigegebene Originale ohne Duplikate an dein Buchhaltungsziel (z. B. Qonto) weiter.",
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
                label={t("Candidates", "Kandidaten")}
                value={view.report.count}
              />
              <Stat
                label={t("Eligible business", "Geschäftlich geeignet")}
                value={view.report.eligible_business}
              />
              <Stat
                label={t("Unresolved", "Ungeklärt")}
                value={view.report.unresolved}
              />
              <Stat
                label={t("Recorded sends", "Erfasste Sendungen")}
                value={view.sends.length}
              />
            </div>
            {view.targets.length > 0 && (
              <div className="toolbar">
                <Choice
                  label={t("Accounting target", "Buchhaltungsziel")}
                  value={target}
                  onChange={setTarget}
                  options={view.targets.map((name) => ({
                    value: name,
                    label: name,
                  }))}
                />
              </div>
            )}
            {view.report.rows.length === 0 ? (
              <Empty
                title={t(
                  "No missed invoices found",
                  "Keine fehlenden Rechnungen gefunden",
                )}
              >
                {t(
                  "Messages matching your business invoice criteria will appear here after scanning.",
                  "Nachrichten, die deinen Kriterien für Geschäftsrechnungen entsprechen, erscheinen hier nach der Erfassung.",
                )}
              </Empty>
            ) : (
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>{t("Sender", "Absender")}</th>
                      <th>{t("Subject", "Betreff")}</th>
                      <th>{t("Role", "Rolle")}</th>
                      <th>{t("Evidence", "Nachweis")}</th>
                      <th>{t("Actions", "Aktionen")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {view.report.rows.map((row) => (
                      <tr key={row.id}>
                        <td>{row.sender}</td>
                        <td>{row.subject}</td>
                        <td>
                          <span className="badge">{row.role}</span>
                        </td>
                        <td>
                          {row.evidence.kind === "known"
                            ? `${row.evidence.attachments.length} ${t("attachments", "Anhänge")}`
                            : row.evidence.kind}
                        </td>
                        <td className="actions">
                          {row.evidence.kind !== "known" && (
                            <Action
                              disabled={busy}
                              onClick={() => void acquire(row.id)}
                            >
                              <Download size={14} />
                              {t("Fetch original", "Original laden")}
                            </Action>
                          )}
                          <Action
                            primary
                            disabled={
                              busy ||
                              !row.send_available ||
                              !target ||
                              (!liveWrites && !view.synthetic)
                            }
                            onClick={() => void openPreview(row.id)}
                          >
                            <Send size={14} />
                            {t("Preview send", "Versand prüfen")}
                          </Action>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        ) : (
          <Empty
            title={t("Loading invoice report…", "Rechnungsbericht wird geladen…")}
          />
        )}
      </section>

      {view && view.sends.length > 0 && (
        <section className="panel">
          <h2>{t("Invoice send history", "Versandverlauf")}</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Target", "Ziel")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th>{t("Digest", "Prüfsumme")}</th>
                  <th>{t("Resolution", "Klärung")}</th>
                </tr>
              </thead>
              <tbody>
                {view.sends.map((send) => (
                  <tr key={send.id}>
                    <td>
                      {send.plan.target_name} ({send.plan.target})
                    </td>
                    <td>
                      <span className="badge">{send.status}</span>
                    </td>
                    <td>
                      <code>{send.plan.content_digest.slice(0, 12)}</code>
                    </td>
                    <td className="actions">
                      {send.status === "uncertain" && (
                        <>
                          {view.synthetic ? (
                            <Action
                              disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                try {
                                  setView(
                                    await recoverInvoice(account.id, send.id),
                                  );
                                } finally {
                                  setBusy(false);
                                }
                              }}
                            >
                              {t("Recover", "Prüfen")}
                            </Action>
                          ) : (
                            <>
                              <Action
                                disabled={busy || !liveWrites}
                                onClick={async () => {
                                  setBusy(true);
                                  try {
                                    setView(
                                      await resolveInvoice(
                                        account.id,
                                        send.id,
                                        "sent",
                                      ),
                                    );
                                  } finally {
                                    setBusy(false);
                                  }
                                }}
                              >
                                {t("Mark sent", "Als gesendet markieren")}
                              </Action>
                              <Action
                                disabled={busy || !liveWrites}
                                onClick={async () => {
                                  setBusy(true);
                                  try {
                                    setView(
                                      await resolveInvoice(
                                        account.id,
                                        send.id,
                                        "not_sent",
                                      ),
                                    );
                                  } finally {
                                    setBusy(false);
                                  }
                                }}
                              >
                                {t("Mark not sent", "Als nicht gesendet")}
                              </Action>
                            </>
                          )}
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      <Modal
        open={preview !== null}
        onOpenChange={(open) => {
          if (!busy && !open) setPreview(null);
        }}
        title={t("Confirm invoice send-on", "Rechnungsversand bestätigen")}
        wide
      >
        {preview && (
          <div className="page-stack">
            <p className="muted">
              <FileText size={15} /> {preview.plan.target_name} &lt;
              {preview.plan.target}&gt; · {t("From", "Von")}{" "}
              {preview.plan.sending_address}
            </p>
            {preview.confirmation && (
              <Details title={t("Exact confirmation details", "Genaue Bestätigungsdetails")}>
                <p>{preview.confirmation.display}</p>
              </Details>
            )}
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "I verified the invoice original and accounting target. Sent mail cannot be undone.",
                "Ich habe das Rechnungsoriginal und das Buchhaltungsziel geprüft. Gesendete E-Mails lassen sich nicht zurücknehmen.",
              )}
            </CheckField>
            <div className="dialog-actions">
              <Action onClick={() => setPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                primary
                disabled={!confirmed || busy}
                onClick={() => void confirmSend()}
              >
                {busy ? (
                  <Busy>{t("Sending…", "Wird gesendet…")}</Busy>
                ) : (
                  <>
                    <Send size={15} />
                    {t("Send invoice", "Rechnung senden")}
                  </>
                )}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
