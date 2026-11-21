import { useCallback, useEffect, useState } from "react";
import {
  BellOff,
  CheckCircle2,
  MailX,
  RefreshCw,
  ShieldAlert,
} from "lucide-react";
import {
  confirmSubscriptionReview,
  openSubscriptionReview,
  previewSubscriptionReview,
  recoverUnsubscribe,
  resolveUnsubscribe,
} from "../api";
import type {
  Account,
  SubscriptionReviewPreview,
  SubscriptionReviewView,
  UnsubscribeMethod,
} from "../api";
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

export function Subscriptions({
  account,
  liveWrites,
  onGoSettings,
}: {
  account: Account;
  liveWrites: boolean;
  onGoSettings: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<SubscriptionReviewView | null>(null);
  const [methods, setMethods] = useState<Record<string, UnsubscribeMethod>>({});
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [personalFolder, setPersonalFolder] = useState("Privat/Stummgeschaltet");
  const [businessFolder, setBusinessFolder] = useState(
    "Geschäftlich/Stummgeschaltet",
  );
  const [preview, setPreview] = useState<SubscriptionReviewPreview | null>(
    null,
  );
  const [confirmed, setConfirmed] = useState(false);
  const [mailConfirmed, setMailConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [asOf, setAsOf] = useState(() => Math.floor(Date.now() / 1000));

  const load = useCallback(async () => {
    setError("");
    const now = Math.floor(Date.now() / 1000);
    setAsOf(now);
    try {
      const next = await openSubscriptionReview(now);
      setView(next);
      const nextMethods: Record<string, UnsubscribeMethod> = {};
      const nextSelected: Record<string, boolean> = {};
      for (const acc of next.accounts) {
        for (const row of acc.report.rows) {
          const key = `${acc.account}:${row.id}`;
          nextMethods[key] = row.methods[0] ?? "mute";
          nextSelected[key] = row.recommended;
        }
      }
      setMethods((prev) => ({ ...nextMethods, ...prev }));
      setSelected((prev) =>
        Object.keys(prev).length > 0 ? prev : nextSelected,
      );
    } catch {
      setError(
        t(
          "Could not load subscriptions. Scan your mailbox first.",
          "Abonnements konnten nicht geladen werden. Erfasse zuerst dein Postfach.",
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  const currentAccount = view?.accounts.find((a) => a.account === account.id);
  const rows = currentAccount?.report.rows ?? [];
  const history = currentAccount?.history ?? [];

  function buildSelections() {
    if (!view) return [];
    const list: {
      account: string;
      subscription: string;
      method: UnsubscribeMethod;
    }[] = [];
    for (const acc of view.accounts) {
      if (acc.account !== account.id) continue;
      for (const row of acc.report.rows) {
        const key = `${acc.account}:${row.id}`;
        if (selected[key]) {
          list.push({
            account: acc.account,
            subscription: row.id,
            method: methods[key] ?? row.methods[0] ?? "mute",
          });
        }
      }
    }
    return list;
  }

  async function runPreview() {
    const selections = buildSelections();
    if (selections.length === 0) {
      setError(
        t(
          "Select at least one subscription first.",
          "Wähle zuerst mindestens ein Abonnement aus.",
        ),
      );
      return;
    }
    setBusy(true);
    setError("");
    setMessage("");
    setConfirmed(false);
    setMailConfirmed(false);
    try {
      setPreview(
        await previewSubscriptionReview({
          as_of: asOf,
          selections,
          folders: {
            personal: personalFolder.trim() || "Privat/Stummgeschaltet",
            business: businessFolder.trim() || "Geschäftlich/Stummgeschaltet",
          },
        }),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Could not preview subscription actions. Check Live writes in Settings if using One-Click POST or SMTP.",
              "Abo-Vorschau fehlgeschlagen. Prüfe Live-Schreibzugriff in den Einstellungen für One-Click-POST oder SMTP.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmActions() {
    if (!preview || !confirmed) return;
    if (preview.local_mail_confirmation && !mailConfirmed) return;
    const selections = buildSelections();
    setBusy(true);
    setError("");
    try {
      const next = await confirmSubscriptionReview(
        {
          as_of: asOf,
          selections,
          folders: {
            personal: personalFolder.trim() || "Privat/Stummgeschaltet",
            business: businessFolder.trim() || "Geschäftlich/Stummgeschaltet",
          },
        },
        preview,
      );
      setView(next);
      setPreview(null);
      setConfirmed(false);
      setMailConfirmed(false);
      setMessage(
        t(
          "Selected subscription actions processed. Review the history below for status and any prepared cards.",
          "Ausgewählte Abo-Aktionen ausgeführt. Prüfe den Verlauf unten für Status und vorbereitete Karten.",
        ),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Subscription action failed or was refused. Refresh before retrying.",
              "Abo-Aktion fehlgeschlagen oder abgelehnt. Aktualisiere vor einem neuen Versuch.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  const methodLabel = (method: UnsubscribeMethod) =>
    ({
      one_click: t("One-Click POST", "One-Click-POST"),
      local_mail: t("Send mailto: via SMTP", "mailto: per SMTP senden"),
      mail_app_preparation: t("Open in Mail app", "In Mail-App öffnen"),
      browser_preparation: t("Browser link card", "Browser-Link anzeigen"),
      retirement_preparation: t(
        "Address retirement prep",
        "Adress-Stilllegung vorbereiten",
      ),
      mute: t("Mute (mark read)", "Stummschalten (gelesen)"),
      remove_mute: t("Remove mute", "Stummschaltung aufheben"),
      keep: t("Keep as is", "Behalten"),
    })[method];

  return (
    <div className="page-stack">
      {!liveWrites && !view?.synthetic && (
        <Notice>
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Ranking and reviewing subscriptions works now; executing One-Click POST, SMTP unsubscribe, or Mail handoff requires enabling Live writes in Settings.",
              "Der Nur-Lese-Modus ist aktiv. Auswertung und Prüfung funktionieren sofort; für One-Click-POST, SMTP-Abmeldung oder Mail-Übergabe aktiviere Live-Schreibzugriff in den Einstellungen.",
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
              {t(
                "Unsubscribe & volume-cutting pass",
                "Abmeldungen & Stummschaltung",
              )}
            </h2>
            <p className="muted">
              {t(
                "Unread bulk senders are ranked by volume. Unsubscribe with One-Click POST / mailto:, or mute directly to dedicated muted folders (marked read).",
                "Ungelesene Massenabsender sind nach Volumen sortiert. Melde dich per One-Click-POST / mailto: ab oder schalte sie direkt stumm (als gelesen markiert).",
              )}
            </p>
          </div>
          <Action onClick={() => void load()} disabled={busy}>
            <RefreshCw size={15} />
            {t("Refresh", "Aktualisieren")}
          </Action>
        </div>

        <div className="stats compact">
          <Stat
            label={t("Subscriptions found", "Gefundene Abos")}
            value={rows.length}
          />
          <Stat
            label={t("Recommended unread", "Empfohlen (ungelesen)")}
            value={rows.filter((r) => r.recommended).length}
          />
          <Stat
            label={t("Selected", "Ausgewählt")}
            value={buildSelections().length}
          />
          <Stat
            label={t("History entries", "Verlaufseinträge")}
            value={history.length}
          />
        </div>

        <div className="settings-form">
          <label className="field">
            {t("Personal muted folder", "Privater Stumm-Ordner")}
            <input
              value={personalFolder}
              onChange={(e) => setPersonalFolder(e.target.value)}
            />
          </label>
          <label className="field">
            {t("Business muted folder", "Geschäftlicher Stumm-Ordner")}
            <input
              value={businessFolder}
              onChange={(e) => setBusinessFolder(e.target.value)}
            />
          </label>
          <Action
            primary
            onClick={() => void runPreview()}
            disabled={busy || buildSelections().length === 0}
          >
            {busy ? (
              <Busy>{t("Preparing preview…", "Vorschau wird erstellt…")}</Busy>
            ) : (
              <>
                <MailX size={15} />
                {t("Preview selected actions", "Ausgewählte Aktionen prüfen")}
              </>
            )}
          </Action>
        </div>

        {rows.length === 0 ? (
          <Empty
            title={t(
              "No bulk subscriptions detected",
              "Keine Massen-Abonnements erkannt",
            )}
          >
            {t(
              "Run a mailbox scan to discover newsletters and bulk senders with List-Unsubscribe headers.",
              "Starte eine Postfacherfassung, um Newsletter und Massenabsender mit List-Unsubscribe-Kopfzeilen zu erkennen.",
            )}
          </Empty>
        ) : (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Select", "Wahl")}</th>
                  <th>{t("Sender", "Absender")}</th>
                  <th className="numeric">{t("12m", "12M")}</th>
                  <th className="numeric">{t("Unread", "Ungelesen")}</th>
                  <th>{t("Action", "Methode")}</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const key = `${account.id}:${row.id}`;
                  return (
                    <tr key={key}>
                      <td>
                        <CheckField
                          checked={Boolean(selected[key])}
                          onChange={(checked) =>
                            setSelected((prev) => ({
                              ...prev,
                              [key]: checked,
                            }))
                          }
                        >
                          <span className="sr-only">{row.sender}</span>
                        </CheckField>
                      </td>
                      <td>
                        <strong>{row.sender}</strong>
                        <div className="help">
                          {row.domain ?? "—"} · {row.role}
                        </div>
                      </td>
                      <td className="numeric">{row.count_12_months}</td>
                      <td className="numeric">{row.unread_volume}</td>
                      <td>
                        <Choice
                          label={t("Method", "Methode")}
                          value={methods[key] ?? row.methods[0] ?? "mute"}
                          onChange={(value) =>
                            setMethods((prev) => ({
                              ...prev,
                              [key]: value as UnsubscribeMethod,
                            }))
                          }
                          options={row.methods.map((m) => ({
                            value: m,
                            label: methodLabel(m),
                          }))}
                        />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {history.length > 0 && (
        <section className="panel">
          <h2>{t("Subscription action history", "Verlauf der Abo-Aktionen")}</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Sender", "Absender")}</th>
                  <th>{t("Method", "Methode")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th>{t("Resolution", "Klärung")}</th>
                </tr>
              </thead>
              <tbody>
                {history.map((entry) => (
                  <tr key={entry.id}>
                    <td>{entry.sender_key}</td>
                    <td>{methodLabel(entry.method)}</td>
                    <td>
                      <span className="badge">{entry.status}</span>
                    </td>
                    <td className="actions">
                      {entry.status === "uncertain" && (
                        <>
                          {view?.synthetic ? (
                            <Action
                              disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                try {
                                  await recoverUnsubscribe(
                                    account.id,
                                    entry.id,
                                    asOf,
                                  );
                                  await load();
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
                                    await resolveUnsubscribe(
                                      account.id,
                                      entry.id,
                                      "sent",
                                      asOf,
                                    );
                                    await load();
                                  } finally {
                                    setBusy(false);
                                  }
                                }}
                              >
                                {t("Mark sent", "Als gesendet")}
                              </Action>
                              <Action
                                disabled={busy || !liveWrites}
                                onClick={async () => {
                                  setBusy(true);
                                  try {
                                    await resolveUnsubscribe(
                                      account.id,
                                      entry.id,
                                      "not_sent",
                                      asOf,
                                    );
                                    await load();
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
        title={t(
          "Review exact subscription actions",
          "Genaue Abo-Aktionen bestätigen",
        )}
        wide
      >
        {preview && (
          <div className="page-stack">
            {preview.previews.flatMap((p) => p.actions).map((act, i) => (
              <div className="proposal-card" key={`${act.subscription}-${i}`}>
                <div>
                  <strong>
                    {act.sender} · {methodLabel(act.method)}
                  </strong>
                  <p className="muted">{act.effect}</p>
                  {act.destination && (
                    <p className="help">
                      <BellOff size={13} /> {act.destination}
                    </p>
                  )}
                  {act.copy_card?.uri && (
                    <p className="help">
                      <code>{act.copy_card.uri}</code>
                    </p>
                  )}
                </div>
              </div>
            ))}
            <Details title={t("Exact confirmation summary", "Bestätigungsdetails")}>
              <p>{preview.confirmation.display}</p>
            </Details>
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {preview.confirmation_action}
            </CheckField>
            {preview.local_mail_confirmation && (
              <CheckField checked={mailConfirmed} onChange={setMailConfirmed}>
                {preview.local_mail_confirmation}
              </CheckField>
            )}
            <div className="dialog-actions">
              <Action onClick={() => setPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                primary
                disabled={
                  busy ||
                  !confirmed ||
                  (Boolean(preview.local_mail_confirmation) && !mailConfirmed)
                }
                onClick={() => void confirmActions()}
              >
                {busy ? (
                  <Busy>{t("Applying…", "Wird ausgeführt…")}</Busy>
                ) : (
                  t("Confirm actions", "Aktionen ausführen")
                )}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
