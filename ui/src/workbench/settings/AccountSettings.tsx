import { useCallback, useContext, useEffect, useState } from "react";
import {
  CheckCircle2,
  FolderOpen,
  RotateCcw,
  Trash2,
  Upload,
} from "lucide-react";
import {
  confirmAccount,
  forgetAccount,
  importTakeout,
  pickTakeoutFile,
  previewForget,
  readLifecycle,
  reconnectAccount,
  recoverAccount,
  rotateAccount,
} from "../../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  ForgetPreview,
  LifecycleStatus,
  TakeoutSummary,
} from "../../api";
import { describeError } from "../status-labels";
import { formatNumber, pluralCount } from "../format";
import {
  Action,
  Busy,
  CheckField,
  Details,
  LanguageContext,
  Modal,
  Notice,
  StatusBadge,
  useCopy,
} from "../components";
import { WorkspaceSettings } from "./WorkspaceSettings";

export function AccountSettings({
  account,
  accounts,
  status,
  view,
  onView,
  onStatus,
  onRefresh,
  onOpenWriteMode,
  shortcutsEnabled,
  onShortcutsEnabled,
}: {
  account: Account;
  accounts: Account[];
  status: AccountStatus;
  view: AccountView | null;
  onView: (value: AccountView) => void;
  onStatus: (value: AccountStatus) => void;
  onRefresh: () => void;
  onOpenWriteMode: () => void;
  shortcutsEnabled?: boolean;
  onShortcutsEnabled?: (next: boolean) => void;
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
  const [picking, setPicking] = useState(false);
  const [takeoutResult, setTakeoutResult] = useState<TakeoutSummary | null>(
    null,
  );
  const [takeoutError, setTakeoutError] = useState("");
  const [takeoutNotice, setTakeoutNotice] = useState("");

  const loadLifecycle = useCallback(async () => {
    if (status.source === "vault" || status.synthetic) {
      try {
        setLifecycle(await readLifecycle());
      } catch (err) {
        setConnError(
          describeError(
            err,
            t(
              "Could not read account lifecycle status.",
              "Konto-Lebenszyklusstatus konnte nicht gelesen werden.",
            ),
          ),
        );
      }
    }
  }, [status.source, status.synthetic, t]);

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
        t("Reconnection check started.", "Verbindungsprüfung gestartet."),
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

  async function browseTakeout() {
    setPicking(true);
    setTakeoutError("");
    try {
      const picked = await pickTakeoutFile();
      if (picked.path) setMboxPath(picked.path);
      else if (!picked.supported) {
        setTakeoutError(
          t(
            "Native file picker is unavailable here. Paste the .mbox path manually.",
            "Nativer Dateiauswahldialog ist hier nicht verfügbar. Füge den .mbox-Pfad manuell ein.",
          ),
        );
      }
    } catch (err) {
      setTakeoutError(
        describeError(
          err,
          t(
            "Could not open file picker.",
            "Dateiauswahl konnte nicht geöffnet werden.",
          ),
        ),
      );
    } finally {
      setPicking(false);
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
          `Imported ${formatNumber(summary.imported, language)} headers from Google Takeout into ${summary.folder} (bodies skipped).${summary.truncated ? " Import reached the 500,000-message cap." : ""}`,
          `${formatNumber(summary.imported, language)} Kopfzeilen aus Google Takeout in ${summary.folder} importiert (Nachrichtentexte übersprungen).${summary.truncated ? " Importlimit von 500.000 Nachrichten erreicht." : ""}`,
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
      <nav
        className="settings-jump-nav"
        aria-label={t("Settings sections", "Einstellungsbereiche")}
      >
        <a href="#settings-connection">{t("Connection", "Verbindung")}</a>
        <a href="#settings-addresses">{t("Addresses", "Adressen")}</a>
        <a href="#settings-takeout">{t("Takeout", "Takeout")}</a>
        <a href="#settings-safety">{t("Safety", "Sicherheit")}</a>
        <a href="#settings-t3">{t("T3 assistant", "T3-Assistent")}</a>
        {(status.source === "vault" || status.synthetic) && (
          <a href="#settings-recovery">{t("Backup & restore", "Sicherung")}</a>
        )}
        {(status.source === "vault" || status.synthetic) && (
          <a href="#settings-danger">{t("Danger zone", "Entfernen & Schlüssel")}</a>
        )}
      </nav>
      <section id="settings-connection" className="panel">
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
        {connError && (
          <div id="conn-error">
            <Notice error>{connError}</Notice>
          </div>
        )}
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
                  {t(
                    `Recover (${rec.state})`,
                    `Wiederherstellen (${rec.state})`,
                  )}
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
              aria-invalid={Boolean(connError)}
              aria-describedby={connError ? "conn-error" : undefined}
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
      </section>

      <section id="settings-addresses" className="panel">
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

      <section id="settings-takeout" className="panel">
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
              "Choose the extracted .mbox file with Browse, or paste its local path. Re-importing replaces this account's Takeout snapshot.",
              "Wähle die entpackte .mbox-Datei über Durchsuchen aus oder füge den lokalen Pfad ein. Ein erneuter Import ersetzt den Takeout-Stand dieses Kontos.",
            )}
          </p>
          <div className="actions">
            {(status.source === "vault" || status.synthetic) && (
              <Action
                disabled={busy || picking}
                onClick={() => void browseTakeout()}
              >
                <FolderOpen size={15} />
                {picking
                  ? t("Opening Finder…", "Finder wird geöffnet…")
                  : t("Browse (.mbox)…", "Durchsuchen (.mbox)…")}
              </Action>
            )}
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
        </div>
        {takeoutResult && (
          <p className="help mt-sm">
            {t("Last import", "Letzter Import")}:{" "}
            {formatNumber(takeoutResult.imported, language)}{" "}
            {t("imported", "importiert")},{" "}
            {formatNumber(takeoutResult.skipped, language)}{" "}
            {t("skipped", "übersprungen")},{" "}
            {formatNumber(takeoutResult.labels, language)}{" "}
            {t("labels", "Labels")}.
          </p>
        )}
      </section>

      <WorkspaceSettings
        accounts={accounts}
        status={status}
        onStatus={onStatus}
        onRefresh={onRefresh}
        onOpenWriteMode={onOpenWriteMode}
        shortcutsEnabled={shortcutsEnabled}
        onShortcutsEnabled={onShortcutsEnabled}
      />

      {(status.source === "vault" || status.synthetic) && (
        <section id="settings-danger" className="panel">
          <div className="section-heading">
            <div>
              <h2>
                {t(
                  "Account key & local removal",
                  "Kontoschlüssel & lokales Entfernen",
                )}
              </h2>
              <p className="muted">
                {t(
                  "Rotating re-encrypts this account's local store with a fresh key. Removing deletes only local Mailroom data and credentials; your mail on the provider stays untouched.",
                  "Rotieren verschlüsselt den lokalen Speicher dieses Kontos mit einem neuen Schlüssel. Entfernen löscht nur lokale Mailroom-Daten und Zugangsdaten; deine Nachrichten beim Anbieter bleiben unberührt.",
                )}
              </p>
            </div>
          </div>
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
        </section>
      )}

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
              {pluralCount(
                forgetPreview.resources.length,
                language,
                "local file",
                "local files",
                "lokale Datei",
                "lokale Dateien",
              )}
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
