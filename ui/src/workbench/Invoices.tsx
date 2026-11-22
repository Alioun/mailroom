import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
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
import { describeError, invoiceRoleText } from "./status-labels";
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

export function Invoices({
  account,
  liveWrites,
  onOpenWriteMode,
}: {
  account: Account;
  liveWrites: boolean;
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<InvoiceView | null>(null);
  const [target, setTarget] = useState("");
  const [preview, setPreview] = useState<InvoicePreview | null>(null);
  const [previewRow, setPreviewRow] = useState<
    InvoiceView["report"]["rows"][number] | null
  >(null);
  const [confirmed, setConfirmed] = useState(false);
  const [page, setPage] = useState(0);
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not load invoice report. Scan your mailbox and save local rules first.",
            "Rechnungsbericht konnte nicht geladen werden. Erfasse zuerst das Postfach und speichere lokale Regeln.",
          ),
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
        describeError(
          err,
          t(
            "Could not read original message from IMAP.",
            "Originalnachricht konnte nicht per IMAP gelesen werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function openPreview(row: InvoiceView["report"]["rows"][number]) {
    if (!target) {
      setError(
        t(
          "Define an accounting target (e.g. Qonto) in Rules & deploy before sending invoices.",
          "Definiere zuerst ein Buchhaltungsziel (z. B. Qonto) unter Regeln & Bereitstellung.",
        ),
      );
      return;
    }
    setBusy(true);
    setError("");
    setConfirmed(false);
    try {
      const mode = view?.synthetic ? undefined : "smtp";
      const nextPreview = await previewInvoice(
        account.id,
        row.id,
        target,
        mode,
      );
      setPreview(nextPreview);
      setPreviewRow(row);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview invoice send-on. Fetch the original message first.",
            "Versandvorschau fehlgeschlagen. Rufe zuerst das Original ab.",
          ),
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
    setMessage("");
    try {
      const next = await sendInvoice(account.id, preview);
      setView(next);
      const latest = next.sends[next.sends.length - 1];
      setPreview(null);
      setPreviewRow(null);
      setConfirmed(false);
      if (latest?.status === "completed") {
        setMessage(
          t(
            "Invoice accepted by SMTP server. Accounting-tool acceptance remains separate from SMTP delivery.",
            "Rechnung vom SMTP-Server angenommen. Die Verarbeitung im Buchhaltungstool ist davon unabhängig.",
          ),
        );
      } else if (latest?.status === "not_sent") {
        setError(
          t(
            "The SMTP server rejected the message before delivery. Nothing was sent.",
            "Der SMTP-Server hat die Nachricht vor dem Versand abgelehnt. Nichts wurde gesendet.",
          ),
        );
      } else if (latest?.status === "uncertain") {
        setError(
          t(
            "SMTP delivery outcome is uncertain. Check your Sent folder or accounting inbox, then resolve the entry below without retrying blindly.",
            "Das SMTP-Ergebnis ist unklar. Prüfe deinen Gesendet-Ordner oder die Buchhaltung und kläre den Eintrag unten ohne blinden Neuversuch.",
          ),
        );
      } else {
        setMessage(
          t(
            "Invoice send recorded. Check the send history below for current status.",
            "Rechnungsversand erfasst. Prüfe den Versandverlauf unten.",
          ),
        );
      }
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Invoice send-on failed or was refused. Check the send history before retrying.",
            "Rechnungsversand fehlgeschlagen oder abgelehnt. Prüfe den Verlauf vor einem neuen Versuch.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const perPage = 25;
  const rows = view?.report.rows ?? [];
  const pages = Math.max(1, Math.ceil(rows.length / perPage));
  const currentPage = Math.min(page, pages - 1);
  const visibleRows = rows.slice(
    currentPage * perPage,
    (currentPage + 1) * perPage,
  );

  return (
    <div className="page-stack">
      {!liveWrites && !view?.synthetic && (
        <Notice tone="info">
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Inspecting candidates, fetching originals, previewing sends, and resolving uncertain entries work now; transmitting via SMTP requires Live writes.",
              "Der Nur-Lese-Modus ist aktiv. Prüfen, Originale laden, Versandvorschau und Klärung unklarer Einträge funktionieren sofort; der SMTP-Versand erfordert Live-Schreibzugriff.",
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
            <h2>
              {t(
                "Missed-invoice review & send-on",
                "Rechnungsprüfung & Weiterleitung",
              )}
            </h2>
            <p className="muted">
              {t(
                "Inspect business invoice candidates, verify attachments read-only, and send approved originals via SMTP to your accounting target (such as Qonto) without duplicate copies.",
                "Prüfe geschäftliche Rechnungskandidaten, verifiziere Anhänge nur lesend und leite freigegebene Originale per SMTP ohne Duplikate an dein Buchhaltungsziel (z. B. Qonto) weiter.",
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
                value={view.report.count.toLocaleString()}
              />
              <Stat
                label={t("Eligible business", "Geschäftlich geeignet")}
                value={view.report.eligible_business.toLocaleString()}
              />
              <Stat
                label={t("Unresolved", "Ungeklärt")}
                value={view.report.unresolved.toLocaleString()}
              />
              <Stat
                label={t("Recorded sends", "Erfasste Sendungen")}
                value={view.sends.length.toLocaleString()}
              />
            </div>
            {view.targets.length > 0 ? (
              <div className="toolbar">
                <Choice
                  fieldLabel={t("Accounting target", "Buchhaltungsziel")}
                  label={t("Accounting target", "Buchhaltungsziel")}
                  value={target}
                  onChange={setTarget}
                  options={view.targets.map((name) => ({
                    value: name,
                    label: name,
                  }))}
                />
              </div>
            ) : (
              <Notice tone="warning">
                {t(
                  "No accounting targets are configured in your local rules yet. Add a target (e.g. Qonto) in Rules & deploy to enable send-on.",
                  "In deinen lokalen Regeln ist noch kein Buchhaltungsziel konfiguriert. Füge ein Ziel (z. B. Qonto) unter Regeln & Bereitstellung hinzu.",
                )}
              </Notice>
            )}
            {rows.length === 0 ? (
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
              <>
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("Sender", "Absender")}</th>
                        <th>{t("Subject & evidence", "Betreff & Nachweis")}</th>
                        <th>{t("Role", "Rolle")}</th>
                        <th>{t("Actions", "Aktionen")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {visibleRows.map((row) => {
                        const roleMeta = invoiceRoleText(row.role, t);
                        const evidenceSummary =
                          row.evidence.kind === "known"
                            ? `${row.evidence.attachments.length} ${t("attachments", "Anhänge")} (${row.evidence.attachments.map((a) => `${a.name} · ${a.media_type}`).join(", ") || row.evidence.body})`
                            : row.evidence.kind === "filename_hint"
                              ? `${t("Filename hints", "Dateinamen-Hinweise")}: ${row.evidence.names.join(", ")}`
                              : t(
                                  "Header only — fetch original to inspect attachments",
                                  "Nur Kopfzeilen — Original laden, um Anhänge zu prüfen",
                                );
                        return (
                          <tr key={row.id}>
                            <td>
                              <strong>{row.sender}</strong>
                            </td>
                            <td>
                              <div>{row.subject}</div>
                              <small className="muted">{evidenceSummary}</small>
                            </td>
                            <td>
                              <StatusBadge
                                status={row.role}
                                label={roleMeta.label}
                                tone={roleMeta.tone}
                              />
                            </td>
                            <td>
                              <div className="cell-actions">
                                {row.evidence.kind !== "known" && (
                                  <Action
                                    size="compact"
                                    disabled={busy}
                                    onClick={() => void acquire(row.id)}
                                  >
                                    <Download size={14} />
                                    {t("Fetch original", "Original laden")}
                                  </Action>
                                )}
                                <Action
                                  size="compact"
                                  primary
                                  disabled={
                                    busy || !row.send_available || !target
                                  }
                                  onClick={() => void openPreview(row)}
                                >
                                  <Send size={14} />
                                  {t("Preview send", "Versand prüfen")}
                                </Action>
                              </div>
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
                {rows.length > perPage && (
                  <div className="pagination">
                    <span className="muted">
                      {currentPage * perPage + 1}–
                      {Math.min((currentPage + 1) * perPage, rows.length)} /{" "}
                      {rows.length}
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
            )}
          </>
        ) : (
          <Empty
            title={t(
              "Loading invoice report…",
              "Rechnungsbericht wird geladen…",
            )}
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
                      <strong>{send.plan.target_name}</strong> ({send.plan.target}
                      )
                    </td>
                    <td>
                      <StatusBadge status={send.status} />
                    </td>
                    <td>
                      <code>{send.plan.content_digest.slice(0, 12)}</code>
                    </td>
                    <td>
                      <div className="cell-actions">
                        {send.status === "uncertain" && (
                          <>
                            {view.synthetic ? (
                              <Action
                                size="compact"
                                disabled={busy}
                                onClick={async () => {
                                  setBusy(true);
                                  setError("");
                                  try {
                                    setView(
                                      await recoverInvoice(account.id, send.id),
                                    );
                                  } catch (err) {
                                    setError(
                                      describeError(
                                        err,
                                        t(
                                          "Could not recover send status.",
                                          "Versandstatus konnte nicht geprüft werden.",
                                        ),
                                      ),
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
                                  size="compact"
                                  disabled={busy}
                                  onClick={async () => {
                                    setBusy(true);
                                    setError("");
                                    try {
                                      setView(
                                        await resolveInvoice(
                                          account.id,
                                          send.id,
                                          "sent",
                                        ),
                                      );
                                      setMessage(
                                        t(
                                          "Recorded as sent.",
                                          "Als gesendet vermerkt.",
                                        ),
                                      );
                                    } catch (err) {
                                      setError(
                                        describeError(
                                          err,
                                          t(
                                            "Could not record resolution.",
                                            "Klärung konnte nicht gespeichert werden.",
                                          ),
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
                                  size="compact"
                                  disabled={busy}
                                  onClick={async () => {
                                    setBusy(true);
                                    setError("");
                                    try {
                                      setView(
                                        await resolveInvoice(
                                          account.id,
                                          send.id,
                                          "not_sent",
                                        ),
                                      );
                                      setMessage(
                                        t(
                                          "Recorded as not sent.",
                                          "Als nicht gesendet vermerkt.",
                                        ),
                                      );
                                    } catch (err) {
                                      setError(
                                        describeError(
                                          err,
                                          t(
                                            "Could not record resolution.",
                                            "Klärung konnte nicht gespeichert werden.",
                                          ),
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
                      </div>
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
          if (!busy && !open) {
            setPreview(null);
            setPreviewRow(null);
          }
        }}
        title={t("Confirm invoice send-on", "Rechnungsversand bestätigen")}
        wide
      >
        {preview && (
          <div className="page-stack">
            <p className="muted">
              <FileText size={15} /> <strong>{preview.plan.target_name}</strong>{" "}
              &lt;{preview.plan.target}&gt; · {t("From", "Von")}{" "}
              {preview.plan.sending_address} ·{" "}
              {t("Via SMTP (exact original bytes)", "Per SMTP (Originalnachricht)")}
            </p>
            {previewRow && (
              <div className="subpanel">
                <div>
                  <strong>{previewRow.subject}</strong>
                  <p className="help">
                    {t("Original sender", "Ursprünglicher Absender")}:{" "}
                    {previewRow.sender} · {t("Digest", "Prüfsumme")}:{" "}
                    <code>{preview.plan.content_digest.slice(0, 16)}</code>
                  </p>
                </div>
                {preview.plan.source.evidence.kind === "known" && (
                  <ul className="evidence-list" style={{ margin: 0 }}>
                    {preview.plan.source.evidence.attachments.map((att, i) => (
                      <li key={i}>
                        <code>{att.name}</code> ({att.media_type})
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            {preview.confirmation && (
              <Details
                title={t(
                  "Exact confirmation details",
                  "Genaue Bestätigungsdetails",
                )}
              >
                <p>{preview.confirmation.display}</p>
              </Details>
            )}
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "I verified the invoice original, attachments, and accounting target. Sent mail cannot be undone.",
                "Ich habe das Rechnungsoriginal, die Anhänge und das Buchhaltungsziel geprüft. Gesendete E-Mails lassen sich nicht zurücknehmen.",
              )}
            </CheckField>
            <div className="dialog-actions">
              <Action
                onClick={() => {
                  setPreview(null);
                  setPreviewRow(null);
                }}
                disabled={busy}
              >
                {t("Cancel", "Abbrechen")}
              </Action>
              {!liveWrites && !view?.synthetic ? (
                <Action tone="warning" onClick={onOpenWriteMode}>
                  <ShieldAlert size={15} />
                  {t(
                    "Enable live writes to send",
                    "Live-Schreibzugriff zum Senden aktivieren",
                  )}
                </Action>
              ) : (
                <Action
                  tone="warning"
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
              )}
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
