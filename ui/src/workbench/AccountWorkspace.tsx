import { Suspense, lazy, useCallback, useEffect, useState } from "react";
import { RefreshCw, Upload } from "lucide-react";
import { openAccount, openWorkspace, readReport } from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  CensusReport,
  WorkspaceView,
} from "../api";
import { Action, Busy, Empty, Notice, useCopy } from "./components";
import { Overview, Senders } from "./Findings";
import type { SenderFocusScope } from "./Findings";

const ActivityOverview = lazy(() =>
  import("./ActivityOverview").then((m) => ({ default: m.ActivityOverview })),
);
const Assistant = lazy(() =>
  import("./Assistant").then((m) => ({ default: m.Assistant })),
);
const Rules = lazy(() =>
  import("./Rules").then((m) => ({ default: m.Rules })),
);
const Backlog = lazy(() =>
  import("./Backlog").then((m) => ({ default: m.Backlog })),
);
const Invoices = lazy(() =>
  import("./Invoices").then((m) => ({ default: m.Invoices })),
);
const Subscriptions = lazy(() =>
  import("./Subscriptions").then((m) => ({ default: m.Subscriptions })),
);
const AddressesWinddown = lazy(() =>
  import("./AddressesWinddown").then((m) => ({ default: m.AddressesWinddown })),
);
const AccountSettings = lazy(() =>
  import("./Settings").then((m) => ({ default: m.AccountSettings })),
);

