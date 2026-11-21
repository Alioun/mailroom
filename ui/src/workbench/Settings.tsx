import { useCallback, useEffect, useState } from "react";
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
  reconnectAccount,
  rotateAccount,
  setWriteMode,
} from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  ForgetPreview,
  TakeoutSummary,
} from "../api";
import {
  capture,
  previewCapture,
  previewRestore,
  recoveryStatus,
  restore,
} from "../recovery-api";
import type {
  CaptureOffer,
  RecoveryStatus,
  RestoreOffer,
} from "../recovery-api";
import { getAssistantConfig, saveAssistantConfig } from "./assistant-api";
import {
  Action,
  Busy,
  CheckField,
  Details,
  Modal,
  Notice,
  useCopy,
} from "./components";

export function AccountSettings({
  account,
  accounts,
  status,
  view,
  onView,
  onStatus,
  onRefresh,
}: {
  account: Account;
  accounts: Account[];
  status: AccountStatus;
  view: AccountView | null;
  onView: (value: AccountView) => void;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
}) {
  const t = useCopy();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  // Safety mode confirmation dialog
  const [armDialog, setArmDialog] = useState(false);
  const [armChecked, setArmChecked] = useState(false);

  // Forget account flow
  const [forgetPreview, setForgetPreview] = useState<ForgetPreview | null>(
    null,
  );
  const [forgetChecked, setForgetChecked] = useState(false);

  // Google Takeout import
  const [mboxPath, setMboxPath] = useState("");
  const [takeoutResult, setTakeoutResult] = useState<TakeoutSummary | null>(
    null,
  );

  // T3 Assistant config
  const [t3Provider, setT3Provider] = useState("codex_home_v2");
  const [t3Model, setT3Model] = useState("gpt-6.1-sol");

  // Local vault password change
  const [curPass, setCurPass] = useState("");
  const [newPass, setNewPass] = useState("");
  const [confirmPass, setConfirmPass] = useState("");

  // Encrypted recovery backup & restore
  const [recStatus, setRecStatus] = useState<RecoveryStatus | null>(null);
  const [recPass, setRecPass] = useState("");
  const [captureOffer, setCaptureOffer] = useState<CaptureOffer | null>(null);
  const [restoreBundle, setRestoreBundle] = useState("");
  const [restoreOffer, setRestoreOffer] = useState<RestoreOffer | null>(null);

  const loadMeta = useCallback(async () => {
    try {
      const cfg = await getAssistantConfig(account.id);
      setT3Provider(cfg.provider);
      setT3Model(cfg.model);
    } catch {}
    if (status.source === "vault" || status.synthetic) {
      try {
        const rec = await recoveryStatus();
        setRecStatus(rec);
        if (rec.bundles.length > 0 && !restoreBundle) {
          setRestoreBundle(rec.bundles[rec.bundles.length - 1] ?? "");
        }
      } catch {}
    }
  }, [account.id, status.source, status.synthetic]);

  useEffect(() => {
    void loadMeta();
  }, [loadMeta]);

  async function reconnect() {
    setBusy(true);
    setError("");
    setNotice("");
    const entered = password;
    setPassword("");
    try {
      onStatus(await reconnectAccount(account.id, entered || undefined));
    } catch {
      setError(
        t(
          "Connection could not start. Check the app password and retry.",
          "Die Verbindung konnte nicht starten. Prüfe das App-Passwort und versuche es erneut.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function toggleWriteMode(enable: boolean) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      onStatus(await setWriteMode(enable, enable));
      setArmDialog(false);
      setArmChecked(false);
      setNotice(
        enable
          ? t(
              "Live provider writes armed for this session. Every action still requires its own preview and confirmation. Restarting or locking the vault resets to read-only.",
              "Live-Schreibzugriff ist für diese Sitzung aktiviert. Jede Aktion erfordert weiterhin eine eigene Vorschau und Bestätigung. Neustart oder Sperren setzt auf Nur-Lese zurück.",
            )
          : t(
              "Returned to read-only mode. No provider writes can execute.",
              "Zurück im Nur-Lese-Modus. Änderungen beim Anbieter sind gesperrt.",
            ),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t("Could not change safety mode.", "Sicherheitsmodus konnte nicht geändert werden."),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runTakeoutImport() {
    if (!mboxPath.trim()) return;
    setBusy(true);
    setError("");
    setNotice("");
    setTakeoutResult(null);
    try {
      const summary = await importTakeout(account.id, mboxPath.trim());
      setTakeoutResult(summary);
      setNotice(
        t(
          `Imported ${summary.imported} headers from Google Takeout into ${summary.folder} (bodies skipped).`,
          `${summary.imported} Kopfzeilen aus Google Takeout in ${summary.folder} importiert (Nachrichtentexte übersprungen).`,
        ),
      );
      onRefresh();
    } catch {
      setError(
        t(
          "Could not import .mbox file. Verify it is an existing, readable regular .mbox file on this Mac.",
          ".mbox-Datei konnte nicht importiert werden. Prüfe, ob es eine lesbare reguläre .mbox-Datei auf diesem Mac ist.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function saveT3Config() {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const saved = await saveAssistantConfig(
        account.id,
        t3Provider.trim(),
        t3Model.trim(),
      );
      setT3Provider(saved.provider);
      setT3Model(saved.model);
      setNotice(
        t(
          "T3 assistant configuration saved.",
          "T3-Assistent-Konfiguration gespeichert.",
        ),
      );
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t(
              "Invalid provider or model name.",
              "Ungültiger Anbieter- oder Modellname.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runChangeMasterPassword() {
    setBusy(true);
    setError("");
    setNotice("");
    const current = curPass;
    const next = newPass;
    const confirm = confirmPass;
    setCurPass("");
    setNewPass("");
    setConfirmPass("");
    try {
      onStatus(await changeVaultPassword(current, next, confirm));
      setNotice(
        t(
          "Master password updated and vault re-encrypted.",
          "Master-Passwort aktualisiert und Tresor neu verschlüsselt.",
        ),
      );
    } catch (err) {
      const code = err instanceof VaultRefusal ? err.code : "";
      setError(
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

  return (
    <div className="page-stack">
      {error && <Notice error>{error}</Notice>}
      {notice && (
        <Notice>
          <CheckCircle2 size={16} />
          {notice}
        </Notice>
      )}

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Provider safety mode", "Sicherheitsmodus")}</h2>
            <p className="muted">
              {t(
                "Mailroom always starts in read-only mode. Arm Live writes only when you are ready to create folders, deploy Sieve rules, sort backlog mail, send invoices, or unsubscribe.",
                "Mailroom startet immer im Nur-Lese-Modus. Aktiviere Live-Schreibzugriff erst, wenn du Ordner anlegen, Sieve-Regeln aktivieren, den Bestand sortieren, Rechnungen senden oder dich abmelden möchtest.",
              )}
            </p>
          </div>
          <span
            className={`badge ${status.live_writes ? "warning" : ""}`}
          >
            {status.live_writes ? (
              <>
                <ShieldAlert size={14} />{" "}
                {t("Live writes armed", "Live-Schreibzugriff aktiv")}
              </>
            ) : (
              <>
                <ShieldCheck size={14} />{" "}
                {t("Read-only mode", "Nur-Lese-Modus")}
              </>
            )}
          </span>
        </div>
        <div className="actions">
          {status.live_writes ? (
            <Action
              primary
              disabled={busy}
              onClick={() => void toggleWriteMode(false)}
            >
              <ShieldCheck size={15} />
              {t("Return to read-only", "Zurück zu Nur-Lese")}
            </Action>
          ) : (
            <Action
              disabled={busy}
              onClick={() => {
                setArmChecked(false);
                setArmDialog(true);
              }}
            >
              <ShieldAlert size={15} />
              {t("Enable live writes for this session", "Live-Schreibzugriff für diese Sitzung aktivieren")}
            </Action>
          )}
        </div>
      </section>

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Account connection & lifecycle", "Kontoverbindung & Lebenszyklus")}</h2>
            <p className="muted">
              {account.login} ·{" "}
              {account.provider === "gmail" ? "Gmail" : "mailbox.org"}
            </p>
          </div>
          <span
            className={`badge ${account.connection === "failed" ? "warning" : ""}`}
          >
            {account.connection === "observed"
              ? t("Connection verified", "Verbindung geprüft")
              : account.connection === "failed"
                ? t("Reconnect needed", "Erneute Verbindung nötig")
                : t("Not yet verified", "Noch nicht geprüft")}
          </span>
        </div>
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
          <div className="actions">
            <Action
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setError("");
                try {
                  await rotateAccount(account.id);
                  setNotice(
                    t(
                      "Local encryption key rotated for this account.",
                      "Lokaler Verschlüsselungsschlüssel für dieses Konto rotiert.",
                    ),
                  );
                } catch {
                  setError(
                    t(
                      "Could not rotate local key.",
                      "Schlüsselrotation fehlgeschlagen.",
                    ),
                  );
                } finally {
                  setBusy(false);
                }
              }}
            >
              <RotateCcw size={15} />
              {t("Rotate local key", "Lokalen Schlüssel rotieren")}
            </Action>
            <Action
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setError("");
                setForgetChecked(false);
                try {
                  setForgetPreview(await previewForget(account.id));
                } catch {
                  setError(
                    t(
                      "Could not preview account removal.",
                      "Entfernen des Kontos konnte nicht geprüft werden.",
                    ),
                  );
                } finally {
                  setBusy(false);
                }
              }}
            >
              <Trash2 size={15} />
              {t("Forget account", "Konto entfernen")}
            </Action>
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
        <Details
          title={`${t("Review address candidates", "Adresskandidaten prüfen")} · ${view?.addresses.length ?? 0}`}
        >
          <div className="address-list">
            {view?.addresses.map((address) => (
              <div className="address-row" key={address.address}>
                <span>{address.address}</span>
                <Action
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void confirmAccount(
                      account.id,
                      address.address,
                      !address.confirmed,
                    )
                      .then(onView)
                      .catch(() =>
                        setError(
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
          {t("Google Takeout (.mbox) local import", "Google Takeout (.mbox) lokal importieren")}
        </h2>
        <p className="muted">
          {t(
            "Import headers and X-Gmail-Labels from a local Takeout .mbox export on this Mac into the selected account's encrypted census. Message bodies are skipped and never stored.",
            "Importiere Kopfzeilen und X-Gmail-Labels aus einer lokalen Takeout-.mbox-Datei auf diesem Mac in die verschlüsselte Erfassung. Nachrichtentexte werden nie gespeichert.",
          )}
        </p>
        <div className="settings-form">
          <label className="field">
            {t("Local .mbox file path", "Lokaler Pfad zur .mbox-Datei")}
            <input
              value={mboxPath}
              onChange={(e) => setMboxPath(e.target.value)}
              placeholder="/Users/you/Downloads/Takeout/Mail/All mail.mbox"
            />
          </label>
          <Action
            primary
            disabled={busy || !mboxPath.trim()}
            onClick={() => void runTakeoutImport()}
          >
            {busy ? (
              <Busy>{t("Importing headers…", "Kopfzeilen werden importiert…")}</Busy>
            ) : (
              t("Import .mbox headers", ".mbox-Kopfzeilen importieren")
            )}
          </Action>
        </div>
        {takeoutResult && (
          <p className="help">
            {t("Last import", "Letzter Import")}: {takeoutResult.imported}{" "}
            {t("imported", "importiert")}, {takeoutResult.skipped}{" "}
            {t("skipped", "übersprungen")}, {takeoutResult.labels}{" "}
            {t("labels", "Labels")}.
          </p>
        )}
      </section>

      <section className="panel">
        <h2>
          <Sparkles size={17} />{" "}
          {t("T3 assistant settings", "T3-Assistent-Einstellungen")}
        </h2>
        <p className="muted">
          {t(
            "Choose which T3 provider instance and model Mailroom passes when launching an isolated recommendation review.",
            "Wähle Anbieterinstanz und Modell für isolierte T3-Vorschlagsprüfungen.",
          )}
        </p>
        <div className="settings-form">
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
          <Action primary disabled={busy} onClick={() => void saveT3Config()}>
            {t("Save T3 settings", "T3-Einstellungen speichern")}
          </Action>
          <Action
            disabled={busy}
            onClick={() => {
              setT3Provider("codex_home_v2");
              setT3Model("gpt-6.1-sol");
            }}
          >
            {t("Reset defaults", "Standardwerte")}
          </Action>
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
                try {
                  onStatus(await lockVault());
                } finally {
                  setBusy(false);
                }
              }}
            >
              <Lock size={15} />
              {t("Lock vault now", "Tresor jetzt sperren")}
            </Action>
          </div>
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
                {t("Confirm new master password", "Neues Master-Passwort bestätigen")}
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
            {t("Encrypted recovery backup & restore", "Verschlüsselte Sicherung & Wiederherstellung")}
          </h2>
          <p className="muted">
            {t(
              "Capture an authenticated encrypted recovery bundle of your accounts and private address ledger using a separate recovery passphrase (at least 12 characters).",
              "Erstelle ein verschlüsseltes Wiederherstellungs-Paket deiner Konten und des Adressverzeichnisses mit einer separaten Passphrase (mindestens 12 Zeichen).",
            )}
          </p>
          {status.source === "vault" && (
            <label className="field">
              {t("Recovery passphrase (min. 12 chars)", "Wiederherstellungs-Passphrase (mind. 12 Zeichen)")}
              <input
                type="password"
                autoComplete="off"
                value={recPass}
                onChange={(e) => setRecPass(e.target.value)}
              />
            </label>
          )}
          <div className="actions">
            <Action
              primary
              disabled={
                busy ||
                accounts.length === 0 ||
                (status.source === "vault" && recPass.length < 12)
              }
              onClick={async () => {
                setBusy(true);
                setError("");
                try {
                  setCaptureOffer(
                    await previewCapture(
                      accounts.map((a) => a.id),
                      true,
                      status.source === "vault" ? recPass : undefined,
                    ),
                  );
                } catch {
                  setError(
                    t(
                      "Could not preview recovery capture.",
                      "Sicherungsvorschau fehlgeschlagen.",
                    ),
                  );
                } finally {
                  setBusy(false);
                }
              }}
            >
              {t("Preview encrypted backup", "Verschlüsselte Sicherung prüfen")}
            </Action>
          </div>
          {captureOffer && (
            <div className="subpanel">
              <p>{captureOffer.confirmation.display}</p>
              <div className="actions">
                <Action
                  primary
                  disabled={busy}
                  onClick={async () => {
                    setBusy(true);
                    setError("");
                    try {
                      const done = await capture(
                        captureOffer,
                        status.source === "vault" ? recPass : undefined,
                      );
                      setCaptureOffer(null);
                      setNotice(
                        t(
                          `Encrypted recovery bundle saved (${done.bundle.slice(0, 12)}).`,
                          `Verschlüsseltes Paket gespeichert (${done.bundle.slice(0, 12)}).`,
                        ),
                      );
                      await loadMeta();
                    } catch {
                      setError(
                        t("Recovery capture failed.", "Sicherung fehlgeschlagen."),
                      );
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t("Create backup bundle", "Sicherungs-Paket erstellen")}
                </Action>
                <Action onClick={() => setCaptureOffer(null)}>
                  {t("Cancel", "Abbrechen")}
                </Action>
              </div>
            </div>
          )}
          {recStatus && recStatus.bundles.length > 0 && (
            <Details
              title={`${t("Saved recovery bundles", "Gespeicherte Sicherungspakete")} · ${recStatus.bundles.length}`}
            >
              <div className="settings-form">
                <label className="field">
                  {t("Bundle ID", "Paket-ID")}
                  <select
                    value={restoreBundle}
                    onChange={(e) => setRestoreBundle(e.target.value)}
                  >
                    {recStatus.bundles.map((b) => (
                      <option key={b} value={b}>
                        {b}
                      </option>
                    ))}
                  </select>
                </label>
                <Action
                  disabled={
                    busy ||
                    !restoreBundle ||
                    (status.source === "vault" && recPass.length < 12)
                  }
                  onClick={async () => {
                    setBusy(true);
                    setError("");
                    try {
                      setRestoreOffer(
                        await previewRestore(
                          restoreBundle,
                          true,
                          status.source === "vault" ? recPass : undefined,
                        ),
                      );
                    } catch {
                      setError(
                        t(
                          "Could not preview restore. Accounts in the bundle must be absent locally and the passphrase must match.",
                          "Wiederherstellungsvorschau fehlgeschlagen. Konten im Paket dürfen lokal noch nicht vorhanden sein und die Passphrase muss stimmen.",
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
              {restoreOffer && (
                <div className="subpanel">
                  <p>{restoreOffer.confirmation.display}</p>
                  <div className="actions">
                    <Action
                      primary
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        try {
                          await restore(
                            restoreOffer,
                            status.source === "vault" ? recPass : undefined,
                          );
                          setRestoreOffer(null);
                          setNotice(
                            t(
                              "Recovery bundle restored locally. Restored accounts stay read-only until reconciled.",
                              "Sicherungspaket lokal wiederhergestellt. Konten bleiben bis zum Abgleich schreibgeschützt.",
                            ),
                          );
                          onRefresh();
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      {t("Restore bundle", "Paket wiederherstellen")}
                    </Action>
                    <Action onClick={() => setRestoreOffer(null)}>
                      {t("Cancel", "Abbrechen")}
                    </Action>
                  </div>
                </div>
              )}
            </Details>
          )}
        </section>
      )}

      <Modal
        open={armDialog}
        onOpenChange={(open) => {
          if (!busy) setArmDialog(open);
        }}
        title={t("Enable live provider writes?", "Live-Schreibzugriff aktivieren?")}
        description={t(
          "This unlocks live execution for ManageSieve deployment, IMAP folder creation, backlog moves/labels, SMTP invoice send-on, and unsubscribe requests for this browser session only.",
          "Dies entsperrt Live-Ausführung für ManageSieve, IMAP-Ordneranlage, Bestandssortierung, SMTP-Rechnungsversand und Abmeldungen nur für diese Sitzung.",
        )}
      >
        <div className="page-stack">
          <CheckField checked={armChecked} onChange={setArmChecked}>
            {t(
              "I understand that confirmed actions will modify my connected mail accounts. Every action still requires its own preview and confirmation.",
              "Mir ist bewusst, dass bestätigte Aktionen meine verbundenen Mailkonten verändern. Jede Aktion erfordert weiterhin Vorschau und Bestätigung.",
            )}
          </CheckField>
          <div className="dialog-actions">
            <Action onClick={() => setArmDialog(false)} disabled={busy}>
              {t("Stay read-only", "Im Nur-Lese-Modus bleiben")}
            </Action>
            <Action
              primary
              disabled={!armChecked || busy}
              onClick={() => void toggleWriteMode(true)}
            >
              {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
            </Action>
          </div>
        </div>
      </Modal>

      <Modal
        open={forgetPreview !== null}
        onOpenChange={(open) => {
          if (!busy && !open) setForgetPreview(null);
        }}
        title={t("Forget account locally?", "Konto lokal entfernen?")}
      >
        {forgetPreview && (
          <div className="page-stack">
            <p className="muted">{forgetPreview.consequences.en}</p>
            <CheckField checked={forgetChecked} onChange={setForgetChecked}>
              {t(
                "Remove this account's local encrypted census, rules, and stored credentials from this Mac. Provider mail is untouched.",
                "Lokale verschlüsselte Erfassung, Regeln und gespeicherte Zugangsdaten dieses Kontos entfernen. Nachrichten beim Anbieter bleiben unberührt.",
              )}
            </CheckField>
            <div className="dialog-actions">
              <Action onClick={() => setForgetPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              <Action
                primary
                disabled={!forgetChecked || busy}
                onClick={async () => {
                  setBusy(true);
                  setError("");
                  try {
                    await forgetAccount(account.id, forgetPreview);
                    setForgetPreview(null);
                    onRefresh();
                  } catch {
                    setError(
                      t(
                        "Could not remove account.",
                        "Konto konnte nicht entfernt werden.",
                      ),
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {t("Remove account", "Konto entfernen")}
              </Action>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
