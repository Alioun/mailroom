import { useRef, useState } from "react";
import {
  AlertTriangle,
  Archive,
  ExternalLink,
  KeyRound,
  Mail,
  ShieldCheck,
} from "lucide-react";
import {
  createVault,
  unlockVault,
  VaultRefusal,
  connectAccount,
  syntheticAccount,
} from "../api";
import type { AccountStatus } from "../api";
import { localizedError, vaultProblemText } from "./status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Notice,
  useCopy,
} from "./components";

export function VaultForm({
  status,
  onOpened,
}: {
  status: AccountStatus;
  onOpened: (value: AccountStatus) => void;
}) {
  const t = useCopy();
  const create = status.vault === "absent";
  const [password, setPassword] = useState("");
  const [repeat, setRepeat] = useState("");
  const [understood, setUnderstood] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const field = useRef<HTMLInputElement>(null);
  const charCount = Array.from(password).length;

  const errors: Record<string, string> = {
    unlock_failed: t(
      "That master password did not open the vault. Try again.",
      "Dieses Master-Passwort hat den Tresor nicht geöffnet. Versuche es erneut.",
    ),
    busy: t(
      "The vault is already opening. Wait a moment and try again.",
      "Der Tresor wird bereits geöffnet. Warte kurz und versuche es erneut.",
    ),
    weak_password: t(
      "Use at least 12 characters without control characters.",
      "Verwende mindestens 12 Zeichen ohne Steuerzeichen.",
    ),
    mismatch: t(
      "The two master passwords do not match.",
      "Die beiden Master-Passwörter stimmen nicht überein.",
    ),
    unavailable: t(
      "The vault cannot be read. Mailroom left it unchanged.",
      "Der Tresor lässt sich nicht lesen. Mailroom hat ihn unverändert gelassen.",
    ),
  };

  async function submit() {
    if (create && password !== repeat) {
      setError(
        t(
          "The passwords do not match.",
          "Die Passwörter stimmen nicht überein.",
        ),
      );
      field.current?.focus();
      return;
    }
    setBusy(true);
    setError("");
    const entered = password;
    const confirmation = repeat;
    setPassword("");
    setRepeat("");
    try {
      onOpened(
        await (create
          ? createVault(entered, confirmation)
          : unlockVault(entered)),
      );
    } catch (e) {
      setError(
        errors[e instanceof VaultRefusal ? e.code : ""] ??
          t(
            "Could not open the vault. Check your master password and try again.",
            "Der Tresor ließ sich nicht öffnen. Prüfe dein Master-Passwort und versuche es erneut.",
          ),
      );
      requestAnimationFrame(() => field.current?.focus());
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="setup-card">
      <div className="setup-icon">
        <KeyRound size={28} />
      </div>
      <h1>
        {create
          ? t("Welcome to Mailroom", "Willkommen bei Mailroom")
          : t("Welcome back", "Willkommen zurück")}
      </h1>
      <p className="muted">
        {create
          ? t(
              "Understand your mailbox. Keep your credentials in an encrypted vault on this Mac.",
              "Verstehe dein Postfach. Deine Zugangsdaten bleiben in einem verschlüsselten Tresor auf diesem Mac.",
            )
          : t(
              "Enter your master password to open your accounts and saved findings.",
              "Gib dein Master-Passwort ein, um deine Konten und gespeicherten Ergebnisse zu öffnen.",
            )}
      </p>
      {status.vault === "unavailable" ? (
        <Notice error>
          <AlertTriangle size={16} />
          <span>{vaultProblemText(status.vault_problem, t)}</span>
        </Notice>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <label className="field">
            {t("Master password", "Master-Passwort")}
            <input
              ref={field}
              type="password"
              autoFocus
              autoComplete={create ? "new-password" : "current-password"}
              minLength={create ? 12 : undefined}
              maxLength={1024}
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={!!error}
              aria-describedby={
                error ? "vault-error" : create ? "vault-help" : undefined
              }
              disabled={busy}
            />
          </label>
          {create && (
            <>
              <small id="vault-help" className="help">
                {charCount >= 12
                  ? t(
                      `${charCount} / 12 characters — length looks good.`,
                      `${charCount} / 12 Zeichen — Mindestlänge erreicht.`,
                    )
                  : t(
                      `${charCount} / 12 characters — use at least 12 characters.`,
                      `${charCount} / 12 Zeichen — verwende mindestens 12 Zeichen.`,
                    )}
              </small>
              <label className="field">
                {t("Repeat master password", "Master-Passwort wiederholen")}
                <input
                  type="password"
                  autoComplete="new-password"
                  required
                  maxLength={1024}
                  value={repeat}
                  onChange={(e) => setRepeat(e.target.value)}
                  aria-invalid={!!error}
                  aria-describedby={error ? "vault-error" : undefined}
                  disabled={busy}
                />
              </label>
              <div className="vault-warning">
                <strong>
                  {t(
                    "Store this master password in your password manager.",
                    "Speichere dieses Master-Passwort in deinem Passwortmanager.",
                  )}
                </strong>{" "}
                {t(
                  "Mailroom never sends it anywhere and cannot recover it if forgotten.",
                  "Mailroom überträgt es nirgendwohin und kann es bei Verlust nicht wiederherstellen.",
                )}
              </div>
              <CheckField checked={understood} onChange={setUnderstood}>
                {t(
                  "I have saved my master password somewhere safe.",
                  "Ich habe mein Master-Passwort sicher aufbewahrt.",
                )}
              </CheckField>
            </>
          )}
          {error && (
            <div id="vault-error">
              <Notice error>{error}</Notice>
            </div>
          )}
          <Action
            primary
            type="submit"
            className="full"
            disabled={
              busy ||
              !password ||
              (create && (!understood || charCount < 12 || !repeat))
            }
          >
            {busy ? (
              <Busy>{t("Opening…", "Wird geöffnet…")}</Busy>
            ) : create ? (
              t("Create vault", "Tresor anlegen")
            ) : (
              t("Unlock Mailroom", "Mailroom entsperren")
            )}
          </Action>
        </form>
      )}
      <div className="setup-foot">
        <ShieldCheck size={16} />
        {t(
          "Encrypted local vault. Provider access starts read-only.",
          "Verschlüsselter lokaler Tresor. Anbieterzugriff startet schreibgeschützt.",
        )}
      </div>
    </div>
  );
}

export function ConnectForm({
  synthetic,
  onConnected,
  onRestore,
}: {
  synthetic: boolean;
  onConnected: (status: AccountStatus) => void;
  onRestore?: () => void;
}) {
  const t = useCopy();
  const [provider, setProvider] = useState<"mailbox_org" | "gmail">(
    "mailbox_org",
  );
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const passwordField = useRef<HTMLInputElement>(null);

  async function submit(demo = false) {
    setBusy(true);
    setError("");
    const entered = password;
    setPassword("");
    try {
      onConnected(
        await (demo
          ? syntheticAccount(provider === "gmail")
          : connectAccount({
              provider,
              login: login.trim(),
              password: entered,
            })),
      );
    } catch (err) {
      setError(
        localizedError(
          err,
          t,
          t(
            "Connection could not start. Check your account details and try again.",
            "Die Verbindung konnte nicht starten. Prüfe deine Zugangsdaten und versuche es erneut.",
          ),
        ),
      );
      requestAnimationFrame(() => passwordField.current?.focus());
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
      className="connect-form"
    >
      <div className="setup-icon small">
        <Mail size={22} />
      </div>
      <p className="muted">
        {t(
          "Connect once. Mailroom scans headers read-only without changing your mail.",
          "Einmal verbinden. Mailroom erfasst Kopfzeilen schreibgeschützt und verändert keine Nachrichten.",
        )}
      </p>
      <Choice
        fieldLabel={t("Provider", "Anbieter")}
        label={t("Provider", "Anbieter")}
        value={provider}
        onChange={(value) => {
          if (value === "gmail" || value === "mailbox_org") setProvider(value);
        }}
        options={[
          { value: "mailbox_org", label: "mailbox.org" },
          { value: "gmail", label: "Gmail" },
        ]}
      />
      <label className="field">
        {t("Email address", "E-Mail-Adresse")}
        <input
          type="email"
          autoComplete="username"
          required
          value={login}
          onChange={(e) => setLogin(e.target.value)}
          placeholder={
            provider === "gmail" ? "name@gmail.com" : "name@mailbox.org"
          }
          aria-invalid={!!error}
          aria-describedby={error ? "connect-error" : "connect-help"}
          disabled={busy}
        />
      </label>
      <label className="field">
        {t("App password", "App-Passwort")}
        <input
          ref={passwordField}
          type="password"
          autoComplete="off"
          maxLength={1024}
          required
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          disabled={busy}
          aria-invalid={!!error}
          aria-describedby={error ? "connect-error" : "connect-help"}
        />
      </label>
      <p id="connect-help" className="help">
        {provider === "gmail" ? (
          <>
            {t(
              "Requires 2-Step Verification and a dedicated 16-character Google app password with IMAP enabled. Your normal Google password will not work. ",
              "Erfordert 2-Faktor-Authentifizierung und ein 16-stelliges Google-App-Passwort mit aktiviertem IMAP. Dein normales Google-Passwort funktioniert hier nicht. ",
            )}
            <a
              href="https://myaccount.google.com/apppasswords"
              target="_blank"
              rel="noreferrer"
            >
              {t("Create Google app password", "Google-App-Passwort erstellen")}{" "}
              <ExternalLink size={12} />
            </a>
          </>
        ) : (
          <>
            {t(
              "Use a mailbox.org app password (Settings → Security → App passwords) with IMAP/ManageSieve access enabled. ",
              "Verwende ein mailbox.org-App-Passwort (Einstellungen → Sicherheit → App-Passwörter) mit IMAP-/ManageSieve-Zugriff. ",
            )}
            <a
              href="https://kb.mailbox.org/en/private/account/app-passwords/"
              target="_blank"
              rel="noreferrer"
            >
              {t("mailbox.org guide", "mailbox.org-Anleitung")}{" "}
              <ExternalLink size={12} />
            </a>
          </>
        )}
      </p>
      {error && (
        <div id="connect-error">
          <Notice error>{error}</Notice>
        </div>
      )}
      <div className="actions">
        <Action primary type="submit" disabled={busy}>
          {busy ? (
            <Busy>{t("Connecting…", "Verbindung läuft…")}</Busy>
          ) : (
            t("Connect and scan", "Verbinden und erfassen")
          )}
        </Action>
        {synthetic && (
          <Action
            type="button"
            disabled={busy}
            onClick={() => void submit(true)}
          >
            {t("Use demo account", "Demokonto verwenden")}
          </Action>
        )}
      </div>
      {onRestore && (
        <div className="first-account-footer">
          <span className="help">
            {t(
              "Moving from another Mac or restoring a backup?",
              "Wechsel von einem anderen Mac oder Sicherung wiederherstellen?",
            )}
          </span>
          <Action type="button" onClick={onRestore}>
            <Archive size={14} />
            {t("Restore recovery bundle", "Sicherung wiederherstellen")}
          </Action>
        </div>
      )}
    </form>
  );
}
