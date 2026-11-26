import { Suspense, lazy, useEffect, useRef, useState } from "react";
import {
  BookUser,
  ChevronRight,
  FileText,
  FolderSync,
  History,
  LayoutDashboard,
  Lock,
  LockKeyhole,
  Mail,
  MailX,
  Menu,
  Monitor,
  Moon,
  Plus,
  RefreshCw,
  Settings,
  ShieldAlert,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Sun,
  Users,
} from "lucide-react";
import {
  cancelAccount,
  finishInterrupted,
  lockVault,
  readAccountStatus,
  scanAccount,
  setWriteMode,
} from "../api";
import type { AccountStatus } from "../api";
import {
  LanguageContext,
  Action,
  ArmLiveWritesModal,
  Busy,
  Choice,
  Empty,
  Modal,
  Notice,
  formatCountdown,
  useCopy,
} from "./components";
import type { Language } from "./components";
import { ConnectForm, VaultForm } from "./Setup";
import { AccountArea } from "./AccountWorkspace";
import type { Page } from "./AccountWorkspace";
import { NavigationDrawer } from "./NavigationDrawer";
import { describeError } from "./status-labels";
import {
  autoScan,
  cycleThemePreference,
  remember,
  stored,
  useThemePreference,
} from "./preferences";
import type { ThemePreference } from "./preferences";
import { useAccountSession } from "./useAccountSession";

const WorkspaceSettings = lazy(() =>
  import("./Settings").then((m) => ({ default: m.WorkspaceSettings })),
);
const ActivityOverview = lazy(() =>
  import("./ActivityOverview").then((m) => ({ default: m.ActivityOverview })),
);

export function App() {
  const [language, setLanguage] = useState<Language>(
    stored("mailroom.language") === "de" ? "de" : "en",
  );
  const [theme, setTheme] = useThemePreference();

  useEffect(() => {
    document.documentElement.lang = language;
    remember("mailroom.language", language);
  }, [language]);

  return (
    <LanguageContext value={language}>
      <Workbench
        language={language}
        onLanguage={setLanguage}
        theme={theme}
        onTheme={setTheme}
      />
    </LanguageContext>
  );
}

function Workbench({
  language,
  onLanguage,
  theme,
  onTheme,
}: {
  language: Language;
  onLanguage: (value: Language) => void;
  theme: ThemePreference;
  onTheme: (value: ThemePreference) => void;
}) {
  const t = useCopy();
  const {
    status,
    accounts,
    selected,
    setSelected,
    page,
    setPage,
    error,
    setError,
    refresh,
    bumpRefresh,
    liveCountdown,
    loadAccounts,
    applyStatus,
    pushUrl,
  } = useAccountSession(t);
  const [add, setAdd] = useState(false);
  const [menu, setMenu] = useState(false);
  const [writeModal, setWriteModal] = useState(false);
  const [writeBusy, setWriteBusy] = useState(false);
  const [writeError, setWriteError] = useState("");
  const [notice, setNotice] = useState("");
  const [pending, setPending] = useState(false);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const heading = useRef<HTMLHeadingElement>(null);
  const menuTrigger = useRef<HTMLButtonElement>(null);
  const account = accounts.find((value) => value.id === selected);

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

  function selectAccountAndPage(nextId: string, nextPage: Page) {
    setSelected(nextId);
    setPage(nextPage);
    setMenu(false);
    pushUrl(nextPage, nextId);
    requestAnimationFrame(() => heading.current?.focus());
    window.scrollTo({ top: 0 });
  }

  function cycleTheme() {
    onTheme(cycleThemePreference(theme));
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

  async function handleWriteMode(enable: boolean, timeoutSecs?: number) {
    setWriteBusy(true);
    setWriteError("");
    try {
      await applyStatus(await setWriteMode(enable, enable, timeoutSecs));
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
    {
      page: "activity",
      label: t("Activity & undo", "Aktivität & Rücknahme"),
      icon: History,
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
    activity: t(
      "Cross-account health, active 90-day undo windows, and persisted action history.",
      "Kontostatus, aktive 90-Tage-Rücknahmefenster und gespeichertes Aktionsprotokoll.",
    ),
    settings: t(
      "Safety timers, account connections, Takeout import, T3 config, and vault backup.",
      "Sicherheitstimer, Konten, Takeout-Import, T3-Konfiguration und Tresor-Sicherung.",
    ),
  };

  const vaultGated =
    !!status &&
    (status.vault === "absent" ||
      status.vault === "locked" ||
      status.vault === "unavailable");

  const runningJob = status?.job?.progress.state === "running";

  const themeLabel =
    theme === "dark"
      ? t("Dark", "Dunkel")
      : theme === "light"
        ? t("Light", "Hell")
        : t("Auto", "Auto");

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
                          "Live writes armed — click to manage or return to read-only",
                          "Live-Schreibzugriff aktiv — klicken zum Verwalten oder Deaktivieren",
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
                      {liveCountdown !== null && (
                        <span className="badge-countdown">
                          {formatCountdown(liveCountdown)}
                        </span>
                      )}
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
              <button
                type="button"
                className="theme-toggle"
                onClick={cycleTheme}
                aria-label={t(
                  `Theme: ${themeLabel} (click to switch)`,
                  `Darstellung: ${themeLabel} (klicken zum Wechseln)`,
                )}
                title={t(
                  `Theme: ${themeLabel}`,
                  `Darstellung: ${themeLabel}`,
                )}
              >
                {theme === "dark" ? (
                  <Moon size={14} />
                ) : theme === "light" ? (
                  <Sun size={14} />
                ) : (
                  <Monitor size={14} />
                )}
                <span className="lock-label">{themeLabel}</span>
              </button>
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
                      .catch((err) =>
                        setError(
                          describeError(
                            err,
                            t(
                              "Still unable to reach Mailroom.",
                              "Mailroom ist weiterhin nicht erreichbar.",
                            ),
                          ),
                        ),
                      )
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
                  bumpRefresh();
                }}
              />
            ) : (
              <>
                <div className="page-heading">
                  <div>
                    <span className="eyebrow">
                      {account && page !== "activity"
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
                {page === "activity" ? (
                  <Suspense
                    fallback={
                      <div className="lazy-fallback">
                        <Busy>{t("Loading activity…", "Aktivität wird geladen…")}</Busy>
                      </div>
                    }
                  >
                    <ActivityOverview
                      selectedAccount={selected}
                      refresh={refresh}
                      onSelectAccountPage={selectAccountAndPage}
                    />
                  </Suspense>
                ) : accounts.length === 0 ? (
                  page === "settings" ? (
                    <Suspense
                      fallback={
                        <div className="lazy-fallback">
                          <Busy>{t("Loading settings…", "Einstellungen werden geladen…")}</Busy>
                        </div>
                      }
                    >
                      <WorkspaceSettings
                        accounts={accounts}
                        status={status}
                        onStatus={(next) => void applyStatus(next)}
                        onRefresh={() => {
                          void loadAccounts();
                          bumpRefresh();
                        }}
                        onOpenWriteMode={() => {
                          setWriteError("");
                          setWriteModal(true);
                        }}
                      />
                    </Suspense>
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
                      onSelectAccountPage={selectAccountAndPage}
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
                        bumpRefresh();
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
        expiresInSecs={liveCountdown}
        timeoutSecs={status?.live_writes_timeout_secs ?? 900}
        busy={writeBusy}
        runningJob={runningJob}
        error={writeError}
        onConfirm={handleWriteMode}
      />
    </>
  );
}
