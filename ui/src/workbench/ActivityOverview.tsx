import { useCallback, useEffect, useState } from "react";
import { ArrowUpRight, Clock, History, RefreshCw } from "lucide-react";
import { readAccountsOverview } from "../api";
import type { AccountsOverview, RecentAction } from "../api";
import { Action, Empty, Notice, Stat, StatusBadge, useCopy } from "./components";
import { describeError } from "./status-labels";
import type { Page } from "./AccountWorkspace";

type ActionFilter = "all" | "undo" | "attention";

function formatShortDate(unixSecs: number): string {
  if (!unixSecs || unixSecs < 1_000_000_000) return "";
  try {
    return new Date(unixSecs * 1000).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  } catch {
    return "";
  }
}

function actionKindLabel(
  kind: RecentAction["kind"],
  t: (en: string, de: string) => string,
) {
  switch (kind) {
    case "backlog":
      return t("Backlog", "Bestand");
    case "sieve":
      return t("Sieve", "Sieve");
    case "invoice":
      return t("Invoice", "Rechnung");
    case "unsubscribe":
      return t("Subscription", "Abonnement");
    case "folders":
      return t("Folders", "Ordner");
  }
}

export function ActivityOverview({
  selectedAccount,
  refresh,
  compact = false,
  onSelectAccountPage,
  onOpenFullActivity,
}: {
  selectedAccount: string;
  refresh: number;
  compact?: boolean;
  onSelectAccountPage: (accountId: string, page: Page) => void;
  onOpenFullActivity?: () => void;
}) {
  const t = useCopy();
  const [data, setData] = useState<AccountsOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<ActionFilter>("all");

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      setData(await readAccountsOverview());
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not load workspace overview.",
            "Arbeitsbereich-Übersicht konnte nicht geladen werden.",
          ),
        ),
      );
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void load();
  }, [load, refresh]);

  if (compact) {
    if (!data || data.accounts.length <= 1) return null;
    const totalUndo = data.accounts.reduce(
      (sum, row) => sum + row.active_undo_windows,
      0,
    );
    const totalUncertain = data.accounts.reduce(
      (sum, row) => sum + row.uncertain_actions,
      0,
    );
    return (
      <section className="panel" aria-label={t("All accounts", "Alle Konten")}>
        <div className="panel-head">
          <div>
            <h2>{t("All connected accounts", "Alle verbundenen Konten")}</h2>
            <p className="muted">
              {t(
                `${data.accounts.length} accounts · ${totalUndo} open undo window(s) · ${totalUncertain} item(s) needing review`,
                `${data.accounts.length} Konten · ${totalUndo} offene Rücknahmefenster · ${totalUncertain} Einträge zur Prüfung`,
              )}
            </p>
          </div>
          {onOpenFullActivity && (
            <Action size="compact" onClick={onOpenFullActivity}>
              <History size={14} />
              {t("Activity & undo", "Aktivität & Rücknahme")}
            </Action>
          )}
        </div>
        <div className="overview-grid">
          {data.accounts.map((row) => (
            <button
              key={row.id}
              type="button"
              className={`account-health-card ${row.id === selectedAccount ? "active" : ""}`}
              onClick={() => onSelectAccountPage(row.id, "overview")}
            >
              <div className="account-health-head">
                <strong>{row.login}</strong>
                <StatusBadge status={row.scan_state} />
              </div>
              <div className="account-health-meta">
                <span>
                  {row.provider === "gmail" ? "Gmail" : "mailbox.org"}
                </span>
                <span>
                  {row.saved_messages.toLocaleString()}{" "}
                  {t("messages", "Nachrichten")}
                </span>
                {row.active_undo_windows > 0 && (
                  <span className="badge info">
                    {row.active_undo_windows} {t("undo open", "Rücknahme")}
                  </span>
                )}
                {row.uncertain_actions > 0 && (
                  <span className="badge warning">
                    {row.uncertain_actions} {t("uncertain", "unklar")}
                  </span>
                )}
                {row.recovery_pending && (
                  <span className="badge warning">
                    {t("recovery pending", "Wiederherstellung offen")}
                  </span>
                )}
              </div>
            </button>
          ))}
        </div>
      </section>
    );
  }

  const totalMessages =
    data?.accounts.reduce((sum, row) => sum + row.saved_messages, 0) ?? 0;
  const totalUndo =
    data?.accounts.reduce((sum, row) => sum + row.active_undo_windows, 0) ?? 0;
  const totalUncertain =
    data?.accounts.reduce((sum, row) => sum + row.uncertain_actions, 0) ?? 0;

  const filteredActions = (data?.recent_actions ?? []).filter((item) => {
    if (filter === "undo") return item.undo_open;
    if (filter === "attention") {
      return (
        item.status === "uncertain" ||
        item.status === "conflict" ||
        item.status === "failed" ||
        item.status === "intent"
      );
    }
    return true;
  });

  return (
    <div className="page-stack">
      {error && (
        <Notice error>
          <span>{error}</span>
          <Action size="compact" onClick={() => void load()}>
            {t("Retry", "Erneut versuchen")}
          </Action>
        </Notice>
      )}
      <div className="stats">
        <Stat
          label={t("Connected accounts", "Verbundene Konten")}
          value={data?.accounts.length ?? 0}
        />
        <Stat
          label={t("Saved messages", "Gespeicherte Nachrichten")}
          value={totalMessages.toLocaleString()}
        />
        <Stat
          label={t("Open undo windows", "Offene Rücknahmefenster")}
          value={totalUndo}
          detail={t("90-day retention", "90 Tage Aufbewahrung")}
        />
        <Stat
          label={t("Needs attention", "Prüfung erforderlich")}
          value={totalUncertain}
        />
      </div>

      {data && data.accounts.length > 0 && (
        <section className="panel">
          <div className="panel-head">
            <div>
              <h2>{t("Account health", "Kontostatus")}</h2>
              <p className="muted">
                {t(
                  "Click any account card to jump straight to its overview.",
                  "Klicke auf eine Kontokarte, um direkt zu deren Übersicht zu wechseln.",
                )}
              </p>
            </div>
            <Action
              size="compact"
              disabled={loading}
              onClick={() => void load()}
            >
              <RefreshCw size={14} />
              {t("Refresh", "Aktualisieren")}
            </Action>
          </div>
          <div className="overview-grid">
            {data.accounts.map((row) => (
              <button
                key={row.id}
                type="button"
                className={`account-health-card ${row.id === selectedAccount ? "active" : ""}`}
                onClick={() => onSelectAccountPage(row.id, "overview")}
              >
                <div className="account-health-head">
                  <strong>{row.login}</strong>
                  <StatusBadge status={row.scan_state} />
                </div>
                <div className="account-health-meta">
                  <span>
                    {row.provider === "gmail" ? "Gmail" : "mailbox.org"}
                  </span>
                  <span>
                    {row.saved_messages.toLocaleString()}{" "}
                    {t("messages", "Nachrichten")}
                  </span>
                  <span>
                    {row.folders} {t("folders", "Ordner")}
                  </span>
                  {row.sieve_deployed && (
                    <span className="badge success">
                      {t("Sieve deployed", "Sieve aktiv")}
                    </span>
                  )}
                  {row.active_undo_windows > 0 && (
                    <span className="badge info">
                      <Clock size={12} /> {row.active_undo_windows}{" "}
                      {t("undo open", "Rücknahme offen")}
                    </span>
                  )}
                  {row.uncertain_actions > 0 && (
                    <span className="badge warning">
                      {row.uncertain_actions}{" "}
                      {t("needs review", "Prüfung nötig")}
                    </span>
                  )}
                </div>
              </button>
            ))}
          </div>
        </section>
      )}

      <section className="panel">
        <div className="panel-head">
          <div>
            <h2>{t("Recent actions & undo trail", "Letzte Aktionen & Rücknahme")}</h2>
            <p className="muted">
              {t(
                "Persisted local audit trail across backlog runs, Sieve deploys, folder provisioning, invoice sends, and subscription actions.",
                "Gespeichertes lokales Protokoll über Bestandssortierungen, Sieve-Deployments, Ordneranlage, Rechnungsversand und Abmeldungen.",
              )}
            </p>
          </div>
          <div className="filter-chips" role="group" aria-label={t("Filter actions", "Aktionen filtern")}>
            {(
              [
                ["all", t("All", "Alle")],
                ["undo", t("Open undo", "Rücknahme offen")],
                ["attention", t("Needs attention", "Prüfung nötig")],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                className={`filter-chip ${filter === key ? "active" : ""}`}
                aria-pressed={filter === key}
                onClick={() => setFilter(key)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {loading && !data ? (
          <Empty
            headingLevel={3}
            title={t("Loading activity…", "Aktivitäten werden geladen…")}
          />
        ) : filteredActions.length === 0 ? (
          <Empty
            headingLevel={3}
            title={t("No matching actions yet", "Noch keine passenden Aktionen")}
          >
            {t(
              "Confirmed backlog runs, Sieve deployments, folder creations, invoice sends, and subscription actions will appear here with direct links to undo or resolve them.",
              "Bestätigte Bestandssortierungen, Sieve-Deployments, Ordneranlagen, Rechnungsversände und Abmeldungen erscheinen hier mit direkten Links zur Rücknahme oder Klärung.",
            )}
          </Empty>
        ) : (
          <div className="activity-list">
            {filteredActions.map((item) => (
              <div className="activity-row" key={item.id}>
                <div className="activity-row-main">
                  <div className="activity-row-meta">
                    <span className="badge subtle">
                      {actionKindLabel(item.kind, t)}
                    </span>
                    <StatusBadge status={item.status} />
                    <strong>{item.account_login}</strong>
                    {item.timestamp !== null && (
                      <span>{formatShortDate(item.timestamp)}</span>
                    )}
                  </div>
                  <span>{item.summary}</span>
                  {item.undo_open && item.undo_until && (
                    <small className="muted">
                      <Clock size={12} />{" "}
                      {t(
                        `Undo window open until ${formatShortDate(item.undo_until)}`,
                        `Rücknahme möglich bis ${formatShortDate(item.undo_until)}`,
                      )}
                    </small>
                  )}
                </div>
                <Action
                  size="compact"
                  onClick={() => onSelectAccountPage(item.account, item.page)}
                >
                  {t("Open", "Öffnen")}
                  <ArrowUpRight size={14} />
                </Action>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
