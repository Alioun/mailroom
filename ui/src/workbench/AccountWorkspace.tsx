import { useCallback, useEffect, useState } from "react";
import { RefreshCw, Upload } from "lucide-react";
import { openAccount, openWorkspace, readReport } from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  CensusReport,
  WorkspaceView,
} from "../api";
import { Action, Empty, Notice, useCopy } from "./components";
import { Overview, Senders } from "./Findings";
import { Assistant } from "./Assistant";
import { Rules } from "./Rules";
import { Backlog } from "./Backlog";
import { Invoices } from "./Invoices";
import { Subscriptions } from "./Subscriptions";
import { AddressesWinddown } from "./AddressesWinddown";
import { AccountSettings } from "./Settings";

export type Page =
  | "overview"
  | "senders"
  | "recommendations"
  | "rules"
  | "backlog"
  | "invoices"
  | "subscriptions"
  | "addresses"
  | "settings";

export function AccountArea({
  account,
  accounts,
  status,
  hidden,
  page,
  refresh,
  running,
  onNavigate,
  onCount,
  onStatus,
  onRefresh,
  onOpenWriteMode,
  onScan,
}: {
  account: Account;
  accounts: Account[];
  status: AccountStatus;
  hidden: boolean;
  page: Page;
  refresh: number;
  running: boolean;
  onNavigate: (page: Page) => void;
  onCount: (count: number) => void;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
  onOpenWriteMode: () => void;
  onScan: () => void;
}) {
  const t = useCopy();
  const [report, setReport] = useState<CensusReport | null>(null);
  const [view, setView] = useState<AccountView | null>(null);
  const [workspace, setWorkspace] = useState<WorkspaceView | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);

  const fetchSaved = useCallback(async () => {
    setLoading(true);
    setError("");
    const results = await Promise.allSettled([
      readReport(account.id, Math.floor(Date.now() / 1000)),
      openAccount(account.id),
      openWorkspace(account.id),
    ]);
    const first = results[0];
    const second = results[1];
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
          <span>{error}</span>
          <Action
            size="compact"
            disabled={running}
            onClick={() => void fetchSaved()}
          >
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
              title={
                running
                  ? t("Scanning your mailbox…", "Dein Postfach wird erfasst…")
                  : report.complete
                    ? t(
                        "Scan complete — 0 messages found",
                        "Erfassung abgeschlossen — 0 Nachrichten gefunden",
                      )
                    : t(
                        "Ready to understand your mailbox",
                        "Bereit für einen Überblick",
                      )
              }
              action={
                <div className="actions" style={{ justifyContent: "center" }}>
                  <Action primary disabled={running} onClick={onScan}>
                    <RefreshCw size={15} />
                    {t("Scan mailbox", "Postfach erfassen")}
                  </Action>
                  <Action onClick={() => onNavigate("settings")}>
                    <Upload size={15} />
                    {t("Import Takeout (.mbox)", "Takeout (.mbox) importieren")}
                  </Action>
                </div>
              }
            >
              {t(
                "Start a read-only mailbox scan or import a Google Takeout .mbox file in Settings. Your saved findings will appear here automatically.",
                "Starte eine schreibgeschützte Postfacherfassung oder importiere eine Google-Takeout-.mbox-Datei in den Einstellungen.",
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
            action={
              !loading ? (
                <Action primary disabled={running} onClick={onScan}>
                  <RefreshCw size={15} />
                  {t("Scan mailbox", "Postfach erfassen")}
                </Action>
              ) : undefined
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
        {report && report.canonical_messages > 0 ? (
          <Senders
            report={report}
            onRecommend={() => onNavigate("recommendations")}
          />
        ) : (
          <Empty
            title={t("Scan your mailbox first", "Erfasse zuerst dein Postfach")}
            action={
              <Action primary disabled={running} onClick={onScan}>
                <RefreshCw size={15} />
                {t("Scan mailbox", "Postfach erfassen")}
              </Action>
            }
          >
            {t(
              "Senders and bulk volume appear here after your first scan or Takeout import.",
              "Absender und Volumen erscheinen hier nach der ersten Erfassung oder dem Takeout-Import.",
            )}
          </Empty>
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
          onGoSettings={() => onNavigate("settings")}
          onGoRules={() => onNavigate("rules")}
        />
      </section>
      <section
        hidden={page !== "rules"}
        aria-label={t("Rules & deploy", "Regeln & Bereitstellung")}
      >
        <Rules
          account={account}
          view={view}
          workspace={workspace}
          liveWrites={status.live_writes}
          liveJob={status.job}
          onWorkspace={setWorkspace}
          onOpenWriteMode={onOpenWriteMode}
          onGoBacklog={() => onNavigate("backlog")}
          onGoAddresses={() => onNavigate("addresses")}
        />
      </section>
      <section
        hidden={page !== "backlog"}
        aria-label={t("Backlog", "Bestand sortieren")}
      >
        {page === "backlog" && (
          <Backlog
            account={account}
            liveWrites={status.live_writes}
            liveJob={status.job}
            onOpenWriteMode={onOpenWriteMode}
          />
        )}
      </section>
      <section
        hidden={page !== "invoices"}
        aria-label={t("Invoices", "Rechnungen")}
      >
        {page === "invoices" && (
          <Invoices
            account={account}
            liveWrites={status.live_writes}
            onOpenWriteMode={onOpenWriteMode}
          />
        )}
      </section>
      <section
        hidden={page !== "subscriptions"}
        aria-label={t("Subscriptions", "Abonnements")}
      >
        {page === "subscriptions" && (
          <Subscriptions
            account={account}
            accounts={accounts}
            liveWrites={status.live_writes}
            onOpenWriteMode={onOpenWriteMode}
          />
        )}
      </section>
      <section
        hidden={page !== "addresses"}
        aria-label={t("Addresses & wind-down", "Adressen & Abbau")}
      >
        {page === "addresses" && <AddressesWinddown account={account} />}
      </section>
      <section
        hidden={page !== "settings"}
        aria-label={t("Settings", "Einstellungen")}
      >
        <AccountSettings
          account={account}
          accounts={accounts}
          status={status}
          view={view}
          onView={(value) => {
            setView(value);
            void fetchSaved();
          }}
          onStatus={onStatus}
          onRefresh={() => {
            onRefresh();
            void fetchSaved();
          }}
          onOpenWriteMode={onOpenWriteMode}
        />
      </section>
    </div>
  );
}
