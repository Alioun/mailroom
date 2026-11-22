import { useCallback, useEffect, useRef, useState } from "react";
import { Archive, CheckCircle2, FileUp } from "lucide-react";
import type { Account, AccountStatus } from "../../api";
import {
  capture,
  importRecoveryBundle,
  previewCapture,
  previewRestore,
  recoveryStatus,
  restore,
  resumeRestore,
} from "../../recovery-api";
import type {
  CaptureOffer,
  RecoveryStatus,
  RestoreOffer,
} from "../../recovery-api";
import { describeError } from "../status-labels";
import {
  Action,
  CheckField,
  Choice,
  Notice,
  useCopy,
} from "../components";

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

export function RecoverySection({
  accounts,
  status,
  onRefresh,
}: {
  accounts: Account[];
  status: AccountStatus;
  onRefresh: () => void;
}) {
  const t = useCopy();
  const [busy, setBusy] = useState(false);
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
  const [dragging, setDragging] = useState(false);
  const [recError, setRecError] = useState("");
  const [recNotice, setRecNotice] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setBackupSelected((prev) => {
      const next: Record<string, boolean> = {};
      for (const acc of accounts) {
        next[acc.id] = prev[acc.id] ?? true;
      }
      return next;
    });
  }, [accounts]);

  const loadRecovery = useCallback(async () => {
    if (status.source !== "vault" && !status.synthetic) return;
    try {
      const rec = await recoveryStatus();
      setRecStatus(rec);
      const newestId =
        rec.bundle_info[0]?.id ?? rec.bundles[rec.bundles.length - 1] ?? "";
      setRestoreBundle((cur) =>
        cur && rec.bundles.includes(cur) ? cur : newestId,
      );
    } catch (err) {
      setRecError(
        describeError(
          err,
          t(
            "Could not load recovery bundle status.",
            "Sicherungsstatus konnte nicht geladen werden.",
          ),
        ),
      );
    }
  }, [status.source, status.synthetic, t]);

  useEffect(() => {
    void loadRecovery();
  }, [loadRecovery]);

  async function handleBundleFile(file: File | null | undefined) {
    if (!file) return;
    setBusy(true);
    setRecError("");
    setRecNotice("");
    try {
      const buf = await file.arrayBuffer();
      const base64 = arrayBufferToBase64(buf);
      const res = await importRecoveryBundle(file.name, base64);
      setRecNotice(
        res.already_present
          ? t(
              `Bundle ${res.id.slice(0, 12)} is already in your local bundles folder and selected below.`,
              `Paket ${res.id.slice(0, 12)} ist bereits lokal vorhanden und unten ausgewählt.`,
            )
          : t(
              `Imported recovery bundle ${res.filename} (${formatBytes(res.bytes)}). Enter its passphrase below to preview restore.`,
              `Sicherungspaket ${res.filename} (${formatBytes(res.bytes)}) importiert. Gib unten die Passphrase ein, um die Wiederherstellung zu prüfen.`,
            ),
      );
      await loadRecovery();
      setRestoreBundle(res.id);
      setRestoreOffer(null);
    } catch (err) {
      setRecError(
        describeError(
          err,
          t(
            "Could not import recovery bundle file. Make sure it is an unmodified <64-hex-id>.bundle or .jwe file.",
            "Sicherungspaket konnte nicht importiert werden. Stelle sicher, dass es eine unveränderte <64-Hex-ID>.bundle- oder .jwe-Datei ist.",
          ),
        ),
      );
    } finally {
      setBusy(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  if (status.source !== "vault" && !status.synthetic) return null;

  const chosenBackupAccounts = accounts
    .filter((a) => backupSelected[a.id])
    .map((a) => a.id);

  const bundleOptions = recStatus?.bundle_info.length
    ? recStatus.bundle_info.map((b) => ({
        value: b.id,
        label: `${b.id.slice(0, 12)} · ${b.modified_at ? new Date(b.modified_at * 1000).toLocaleString() : "—"} · ${formatBytes(b.bytes)}`,
      }))
    : (recStatus?.bundles ?? []).map((id) => ({
        value: id,
        label: id.slice(0, 16),
      }));

  return (
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
        <p className="help mt-xs">
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
              busy || (status.source === "vault" && restorePass.length < 12)
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
                await loadRecovery();
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
          <h3>
            {t("Create encrypted backup", "Verschlüsselte Sicherung erstellen")}
          </h3>
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
                      await loadRecovery();
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
        <div
          className={`dropzone ${dragging ? "dragging" : ""}`}
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragging(false);
            void handleBundleFile(e.dataTransfer.files?.[0]);
          }}
        >
          <FileUp size={20} />
          <span className="muted">
            {t(
              "Drop a .bundle or .jwe file here to import it into your local bundles folder",
              "Ziehe eine .bundle- oder .jwe-Datei hierher, um sie in den lokalen Sicherungsordner zu importieren",
            )}
          </span>
          <input
            ref={fileInputRef}
            type="file"
            accept=".bundle,.jwe"
            aria-label={t(
              "Upload recovery bundle file",
              "Sicherungspaket-Datei hochladen",
            )}
            onChange={(e) => void handleBundleFile(e.target.files?.[0])}
          />
          <Action
            size="compact"
            disabled={busy}
            onClick={() => fileInputRef.current?.click()}
          >
            {t("Choose bundle file…", "Paketdatei auswählen…")}
          </Action>
        </div>
        {bundleOptions.length === 0 ? (
          <p className="muted mt-sm">
            {t(
              "No recovery bundles found in the local bundles folder yet.",
              "Noch keine Sicherungspakete im lokalen Sicherungsordner vorhanden.",
            )}
          </p>
        ) : (
          <div className="settings-form mt-sm">
            <Choice
              fieldLabel={t(
                "Saved recovery bundle",
                "Gespeichertes Sicherungspaket",
              )}
              label={t(
                "Saved recovery bundle",
                "Gespeichertes Sicherungspaket",
              )}
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
                    await loadRecovery();
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
  );
}
