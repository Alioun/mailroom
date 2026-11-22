import { useCallback, useEffect, useRef, useState } from "react";
import {
  BookUser,
  ChevronRight,
  FileText,
  FolderSync,
  LayoutDashboard,
  Lock,
  LockKeyhole,
  Mail,
  MailX,
  Menu,
  Plus,
  RefreshCw,
  Settings,
  ShieldAlert,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Users,
} from "lucide-react";
import {
  cancelAccount,
  finishInterrupted,
  listAccounts,
  lockVault,
  readAccountStatus,
  scanAccount,
  setWriteMode,
} from "../api";
import type { Account, AccountStatus } from "../api";
import {
  LanguageContext,
  Action,
  ArmLiveWritesModal,
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
import { WorkspaceSettings } from "./Settings";
import { NavigationDrawer } from "./NavigationDrawer";
import { describeError } from "./status-labels";

const pages: Page[] = [
  "overview",
  "senders",
  "recommendations",
  "rules",
  "backlog",
  "invoices",
  "subscriptions",
  "addresses",
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
function initialAccount(): string {
  const fromUrl = new URLSearchParams(location.search).get("account");
  return fromUrl ?? stored("mailroom.account") ?? "";
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
  const t = useCopy();
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selected, setSelected] = useState(initialAccount);
  const [page, setPage] = useState<Page>(initialPage);
  const [add, setAdd] = useState(false);
  const [menu, setMenu] = useState(false);
  const [writeModal, setWriteModal] = useState(false);
  const [writeBusy, setWriteBusy] = useState(false);
  const [writeError, setWriteError] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [pending, setPending] = useState(false);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const heading = useRef<HTMLHeadingElement>(null);
  const polling = useRef(false);
  const lastJob = useRef<string | null>(null);
  const wasUsable = useRef(false);
  const menuTrigger = useRef<HTMLButtonElement>(null);
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
        (job.operation === "connect" || job.operation === "reconnect") &&
        job.account
      ) {
        autoScan(false);
        setSelected(job.account.id);
        setPage("overview");
        setStatus(await scanAccount(job.account.id));
        setRefresh((value) => value + 1);
      }
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
    const syncFromUrl = () => {
      const params = new URLSearchParams(location.search);
      const v = params.get("view");
      const nextPage = pages.find((p) => p === v) ?? "overview";
      setPage(nextPage);
      const acc = params.get("account");
      if (acc) setSelected(acc);
    };
    window.addEventListener("popstate", syncFromUrl);
    return () => window.removeEventListener("popstate", syncFromUrl);
  }, []);

  const pushUrl = useCallback((nextPage: Page, nextAccount: string) => {
    const query = new URLSearchParams(location.search);
    const same =
      query.get("view") === nextPage &&
      (nextAccount ? query.get("account") === nextAccount : true);
    query.set("view", nextPage);
    if (nextAccount) query.set("account", nextAccount);
    else query.delete("account");
    const target = `${location.pathname}?${query}`;
    if (!same) history.pushState(null, "", target);
  }, []);

  function navigate(next: Page) {
    setPage(next);
    setMenu(false);
    pushUrl(next, selected);
    requestAnimationFrame(() => heading.current?.focus());
    window.scrollTo({ top: 0 });
  }

  function selectAccount(nextId: string) {
    setSelected(nextId);
    setMenu(false);
    pushUrl(page, nextId);
  }

  async function action(work: () => Promise<AccountStatus>) {
    setPending(true);
    setNotice("");
    try {
      await applyStatus(await work());
    } catch (err) {
      setNotice(
        describeError(
          err,
          t(
            "The action could not start. Your saved work is retained. Try again.",
            "Die Aktion konnte nicht starten. Deine gespeicherte Arbeit bleibt erhalten. Versuche es erneut.",
          ),
        ),
      );
    } finally {
      setPending(false);
    }
  }

  async function handleWriteMode(enable: boolean) {
    setWriteBusy(true);
    setWriteError("");
    try {
      await applyStatus(await setWriteMode(enable, enable));
      setWriteModal(false);
    } catch (err) {
      setWriteError(
        describeError(
          err,
          t(
            "Could not change safety mode.",
            "Sicherheitsmodus konnte nicht geändert werden.",
          ),
        ),
      );
    } finally {
      setWriteBusy(false);
    }
  }

  const mailboxNav = [
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
  ] satisfies { page: Page; label: string; icon: typeof Mail }[];

  const organizeNav = [
    {
      page: "rules",
      label: t("Rules & deploy", "Regeln & Bereitstellung"),
      icon: SlidersHorizontal,
    },
    {
      page: "backlog",
      label: t("Backlog", "Bestand sortieren"),
      icon: FolderSync,
    },
    {
      page: "invoices",
      label: t("Invoices", "Rechnungen"),
      icon: FileText,
    },
    {
      page: "subscriptions",
      label: t("Subscriptions", "Abonnements"),
      icon: MailX,
    },
    {
      page: "addresses",
      label: t("Addresses & wind-down", "Adressen & Abbau"),
      icon: BookUser,
    },
  ] satisfies { page: Page; label: string; icon: typeof Mail }[];

  const allNav = [...mailboxNav, ...organizeNav];
  const currentTitle =
    allNav.find((item) => item.page === page)?.label ??
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
      "Edit rules, simulate saved mail, provision folders, and deploy Sieve safely.",
      "Regeln bearbeiten, lokal prüfen, Ordner anlegen und Sieve sicher bereitstellen.",
    ),
    backlog: t(
      "Sort existing mail with previews, journals, flag protection, and undo.",
      "Sortiere deinen Bestand mit Vorschau, Protokoll, Markierungsschutz und Rücknahme.",
    ),
    invoices: t(
      "Review business invoices and forward verified originals to accounting.",
      "Prüfe Geschäftsrechnungen und leite verifizierte Originale an die Buchhaltung weiter.",
    ),
    subscriptions: t(
      "Unsubscribe from bulk senders or mute them into dedicated folders.",
      "Melde Massenabsender ab oder schalte sie in eigene Ordner stumm.",
    ),
    addresses: t(
      "Manage per-service addresses, Gmail forwarding, and account wind-down.",
      "Verwalte Dienstadressen, Gmail-Weiterleitung und den Konto-Abbau.",
    ),
    settings: t(
      "Safety mode, account connections, Takeout import, T3 config, and vault backup.",
      "Sicherheitsmodus, Konten, Takeout-Import, T3-Konfiguration und Tresor-Sicherung.",
    ),
  };

  const vaultGated =
    !!status &&
    (status.vault === "absent" ||
      status.vault === "locked" ||
      status.vault === "unavailable");

  const runningJob = status?.job?.progress.state === "running";

  return (
    <>
      <a className="skip-link" href="#main-content">
        {t("Skip to content", "Zum Inhalt springen")}
      </a>
      <div
        className={`workbench ${menu ? "menu-open" : ""} ${vaultGated ? "gated" : ""} ${status?.live_writes ? "live-armed" : ""}`}
      >
        {!vaultGated && (
          <NavigationDrawer
            open={menu}
            onOpenChange={setMenu}
            triggerRef={menuTrigger}
          >
            <a href="/" className="brand">
              <span className="brand-icon">
                <Mail size={20} />
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
                  onChange={selectAccount}
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
              <span className="nav-section-label">
                {t("Mailbox", "Postfach")}
              </span>
              {mailboxNav.map((item) => (
                <button
                  className={`nav-item ${page === item.page ? "active" : ""}`}
                  key={item.page}
                  aria-current={page === item.page ? "page" : undefined}
                  onClick={() => navigate(item.page)}
                >
                  <item.icon size={17} />
                  <span>{item.label}</span>
                  {item.page === "recommendations" &&
                    (counts[selected] ?? 0) > 0 && (
                      <span className="nav-count">{counts[selected]}</span>
                    )}
                </button>
              ))}
              <span className="nav-section-label">
                {t("Organize", "Ordnen")}
              </span>
              {organizeNav.map((item) => (
                <button
                  className={`nav-item ${page === item.page ? "active" : ""}`}
                  key={item.page}
                  aria-current={page === item.page ? "page" : undefined}
                  onClick={() => navigate(item.page)}
                >
                  <item.icon size={17} />
                  <span>{item.label}</span>
                </button>
              ))}
            </nav>
            <div className="sidebar-bottom">
              <button
                className={`nav-item ${page === "settings" ? "active" : ""}`}
                aria-current={page === "settings" ? "page" : undefined}
                onClick={() => navigate("settings")}
              >
                <Settings size={17} />
                <span>{t("Settings", "Einstellungen")}</span>
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
                  <span className="brand-icon">
                    <Mail size={18} />
                  </span>
                  mailroom<span className="brand-dot">.</span>
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
              {!vaultGated && (
                <button
                  type="button"
                  className={`readonly-badge ${status?.live_writes ? "live" : ""}`}
                  onClick={() => {
                    setWriteError("");
                    setWriteModal(true);
                  }}
                  title={
                    status?.live_writes
                      ? t(
                          "Live writes armed — click to return to read-only",
                          "Live-Schreibzugriff aktiv — klicken für Nur-Lese-Modus",
                        )
                      : t(
                          "Read-only mode — click to enable Live writes",
                          "Nur-Lese-Modus — klicken für Live-Schreibzugriff",
                        )
                  }
                >
                  {status?.live_writes ? (
                    <>
                      <ShieldAlert size={14} />
                      <span className="badge-full">
                        {t("Live writes armed", "Live-Schreibzugriff aktiv")}
                      </span>
                      <span className="badge-short">{t("Live", "Live")}</span>
                    </>
                  ) : (
                    <>
                      <ShieldCheck size={14} />
                      <span className="badge-full">
                        {t("Provider read-only", "Anbieter nur lesbar")}
                      </span>
                      <span className="badge-short">
                        {t("Read-only", "Nur-Lese")}
                      </span>
                    </>
                  )}
                </button>
              )}
              {!vaultGated && status?.source === "vault" && (
                <Action
                  size="compact"
                  className="lock-button"
                  disabled={pending}
                  onClick={() => void action(lockVault)}
                  aria-label={t("Lock vault", "Tresor sperren")}
                >
                  <Lock size={14} />
                  <span className="lock-label">{t("Lock", "Sperren")}</span>
                </Action>
              )}
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
                <span>{error}</span>
                <Action
                  size="compact"
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
                      {account
                        ? `${account.login} · ${account.provider === "gmail" ? "Gmail" : "mailbox.org"}`
                        : t("Your workspace", "Dein Arbeitsbereich")}
                    </span>
                    <h1 ref={heading} tabIndex={-1}>
                      {currentTitle}
                    </h1>
                    <p className="muted">{subtitles[page]}</p>
                  </div>
                  {account && page === "overview" && (
                    <Action
                      primary
                      disabled={pending || runningJob}
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
                {runningJob && status.job && (
                  <div className="activity-strip">
                    <span role="status">
                      <Busy>
                        {status.job.operation === "connect"
                          ? t("Connecting account…", "Konto wird verbunden…")
                          : status.job.operation === "scan"
                            ? t("Scanning mailbox…", "Postfach wird erfasst…")
                            : status.job.operation === "backlog"
                              ? t("Sorting backlog…", "Bestand wird sortiert…")
                              : status.job.operation === "folders"
                                ? t("Creating folders…", "Ordner werden angelegt…")
                                : status.job.operation === "sieve"
                                  ? t("Running ManageSieve…", "ManageSieve läuft…")
                                  : t("Working…", "In Arbeit…")}
                      </Busy>
                    </span>
                    <span>
                      {status.job.account?.login} ·{" "}
                      {status.job.progress.saved.toLocaleString()}{" "}
                      {t("headers saved", "Kopfzeilen gespeichert")} ·{" "}
                      {status.job.progress.folders.toLocaleString()}{" "}
                      {t("folders", "Ordner")}
                    </span>
                    <Action
                      size="compact"
                      onClick={() => void action(cancelAccount)}
                      disabled={pending}
                    >
                      {t("Stop", "Stoppen")}
                    </Action>
                  </div>
                )}
                {status.job?.progress.state === "failed" && (
                  <Notice error>
                    <span>
                      {status.job.operation === "connect"
                        ? t(
                            "Connection failed. Check the app password in Settings and reconnect.",
                            "Verbindung fehlgeschlagen. Prüfe das App-Passwort in den Einstellungen und verbinde erneut.",
                          )
                        : t(
                            "The operation was interrupted. Saved progress is retained. Resume when ready.",
                            "Die Aktion wurde unterbrochen. Gespeicherter Fortschritt bleibt erhalten. Setze sie bei Bedarf fort.",
                          )}
                    </span>
                    <Action
                      size="compact"
                      onClick={() => {
                        const failedAccount = status.job?.account;
                        if (failedAccount) {
                          selectAccount(failedAccount.id);
                          navigate("settings");
                        } else setAdd(true);
                      }}
                    >
                      {t("Connection settings", "Verbindungseinstellungen")}
                    </Action>
                  </Notice>
                )}
                {status.interrupted.length > 0 && (
                  <Notice tone="warning">
                    <span>
                      {t(
                        `Account setup was interrupted (${status.interrupted.join(", ")}). Clean up or finish it so you can reconnect cleanly.`,
                        `Eine Kontoeinrichtung wurde unterbrochen (${status.interrupted.join(", ")}). Bereinige oder schließe sie ab, um sauber neu zu verbinden.`,
                      )}
                    </span>
                    <div className="actions">
                      {status.interrupted.map((id) => (
                        <Action
                          key={id}
                          size="compact"
                          onClick={() =>
                            void action(() => finishInterrupted(id))
                          }
                          disabled={pending}
                        >
                          {t(`Clean up ${id.slice(0, 8)}`, `${id.slice(0, 8)} bereinigen`)}
                        </Action>
                      ))}
                    </div>
                  </Notice>
                )}
                {accounts.length === 0 ? (
                  page === "settings" ? (
                    <WorkspaceSettings
                      accounts={accounts}
                      status={status}
                      onStatus={(next) => void applyStatus(next)}
                      onRefresh={() => {
                        void loadAccounts();
                        setRefresh((v) => v + 1);
                      }}
                      onOpenWriteMode={() => {
                        setWriteError("");
                        setWriteModal(true);
                      }}
                    />
                  ) : (
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
                        onRestore={
                          status.source === "vault" || status.synthetic
                            ? () => navigate("settings")
                            : undefined
                        }
                      />
                    </section>
                  )
                ) : (
                  accounts.map((value) => (
                    <AccountArea
                      key={value.id}
                      account={value}
                      accounts={accounts}
                      status={status}
                      hidden={value.id !== selected}
                      page={page}
                      refresh={refresh}
                      running={runningJob}
                      onNavigate={navigate}
                      onCount={(count) =>
                        setCounts((previous) =>
                          previous[value.id] === count
                            ? previous
                            : { ...previous, [value.id]: count },
                        )
                      }
                      onStatus={(next) => void applyStatus(next)}
                      onRefresh={() => {
                        void loadAccounts();
                        setRefresh((v) => v + 1);
                      }}
                      onOpenWriteMode={() => {
                        setWriteError("");
                        setWriteModal(true);
                      }}
                      onScan={() => void action(() => scanAccount(value.id))}
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
                  {status?.live_writes
                    ? t(
                        "Live writes armed for all accounts in this session; every change still requires preview and confirmation.",
                        "Live-Schreibzugriff für alle Konten dieser Sitzung aktiv; jede Änderung erfordert Vorschau und Bestätigung.",
                      )
                    : t(
                        "Read-only mode active. Click the safety badge above when ready to deploy or sort.",
                        "Nur-Lese-Modus aktiv. Klicke oben auf das Sicherheitssymbol, wenn du bereit zur Ausführung bist.",
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
      <ArmLiveWritesModal
        open={writeModal}
        onOpenChange={setWriteModal}
        liveWrites={Boolean(status?.live_writes)}
        busy={writeBusy}
        runningJob={runningJob}
        error={writeError}
        onConfirm={handleWriteMode}
      />
    </>
  );
}