export type Page =
  | "overview"
  | "senders"
  | "recommendations"
  | "rules"
  | "backlog"
  | "invoices"
  | "subscriptions"
  | "addresses"
  | "activity"
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
  onSelectAccountPage,
  onCount,
  onStatus,
  onRefresh,
  onOpenWriteMode,
  onScan,
  shortcutsEnabled,
  onShortcutsEnabled,
}: {
  account: Account;
  accounts: Account[];
  status: AccountStatus;
  hidden: boolean;
  page: Page;
  refresh: number;
  running: boolean;
  onNavigate: (page: Page) => void;
  onSelectAccountPage: (accountId: string, page: Page) => void;
  onCount: (count: number) => void;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
  onOpenWriteMode: () => void;
  onScan: () => void;
  shortcutsEnabled?: boolean;
  onShortcutsEnabled?: (next: boolean) => void;
}) {
  const t = useCopy();
  const [report, setReport] = useState<CensusReport | null>(null);
  const [view, setView] = useState<AccountView | null>(null);
  const [workspace, setWorkspace] = useState<WorkspaceView | null>(null);
  const [senderScope, setSenderScope] = useState<SenderFocusScope | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [visited, setVisited] = useState<Set<Page>>(() => new Set([page]));

  useEffect(() => {
    setVisited((prev) => {
      if (prev.has(page)) return prev;
      const next = new Set(prev);
      next.add(page);
      return next;
    });
  }, [page]);

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

  const fallback = (
    <div className="lazy-fallback">
      <Busy>{t("Loading view…", "Ansicht wird geladen…")}</Busy>
    </div>
  );

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
        <div className="page-stack">
          {accounts.length > 1 && page === "overview" && (
            <Suspense fallback={null}>
              <ActivityOverview
                selectedAccount={account.id}
                refresh={refresh}
                compact
                onSelectAccountPage={onSelectAccountPage}
                onOpenFullActivity={() => onNavigate("activity")}
              />
            </Suspense>
          )}
          {report ? (
            report.canonical_messages > 0 ? (
              <Overview
                report={report}
                onSenders={() => onNavigate("senders")}
                onRecommend={() => {
                  setSenderScope(null);
                  onNavigate("recommendations");
                }}
              />
            ) : (
              <Empty
                title={
                  running
                    ? t("Scanning your mailbox…", "Dein Postfach wird erfasst…")
                    : report.complete
                      ? t(
                          "Scan complete: 0 messages found",
                          "Erfassung abgeschlossen: 0 Nachrichten gefunden",
                        )
                      : t(
                          "Ready to understand your mailbox",
                          "Bereit für einen Überblick",
                        )
                }
                action={
                  <div className="actions actions-center">
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
        </div>
      </section>
      <section
        hidden={page !== "senders"}
        aria-label={t("Senders", "Absender")}
      >
        {report && report.canonical_messages > 0 ? (
          <Senders
            report={report}
            onRecommend={() => {
              setSenderScope(null);
              onNavigate("recommendations");
            }}
            onRecommendSenders={(scope) => {
              setSenderScope(scope);
              onNavigate("recommendations");
            }}
            onUnsubscribeSenders={(scope) => {
              setSenderScope(scope);
              onNavigate("subscriptions");
            }}
            onGoRules={() => onNavigate("rules")}
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
        <Suspense fallback={fallback}>
          <Assistant
            account={account.id}
            active={!hidden && page === "recommendations"}
            report={report}
            workspace={workspace}
            focusedSenders={senderScope}
            onClearFocusedSenders={() => setSenderScope(null)}
            onWorkspace={setWorkspace}
            onInbox={onCount}
            onGoSettings={() => onNavigate("settings")}
            onGoRules={() => onNavigate("rules")}
          />
        </Suspense>
      </section>
      <section
        hidden={page !== "rules"}
        aria-label={t("Rules & deploy", "Regeln & Bereitstellung")}
      >
        {visited.has("rules") && (
          <Suspense fallback={fallback}>
            <Rules
              account={account}
              view={view}
              workspace={workspace}
              liveWrites={status.live_writes}
              liveJob={status.job}
              synthetic={status.synthetic}
              onWorkspace={setWorkspace}
              onOpenWriteMode={onOpenWriteMode}
              onGoBacklog={() => onNavigate("backlog")}
              onGoAddresses={() => onNavigate("addresses")}
            />
          </Suspense>
        )}
      </section>
      <section
        hidden={page !== "backlog"}
        aria-label={t("Backlog", "Bestand sortieren")}
      >
        {page === "backlog" && (
          <Suspense fallback={fallback}>
            <Backlog
              account={account}
              liveWrites={status.live_writes}
              liveJob={status.job}
              hasLocalRules={Boolean(workspace?.workspace)}
              synthetic={status.synthetic}
              onOpenWriteMode={onOpenWriteMode}
              onGoRules={() => onNavigate("rules")}
            />
          </Suspense>
        )}
      </section>
      <section
        hidden={page !== "invoices"}
        aria-label={t("Invoices", "Rechnungen")}
      >
        {page === "invoices" && (
          <Suspense fallback={fallback}>
            <Invoices
              account={account}
              liveWrites={status.live_writes}
              hasLocalRules={workspace ? Boolean(workspace.workspace) : undefined}
              onOpenWriteMode={onOpenWriteMode}
              onGoRules={() => onNavigate("rules")}
            />
          </Suspense>
        )}
      </section>
      <section
        hidden={page !== "subscriptions"}
        aria-label={t("Subscriptions", "Abonnements")}
      >
        {page === "subscriptions" && (
          <Suspense fallback={fallback}>
            <Subscriptions
              account={account}
              accounts={accounts}
              liveWrites={status.live_writes}
              focusedSenders={senderScope}
              onClearFocusedSenders={() => setSenderScope(null)}
              onOpenWriteMode={onOpenWriteMode}
            />
          </Suspense>
        )}
      </section>
      <section
        hidden={page !== "addresses"}
        aria-label={t("Addresses & wind-down", "Adressen & Abbau")}
      >
        {page === "addresses" && (
          <Suspense fallback={fallback}>
            <AddressesWinddown
              account={account}
              hasLocalRules={workspace ? Boolean(workspace.workspace) : undefined}
              onGoRules={() => onNavigate("rules")}
            />
          </Suspense>
        )}
      </section>
      <section
        hidden={page !== "settings"}
        aria-label={t("Settings", "Einstellungen")}
      >
        {visited.has("settings") && (
          <Suspense fallback={fallback}>
            <AccountSettings
              account={account}
              accounts={accounts}
              status={status}
              view={view}
              shortcutsEnabled={shortcutsEnabled}
              onShortcutsEnabled={onShortcutsEnabled}
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
          </Suspense>
        )}
      </section>
    </div>
  );
}
