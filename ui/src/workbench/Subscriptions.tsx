import { useCallback, useEffect, useMemo, useState } from "react";
import {
  BellOff,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  Copy,
  ExternalLink,
  MailX,
  RefreshCw,
  Search,
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

interface SavedCard {
  sender: string;
  method: UnsubscribeMethod;
  uri: string | null;
  to: string | null;
  subject: string | null;
}

const EXTERNAL_METHODS = new Set<UnsubscribeMethod>([
  "one_click",
  "local_mail",
  "mail_app_preparation",
]);

function saferDefaultMethod(methods: UnsubscribeMethod[]): UnsubscribeMethod {
  for (const preferred of [
    "browser_preparation",
    "mute",
    "keep",
    "one_click",
    "local_mail",
    "mail_app_preparation",
  ] as UnsubscribeMethod[]) {
    if (methods.includes(preferred)) return preferred;
  }
  return methods[0] ?? "keep";
}

export function Subscriptions({
  account,
  accounts,
  liveWrites,
  onOpenWriteMode,
}: {
  account: Account;
  accounts: Account[];
  liveWrites: boolean;
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<SubscriptionReviewView | null>(null);
  const [allAccounts, setAllAccounts] = useState(false);
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const [methods, setMethods] = useState<Record<string, UnsubscribeMethod>>({});
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [personalFolder, setPersonalFolder] = useState("Privat/Stummgeschaltet");
  const [businessFolder, setBusinessFolder] = useState(
    "Geschäftlich/Stummgeschaltet",
  );
  const [preview, setPreview] = useState<SubscriptionReviewPreview | null>(
    null,
  );
  const [savedCards, setSavedCards] = useState<SavedCard[]>([]);
  const [confirmed, setConfirmed] = useState(false);
  const [mailConfirmed, setMailConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [asOf, setAsOf] = useState(() => Math.floor(Date.now() / 1000));

  const accountLogins = useMemo(
    () => new Map(accounts.map((a) => [a.id, a.login])),
    [accounts],
  );

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
          nextMethods[key] = saferDefaultMethod(row.methods);
          nextSelected[key] = row.recommended;
        }
      }
      setMethods((prev) => ({ ...nextMethods, ...prev }));
      setSelected((prev) =>
        Object.keys(prev).length > 0 ? prev : nextSelected,
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not load subscriptions. Scan your mailbox first.",
            "Abonnements konnten nicht geladen werden. Erfasse zuerst dein Postfach.",
          ),
        ),
      );
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  const visibleAccounts = useMemo(
    () =>
      (view?.accounts ?? []).filter(
        (a) => allAccounts || a.account === account.id,
      ),
    [view, allAccounts, account.id],
  );

  const allRows = useMemo(
    () =>
      visibleAccounts.flatMap((acc) =>
        acc.report.rows.map((row) => ({
          accountId: acc.account,
          row,
          key: `${acc.account}:${row.id}`,
        })),
      ),
    [visibleAccounts],
  );

  const filteredRows = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allRows;
    return allRows.filter(({ row, accountId }) =>
      `${row.sender} ${row.domain ?? ""} ${row.recipient} ${accountLogins.get(accountId) ?? ""}`
        .toLowerCase()
        .includes(q),
    );
  }, [allRows, query, accountLogins]);

  const history = useMemo(
    () =>
      visibleAccounts.flatMap((acc) =>
        acc.history.map((entry) => ({ accountId: acc.account, entry })),
      ),
    [visibleAccounts],
  );

  const perPage = 25;
  const pages = Math.max(1, Math.ceil(filteredRows.length / perPage));
  const currentPage = Math.min(page, pages - 1);
  const pageRows = filteredRows.slice(
    currentPage * perPage,
    (currentPage + 1) * perPage,
  );

  function buildSelections() {
    const list: {
      account: string;
      subscription: string;
      method: UnsubscribeMethod;
    }[] = [];
    for (const { accountId, row, key } of allRows) {
      if (selected[key]) {
        list.push({
          account: accountId,
          subscription: row.id,
          method: methods[key] ?? saferDefaultMethod(row.methods),
        });
      }
    }
    return list;
  }

  const selections = buildSelections();
  const hasExternalSelection = selections.some((s) =>
    EXTERNAL_METHODS.has(s.method),
  );
  const previewHasExternal = Boolean(
    preview?.previews
      .flatMap((p) => p.actions)
      .some((a) => EXTERNAL_METHODS.has(a.method)),
  );

  async function runPreview() {
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
        describeError(
          err,
          t(
            "Could not preview subscription actions.",
            "Abo-Vorschau fehlgeschlagen.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmActions() {
    if (!preview || !confirmed) return;
    if (preview.local_mail_confirmation && !mailConfirmed) return;
    setBusy(true);
    setError("");
    try {
      const preparedCards: SavedCard[] = preview.previews
        .flatMap((p) => p.actions)
        .filter((a) => a.copy_card?.uri || a.copy_card?.to)
        .map((a) => ({
          sender: a.sender,
          method: a.method,
          uri: a.copy_card?.uri ?? null,
          to: a.copy_card?.to ?? null,
          subject: a.copy_card?.subject ?? null,
        }));
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
      if (preparedCards.length > 0) setSavedCards(preparedCards);
      setMessage(
        t(
          "Selected subscription actions processed. Review prepared links and history below.",
          "Ausgewählte Abo-Aktionen ausgeführt. Prüfe vorbereitete Links und den Verlauf unten.",
        ),
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Subscription action failed or was refused. Refresh before retrying.",
            "Abo-Aktion fehlgeschlagen oder abgelehnt. Aktualisiere vor einem neuen Versuch.",
          ),
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
      mute: t("Mute (local rule, mark read)", "Stummschalten (lokale Regel)"),
      remove_mute: t("Remove mute", "Stummschaltung aufheben"),
      keep: t("Keep as is", "Behalten"),
    })[method];

  return (
    <div className="page-stack">
      {!liveWrites && !view?.synthetic && (
        <Notice tone="info">
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Previewing actions, saving local mutes, preparing browser cards, and resolving uncertain entries work now; One-Click POST, SMTP mailto, and Mail handoff require Live writes.",
              "Der Nur-Lese-Modus ist aktiv. Vorschau, lokale Stummschaltungen, Browser-Karten und Klärung funktionieren sofort; One-Click-POST, SMTP-mailto und Mail-Übergabe erfordern Live-Schreibzugriff.",
            )}
          </span>
          <Action size="compact" tone="warning" onClick={onOpenWriteMode}>
            {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
          </Action>
        </Notice>
      )}
      {view && view.recovering_accounts.length > 0 && (
        <Notice tone="warning">
          {t(
            `Skipped recovering accounts until fresh scan completes: ${view.recovering_accounts.join(", ")}`,
            `Wiederhergestellte Konten bis zur neuen Erfassung übersprungen: ${view.recovering_accounts.join(", ")}`,
          )}
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          <span>{message}</span>
        </Notice>
      )}

      {savedCards.length > 0 && (
        <section className="panel">
          <div className="section-heading">
            <h2>
              {t(
                "Prepared unsubscribe links & cards",
                "Vorbereitete Abmelde-Links & Karten",
              )}
            </h2>
            <Action size="compact" onClick={() => setSavedCards([])}>
              {t("Clear cards", "Karten schließen")}
            </Action>
          </div>
          <div className="page-stack">
            {savedCards.map((card, i) => (
              <div className="proposal-card" key={i}>
                <div>
                  <strong>{card.sender}</strong>
                  {card.uri && (
                    <p className="help">
                      <code>{card.uri}</code>
                    </p>
                  )}
                  {card.to && (
                    <p className="help">
                      {t("To", "An")}: <code>{card.to}</code>
                      {card.subject ? ` · ${card.subject}` : ""}
                    </p>
                  )}
                </div>
                <div className="actions">
                  {card.uri && (
                    <>
                      <Action
                        size="compact"
                        onClick={() => {
                          void navigator.clipboard?.writeText(card.uri!);
                          setMessage(t("Link copied.", "Link kopiert."));
                        }}
                      >
                        <Copy size={13} />
                        {t("Copy link", "Link kopieren")}
                      </Action>
                      <a
                        className="button compact"
                        href={card.uri}
                        target="_blank"
                        rel="noreferrer"
                      >
                        <ExternalLink size={13} />
                        {t("Open link", "Link öffnen")}
                      </a>
                    </>
                  )}
                </div>
              </div>
            ))}
          </div>
        </section>
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
                "Unread bulk senders are ranked by volume. Unsubscribe with One-Click POST, mailto:, browser card, or mute directly to dedicated muted folders (marked read).",
                "Ungelesene Massenabsender sind nach Volumen sortiert. Melde dich per One-Click-POST, mailto:, Browser-Link ab oder schalte sie lokal stumm.",
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
            value={allRows.length.toLocaleString()}
          />
          <Stat
            label={t("Recommended unread", "Empfohlen (ungelesen)")}
            value={allRows
              .filter(({ row }) => row.recommended)
              .length.toLocaleString()}
          />
          <Stat
            label={t("Selected", "Ausgewählt")}
            value={selections.length.toLocaleString()}
          />
          <Stat
            label={t("History entries", "Verlaufseinträge")}
            value={history.length.toLocaleString()}
          />
        </div>

        <Details
          title={t(
            "Muted destination folders",
            "Zielordner für Stummschaltung",
          )}
        >
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
          </div>
        </Details>

        <div className="toolbar">
          <label className="search">
            <Search size={16} />
            <input
              type="search"
              aria-label={t("Search subscriptions", "Abonnements suchen")}
              placeholder={t(
                "Filter sender, domain, or recipient…",
                "Absender, Domain oder Empfänger filtern…",
              )}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setPage(0);
              }}
            />
          </label>
          {accounts.length > 1 && (
            <CheckField
              checked={allAccounts}
              onChange={(next) => {
                setAllAccounts(next);
                setPage(0);
              }}
            >
              {t("All connected accounts", "Alle verbundenen Konten")}
            </CheckField>
          )}
          {filteredRows.length > 0 && (
            <div className="actions">
              <Action
                size="compact"
                onClick={() => {
                  const next = { ...selected };
                  for (const { key } of filteredRows.slice(0, 64))
                    next[key] = true;
                  setSelected(next);
                }}
              >
                {t("Select visible", "Sichtbare wählen")}
              </Action>
              <Action
                size="compact"
                onClick={() => {
                  setSelected({});
                }}
              >
                {t("Clear selection", "Auswahl leeren")}
              </Action>
            </div>
          )}
        </div>

        {allRows.length === 0 ? (
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
          <>
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>{t("Select", "Wahl")}</th>
                    <th>{t("Sender", "Absender")}</th>
                    {allAccounts && <th>{t("Account", "Konto")}</th>}
                    <th className="numeric">{t("12m", "12M")}</th>
                    <th className="numeric">{t("Unread", "Ungelesen")}</th>
                    <th>{t("Method", "Methode")}</th>
                  </tr>
                </thead>
                <tbody>
                  {pageRows.map(({ accountId, row, key }) => (
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
                        <small className="muted">
                          {row.domain ?? "—"} · {row.recipient || "—"}
                        </small>
                      </td>
                      {allAccounts && (
                        <td>{accountLogins.get(accountId) ?? accountId}</td>
                      )}
                      <td className="numeric">
                        {row.count_12_months.toLocaleString()}
                      </td>
                      <td className="numeric">
                        {row.unread_volume.toLocaleString()}
                      </td>
                      <td>
                        <Choice
                          label={`${t("Method for", "Methode für")} ${row.sender}`}
                          value={methods[key] ?? saferDefaultMethod(row.methods)}
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
                  ))}
                </tbody>
              </table>
            </div>
            {filteredRows.length > perPage && (
              <div className="pagination">
                <span className="muted">
                  {currentPage * perPage + 1}–
                  {Math.min((currentPage + 1) * perPage, filteredRows.length)} /{" "}
                  {filteredRows.length}
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
      </section>

      {selections.length > 0 && (
        <div className="sticky-action-bar">
          <div>
            <strong>
              {selections.length}{" "}
              {t("subscriptions selected", "Abonnements ausgewählt")}
            </strong>
            <span className="help" style={{ marginLeft: 10 }}>
              {hasExternalSelection
                ? t(
                    "Includes external unsubscribe action(s)",
                    "Enthält externe Abmeldeaktion(en)",
                  )
                : t(
                    "Local / preparation actions only (works in read-only mode)",
                    "Nur lokale / vorbereitende Aktionen (auch im Nur-Lese-Modus)",
                  )}
            </span>
          </div>
          <Action primary onClick={() => void runPreview()} disabled={busy}>
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
      )}

      {history.length > 0 && (
        <section className="panel">
          <h2>
            {t("Subscription action history", "Verlauf der Abo-Aktionen")}
          </h2>
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
                {history.map(({ accountId, entry }) => (
                  <tr key={`${accountId}:${entry.id}`}>
                    <td>{entry.sender_key}</td>
                    <td>{methodLabel(entry.method)}</td>
                    <td>
                      <StatusBadge status={entry.status} />
                    </td>
                    <td>
                      <div className="cell-actions">
                        {(entry.status === "uncertain" ||
                          entry.status === "intent") && (
                          <>
                            {view?.synthetic ? (
                              <Action
                                size="compact"
                                disabled={busy}
                                onClick={async () => {
                                  setBusy(true);
                                  setError("");
                                  try {
                                    await recoverUnsubscribe(
                                      accountId,
                                      entry.id,
                                      asOf,
                                    );
                                    await load();
                                  } catch (err) {
                                    setError(
                                      describeError(
                                        err,
                                        t(
                                          "Could not recover entry.",
                                          "Eintrag konnte nicht geprüft werden.",
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
                                      await resolveUnsubscribe(
                                        accountId,
                                        entry.id,
                                        "sent",
                                        asOf,
                                      );
                                      setMessage(
                                        t(
                                          "Marked as sent.",
                                          "Als gesendet vermerkt.",
                                        ),
                                      );
                                      await load();
                                    } catch (err) {
                                      setError(
                                        describeError(
                                          err,
                                          t(
                                            "Could not resolve entry.",
                                            "Klärung fehlgeschlagen.",
                                          ),
                                        ),
                                      );
                                    } finally {
                                      setBusy(false);
                                    }
                                  }}
                                >
                                  {t("Mark sent", "Als gesendet")}
                                </Action>
                                <Action
                                  size="compact"
                                  disabled={busy}
                                  onClick={async () => {
                                    setBusy(true);
                                    setError("");
                                    try {
                                      await resolveUnsubscribe(
                                        accountId,
                                        entry.id,
                                        "not_sent",
                                        asOf,
                                      );
                                      setMessage(
                                        t(
                                          "Marked as not sent.",
                                          "Als nicht gesendet vermerkt.",
                                        ),
                                      );
                                      await load();
                                    } catch (err) {
                                      setError(
                                        describeError(
                                          err,
                                          t(
                                            "Could not resolve entry.",
                                            "Klärung fehlgeschlagen.",
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
            {preview.previews
              .flatMap((p) => p.actions)
              .map((act, i) => (
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
                    {act.copy_card?.to && (
                      <p className="help">
                        {t("To", "An")}: <code>{act.copy_card.to}</code> ·{" "}
                        {t("From", "Von")}: <code>{act.copy_card.from}</code>
                      </p>
                    )}
                  </div>
                </div>
              ))}
            <Details
              title={t("Exact confirmation summary", "Bestätigungsdetails")}
            >
              <p>{preview.confirmation.display}</p>
            </Details>
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "I reviewed this exact list of separate subscription actions and confirm applying them.",
                "Ich habe diese genaue Liste einzelner Abo-Aktionen geprüft und bestätige deren Ausführung.",
              )}
            </CheckField>
            {preview.local_mail_confirmation && (
              <CheckField checked={mailConfirmed} onChange={setMailConfirmed}>
                {t(
                  "I confirm sending the listed mailto: unsubscribe messages by SMTP from the displayed receiving addresses. Sent mail cannot be undone.",
                  "Ich bestätige den SMTP-Versand der aufgeführten mailto:-Abmeldungen von den angezeigten Empfängeradressen. Gesendete E-Mails lassen sich nicht zurücknehmen.",
                )}
              </CheckField>
            )}
            <div className="dialog-actions">
              <Action onClick={() => setPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              {previewHasExternal && !liveWrites && !view?.synthetic ? (
                <Action tone="warning" onClick={onOpenWriteMode}>
                  <ShieldAlert size={15} />
                  {t(
                    "Enable live writes to execute",
                    "Live-Schreibzugriff aktivieren",
                  )}
                </Action>
              ) : (
                <Action
                  tone={previewHasExternal ? "warning" : "primary"}
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
              )}
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
