import { useRef, useState } from "react";
import { KeyRound, Mail, ShieldCheck } from "lucide-react";
import {
  createVault,
  unlockVault,
  VaultRefusal,
  connectAccount,
  syntheticAccount,
} from "../api";
import type { AccountStatus } from "../api";
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
  const t = useCopy(),
    create = status.vault === "absent";
  const [password, setPassword] = useState(""),
    [repeat, setRepeat] = useState(""),
    [understood, setUnderstood] = useState(false);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const field = useRef<HTMLInputElement>(null);
  const errors: Record<string, string> = {
    unlock_failed: t(
      "That password did not open the vault. Try again.",
      "Dieses Passwort hat den Tresor nicht geöffnet. Versuche es erneut.",
    ),
    busy: t(
      "The vault is already opening. Wait a moment and try again.",
      "Der Tresor wird bereits geöffnet. Warte kurz und versuche es erneut.",
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
    const entered = password,
      confirmation = repeat;
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
            "Could not open the vault. Check your password and try again.",
            "Der Tresor ließ sich nicht öffnen. Prüfe dein Passwort und versuche es erneut.",
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
              "Gib dein Hauptpasswort ein, um deine Konten und gespeicherten Ergebnisse zu öffnen.",
            )}
      </p>
      {status.vault === "unavailable" ? (
        <Notice error>
          {t("Your vault is unavailable.", "Dein Tresor ist nicht verfügbar.")}{" "}
          {status.vault_problem}
        </Notice>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <label className="field">
            {t("Master password", "Hauptpasswort")}
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
              <small id="vault-help" className="muted">
                {t(
                  "Use at least 12 characters.",
                  "Verwende mindestens 12 Zeichen.",
                )}
              </small>
              <label className="field">
                {t("Repeat password", "Passwort wiederholen")}
                <input
                  type="password"
                  autoComplete="new-password"
                  required
                  maxLength={1024}
                  value={repeat}
                  onChange={(e) => setRepeat(e.target.value)}
                  disabled={busy}
                />
              </label>
              <p className="help">
                {t(
                  "A forgotten master password cannot be recovered. Store it in your password manager.",
                  "Ein vergessenes Hauptpasswort kann nicht wiederhergestellt werden. Bewahre es im Passwortmanager auf.",
                )}
              </p>
              <CheckField checked={understood} onChange={setUnderstood}>
                {t(
                  "I have saved my master password somewhere safe.",
                  "Ich habe mein Hauptpasswort sicher aufbewahrt.",
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
              (create &&
                (!understood || Array.from(password).length < 12 || !repeat))
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
          "Local storage. Provider access is read-only.",
          "Lokale Speicherung. Nur Lesezugriff auf dein Postfach.",
        )}
      </div>
    </div>
  );
}

export function ConnectForm({
  synthetic,
  onConnected,
}: {
  synthetic: boolean;
  onConnected: (status: AccountStatus) => void;
}) {
  const t = useCopy(),
    [provider, setProvider] = useState<"mailbox_org" | "gmail">("mailbox_org");
  const [login, setLogin] = useState(""),
    [password, setPassword] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
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
    } catch {
      setError(
        t(
          "Connection could not start. Check your account details and try again.",
          "Die Verbindung konnte nicht starten. Prüfe deine Zugangsdaten und versuche es erneut.",
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
          "Connect once. Mailroom scans headers without changing your mail.",
          "Einmal verbinden. Mailroom erfasst Kopfzeilen und verändert keine Nachrichten.",
        )}
      </p>
      <label className="field">
        {t("Provider", "Anbieter")}
        <Choice
          label={t("Provider", "Anbieter")}
          value={provider}
          onChange={(value) => {
            if (value === "gmail" || value === "mailbox_org")
              setProvider(value);
          }}
          options={[
            { value: "mailbox_org", label: "mailbox.org" },
            { value: "gmail", label: "Gmail" },
          ]}
        />
      </label>
      <label className="field">
        {t("Email address", "E-Mail-Adresse")}
        <input
          type="email"
          autoComplete="username"
          required
          value={login}
          onChange={(e) => setLogin(e.target.value)}
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
          aria-describedby="connect-help"
        />
      </label>
      <p id="connect-help" className="help">
        {provider === "gmail"
          ? t(
              "Use a Google app password with IMAP enabled. Your normal Google password will not work.",
              "Verwende ein Google-App-Passwort mit aktiviertem IMAP. Dein normales Google-Passwort funktioniert hier nicht.",
            )
          : t(
              "Use your mailbox.org app password with IMAP access.",
              "Verwende dein mailbox.org-App-Passwort mit IMAP-Zugriff.",
            )}
      </p>
      {error && <Notice error>{error}</Notice>}
      <Action primary type="submit" disabled={busy}>
        {busy ? (
          <Busy>{t("Connecting…", "Verbindung läuft…")}</Busy>
        ) : (
          t("Connect and scan", "Verbinden und erfassen")
        )}
      </Action>
      {synthetic && (
        <Action type="button" disabled={busy} onClick={() => void submit(true)}>
          {t("Use demo account", "Demokonto verwenden")}
        </Action>
      )}
    </form>
  );
}
