import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  KeyRound,
  Lock,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import {
  VaultRefusal,
  changeVaultPassword,
  lockVault,
  setSafetySettings,
} from "../../api";
import type { Account, AccountStatus } from "../../api";
import { getAssistantConfig, saveAssistantConfig } from "../assistant-api";
import { describeError } from "../status-labels";
import {
  Action,
  Choice,
  Details,
  Notice,
  StatusBadge,
  formatCountdown,
  useCopy,
} from "../components";
import { RecoverySection } from "./RecoverySection";

const T3_PRESETS: {
  id: string;
  label: string;
  provider: string;
  model: string;
}[] = [
  {
    id: "codex_home_v2::gpt-6.1-sol",
    label: "Codex — gpt-6.1-sol (default)",
    provider: "codex_home_v2",
    model: "gpt-6.1-sol",
  },
  {
    id: "claude_code::claude-opus-5-5",
    label: "Claude Code — claude-opus-5-5",
    provider: "claude_code",
    model: "claude-opus-5-5",
  },
  {
    id: "claude_code::claude-fable-5-2",
    label: "Claude Code — claude-fable-5-2",
    provider: "claude_code",
    model: "claude-fable-5-2",
  },
  {
    id: "antigravity::gemini-4-neon",
    label: "Antigravity — gemini-4-neon",
    provider: "antigravity",
    model: "gemini-4-neon",
  },
];

/**
 * Workspace-scoped settings: always reachable even with 0 connected accounts
 * so users can restore a recovery backup, configure T3, lock/update the vault,
 * or configure safety timers.
 */
