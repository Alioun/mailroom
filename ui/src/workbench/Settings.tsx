import { useCallback, useContext, useEffect, useState } from "react";
import {
  Archive,
  CheckCircle2,
  KeyRound,
  Lock,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Trash2,
  Upload,
} from "lucide-react";
import {
  VaultRefusal,
  changeVaultPassword,
  confirmAccount,
  forgetAccount,
  importTakeout,
  lockVault,
  previewForget,
  readLifecycle,
  reconnectAccount,
  recoverAccount,
  rotateAccount,
} from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  ForgetPreview,
  LifecycleStatus,
  TakeoutSummary,
} from "../api";
import {
  capture,
  previewCapture,
  previewRestore,
  recoveryStatus,
  restore,
  resumeRestore,
} from "../recovery-api";
import type {
  CaptureOffer,
  RecoveryStatus,
  RestoreOffer,
} from "../recovery-api";
import { getAssistantConfig, saveAssistantConfig } from "./assistant-api";
import { describeError } from "./status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  LanguageContext,
  Modal,
  Notice,
  StatusBadge,
  useCopy,
} from "./components";

const T3_PRESETS: { id: string; label: string; provider: string; model: string }[] = [
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

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Workspace-scoped settings: always reachable even with 0 connected accounts
 * so users can restore a recovery backup, configure T3, lock/update the vault,
 * or toggle session safety mode.
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

  // Recovery backup & restore (separate passphrases!)
  const [recStatus, setRecStatus] = useState<RecoveryStatus | null>(null);
  const [backupPass, setBackupPass] = useState("");
  const [backupRepeat, setBackupRepeat] = useState("");
  const [backupIncludeLedger, setBackupIncludeLedger] = useState(true);
  const [backupSelected, setBackupSelected] = useState<Record<string, boolean>>(
    {},
  );
  const [captureOffer, setCaptureOffer] = useState<CaptureOffer | null>(null);

  const [restorePass, setRestorePass] = useState("");
  const [restoreBundle, setRestoreBundle] = useState("");
  const [restoreLedger, setRestoreLedger] = useState(true);
  const [restoreOffer, setRestoreOffer] = useState<RestoreOffer | null>(null);
  const [recError, setRecError] = useState("");
  const [recNotice, setRecNotice] = useState("");

  useEffect(() => {
    setBackupSelected((prev) => {
      const next: Record<string, boolean> = {};
      for (const acc of accounts) {
        next[acc.id] = prev[acc.id] ?? true;
      }
      return next;
    });
  }, [accounts]);

  const loadMeta = useCallback(async () => {
    try {
      const cfg = await getAssistantConfig();
      setT3Provider(cfg.provider);
      setT3Model(cfg.model);
      setSavedT3(cfg);
    } catch {}
    if (status.source === "vault" || status.synthetic) {
      try {
        const rec = await recoveryStatus();
        setRecStatus(rec);
        const newestId = rec.bundle_info[0]?.id ?? rec.bundles[rec.bundles.length - 1] ?? "";
        setRestoreBundle((cur) =>
          cur && rec.bundles.includes(cur) ? cur : newestId,
        );
      } catch {}
    }
  }, [status.source, status.synthetic]);

  useEffect(() => {
    void loadMeta();
  }, [loadMeta]);

  const t3Dirty =
    t3Provider.trim() !== savedT3.provider || t3Model.trim() !== savedT3.model;
  const matchedPreset =
    T3_PRESETS.find(
      (p) => p.provider === t3Provider.trim() && p.model === t3Model.trim(),
    )?.id ?? "custom";

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

  const chosenBackupAccounts = accounts
    .filter((a) => backupSelected[a.id])
    .map((a) => a.id);

  const bundleOptions = (
    recStatus?.bundle_info.length
      ? recStatus.bundle_info.map((b) => ({
          value: b.id,
          label: `${b.id.slice(0, 12)} · ${b.modified_at ? new Date(b.modified_at * 1000).toLocaleString() : "—"} · ${formatBytes(b.bytes)}`,
        }))
      : (recStatus?.bundles ?? []).map((id) => ({
          value: id,
          label: id.slice(0, 16),
        }))
  );

  return (
    <div className="page-stack">
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Provider safety mode", "Sicherheitsmodus")}</h2>
            <p className="muted">
              {t(
                "Mailroom always starts read-only. Arm Live writes only when you are ready to create folders, deploy Sieve rules, sort backlog mail, send invoices, or run external unsubscribes. Applies to all accounts until you disarm, lock the vault, or quit Mailroom.",
                "Mailroom startet immer im Nur-Lese-Modus. Aktiviere Live-Schreibzugriff erst, wenn du Ordner anlegen, Sieve-Regeln bereitstellen, den Bestand sortieren, Rechnungen senden oder externe Abmeldungen ausführen möchtest. Gilt für alle Konten bis zum Deaktivieren, Sperren oder Beenden.",
              )}
            </p>
          </div>
          <StatusBadge
            status={status.live_writes ? "warning" : "active"}
            label={
              status.live_writes
                ? t("Live writes armed", "Live-Schreibzugriff aktiv")
                : t("Read-only mode", "Nur-Lese-Modus")
            }
          />
        </div>
        <div className="actions">
          {status.live_writes ? (
            <Action primary disabled={busy} onClick={onOpenWriteMode}>
              <ShieldCheck size={15} />
              {t("Return to read-only", "Zurück zu Nur-Lese")}
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
      </section>

      <section className="panel">
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
        <section className="panel">
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
          <Details title={t("Change master password", "Master-Passwort ändern")}>
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

      {(status.source === "vault" || status.synthetic) && (
        <section className="panel">
          <h2>
            <Archive size={17} />{" "}
            {t(
              "Encrypted recovery backup & restore",
              "Verschlüsselte Sicherung & Wiederherstellung",
            )}
          </h2>
          <p className="muted">
            {t(
              "Create an authenticated encrypted backup of selected accounts and your private address ledger, or restore a saved bundle when those accounts are not present locally.",
              "Erstelle eine verschlüsselte Sicherung ausgewählter Konten und deines Adressverzeichnisses oder stelle ein gespeichertes Paket wieder her, wenn die Konten lokal noch nicht vorhanden sind.",
            )}
          </p>
          {recStatus?.bundles_dir && (
            <p className="help" style={{ marginTop: 6 }}>
              {t("Bundles folder", "Sicherungsordner")}:{" "}
              <code>{recStatus.bundles_dir}</code>
            </p>
          )}
          {recError && <Notice error>{recError}</Notice>}
          {recNotice && (
            <Notice>
              <CheckCircle2 size={16} />
              <span>{recNotice}</span>
            </Notice>
          )}
          {recStatus && recStatus.selected_pending.length > 0 && (
            <Notice tone="warning">
              <span>
                {t(
                  `An interrupted restore is pending (${recStatus.selected_pending.join(", ")}). Enter the restore passphrase below and resume.`,
                  `Eine unterbrochene Wiederherstellung steht aus (${recStatus.selected_pending.join(", ")}). Gib unten die Passphrase ein und setze sie fort.`,
                )}
              </span>
              <Action
                size="compact"
                tone="warning"
                disabled={
                  busy ||
                  (status.source === "vault" && restorePass.length < 12)
                }
                onClick={async () => {
                  setBusy(true);
                  setRecError("");
                  try {
                    await resumeRestore(
                      status.source === "vault" ? restorePass : undefined,
                    );
                    setRestorePass("");
                    setRecNotice(
                      t(
                        "Interrupted restore resumed cleanly.",
                        "Unterbrochene Wiederherstellung abgeschlossen.",
                      ),
                    );
                    await loadMeta();
                    onRefresh();
                  } catch (err) {
                    setRecError(
                      describeError(
                        err,
                        t(
                          "Could not resume restore. Check the recovery passphrase.",
                          "Wiederherstellung konnte nicht fortgesetzt werden. Prüfe die Passphrase.",
                        ),
                      ),
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {t("Resume restore", "Wiederherstellung fortsetzen")}
              </Action>
            </Notice>
          )}
          {recStatus &&
            recStatus.restored.some((r) => r.reconciliation_required) && (
              <Notice tone="info">
                {t(
                  "Restored accounts remain read-only for provider mutations until you run a fresh mailbox scan to reconcile live provider state.",
                  "Wiederhergestellte Konten bleiben für Anbieteränderungen gesperrt, bis eine neue Postfacherfassung den aktuellen Stand abgleicht.",
                )}
              </Notice>
            )}

          {accounts.length > 0 && (
            <div className="subpanel">
              <h3>{t("Create encrypted backup", "Verschlüsselte Sicherung erstellen")}</h3>
              <div className="domain-options">
                {accounts.map((acc) => (
                  <CheckField
                    key={acc.id}
                    checked={Boolean(backupSelected[acc.id])}
                    onChange={(checked) => {
                      setBackupSelected((prev) => ({
                        ...prev,
                        [acc.id]: checked,
                      }));
                      setCaptureOffer(null);
                    }}
                  >
                    {acc.login}
                  </CheckField>
                ))}
              </div>
              <CheckField
                checked={backupIncludeLedger}
                onChange={(val) => {
                  setBackupIncludeLedger(val);
                  setCaptureOffer(null);
                }}
              >
                {t(
                  "Include private address ledger",
                  "Privates Adressverzeichnis einschließen",
                )}
              </CheckField>
              {status.source === "vault" && (
                <div className="settings-form">
                  <label className="field">
                    {t(
                      "Backup passphrase (min. 12 chars, separate from master password)",
                      "Sicherungs-Passphrase (mind. 12 Zeichen, getrennt vom Master-Passwort)",
                    )}
                    <input
                      type="password"
                      autoComplete="new-password"
                      value={backupPass}
                      onChange={(e) => setBackupPass(e.target.value)}
                    />
                  </label>
                  <label className="field">
                    {t(
                      "Repeat backup passphrase",
                      "Sicherungs-Passphrase wiederholen",
                    )}
                    <input
                      type="password"
                      autoComplete="new-password"
                      value={backupRepeat}
                      onChange={(e) => setBackupRepeat(e.target.value)}
                    />
                  </label>
                </div>
              )}
              <div className="actions">
                <Action
                  primary
                  disabled={
                    busy ||
                    chosenBackupAccounts.length === 0 ||
                    (status.source === "vault" &&
                      (backupPass.length < 12 || backupPass !== backupRepeat))
                  }
                  onClick={async () => {
                    setBusy(true);
                    setRecError("");
                    setRecNotice("");
                    try {
                      setCaptureOffer(
                        await previewCapture(
                          chosenBackupAccounts,
                          backupIncludeLedger,
                          status.source === "vault" ? backupPass : undefined,
                        ),
                      );
                    } catch (err) {
                      setRecError(
                        describeError(
                          err,
                          t(
                            "Could not preview recovery capture.",
                            "Sicherungsvorschau fehlgeschlagen.",
                          ),
                        ),
                      );
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t(
                    "Preview encrypted backup",
                    "Verschlüsselte Sicherung prüfen",
                  )}
                </Action>
              </div>
              {captureOffer && (
                <div className="subpanel">
                  <p>{captureOffer.confirmation.display}</p>
                  <div className="actions">
                    <Action onClick={() => setCaptureOffer(null)}>
                      {t("Cancel", "Abbrechen")}
                    </Action>
                    <Action
                      primary
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        setRecError("");
                        try {
                          const done = await capture(
                            captureOffer,
                            status.source === "vault" ? backupPass : undefined,
                          );
                          setCaptureOffer(null);
                          setBackupPass("");
                          setBackupRepeat("");
                          setRecNotice(
                            t(
                              `Encrypted recovery bundle saved (${done.bundle.slice(0, 12)}). Store your recovery passphrase separately.`,
                              `Verschlüsseltes Sicherungspaket gespeichert (${done.bundle.slice(0, 12)}). Bewahre die Passphrase getrennt auf.`,
                            ),
                          );
                          await loadMeta();
                        } catch (err) {
                          setRecError(
                            describeError(
                              err,
                              t(
                                "Recovery capture failed.",
                                "Sicherung fehlgeschlagen.",
                              ),
                            ),
                          );
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      {t("Create backup bundle", "Sicherungs-Paket erstellen")}
                    </Action>
                  </div>
                </div>
              )}
            </div>
          )}

          <div className="subpanel">
            <h3>
              {t(
                "Restore from saved recovery bundle",
                "Aus Sicherungspaket wiederherstellen",
              )}
            </h3>
            {bundleOptions.length === 0 ? (
              <p className="muted">
                {t(
                  "No recovery bundles found in the local bundles folder yet.",
                  "Noch keine Sicherungspakete im lokalen Sicherungsordner vorhanden.",
                )}
              </p>
            ) : (
              <div className="settings-form">
                <Choice
                  fieldLabel={t("Saved recovery bundle", "Gespeichertes Sicherungspaket")}
                  label={t("Saved recovery bundle", "Gespeichertes Sicherungspaket")}
                  value={restoreBundle}
                  onChange={(val) => {
                    setRestoreBundle(val);
                    setRestoreOffer(null);
                  }}
                  options={bundleOptions}
                />
                <CheckField
                  checked={restoreLedger}
                  onChange={(val) => {
                    setRestoreLedger(val);
                    setRestoreOffer(null);
                  }}
                >
                  {t(
                    "Also restore private address ledger (when included in bundle)",
                    "Auch privates Adressverzeichnis wiederherstellen (falls enthalten)",
                  )}
                </CheckField>
                {status.source === "vault" && (
                  <label className="field">
                    {t(
                      "Bundle recovery passphrase",
                      "Wiederherstellungs-Passphrase des Pakets",
                    )}
                    <input
                      type="password"
                      autoComplete="off"
                      value={restorePass}
                      onChange={(e) => setRestorePass(e.target.value)}
                    />
                  </label>
                )}
                <Action
                  disabled={
                    busy ||
                    !restoreBundle ||
                    (status.source === "vault" && restorePass.length < 12)
                  }
                  onClick={async () => {
                    setBusy(true);
                    setRecError("");
                    setRecNotice("");
                    try {
                      setRestoreOffer(
                        await previewRestore(
                          restoreBundle,
                          restoreLedger,
                          status.source === "vault" ? restorePass : undefined,
                        ),
                      );
                    } catch (err) {
                      setRecError(
                        describeError(
                          err,
                          t(
                            "Could not preview restore. Accounts in the bundle must be absent locally and the passphrase must match.",
                            "Wiederherstellungsvorschau fehlgeschlagen. Konten im Paket dürfen lokal noch nicht vorhanden sein und die Passphrase muss stimmen.",
                          ),
                        ),
                      );
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t("Preview restore", "Wiederherstellung prüfen")}
                </Action>
              </div>
            )}
            {restoreOffer && (
              <div className="subpanel">
                <p>{restoreOffer.confirmation.display}</p>
                <div className="actions">
                  <Action onClick={() => setRestoreOffer(null)}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                  <Action
                    tone="warning"
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      setRecError("");
                      try {
                        await restore(
                          restoreOffer,
                          status.source === "vault" ? restorePass : undefined,
                        );
                        setRestoreOffer(null);
                        setRestorePass("");
                        setRecNotice(
                          t(
                            "Recovery bundle restored locally. Restored accounts stay read-only until reconciled with a fresh scan.",
                            "Sicherungspaket lokal wiederhergestellt. Konten bleiben bis zum Abgleich durch eine neue Erfassung schreibgeschützt.",
                          ),
                        );
                        await loadMeta();
                        onRefresh();
                      } catch (err) {
                        setRecError(
                          describeError(
                            err,
                            t(
                              "Recovery restore failed. Existing files were preserved.",
                              "Wiederherstellung fehlgeschlagen. Bestehende Dateien blieben erhalten.",
                            ),
                          ),
                        );
                      } finally {
                        setBusy(false);
                      }
                    }}
                  >
                    {t("Restore bundle", "Paket wiederherstellen")}
                  </Action>
                </div>
              </div>
            )}
          </div>
        </section>
      )}
    </div>
  );
}

export function AccountSettings({
  account,
  accounts,
  status,
  view,
  onView,
  onStatus,
  onRefresh,
  onOpenWriteMode,
}: {
  account: Account;
  accounts: Account[];
  status: AccountStatus;
  view: AccountView | null;
  onView: (value: AccountView) => void;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [connError, setConnError] = useState("");
  const [connNotice, setConnNotice] = useState("");

  // Lifecycle recovery status
  const [lifecycle, setLifecycle] = useState<LifecycleStatus | null>(null);
  const [rotateModal, setRotateModal] = useState(false);

  // Forget account flow
  const [forgetPreview, setForgetPreview] = useState<ForgetPreview | null>(
    null,
  );
  const [forgetChecked, setForgetChecked] = useState(false);
  const [forgetError, setForgetError] = useState("");

  // Addresses
  const [addrError, setAddrError] = useState("");

  // Google Takeout import
  const [mboxPath, setMboxPath] = useState("");
  const [takeoutResult, setTakeoutResult] = useState<TakeoutSummary | null>(
    null,
  );
  const [takeoutError, setTakeoutError] = useState("");
  const [takeoutNotice, setTakeoutNotice] = useState("");

  const loadLifecycle = useCallback(async () => {
    if (status.source === "vault" || status.synthetic) {
      try {
        setLifecycle(await readLifecycle());
      } catch {}
    }
  }, [status.source, status.synthetic]);

  useEffect(() => {
    void loadLifecycle();
  }, [loadLifecycle]);

  async function reconnect() {
    setBusy(true);
    setConnError("");
    setConnNotice("");
    const entered = password;
    setPassword("");
    try {
      onStatus(await reconnectAccount(account.id, entered || undefined));
      setConnNotice(
        t(
          "Reconnection check started.",
          "Verbindungsprüfung gestartet.",
        ),
      );
    } catch (err) {
      setConnError(
        describeError(
          err,
          t(
            "Connection could not start. Check the app password and retry.",
            "Die Verbindung konnte nicht starten. Prüfe das App-Passwort und versuche es erneut.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runTakeoutImport() {
    if (!mboxPath.trim()) return;
    setBusy(true);
    setTakeoutError("");
    setTakeoutNotice("");
    setTakeoutResult(null);
    try {
      const summary = await importTakeout(account.id, mboxPath.trim());
      setTakeoutResult(summary);
      setTakeoutNotice(
        t(
          `Imported ${summary.imported.toLocaleString()} headers from Google Takeout into ${summary.folder} (bodies skipped).${summary.truncated ? " Import reached the 500,000-message cap." : ""}`,
          `${summary.imported.toLocaleString()} Kopfzeilen aus Google Takeout in ${summary.folder} importiert (Nachrichtentexte übersprungen).${summary.truncated ? " Importlimit von 500.000 Nachrichten erreicht." : ""}`,
        ),
      );
      onRefresh();
    } catch (err) {
      setTakeoutError(
        describeError(
          err,
          t(
            "Could not import .mbox file. Verify it is an existing, extracted .mbox file on this Mac.",
            ".mbox-Datei konnte nicht importiert werden. Prüfe, ob es eine entpackte .mbox-Datei auf diesem Mac ist.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const pendingLifecycle = (lifecycle?.recovery ?? []).filter(
    (r) => r.state !== "forgotten",
  );

  return (
    <div className="page-stack">
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "Account connection & lifecycle",
                "Kontoverbindung & Lebenszyklus",
              )}
            </h2>
            <p className="muted">
              {account.login} ·{" "}
              {account.provider === "gmail" ? "Gmail" : "mailbox.org"}
            </p>
          </div>
          <StatusBadge
            status={
              account.connection === "observed"
                ? "observed"
                : account.connection === "failed"
                  ? "failed"
                  : "unverified"
            }
            label={
              account.connection === "observed"
                ? t("Connection verified", "Verbindung geprüft")
                : account.connection === "failed"
                  ? t("Reconnect needed", "Erneute Verbindung nötig")
                  : t("Not yet verified", "Noch nicht geprüft")
            }
          />
        </div>
        {connError && <Notice error>{connError}</Notice>}
        {connNotice && (
          <Notice>
            <CheckCircle2 size={16} />
            <span>{connNotice}</span>
          </Notice>
        )}
        {pendingLifecycle.length > 0 && (
          <Notice tone="warning">
            <span>
              {t(
                "An account lifecycle step was interrupted and needs recovery:",
                "Ein Konto-Lebenszyklusschritt wurde unterbrochen und muss abgeschlossen werden:",
              )}
            </span>
            <div className="actions">
              {pendingLifecycle.map((rec) => (
                <Action
                  key={rec.account}
                  size="compact"
                  disabled={busy}
                  onClick={async () => {
                    setBusy(true);
                    setConnError("");
                    try {
                      await recoverAccount(rec.account);
                      setConnNotice(
                        t(
                          `Recovered pending state for ${rec.account.slice(0, 8)}.`,
                          `Ausstehender Zustand für ${rec.account.slice(0, 8)} abgeschlossen.`,
                        ),
                      );
                      await loadLifecycle();
                      onRefresh();
                    } catch (err) {
                      setConnError(
                        describeError(
                          err,
                          t(
                            "Could not recover lifecycle state.",
                            "Wiederherstellung fehlgeschlagen.",
                          ),
                        ),
                      );
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t(`Recover (${rec.state})`, `Wiederherstellen (${rec.state})`)}
                </Action>
              ))}
            </div>
          </Notice>
        )}
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
        {(status.source === "vault" || status.synthetic) && (
          <div className="danger-zone">
            <h3>{t("Account key & local removal", "Kontoschlüssel & lokales Entfernen")}</h3>
            <p className="help">
              {t(
                "Rotating re-encrypts this account's local store with a fresh key. Removing deletes only local Mailroom data and credentials — your mail on the provider stays untouched.",
                "Rotieren verschlüsselt den lokalen Speicher dieses Kontos mit einem neuen Schlüssel. Entfernen löscht nur lokale Mailroom-Daten und Zugangsdaten — deine Nachrichten beim Anbieter bleiben unberührt.",
              )}
            </p>
            <div className="actions">
              <Action disabled={busy} onClick={() => setRotateModal(true)}>
                <RotateCcw size={15} />
                {t("Rotate local key", "Lokalen Schlüssel rotieren")}
              </Action>
              <Action
                tone="danger-outline"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setConnError("");
                  setForgetError("");
                  setForgetChecked(false);
                  try {
                    setForgetPreview(await previewForget(account.id));
                  } catch (err) {
                    setConnError(
                      describeError(
                        err,
                        t(
                          "Could not preview account removal.",
                          "Entfernen des Kontos konnte nicht geprüft werden.",
                        ),
                      ),
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                <Trash2 size={15} />
                {t("Remove account", "Konto entfernen")}
              </Action>
            </div>
          </div>
        )}
      </section>

      <section className="panel">
        <h2>{t("Your addresses", "Deine Adressen")}</h2>
        <p className="muted">
          {t(
            "Confirm only addresses you own. Recipient observations can include other people.",
            "Bestätige nur Adressen, die dir gehören. Empfängerbeobachtungen können andere Personen enthalten.",
          )}
        </p>
        {addrError && <Notice error>{addrError}</Notice>}
        <Details
          title={`${t("Review address candidates", "Adresskandidaten prüfen")} · ${view?.addresses.length ?? 0}`}
        >
          <div className="address-list">
            {view?.addresses.map((address) => (
              <div className="address-row" key={address.address}>
                <span>{address.address}</span>
                <Action
                  size="compact"
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    setAddrError("");
                    void confirmAccount(
                      account.id,
                      address.address,
                      !address.confirmed,
                    )
                      .then(onView)
                      .catch(() =>
                        setAddrError(
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

      <section className="panel">
        <h2>
          <Upload size={17} />{" "}
          {t(
            "Google Takeout (.mbox) local import",
            "Google Takeout (.mbox) lokal importieren",
          )}
        </h2>
        <p className="muted">
          {t(
            "Import headers and X-Gmail-Labels from an extracted local Takeout .mbox file into this account's encrypted census. Message bodies are skipped and never stored.",
            "Importiere Kopfzeilen und X-Gmail-Labels aus einer entpackten lokalen Takeout-.mbox-Datei in die verschlüsselte Erfassung. Nachrichtentexte werden nie gespeichert.",
          )}
        </p>
        {takeoutError && <Notice error>{takeoutError}</Notice>}
        {takeoutNotice && (
          <Notice>
            <CheckCircle2 size={16} />
            <span>{takeoutNotice}</span>
          </Notice>
        )}
        <div className="settings-form">
          <label className="field">
            {t("Local .mbox file path", "Lokaler Pfad zur .mbox-Datei")}
            <input
              value={mboxPath}
              onChange={(e) => setMboxPath(e.target.value)}
              placeholder="~/Downloads/Takeout/Mail/All mail Including Spam and Trash.mbox"
            />
          </label>
          <p className="help">
            {t(
              "Tip on Mac: select the extracted .mbox file in Finder and press ⌥⌘C to copy its path (leading ~/ and surrounding quotes are supported). Re-importing replaces this account's Takeout folder snapshot.",
              "Mac-Tipp: Markiere die entpackte .mbox-Datei im Finder und drücke ⌥⌘C, um den Pfad zu kopieren (~/ und Anführungszeichen werden automatisch verarbeitet). Ein erneuter Import ersetzt den Takeout-Stand dieses Kontos.",
            )}
          </p>
          <Action
            primary
            disabled={busy || !mboxPath.trim()}
            onClick={() => void runTakeoutImport()}
          >
            {busy ? (
              <Busy>
                {t("Importing headers…", "Kopfzeilen werden importiert…")}
              </Busy>
            ) : (
              t("Import .mbox headers", ".mbox-Kopfzeilen importieren")
            )}
          </Action>
        </div>
        {takeoutResult && (
          <p className="help" style={{ marginTop: 10 }}>
            {t("Last import", "Letzter Import")}:{" "}
            {takeoutResult.imported.toLocaleString()}{" "}
            {t("imported", "importiert")},{" "}
            {takeoutResult.skipped.toLocaleString()}{" "}
            {t("skipped", "übersprungen")},{" "}
            {takeoutResult.labels.toLocaleString()} {t("labels", "Labels")}.
          </p>
        )}
      </section>

      <WorkspaceSettings
        accounts={accounts}
        status={status}
        onStatus={onStatus}
        onRefresh={onRefresh}
        onOpenWriteMode={onOpenWriteMode}
      />

      <Modal
        open={rotateModal}
        onOpenChange={(open) => {
          if (!busy) setRotateModal(open);
        }}
        title={t("Rotate local encryption key?", "Lokalen Schlüssel rotieren?")}
        description={t(
          `Re-encrypts ${account.login}'s local census and rules with a newly generated key. Your provider account is not contacted.`,
          `Verschlüsselt die lokale Erfassung und Regeln von ${account.login} mit einem neuen Schlüssel. Dein Anbieterkonto wird nicht kontaktiert.`,
        )}
      >
        <div className="dialog-actions">
          <Action onClick={() => setRotateModal(false)} disabled={busy}>
            {t("Cancel", "Abbrechen")}
          </Action>
          <Action
            primary
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setConnError("");
              try {
                await rotateAccount(account.id);
                setRotateModal(false);
                setConnNotice(
                  t(
                    "Local encryption key rotated for this account.",
                    "Lokaler Verschlüsselungsschlüssel für dieses Konto rotiert.",
                  ),
                );
              } catch (err) {
                setConnError(
                  describeError(
                    err,
                    t(
                      "Could not rotate local key.",
                      "Schlüsselrotation fehlgeschlagen.",
                    ),
                  ),
                );
                setRotateModal(false);
              } finally {
                setBusy(false);
              }
            }}
          >
            <RotateCcw size={15} />
            {t("Rotate key", "Schlüssel rotieren")}
          </Action>
        </div>
      </Modal>

      <Modal
        open={forgetPreview !== null}
        onOpenChange={(open) => {
          if (!busy && !open) {
            setForgetPreview(null);
            setForgetError("");
          }
        }}
        title={t("Remove account locally?", "Konto lokal entfernen?")}
      >
        {forgetPreview && (
          <div className="page-stack">
            <p>
              <strong>{account.login}</strong> ·{" "}
              {forgetPreview.resources.length}{" "}
              {t("local files", "lokale Dateien")}
            </p>
            <p className="muted">{forgetPreview.consequences[language]}</p>
            <CheckField checked={forgetChecked} onChange={setForgetChecked}>
              {t(
                "Remove this account's local encrypted census, rules, and stored credentials from this Mac. Provider mail is untouched.",
                "Lokale verschlüsselte Erfassung, Regeln und gespeicherte Zugangsdaten dieses Kontos entfernen. Nachrichten beim Anbieter bleiben unberührt.",
              )}
            </CheckField>
            {forgetError && <Notice error>{forgetError}</Notice>}
            <div className="dialog-actions">
              <Action
                onClick={() => {
                  setForgetPreview(null);
                  setForgetError("");
                }}
                disabled={busy}
              >
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                tone="danger"
                disabled={!forgetChecked || busy}
                onClick={async () => {
                  setBusy(true);
                  setForgetError("");
                  try {
                    await forgetAccount(account.id, forgetPreview);
                    setForgetPreview(null);
                    onRefresh();
                  } catch (err) {
                    setForgetError(
                      describeError(
                        err,
                        t(
                          "Could not remove account.",
                          "Konto konnte nicht entfernt werden.",
                        ),
                      ),
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                <Trash2 size={15} />
                {t("Remove account", "Konto entfernen")}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
