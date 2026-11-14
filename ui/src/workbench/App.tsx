import { useCallback, useEffect, useRef, useState } from "react";
import {
  ChevronRight,
  LayoutDashboard,
  LockKeyhole,
  Mail,
  Menu,
  Plus,
  RefreshCw,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Users,
} from "lucide-react";
import {
  cancelAccount,
  finishInterrupted,
  listAccounts,
  readAccountStatus,
  scanAccount,
} from "../api";
import type { Account, AccountStatus } from "../api";
import {
  LanguageContext,
  Action,
  Busy,
  Choice,
  Empty,
  Modal,
  Notice,
  useCopy,
} from "./components";
import type { Language } from "./components";
import { ConnectForm, VaultForm } from "./Setup";
import { AccountArea } from "./AccountWorkspace";
import type { Page } from "./AccountWorkspace";
import { NavigationDrawer } from "./NavigationDrawer";

const pages: Page[] = [
  "overview",
  "senders",
  "recommendations",
  "rules",
  "settings",
];
function stored(key: string) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}
function initialPage(): Page {
  const value = new URLSearchParams(location.search).get("view");
  return pages.find((page) => page === value) ?? "overview";
}
function pendingAutoScan() {
  try {
    return sessionStorage.getItem("mailroom.autoscan") === "waiting";
  } catch {
    return false;
  }
}
function autoScan(value: boolean) {
  try {
    value
      ? sessionStorage.setItem("mailroom.autoscan", "waiting")
      : sessionStorage.removeItem("mailroom.autoscan");
  } catch {}
}
export function App() {
  const [language, setLanguage] = useState<Language>(
    stored("mailroom.language") === "de" ? "de" : "en",
  );
  useEffect(() => {
    document.documentElement.lang = language;
    remember("mailroom.language", language);
  }, [language]);
  return (
    <LanguageContext value={language}>
      <Workbench language={language} onLanguage={setLanguage} />
    </LanguageContext>
  );
}
function Workbench({
  language,
  onLanguage,
}: {
  language: Language;
  onLanguage: (value: Language) => void;
}) {
  const t = useCopy(),
    [status, setStatus] = useState<AccountStatus | null>(null),
    [accounts, setAccounts] = useState<Account[]>([]),
    [selected, setSelected] = useState(stored("mailroom.account") ?? "");
  const [page, setPage] = useState<Page>(initialPage),
    [add, setAdd] = useState(false),
    [menu, setMenu] = useState(false),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [refresh, setRefresh] = useState(0),
    [pending, setPending] = useState(false),
    [counts, setCounts] = useState<Record<string, number>>({});
  const heading = useRef<HTMLHeadingElement>(null),
    polling = useRef(false),
    lastJob = useRef<string | null>(null),
    wasUsable = useRef(false),
    menuTrigger = useRef<HTMLButtonElement>(null);
  const account = accounts.find((value) => value.id === selected);
  const loadAccounts = useCallback(async () => {
    const values = await listAccounts();
    setAccounts(values);
    setSelected((current) =>
      values.some((value) => value.id === current)
        ? current
        : (values[0]?.id ?? ""),
    );
    return values;
  }, []);
  const applyStatus = useCallback(
    async (next: AccountStatus) => {
      setStatus(next);
      const job = next.job;
      const signature = job
        ? `${job.account?.id}-${job.progress.state}-${job.operation ?? ""}`
        : "none";
      const usable =
        next.vault !== "locked" &&
        next.vault !== "absent" &&
        next.vault !== "unavailable";
      const becameUsable = usable && !wasUsable.current;
      wasUsable.current = usable;
      if (signature !== lastJob.current || becameUsable) {
        const previous = lastJob.current;
        lastJob.current = signature;
        if (usable) {
          await loadAccounts();
          if (previous !== null && job?.progress.state !== "running")
            setRefresh((value) => value + 1);
        }
      }
      if (
        pendingAutoScan() &&
        job?.progress.state === "complete" &&
        job.account
      ) {
        autoScan(false);
        setSelected(job.account.id);
        setPage("overview");
        setStatus(await scanAccount(job.account.id));
        setRefresh((value) => value + 1);
      } else if (pendingAutoScan() && job?.progress.state === "failed")
        autoScan(false);
    },
    [loadAccounts],
  );
  useEffect(() => {
    let active = true;
    const poll = async () => {
      if (polling.current) return;
      polling.current = true;
      try {
        const next = await readAccountStatus();
        if (active) {
          await applyStatus(next);
          setError("");
        }
      } catch {
        if (active)
          setError(
            t(
              "Mailroom is not responding. Check that the app is running, then retry.",
              "Mailroom antwortet nicht. Prüfe, ob die App läuft, und versuche es erneut.",
            ),
          );
      } finally {
        polling.current = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 1800);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [applyStatus]);
  useEffect(() => {
    if (selected) remember("mailroom.account", selected);
  }, [selected]);
  useEffect(() => {
    const query = new URLSearchParams(location.search);
    query.set("view", page);
    history.replaceState(null, "", `${location.pathname}?${query}`);
  }, [page]);
  function navigate(next: Page) {
    setPage(next);
    setMenu(false);
    requestAnimationFrame(() => heading.current?.focus());
    window.scrollTo({ top: 0 });
  }
  async function action(work: () => Promise<AccountStatus>) {
    setPending(true);
    setNotice("");
    try {
      await applyStatus(await work());
    } catch {
      setNotice(
        t(
          "The action could not start. Your saved work is retained. Try again.",
          "Die Aktion konnte nicht starten. Deine gespeicherte Arbeit bleibt erhalten. Versuche es erneut.",
        ),
      );
    } finally {
      setPending(false);
    }
  }
  const nav = [
    {
      page: "overview",
      label: t("Overview", "Übersicht"),
      icon: LayoutDashboard,
    },
    { page: "senders", label: t("Senders", "Absender"), icon: Users },
    {
      page: "recommendations",
      label: t("Recommendations", "Vorschläge"),
      icon: Sparkles,
    },
    {
      page: "rules",
      label: t("Local rules", "Lokale Regeln"),
      icon: SlidersHorizontal,
    },
  ] satisfies { page: Page; label: string; icon: typeof Mail }[];
  const currentTitle =
    nav.find((item) => item.page === page)?.label ??
    t("Settings", "Einstellungen");
  const subtitles: Record<Page, string> = {
    overview: t(
      "A clearer view of your mailbox.",
      "Ein klarer Blick auf dein Postfach.",
    ),
    senders: t(
      "Find patterns in who sends you mail.",
      "Erkenne Muster in deinen Absendern.",
    ),
    recommendations: t(
      "Useful suggestions, ready for your review.",
      "Nützliche Vorschläge, bereit für deine Prüfung.",
    ),
    rules: t(
      "Review and test how your mail could be organized.",
      "Prüfe, wie deine Nachrichten geordnet werden könnten.",
    ),
    settings: t(
      "Your account, connections, and local data.",
      "Dein Konto, Verbindungen und lokale Daten.",
    ),
  };
  const vaultGated =
    !!status &&
    (status.vault === "absent" ||
      status.vault === "locked" ||
      status.vault === "unavailable");
  return (
    <>
      <a className="skip-link" href="#main-content">
        {t("Skip to content", "Zum Inhalt springen")}
      </a>
      <div
        className={`workbench ${menu ? "menu-open" : ""} ${vaultGated ? "gated" : ""}`}
      >
        {!vaultGated && (
          <>
            <NavigationDrawer
              open={menu}
              onOpenChange={setMenu}
              triggerRef={menuTrigger}
            >
              <a href="/" className="brand">
                <span className="brand-icon">
                  <Mail size={21} />
                </span>
                mailroom<span className="brand-dot">.</span>
              </a>
              <div className="account-switcher">
                <span className="eyebrow">
                  {t("Workspace", "Arbeitsbereich")}
                </span>
                {accounts.length > 0 ? (
                  <Choice
                    label={t("Select account", "Konto auswählen")}
                    value={selected}
                    onChange={(value) => {
                      setSelected(value);
                      setMenu(false);
                    }}
                    options={accounts.map((value) => ({
                      value: value.id,
                      label: `${value.login} · ${value.provider === "gmail" ? "Gmail" : "mailbox.org"}`,
                    }))}
                  />
                ) : (
                  <span className="muted">
                    {t("No account yet", "Noch kein Konto")}
                  </span>
                )}
                <Action className="add-account" onClick={() => setAdd(true)}>
                  <Plus size={15} />
                  {t("Add account", "Konto hinzufügen")}
                </Action>
              </div>
              <nav aria-label={t("Main navigation", "Hauptnavigation")}>
                {nav.map((item) => (
                  <button
                    className={`nav-item ${page === item.page ? "active" : ""}`}
                    key={item.page}
                    aria-current={page === item.page ? "page" : undefined}
                    onClick={() => navigate(item.page)}
                  >
                    <item.icon size={19} />
                    <span>{item.label}</span>
                    {item.page === "recommendations" &&
                      (counts[selected] ?? 0) > 0 && (
                        <span className="nav-count">{counts[selected]}</span>
                      )}
                  </button>
                ))}
              </nav>
              <div className="sidebar-bottom">
                <button
                  className={`nav-item ${page === "settings" ? "active" : ""}`}
                  onClick={() => navigate("settings")}
                >
                  <Settings size={19} />
                  {t("Settings", "Einstellungen")}
                </button>
                <div className="local-status">
                  <span className="status-dot" />
                  {t("Running on this Mac", "Läuft auf diesem Mac")}
                  <small>
                    {t(
                      "Your workspace stays local",
                      "Dein Arbeitsbereich bleibt lokal",
                    )}
                  </small>
                </div>
              </div>
            </NavigationDrawer>
          </>
        )}
        <div className="main-shell">
          <header className="topbar">
            {!vaultGated && (
              <button
                className="icon-button mobile-menu"
                ref={menuTrigger}
                onClick={() => setMenu(!menu)}
                aria-label={t("Open navigation", "Navigation öffnen")}
                aria-expanded={menu}
              >
                <Menu size={21} />
              </button>
            )}
            <div className="breadcrumb">
              {vaultGated ? (
                <a href="/" className="brand compact">
                  <Mail size={22} />
                  mailroom.
                </a>
              ) : (
                <>
                  <span>Mailroom</span>
                  <ChevronRight size={14} />
                  <span>{currentTitle}</span>
                </>
              )}
            </div>
            <div className="topbar-actions">
              <span className="readonly-badge">
                <ShieldCheck size={14} />
                {t("Provider read-only", "Anbieter nur lesbar")}
              </span>
              <Choice
                label={t("Language", "Sprache")}
                value={language}
                onChange={(value) => {
                  if (value === "en" || value === "de") onLanguage(value);
                }}
                options={[
                  { value: "en", label: "English" },
                  { value: "de", label: "Deutsch" },
                ]}
              />
            </div>
          </header>
          <main
            id="main-content"
            tabIndex={-1}
            className={vaultGated ? "setup-main" : "main-content"}
          >
            {error && (
              <Notice error>
                {error}
                <Action
                  onClick={() =>
                    void readAccountStatus()
                      .then(applyStatus)
                      .catch(() => {})
                  }
                >
                  {t("Retry", "Erneut versuchen")}
                </Action>
              </Notice>
            )}
            {!status ? (
              <Empty
                title={t(
                  "Opening your workspace…",
                  "Dein Arbeitsbereich wird geöffnet…",
                )}
              />
            ) : vaultGated ? (
              <VaultForm
                status={status}
                onOpened={(next) => {
                  void applyStatus(next);
                  setRefresh((value) => value + 1);
                }}
              />
            ) : (
              <>
                <div className="page-heading">
                  <div>
                    <span className="eyebrow">
                      {account?.login ?? t("Your mailbox", "Dein Postfach")}
                    </span>
                    <h1 ref={heading} tabIndex={-1}>
                      {currentTitle}
                    </h1>
                    <p className="muted">{subtitles[page]}</p>
                  </div>
                  {account && page === "overview" && (
                    <Action
                      primary
                      disabled={
                        pending || status.job?.progress.state === "running"
                      }
                      onClick={() => void action(() => scanAccount(account.id))}
                    >
                      <RefreshCw size={16} />
                      {status.job?.progress.state === "cancelled" ||
                      status.job?.progress.state === "failed"
                        ? t("Resume scan", "Erfassung fortsetzen")
                        : t("Scan mailbox", "Postfach erfassen")}
                    </Action>
                  )}
                </div>
                {notice && <Notice error>{notice}</Notice>}
                {status.job?.progress.state === "running" && (
                  <div className="activity-strip" role="status">
                    <Busy>
                      {status.job.operation === "connect"
                        ? t("Connecting account…", "Konto wird verbunden…")
                        : status.job.operation === "scan"
                          ? t("Scanning mailbox…", "Postfach wird erfasst…")
                          : t("Working…", "In Arbeit…")}
                    </Busy>
                    <span>
                      {status.job.account?.login} · {status.job.progress.saved}{" "}
                      {t("headers saved", "Kopfzeilen gespeichert")} ·{" "}
                      {status.job.progress.folders} {t("folders", "Ordner")}
                    </span>
                    <Action
                      onClick={() => void action(cancelAccount)}
                      disabled={pending}
                    >
                      {t("Stop", "Stoppen")}
                    </Action>
                  </div>
                )}
                {status.job?.progress.state === "failed" && (
                  <Notice error>
                    {status.job.operation === "connect"
                      ? t(
                          "Connection failed. Check the app password in Settings and reconnect.",
                          "Verbindung fehlgeschlagen. Prüfe das App-Passwort in den Einstellungen und verbinde erneut.",
                        )
                      : t(
                          "The scan was interrupted. Saved progress is retained. Resume the scan when the connection is available.",
                          "Die Erfassung wurde unterbrochen. Gespeicherter Fortschritt bleibt erhalten. Setze die Erfassung fort, sobald die Verbindung verfügbar ist.",
                        )}
                    <Action
                      onClick={() => {
                        const failedAccount = status.job?.account;
                        if (failedAccount) {
                          setSelected(failedAccount.id);
                          navigate("settings");
                        } else setAdd(true);
                      }}
                    >
                      {t("Connection settings", "Verbindungseinstellungen")}
                    </Action>
                  </Notice>
                )}
                {status.interrupted.length > 0 && (
                  <Notice>
                    {t(
                      "An account setup was interrupted.",
                      "Eine Kontoeinrichtung wurde unterbrochen.",
                    )}
                    {status.interrupted.map((id) => (
                      <Action
                        key={id}
                        onClick={() => void action(() => finishInterrupted(id))}
                        disabled={pending}
                      >
                        {t("Finish setup", "Einrichtung abschließen")}
                      </Action>
                    ))}
                  </Notice>
                )}
                {accounts.length === 0 ? (
                  <section className="panel first-account">
                    <h2>
                      {t(
                        "Bring your mailbox into focus",
                        "Verschaffe dir einen Überblick",
                      )}
                    </h2>
                    <ConnectForm
                      synthetic={status.synthetic}
                      onConnected={(next) => {
                        autoScan(true);
                        void applyStatus(next);
                      }}
                    />
                  </section>
                ) : (
                  accounts.map((value) => (
                    <AccountArea
                      key={value.id}
                      account={value}
                      hidden={value.id !== selected}
                      page={page}
                      refresh={refresh}
                      running={status.job?.progress.state === "running"}
                      onNavigate={navigate}
                      onCount={(count) =>
                        setCounts((previous) =>
                          previous[value.id] === count
                            ? previous
                            : { ...previous, [value.id]: count },
                        )
                      }
                      onStatus={(next) => void applyStatus(next)}
                    />
                  ))
                )}
              </>
            )}
            {!vaultGated && (
              <footer className="workbench-footer">
                <span>
                  <LockKeyhole size={13} />
                  {t("Encrypted local vault", "Verschlüsselter lokaler Tresor")}
                </span>
                <span>
                  {t(
                    "Mailroom never changes provider mail in this mode.",
                    "Mailroom verändert in diesem Modus keine Nachrichten beim Anbieter.",
                  )}
                </span>
              </footer>
            )}
          </main>
        </div>
      </div>
      <Modal
        open={add}
        onOpenChange={setAdd}
        title={t("Add an account", "Konto hinzufügen")}
      >
        <ConnectForm
          synthetic={status?.synthetic ?? false}
          onConnected={(next) => {
            autoScan(true);
            setAdd(false);
            void applyStatus(next);
          }}
        />
      </Modal>
    </>
  );
}
