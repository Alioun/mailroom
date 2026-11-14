import { useCallback, useEffect, useState } from "react";
import { ArrowUpRight, CheckCircle2 } from "lucide-react";
import {
  confirmAccount,
  openAccount,
  openWorkspace,
  readReport,
  reconnectAccount,
} from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  CensusReport,
  WorkspaceView,
} from "../api";
import { Action, Details, Empty, Notice, useCopy } from "./components";
import { Overview, Senders } from "./Findings";
import { Assistant } from "./Assistant";
import { Rules } from "./Rules";

export type Page =
  "overview" | "senders" | "recommendations" | "rules" | "settings";
export function AccountArea({
  account,
  hidden,
  page,
  refresh,
  running,
  onNavigate,
  onCount,
  onStatus,
}: {
  account: Account;
  hidden: boolean;
  page: Page;
  refresh: number;
  running: boolean;
  onNavigate: (page: Page) => void;
  onCount: (count: number) => void;
  onStatus: (value: AccountStatus) => void;
}) {
  const t = useCopy(),
    [report, setReport] = useState<CensusReport | null>(null),
    [view, setView] = useState<AccountView | null>(null),
    [workspace, setWorkspace] = useState<WorkspaceView | null>(null),
    [error, setError] = useState(""),
    [loading, setLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const fetchSaved = useCallback(async () => {
    setLoading(true);
    setError("");
    const results = await Promise.allSettled([
      readReport(account.id, Math.floor(Date.now() / 1000)),
      openAccount(account.id),
      openWorkspace(account.id),
    ]);
    const first = results[0],
      second = results[1];
    if (first.status === "fulfilled") setReport(first.value);
    else
      setError(
        t(
          "Saved findings could not load. Retry after any running operation finishes.",
          "Gespeicherte Ergebnisse konnten nicht geladen werden. Versuche es nach Abschluss laufender Aktionen erneut.",
        ),
      );
    if (second.status === "fulfilled") setView(second.value);
    if (results[2].status === "fulfilled") setWorkspace(results[2].value);
    setLoading(false);
  }, [account.id]);
  useEffect(() => {
    if (hidden && !loaded) return;
    setLoaded(true);
    if (!running) void fetchSaved();
  }, [hidden, refresh, running, fetchSaved]);
  if (!loaded) return null;
  return (
    <div hidden={hidden}>
      {error && (
        <Notice error>
          {error}
          <Action disabled={running} onClick={() => void fetchSaved()}>
            {t("Retry", "Erneut versuchen")}
          </Action>
        </Notice>
      )}
      <section
        hidden={page !== "overview"}
        aria-label={t("Overview", "Übersicht")}
      >
        {report ? (
          report.canonical_messages > 0 ? (
            <Overview
              report={report}
              onSenders={() => onNavigate("senders")}
              onRecommend={() => onNavigate("recommendations")}
            />
          ) : (
            <Empty
              title={t(
                "Ready to understand your mailbox",
                "Bereit für einen Überblick",
              )}
            >
              {t(
                "Start a scan above. Your saved findings will appear here automatically.",
                "Starte oben eine Erfassung. Deine gespeicherten Ergebnisse erscheinen hier automatisch.",
              )}
            </Empty>
          )
        ) : (
          <Empty
            title={
              loading
                ? t(
                    "Loading saved findings…",
                    "Gespeicherte Ergebnisse werden geladen…",
                  )
                : t("No findings yet", "Noch keine Ergebnisse")
            }
          >
            {t(
              "Run a mailbox scan to get started.",
              "Starte eine Postfacherfassung.",
            )}
          </Empty>
        )}
      </section>
      <section
        hidden={page !== "senders"}
        aria-label={t("Senders", "Absender")}
      >
        {report ? (
          <Senders
            report={report}
            onRecommend={() => onNavigate("recommendations")}
          />
        ) : (
          <Empty
            title={t("Scan your mailbox first", "Erfasse zuerst dein Postfach")}
          />
        )}
      </section>
      <section
        hidden={page !== "recommendations"}
        aria-label={t("Recommendations", "Vorschläge")}
      >
        <Assistant
          account={account.id}
          report={report}
          workspace={workspace}
          onWorkspace={setWorkspace}
          onInbox={onCount}
        />
      </section>
      <section
        hidden={page !== "rules"}
        aria-label={t("Local rules", "Lokale Regeln")}
      >
        <Rules
          account={account.id}
          workspace={workspace}
          onWorkspace={setWorkspace}
        />
      </section>
      <section
        hidden={page !== "settings"}
        aria-label={t("Settings", "Einstellungen")}
      >
        <AccountSettings
          account={account}
          view={view}
          onView={(value) => {
            setView(value);
            void fetchSaved();
          }}
          onStatus={onStatus}
        />
      </section>
    </div>
  );
}
function AccountSettings({
  account,
  view,
  onView,
  onStatus,
}: {
  account: Account;
  view: AccountView | null;
  onView: (value: AccountView) => void;
  onStatus: (value: AccountStatus) => void;
}) {
  const t = useCopy(),
    [password, setPassword] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  async function reconnect() {
    setBusy(true);
    setError("");
    const entered = password;
    setPassword("");
    try {
      onStatus(await reconnectAccount(account.id, entered || undefined));
    } catch {
      setError(
        t(
          "Connection could not start. Check the app password and retry.",
          "Die Verbindung konnte nicht starten. Prüfe das App-Passwort und versuche es erneut.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="page-stack">
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Account connection", "Kontoverbindung")}</h2>
            <p className="muted">
              {account.login} ·{" "}
              {account.provider === "gmail" ? "Gmail" : "mailbox.org"}
            </p>
          </div>
          <span
            className={`badge ${account.connection === "failed" ? "warning" : ""}`}
          >
            {account.connection === "observed"
              ? t("Connection verified", "Verbindung geprüft")
              : account.connection === "failed"
                ? t("Reconnect needed", "Erneute Verbindung nötig")
                : t("Not yet verified", "Noch nicht geprüft")}
          </span>
        </div>
        <form
          className="settings-form"
          onSubmit={(e) => {
            e.preventDefault();
            void reconnect();
          }}
        >
          <label className="field">
            {t("Replacement app password", "Neues App-Passwort")}
            <input
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={t(
                "Leave empty to retry saved password",
                "Leer lassen, um gespeichertes Passwort zu verwenden",
              )}
            />
          </label>
          <Action primary type="submit" disabled={busy}>
            {t("Reconnect", "Erneut verbinden")}
          </Action>
        </form>
        {error && <Notice error>{error}</Notice>}
      </section>
      <section className="panel">
        <h2>{t("Your addresses", "Deine Adressen")}</h2>
        <p className="muted">
          {t(
            "Confirm only addresses you own. Recipient observations can include other people.",
            "Bestätige nur Adressen, die dir gehören. Empfängerbeobachtungen können andere Personen enthalten.",
          )}
        </p>
        <Details
          title={`${t("Review address candidates", "Adresskandidaten prüfen")} · ${view?.addresses.length ?? 0}`}
        >
          <div className="address-list">
            {view?.addresses.map((address) => (
              <div className="address-row" key={address.address}>
                <span>{address.address}</span>
                <Action
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void confirmAccount(
                      account.id,
                      address.address,
                      !address.confirmed,
                    )
                      .then(onView)
                      .catch(() =>
                        setError(
                          t(
                            "Address confirmation failed. Try again.",
                            "Adressbestätigung fehlgeschlagen. Versuche es erneut.",
                          ),
                        ),
                      )
                      .finally(() => setBusy(false));
                  }}
                >
                  {address.confirmed ? (
                    <>
                      <CheckCircle2 size={15} />
                      {t("Confirmed · undo", "Bestätigt · zurücknehmen")}
                    </>
                  ) : (
                    t("Confirm ownership", "Eigentum bestätigen")
                  )}
                </Action>
              </div>
            ))}
          </div>
        </Details>
      </section>
      <section className="panel advanced-link">
        <div>
          <h2>{t("Advanced tools", "Erweiterte Werkzeuge")}</h2>
          <p className="muted">
            {t(
              "Invoices, subscriptions, recovery, provider diagnostics, and detailed rule tools.",
              "Rechnungen, Abonnements, Wiederherstellung, Anbieterdiagnose und detaillierte Regelwerkzeuge.",
            )}
          </p>
        </div>
        <a className="button" href="/advanced.html">
          {t("Open advanced tools", "Erweiterte Werkzeuge öffnen")}
          <ArrowUpRight size={16} />
        </a>
      </section>
      <section className="panel">
        <h2>{t("About this workspace", "Über diesen Arbeitsbereich")}</h2>
        <p className="muted">
          {t(
            "Accounts and saved mail headers are stored locally. Programs running under your Mac user can access the open local session. Other websites cannot.",
            "Konten und gespeicherte Kopfzeilen liegen lokal. Programme unter deinem Mac-Benutzer können auf die offene lokale Sitzung zugreifen. Andere Websites können das nicht.",
          )}
        </p>
        <p className="help">
          {t(
            "Provider changes stay disabled. Local rule saves never deploy rules, send mail, unsubscribe, or move messages.",
            "Änderungen beim Anbieter bleiben deaktiviert. Lokales Speichern aktiviert keine Regeln, sendet keine Nachrichten, meldet keine Abonnements ab und verschiebt keine Nachrichten.",
          )}
        </p>
      </section>
    </div>
  );
}
