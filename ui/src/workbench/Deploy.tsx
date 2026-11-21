import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  FolderPlus,
  RefreshCw,
  Rocket,
  Server,
  ShieldAlert,
  Trash2,
  Undo2,
} from "lucide-react";
import type { Account, AccountView } from "../api";
import {
  folderCommand,
  folderPreview,
  folderReconcile,
  folderStatus,
} from "../folder-provisioning-api";
import type { FolderPreview, FolderView } from "../folder-provisioning-api";
import {
  cancelSieve,
  exactReview,
  openSieve,
  sieveStatus,
  startSieve,
} from "../managesieve-api";
import type { SieveView } from "../managesieve-api";
import {
  reconcileTransaction,
  recordPlacement,
  transactionCommand,
  transactionPreview,
  transactionStatus,
} from "../sieve-transactions-api";
import type {
  Placement,
  TransactionPreview,
  TransactionView,
} from "../sieve-transactions-api";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  Notice,
  Stat,
  useCopy,
} from "./components";

export function FoldersPanel({
  account,
  liveWrites,
  onGoSettings,
}: {
  account: Account;
  liveWrites: boolean;
  onGoSettings: () => void;
}) {
  const t = useCopy();
  const [view, setView] = useState<FolderView | null>(null);
  const [preview, setPreview] = useState<FolderPreview | null>(null);
  const [intent, setIntent] = useState<"sort" | "archive_unmatched">("sort");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    setError("");
    setUnavailable(false);
    try {
      setView(await folderStatus(account.id));
    } catch {
      setUnavailable(true);
    }
  }, [account.id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function runPreview() {
    setBusy(true);
    setError("");
    setMessage("");
    setConfirmed(false);
    try {
      setPreview(await folderPreview(account.id, intent));
    } catch {
      setError(
        t(
          "Could not preview folders. Make sure local rules are saved and pass validation, and the server is reachable.",
          "Ordnervorschau fehlgeschlagen. Prüfe, ob lokale Regeln gespeichert und gültig sind und der Server erreichbar ist.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmCreate() {
    if (!preview || !confirmed) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await folderCommand(account.id, {
        action: "confirm",
        receipt: preview.receipt,
        confirmed: true,
        intent,
      });
      setPreview(null);
      setConfirmed(false);
      setMessage(
        t(
          "Folder creation started. Refresh status to inspect verified results.",
          "Ordnererstellung gestartet. Aktualisiere den Stand, um geprüfte Ergebnisse zu sehen.",
        ),
      );
      setTimeout(() => void load(), 1200);
    } catch {
      setError(
        liveWrites
          ? t(
              "Folder creation was refused or interrupted. Refresh status before retrying.",
              "Ordnererstellung wurde abgelehnt oder unterbrochen. Aktualisiere den Stand vor einem neuen Versuch.",
            )
          : t(
              "Live provider writes are locked in Read-only mode. Enable Live writes in Settings first.",
              "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff in den Einstellungen.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  if (unavailable) {
    return (
      <Empty
        title={t(
          "Folder provisioning unavailable",
          "Ordneranlage nicht verfügbar",
        )}
      >
        {t(
          "Connect a vault account or run with an installed folder fixture to preview and create IMAP folders.",
          "Verbinde ein Tresor-Konto, um IMAP-Ordner vorzuschauen und anzulegen.",
        )}
      </Empty>
    );
  }

  const missingEntries =
    preview?.operation.plan.entries.filter((e) => e.existing === null) ?? [];

  return (
    <div className="page-stack">
      {!liveWrites && (
        <Notice>
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. You can preview missing folders, but creating them requires enabling Live writes in Settings.",
              "Der Nur-Lese-Modus ist aktiv. Du kannst fehlende Ordner prüfen, aber zum Anlegen musst du Live-Schreibzugriff in den Einstellungen aktivieren.",
            )}
          </span>
          <Action onClick={onGoSettings}>
            {t("Open Settings", "Einstellungen öffnen")}
          </Action>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          {message}
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("IMAP folder provisioning", "IMAP-Ordner anlegen")}</h2>
            <p className="muted">
              {t(
                "Compare folders required by your saved rules against live IMAP mailboxes and create only what is missing.",
                "Vergleiche die Zielordner deiner gespeicherten Regeln mit dem IMAP-Postfach und lege nur fehlende Ordner an.",
              )}
            </p>
          </div>
          <Action onClick={() => void load()} disabled={busy}>
            <RefreshCw size={15} />
            {t("Refresh", "Aktualisieren")}
          </Action>
        </div>
        <div className="toolbar">
          <Choice
            label={t("Folder intent", "Ordnerzweck")}
            value={intent}
            onChange={(value) => {
              if (value === "sort" || value === "archive_unmatched")
                setIntent(value);
            }}
            options={[
              {
                value: "sort",
                label: t("Rule target folders", "Zielordner der Regeln"),
              },
              {
                value: "archive_unmatched",
                label: t(
                  "Archive/<year> unmatched folders",
                  "Archive/<Jahr> für nicht zugeordnete Mail",
                ),
              },
            ]}
          />
          <Action primary onClick={() => void runPreview()} disabled={busy}>
            {busy ? (
              <Busy>{t("Inspecting IMAP…", "IMAP wird geprüft…")}</Busy>
            ) : (
              <>
                <FolderPlus size={15} />
                {t("Preview missing folders", "Fehlende Ordner prüfen")}
              </>
            )}
          </Action>
        </div>
        {preview && (
          <div className="subpanel">
            <h3>
              {t("Folder creation plan", "Plan zur Ordnererstellung")} ·{" "}
              {missingEntries.length} {t("missing", "fehlend")}
            </h3>
            {missingEntries.length === 0 ? (
              <p className="muted">
                {t(
                  "All target folders already exist on the server.",
                  "Alle Zielordner existieren bereits auf dem Server.",
                )}
              </p>
            ) : (
              <>
                <ul className="evidence-list">
                  {preview.operation.plan.entries.map((entry) => (
                    <li key={entry.name.wire}>
                      <code>{entry.name.decoded}</code> ·{" "}
                      {entry.existing
                        ? t("Already exists", "Bereits vorhanden")
                        : entry.role === "ancestor"
                          ? t("Will create parent", "Überordner wird angelegt")
                          : t("Will create target", "Zielordner wird angelegt")}
                    </li>
                  ))}
                </ul>
                <CheckField checked={confirmed} onChange={setConfirmed}>
                  {t(
                    "I reviewed the exact folder list above and want to create these folders on the mail server.",
                    "Ich habe die genaue Ordnerliste oben geprüft und möchte diese Ordner auf dem Mailserver anlegen.",
                  )}
                </CheckField>
                <div className="actions">
                  <Action
                    primary
                    disabled={!confirmed || !liveWrites || busy}
                    onClick={() => void confirmCreate()}
                  >
                    <FolderPlus size={15} />
                    {t("Create folders", "Ordner anlegen")}
                  </Action>
                  <Action onClick={() => setPreview(null)} disabled={busy}>
                    {t("Cancel", "Abbrechen")}
                  </Action>
                </div>
              </>
            )}
          </div>
        )}
      </section>
      {view && view.journal.history.length > 0 && (
        <section className="panel">
          <h2>{t("Folder operation history", "Verlauf der Ordneranlage")}</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Operation", "Vorgang")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th>{t("Folders", "Ordner")}</th>
                  <th>{t("Action", "Aktion")}</th>
                </tr>
              </thead>
              <tbody>
                {view.journal.history.map((op) => (
                  <tr key={op.id}>
                    <td>
                      <code>{op.id.slice(0, 12)}</code>
                    </td>
                    <td>
                      <span className="badge">{op.status}</span>
                    </td>
                    <td>{op.plan.entries.length}</td>
                    <td>
                      {(op.status === "uncertain" ||
                        op.status === "stopped") && (
                        <Action
                          disabled={busy}
                          onClick={async () => {
                            setBusy(true);
                            try {
                              setView(await folderReconcile(account.id, op.id));
                            } finally {
                              setBusy(false);
                            }
                          }}
                        >
                          {t("Reconcile", "Abgleichen")}
                        </Action>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  );
}

export function SieveDeployPanel({
  account,
  liveWrites,
  onGoSettings,
}: {
  account: Account;
  liveWrites: boolean;
  onGoSettings: () => void;
}) {
  const t = useCopy();
  const [discovery, setDiscovery] = useState<SieveView | null>(null);
  const [txView, setTxView] = useState<TransactionView | null>(null);
  const [running, setRunning] = useState(false);
  const [placementKind, setPlacementKind] = useState<"block" | "include">(
    "block",
  );
  const [includeScript, setIncludeScript] = useState("mailroom");
  const [preview, setPreview] = useState<TransactionPreview | null>(null);
  const [previewKind, setPreviewKind] = useState<
    "deploy" | "removal" | { compensates: string }
  >("deploy");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  const load = useCallback(async () => {
    if (account.provider !== "mailbox_org") return;
    setError("");
    setUnavailable(false);
    try {
      const [sieve, status, tx] = await Promise.all([
        openSieve(account.id),
        sieveStatus(account.id),
        transactionStatus(account.id),
      ]);
      setDiscovery(sieve);
      setRunning(status.running);
      setTxView(tx);
      if (tx.placement) {
        setPlacementKind(tx.placement.kind);
        if (tx.placement.kind === "include")
          setIncludeScript(tx.placement.script);
      } else if (tx.recommendation?.recommended) {
        setPlacementKind(tx.recommendation.recommended);
      }
    } catch {
      setUnavailable(true);
    }
  }, [account.id, account.provider]);

  useEffect(() => {
    void load();
  }, [load]);

  if (account.provider !== "mailbox_org") {
    return (
      <Empty
        title={t(
          "ManageSieve is for mailbox.org accounts",
          "ManageSieve gilt für mailbox.org-Konten",
        )}
      >
        {t(
          "Your Gmail plan forwards incoming mail to mailbox.org and sorts existing Gmail mail via labels in the Backlog tab.",
          "Dein Gmail-Plan leitet neue Nachrichten an mailbox.org weiter und sortiert den Gmail-Bestand über Labels im Tab Bestand sortieren.",
        )}
      </Empty>
    );
  }

  if (unavailable) {
    return (
      <Empty
        title={t(
          "ManageSieve deployment unavailable",
          "ManageSieve-Bereitstellung nicht verfügbar",
        )}
      >
        {t(
          "Connect a mailbox.org vault account to inspect Open-Xchange rules and deploy Sieve scripts.",
          "Verbinde ein mailbox.org-Tresorkonto, um Open-Xchange-Regeln zu prüfen und Sieve-Skripte bereitzustellen.",
        )}
      </Empty>
    );
  }

  async function runDiscovery() {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await startSieve(account.id);
      setRunning(true);
      setMessage(
        t(
          "ManageSieve discovery started (read-only). Click Refresh shortly to inspect capabilities and Open-Xchange rules.",
          "ManageSieve-Prüfung gestartet (nur lesend). Klicke gleich auf Aktualisieren, um Fähigkeiten und Open-Xchange-Regeln zu sehen.",
        ),
      );
      setTimeout(() => void load(), 1500);
    } catch {
      setError(
        t(
          "Could not start ManageSieve discovery. Check connectivity and credentials.",
          "ManageSieve-Prüfung konnte nicht starten. Prüfe Verbindung und Zugangsdaten.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function savePlacement() {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      const next: Placement =
        placementKind === "include"
          ? { kind: "include", script: includeScript.trim() || "mailroom" }
          : { kind: "block" };
      const updated = await recordPlacement(account.id, next);
      setTxView(updated);
      setPreview(null);
      setMessage(
        t(
          "Coexistence placement choice saved.",
          "Koexistenz-Strategie gespeichert.",
        ),
      );
    } catch {
      setError(
        t(
          "Could not save placement choice. Check the included script name.",
          "Koexistenz-Strategie konnte nicht gespeichert werden. Prüfe den Skriptnamen.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function runPreview(
    kind: "deploy" | "removal" | { compensates: string },
  ) {
    setBusy(true);
    setError("");
    setMessage("");
    setConfirmed(false);
    setPreviewKind(kind);
    try {
      setPreview(await transactionPreview(account.id, kind));
    } catch {
      setError(
        t(
          "Could not preview Sieve transaction. Save valid local rules, choose a placement strategy, and check connectivity.",
          "Sieve-Vorschau fehlgeschlagen. Speichere gültige lokale Regeln, wähle eine Platzierung und prüfe die Verbindung.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function confirmTransaction() {
    if (!preview || preview.outcome !== "ready" || !confirmed) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      if (previewKind === "deploy") {
        await transactionCommand(account.id, {
          action: "confirm",
          receipt: preview.receipt,
          confirmed: true,
        });
      } else if (previewKind === "removal") {
        await transactionCommand(account.id, {
          action: "removal_confirm",
          receipt: preview.receipt,
          confirmed: true,
        });
      } else {
        await transactionCommand(account.id, {
          action: "compensation_confirm",
          operation: previewKind.compensates,
          receipt: preview.receipt,
          confirmed: true,
        });
      }
      setPreview(null);
      setConfirmed(false);
      setMessage(
        t(
          "ManageSieve transaction started. Refresh to confirm final observed server state.",
          "ManageSieve-Transaktion gestartet. Aktualisiere den Stand zur Bestätigung.",
        ),
      );
      setTimeout(() => void load(), 1500);
    } catch {
      setError(
        liveWrites
          ? t(
              "Transaction was refused or interrupted. Refresh and reconcile before retrying.",
              "Transaktion wurde abgelehnt oder unterbrochen. Aktualisiere und gleiche den Stand vor einem neuen Versuch ab.",
            )
          : t(
              "Live provider writes are locked in Read-only mode. Enable Live writes in Settings first.",
              "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff in den Einstellungen.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  const rec = txView?.recommendation ?? discovery?.recommendation;
  const baseline = discovery?.facts.last_successful;

  return (
    <div className="page-stack">
      {!liveWrites && (
        <Notice>
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Live ManageSieve discovery and deploy previews work right now; uploading or activating scripts requires enabling Live writes in Settings.",
              "Der Nur-Lese-Modus ist aktiv. ManageSieve-Prüfung und Vorschau funktionieren sofort; zum Hochladen oder Aktivieren aktiviere Live-Schreibzugriff in den Einstellungen.",
            )}
          </span>
          <Action onClick={onGoSettings}>
            {t("Open Settings", "Einstellungen öffnen")}
          </Action>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          {message}
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "1. Live ManageSieve discovery (read-only)",
                "1. Live-ManageSieve-Prüfung (nur lesend)",
              )}
            </h2>
            <p className="muted">
              {t(
                "Inspect sieve.mailbox.org capabilities, active scripts, and existing Open-Xchange rules without changing anything.",
                "Prüfe Fähigkeiten von sieve.mailbox.org, aktive Skripte und bestehende Open-Xchange-Regeln ohne Änderungen.",
              )}
            </p>
          </div>
          <div className="actions">
            <Action onClick={() => void load()} disabled={busy}>
              <RefreshCw size={15} />
              {t("Refresh", "Aktualisieren")}
            </Action>
            {running ? (
              <Action
                onClick={() => void cancelSieve(account.id).then(load)}
                disabled={busy}
              >
                {t("Stop", "Stoppen")}
              </Action>
            ) : (
              <Action primary onClick={() => void runDiscovery()} disabled={busy}>
                <Server size={15} />
                {t("Inspect ManageSieve", "ManageSieve prüfen")}
              </Action>
            )}
          </div>
        </div>
        {baseline ? (
          <>
            <div className="stats compact">
              <Stat
                label={t("Active script", "Aktives Skript")}
                value={
                  baseline.names.find((n) => n.active)?.name ??
                  t("None", "Keines")
                }
              />
              <Stat
                label={t("Stored scripts", "Gespeicherte Skripte")}
                value={baseline.names.length}
              />
              <Stat
                label={t("Sieve extensions", "Sieve-Erweiterungen")}
                value={
                  baseline.capabilities.SIEVE
                    ? baseline.capabilities.SIEVE.split(/\s+/).filter(Boolean)
                        .length
                    : 0
                }
              />
            </div>
            <Details
              title={t(
                "Capabilities and observed scripts",
                "Fähigkeiten und gelesene Skripte",
              )}
            >
              <p className="help">
                <strong>SIEVE:</strong> {baseline.capabilities.SIEVE ?? "—"}
              </p>
              {baseline.bodies.map((body) => (
                <div key={body.name}>
                  <h4>
                    {body.name} ({body.bytes.length} B)
                  </h4>
                  <pre tabIndex={0}>{exactReview(body)}</pre>
                </div>
              ))}
            </Details>
          </>
        ) : (
          <p className="muted">
            {t(
              "No ManageSieve baseline observed yet. Run Inspect ManageSieve above.",
              "Noch keine ManageSieve-Basis erfasst. Starte oben ManageSieve prüfen.",
            )}
          </p>
        )}
      </section>

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "2. Coexistence strategy with existing Open-Xchange rules",
                "2. Koexistenz mit bestehenden Open-Xchange-Regeln",
              )}
            </h2>
            <p className="muted">
              {t(
                "Your existing rules always stay byte-identical and win filing conflicts. Approved invoice copies run ahead of them.",
                "Deine bestehenden Regeln bleiben byte-identisch und gewinnen Ablagekonflikte. Freigegebene Rechnungskopien laufen davor.",
              )}
            </p>
          </div>
          {rec && (
            <span className="badge">
              {t("Recommended", "Empfohlen")}:{" "}
              {rec.recommended === "include"
                ? t("Include script", "Include-Skript")
                : t("Marked block", "Markierter Block")}
            </span>
          )}
        </div>
        {rec && <p className="help">{rec.reason}</p>}
        <div className="settings-form">
          <Choice
            label={t("Placement mode", "Platzierungsmodus")}
            value={placementKind}
            onChange={(value) => {
              if (value === "block" || value === "include")
                setPlacementKind(value);
            }}
            options={[
              {
                value: "block",
                label: t(
                  "Block — marked OX block appended inside Open-Xchange",
                  "Block — markierter OX-Block am Ende von Open-Xchange",
                ),
              },
              {
                value: "include",
                label: t(
                  "Include — separate Mailroom script referenced via include :personal",
                  "Include — eigenes Mailroom-Skript über include :personal",
                ),
              },
            ]}
          />
          {placementKind === "include" && (
            <label className="field">
              {t("Included script name", "Name des eingebundenen Skripts")}
              <input
                value={includeScript}
                onChange={(e) => setIncludeScript(e.target.value)}
                placeholder="mailroom"
              />
            </label>
          )}
          <Action onClick={() => void savePlacement()} disabled={busy}>
            {t("Save placement choice", "Platzierung speichern")}
          </Action>
        </div>
      </section>

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "3. Preview and deploy to mailbox.org",
                "3. Vorschau und Bereitstellung bei mailbox.org",
              )}
            </h2>
            <p className="muted">
              {t(
                "Preview checks live Open-Xchange state, validates syntax via CHECKSCRIPT, and computes exact rule precedence before any upload.",
                "Die Vorschau prüft den aktuellen Open-Xchange-Stand, validiert die Syntax per CHECKSCRIPT und zeigt die genaue Regelrangfolge vor jedem Upload.",
              )}
            </p>
          </div>
          <div className="actions">
            <Action
              primary
              onClick={() => void runPreview("deploy")}
              disabled={busy || !txView?.placement}
            >
              <Rocket size={15} />
              {t("Preview deploy", "Bereitstellung prüfen")}
            </Action>
            <Action
              onClick={() => void runPreview("removal")}
              disabled={busy}
            >
              <Trash2 size={15} />
              {t("Preview removal", "Entfernung prüfen")}
            </Action>
          </div>
        </div>
        {preview && preview.outcome === "refused" && (
          <Notice error>
            {t("Deployment refused", "Bereitstellung abgelehnt")}:{" "}
            <code>{preview.refusal.reason}</code>
            {preview.refusal.detail ? ` — ${preview.refusal.detail}` : ""}
          </Notice>
        )}
        {preview && preview.outcome === "ready" && (
          <div className="subpanel">
            <h3>
              {t("Ready transaction preview", "Bereite Transaktionsvorschau")} ·{" "}
              {preview.effects.join(", ")}
            </h3>
            {preview.precedence && (
              <div className="stats compact">
                <Stat
                  label={t("Mailroom files", "Mailroom legt ab")}
                  value={preview.precedence.counts.mailroom}
                />
                <Stat
                  label={t("Existing rules win", "Bestehende Regeln gewinnen")}
                  value={preview.precedence.counts.existing_wins}
                />
                <Stat
                  label={t("Conflicts", "Konflikte")}
                  value={preview.precedence.counts.conflicting}
                />
                <Stat
                  label={t("Accounting copies", "Rechnungskopien")}
                  value={preview.precedence.counts.copies.copied}
                />
              </div>
            )}
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "I reviewed the exact precedence and effects above and confirm applying this ManageSieve transaction.",
                "Ich habe Rangfolge und Auswirkungen oben geprüft und bestätige diese ManageSieve-Transaktion.",
              )}
            </CheckField>
            <div className="actions">
              <Action
                primary
                disabled={!confirmed || !liveWrites || busy}
                onClick={() => void confirmTransaction()}
              >
                <Rocket size={15} />
                {t("Apply transaction", "Transaktion ausführen")}
              </Action>
              <Action onClick={() => setPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
            </div>
          </div>
        )}
        {txView && txView.journal.history.length > 0 && (
          <Details
            title={`${t("Transaction history", "Transaktionsverlauf")} · ${txView.journal.history.length}`}
          >
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>{t("ID", "ID")}</th>
                    <th>{t("Target", "Ziel")}</th>
                    <th>{t("Status", "Status")}</th>
                    <th>{t("Actions", "Aktionen")}</th>
                  </tr>
                </thead>
                <tbody>
                  {txView.journal.history.map((tx) => (
                    <tr key={tx.id}>
                      <td>
                        <code>{tx.id.slice(0, 12)}</code>
                      </td>
                      <td>{tx.target}</td>
                      <td>
                        <span className="badge">{tx.status}</span>
                      </td>
                      <td className="actions">
                        {(tx.status === "uncertain" ||
                          tx.status === "stopped") && (
                          <Action
                            disabled={busy}
                            onClick={async () => {
                              setBusy(true);
                              try {
                                setTxView(
                                  await reconcileTransaction(account.id, tx.id),
                                );
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            {t("Reconcile", "Abgleichen")}
                          </Action>
                        )}
                        {tx.status === "complete" && !tx.compensates && (
                          <Action
                            disabled={busy}
                            onClick={() =>
                              void runPreview({ compensates: tx.id })
                            }
                          >
                            <Undo2 size={14} />
                            {t("Preview rollback", "Rücknahme prüfen")}
                          </Action>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Details>
        )}
      </section>
    </div>
  );
}

export function PlacementPanel({ view }: { view: AccountView | null }) {
  const t = useCopy();
  const [page, setPage] = useState(0);
  if (!view) {
    return (
      <Empty
        title={t(
          "No saved census yet",
          "Noch keine gespeicherte Erfassung",
        )}
      />
    );
  }
  const landing = view.landing;
  if (landing.kind !== "compared") {
    return (
      <Empty
        title={t(
          "No recorded deployment to compare",
          "Keine gespeicherte Bereitstellung zum Vergleichen",
        )}
      >
        {t(
          "Once you deploy rules via ManageSieve and run a fresh mailbox scan, Mailroom compares observed folders against your deployed rules here.",
          "Sobald du Regeln per ManageSieve bereitstellst und das Postfach erneut erfasst, vergleicht Mailroom hier die tatsächlichen Ordner mit deinen bereitgestellten Regeln.",
        )}
      </Empty>
    );
  }
  const perPage = 50;
  const rows = landing.rows;
  const slice = rows.slice(page * perPage, (page + 1) * perPage);
  return (
    <div className="page-stack">
      <section className="panel">
        <h2>{t("Placement comparison", "Ablagevergleich")}</h2>
        <p className="muted">
          {t(
            "Compares observed folders in your saved census with the last recorded deployment. Old mail and manual moves are included; matching folders do not prove initial delivery.",
            "Vergleicht die beobachteten Ordner deiner gespeicherten Erfassung mit der letzten Bereitstellung. Alte Mails und manuelle Verschiebungen sind enthalten; übereinstimmende Ordner beweisen keine Erstzustellung.",
          )}
        </p>
        <div className="stats compact">
          <Stat
            label={t("Matched", "Übereinstimmend")}
            value={landing.counts.matches}
          />
          <Stat
            label={t("Elsewhere", "Woanders")}
            value={landing.counts.elsewhere}
          />
          <Stat
            label={t("Additional folders", "Zusätzliche Ordner")}
            value={landing.counts.additional_folders}
          />
          <Stat
            label={t("Uncheckable", "Nicht prüfbar")}
            value={landing.counts.uncheckable}
          />
          <Stat
            label={t("Total messages", "Nachrichten gesamt")}
            value={rows.length}
          />
        </div>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>{t("From", "Absender")}</th>
                <th>{t("Subject", "Betreff")}</th>
                <th>{t("Predicted", "Erwartet")}</th>
                <th>{t("Observed", "Beobachtet")}</th>
                <th>{t("Status", "Status")}</th>
              </tr>
            </thead>
            <tbody>
              {slice.map((row) => {
                const predicted =
                  row.prediction.kind === "folder"
                    ? row.prediction.destination.kind === "inbox"
                      ? "INBOX"
                      : row.prediction.destination.name
                    : "—";
                const status =
                  row.prediction.kind === "folder"
                    ? row.prediction.comparison
                    : row.prediction.reason;
                return (
                  <tr key={row.id}>
                    <td>{row.from}</td>
                    <td>{row.subject}</td>
                    <td>{predicted}</td>
                    <td>{row.observed.join(", ") || "—"}</td>
                    <td>
                      <span className="badge">{status}</span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        {rows.length > perPage && (
          <div className="actions">
            <Action
              disabled={page === 0}
              onClick={() => setPage((p) => Math.max(0, p - 1))}
            >
              {t("Previous", "Zurück")}
            </Action>
            <span className="help">
              {page * perPage + 1}–{Math.min((page + 1) * perPage, rows.length)}{" "}
              / {rows.length}
            </span>
            <Action
              disabled={(page + 1) * perPage >= rows.length}
              onClick={() => setPage((p) => p + 1)}
            >
              {t("Next", "Weiter")}
            </Action>
          </div>
        )}
      </section>
    </div>
  );
}