export function WorkspaceSettings({
  accounts,
  status,
  onStatus,
  onRefresh,
  onOpenWriteMode,
}: {
  accounts: Account[];
  status: AccountStatus;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const [busy, setBusy] = useState(false);

  // Safety timers
  const [safetyError, setSafetyError] = useState("");
  const [safetyNotice, setSafetyNotice] = useState("");

  // T3 Assistant config (workspace-wide)
  const [t3Provider, setT3Provider] = useState("codex_home_v2");
  const [t3Model, setT3Model] = useState("gpt-6.1-sol");
  const [savedT3, setSavedT3] = useState({
    provider: "codex_home_v2",
    model: "gpt-6.1-sol",
  });
  const [t3Error, setT3Error] = useState("");
  const [t3Notice, setT3Notice] = useState("");

  // Local vault password change
  const [curPass, setCurPass] = useState("");
  const [newPass, setNewPass] = useState("");
  const [confirmPass, setConfirmPass] = useState("");
  const [vaultError, setVaultError] = useState("");
  const [vaultNotice, setVaultNotice] = useState("");

  const loadConfig = useCallback(async () => {
    try {
      const cfg = await getAssistantConfig();
      setT3Provider(cfg.provider);
      setT3Model(cfg.model);
      setSavedT3(cfg);
    } catch (err) {
      setT3Error(
        describeError(
          err,
          t(
            "Could not load saved T3 assistant settings.",
            "Gespeicherte T3-Einstellungen konnten nicht geladen werden.",
          ),
        ),
      );
    }
  }, [t]);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  const t3Dirty =
    t3Provider.trim() !== savedT3.provider || t3Model.trim() !== savedT3.model;
  const matchedPreset =
    T3_PRESETS.find(
      (p) => p.provider === t3Provider.trim() && p.model === t3Model.trim(),
    )?.id ?? "custom";

  async function updateTimers(input: {
    live_writes_timeout_secs?: number;
    vault_idle_timeout_secs?: number;
  }) {
    setBusy(true);
    setSafetyError("");
    setSafetyNotice("");
    try {
      onStatus(await setSafetySettings(input));
      setSafetyNotice(
        t("Session safety timers updated.", "Sitzungs-Sicherheitstimer aktualisiert."),
      );
    } catch (err) {
      setSafetyError(
        describeError(
          err,
          t(
            "Could not update safety timers.",
            "Sicherheitstimer konnten nicht aktualisiert werden.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function saveT3Config(providerVal = t3Provider, modelVal = t3Model) {
    setBusy(true);
    setT3Error("");
    setT3Notice("");
    try {
      const saved = await saveAssistantConfig(
        undefined,
        providerVal.trim(),
        modelVal.trim(),
      );
      setT3Provider(saved.provider);
      setT3Model(saved.model);
      setSavedT3(saved);
      setT3Notice(
        t(
          "T3 assistant configuration saved for all accounts.",
          "T3-Assistent-Konfiguration für alle Konten gespeichert.",
        ),
      );
    } catch (err) {
      setT3Error(
        describeError(
          err,
          t(
            "Invalid provider or model name.",
            "Ungültiger Anbieter- oder Modellname.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runChangeMasterPassword() {
    setVaultError("");
    setVaultNotice("");
    if (!curPass) {
      setVaultError(
        t(
          "Enter your current master password.",
          "Gib dein aktuelles Master-Passwort ein.",
        ),
      );
      return;
    }
    if (Array.from(newPass).length < 12) {
      setVaultError(
        t(
          "Use at least 12 characters for the new master password.",
          "Verwende mindestens 12 Zeichen für das neue Master-Passwort.",
        ),
      );
      return;
    }
    if (newPass !== confirmPass) {
      setVaultError(
        t(
          "The two new master passwords do not match.",
          "Die beiden neuen Master-Passwörter stimmen nicht überein.",
        ),
      );
      return;
    }
    setBusy(true);
    const current = curPass;
    const next = newPass;
    const confirm = confirmPass;
    try {
      onStatus(await changeVaultPassword(current, next, confirm));
      setCurPass("");
      setNewPass("");
      setConfirmPass("");
      setVaultNotice(
        t(
          "Master password updated and vault re-encrypted.",
          "Master-Passwort aktualisiert und Tresor neu verschlüsselt.",
        ),
      );
    } catch (err) {
      const code = err instanceof VaultRefusal ? err.code : "";
      setCurPass("");
      setVaultError(
        code === "mismatch"
          ? t(
              "The two new master passwords do not match.",
              "Die beiden neuen Master-Passwörter stimmen nicht überein.",
            )
          : code === "weak_password"
            ? t(
                "Use at least 12 characters without control characters.",
                "Verwende mindestens 12 Zeichen ohne Steuerzeichen.",
              )
            : t(
                "Current master password was rejected.",
                "Aktuelles Master-Passwort wurde abgelehnt.",
              ),
      );
    } finally {
      setBusy(false);
    }
  }

  const liveTimeoutValue = String(status.live_writes_timeout_secs ?? 900);
  const vaultIdleValue = String(status.vault_idle_timeout_secs ?? 1800);

  return (
    <div className="page-stack">
      <section id="settings-safety" className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Provider safety mode", "Sicherheitsmodus")}</h2>
            <p className="muted">
              {t(
                "Mailroom always starts read-only. Arm Live writes only when you are ready to create folders, deploy Sieve rules, sort backlog mail, send invoices, or run external unsubscribes.",
                "Mailroom startet immer im Nur-Lese-Modus. Aktiviere Live-Schreibzugriff erst, wenn du Ordner anlegen, Sieve-Regeln bereitstellen, den Bestand sortieren, Rechnungen senden oder externe Abmeldungen ausführen möchtest.",
              )}
            </p>
          </div>
          <StatusBadge
            status={status.live_writes ? "warning" : "active"}
            label={
              status.live_writes
                ? status.live_writes_expires_in_secs != null
                  ? t(
                      `Live armed · ${formatCountdown(status.live_writes_expires_in_secs)}`,
                      `Live aktiv · ${formatCountdown(status.live_writes_expires_in_secs)}`,
                    )
                  : t("Live writes armed", "Live-Schreibzugriff aktiv")
                : t("Read-only mode", "Nur-Lese-Modus")
            }
          />
        </div>
        {safetyError && <Notice error>{safetyError}</Notice>}
        {safetyNotice && (
          <Notice>
            <CheckCircle2 size={16} />
            <span>{safetyNotice}</span>
          </Notice>
        )}
        <div className="actions">
          {status.live_writes ? (
            <Action primary disabled={busy} onClick={onOpenWriteMode}>
              <ShieldCheck size={15} />
              {t("Manage / return to read-only", "Verwalten / Nur-Lese")}
            </Action>
          ) : (
            <Action tone="warning" disabled={busy} onClick={onOpenWriteMode}>
              <ShieldAlert size={15} />
              {t(
                "Enable live writes for this session",
                "Live-Schreibzugriff für diese Sitzung aktivieren",
              )}
            </Action>
          )}
        </div>
        <div className="settings-form mt-md">
          <Choice
            fieldLabel={t(
              "Auto-disarm Live writes after",
              "Live-Schreibzugriff automatisch deaktivieren nach",
            )}
            label={t("Live writes auto-disarm", "Auto-Deaktivierung")}
            value={
              ["300", "900", "1800", "3600", "0"].includes(liveTimeoutValue)
                ? liveTimeoutValue
                : "900"
            }
            disabled={busy}
            onChange={(val) =>
              void updateTimers({ live_writes_timeout_secs: Number(val) })
            }
            options={[
              { value: "300", label: t("5 minutes", "5 Minuten") },
              {
                value: "900",
                label: t("15 minutes (default)", "15 Minuten (Standard)"),
              },
              { value: "1800", label: t("30 minutes", "30 Minuten") },
              { value: "3600", label: t("60 minutes", "60 Minuten") },
              {
                value: "0",
                label: t("Off (until locked or quit)", "Aus (bis Sperre/Ende)"),
              },
            ]}
          />
          {status.source === "vault" && (
            <Choice
              fieldLabel={t(
                "Auto-lock vault after inactivity",
                "Tresor nach Inaktivität automatisch sperren",
              )}
              label={t("Vault idle auto-lock", "Automatische Tresorsperre")}
              value={
                ["0", "900", "1800", "3600"].includes(vaultIdleValue)
                  ? vaultIdleValue
                  : "1800"
              }
              disabled={busy}
              onChange={(val) =>
                void updateTimers({ vault_idle_timeout_secs: Number(val) })
              }
              options={[
                { value: "0", label: t("Off (manual lock)", "Aus (manuell)") },
                { value: "900", label: t("15 minutes", "15 Minuten") },
                {
                  value: "1800",
                  label: t("30 minutes (default)", "30 Minuten (Standard)"),
                },
                { value: "3600", label: t("60 minutes", "60 Minuten") },
              ]}
            />
          )}
        </div>
      </section>

      <section id="settings-t3" className="panel">
        <div className="section-heading">
          <div>
            <h2>
              <Sparkles size={17} />{" "}
              {t("T3 assistant settings", "T3-Assistent-Einstellungen")}
            </h2>
            <p className="muted">
              {t(
                "Applies to all accounts in this workspace when launching isolated T3 recommendation reviews.",
                "Gilt für alle Konten dieses Arbeitsbereichs beim Start isolierter T3-Vorschlagsprüfungen.",
              )}
            </p>
          </div>
          {t3Dirty && (
            <StatusBadge
              status="warning"
              label={t("Unsaved changes", "Ungespeichert")}
            />
          )}
        </div>
        {t3Error && <Notice error>{t3Error}</Notice>}
        {t3Notice && (
          <Notice>
            <CheckCircle2 size={16} />
            <span>{t3Notice}</span>
          </Notice>
        )}
        <div className="settings-form">
          <Choice
            fieldLabel={t("Preset", "Voreinstellung")}
            label={t("T3 preset", "T3-Voreinstellung")}
            value={matchedPreset}
            onChange={(val) => {
              const found = T3_PRESETS.find((p) => p.id === val);
              if (found) {
                setT3Provider(found.provider);
                setT3Model(found.model);
              }
            }}
            options={[
              ...T3_PRESETS.map((p) => ({ value: p.id, label: p.label })),
              { value: "custom", label: t("Custom…", "Benutzerdefiniert…") },
            ]}
          />
          <label className="field">
            {t("T3 provider", "T3-Anbieter")}
            <input
              value={t3Provider}
              onChange={(e) => setT3Provider(e.target.value)}
            />
          </label>
          <label className="field">
            {t("T3 model", "T3-Modell")}
            <input
              value={t3Model}
              onChange={(e) => setT3Model(e.target.value)}
            />
          </label>
          <div className="actions">
            <Action
              primary
              disabled={busy || !t3Provider.trim() || !t3Model.trim()}
              onClick={() => void saveT3Config()}
            >
              {t("Save T3 settings", "T3-Einstellungen speichern")}
            </Action>
            <Action
              disabled={busy}
              onClick={() => void saveT3Config("codex_home_v2", "gpt-6.1-sol")}
            >
              {t("Reset & save defaults", "Standard wiederherstellen")}
            </Action>
          </div>
        </div>
      </section>

      {status.source === "vault" && (
        <section id="settings-vault" className="panel">
          <div className="section-heading">
            <div>
              <h2>
                <KeyRound size={17} />{" "}
                {t("Local vault security", "Sicherheit des lokalen Tresors")}
              </h2>
              <p className="muted">
                {t(
                  "Locking clears the decrypted key from memory and returns Mailroom to read-only mode.",
                  "Sperren löscht den entschlüsselten Schlüssel aus dem Speicher und setzt Mailroom auf Nur-Lese zurück.",
                )}
              </p>
            </div>
            <Action
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setVaultError("");
                try {
                  onStatus(await lockVault());
                } catch (err) {
                  setVaultError(
                    describeError(
                      err,
                      t(
                        "Could not lock vault.",
                        "Tresor konnte nicht gesperrt werden.",
                      ),
                    ),
                  );
                } finally {
                  setBusy(false);
                }
              }}
            >
              <Lock size={15} />
              {t("Lock vault now", "Tresor jetzt sperren")}
            </Action>
          </div>
          {vaultError && <Notice error>{vaultError}</Notice>}
          {vaultNotice && (
            <Notice>
              <CheckCircle2 size={16} />
              <span>{vaultNotice}</span>
            </Notice>
          )}
          <Details
            title={t("Change master password", "Master-Passwort ändern")}
          >
            <form
              className="settings-form"
              onSubmit={(e) => {
                e.preventDefault();
                void runChangeMasterPassword();
              }}
            >
              <label className="field">
                {t("Current master password", "Aktuelles Master-Passwort")}
                <input
                  type="password"
                  autoComplete="current-password"
                  value={curPass}
                  onChange={(e) => setCurPass(e.target.value)}
                />
              </label>
              <label className="field">
                {t(
                  "New master password (min. 12 chars)",
                  "Neues Master-Passwort (mind. 12 Zeichen)",
                )}
                <input
                  type="password"
                  autoComplete="new-password"
                  value={newPass}
                  onChange={(e) => setNewPass(e.target.value)}
                />
              </label>
              <label className="field">
                {t(
                  "Confirm new master password",
                  "Neues Master-Passwort bestätigen",
                )}
                <input
                  type="password"
                  autoComplete="new-password"
                  value={confirmPass}
                  onChange={(e) => setConfirmPass(e.target.value)}
                />
              </label>
              <Action primary type="submit" disabled={busy}>
                {t("Update master password", "Master-Passwort aktualisieren")}
              </Action>
            </form>
          </Details>
        </section>
      )}

      <RecoverySection
        accounts={accounts}
        status={status}
        onRefresh={onRefresh}
      />
    </div>
  );
}
