import { useCallback, useContext, useEffect, useState } from "react";
import {
  CheckCircle2,
  RefreshCw,
  Rocket,
  Server,
  ShieldAlert,
  Trash2,
  Undo2,
} from "lucide-react";
import type { Account, AccountStatus } from "../../api";
import {
  cancelSieve,
  exactReview,
  openSieve,
  sieveStatus,
  startSieve,
} from "../../managesieve-api";
import type { SieveView } from "../../managesieve-api";
import {
  reconcileTransaction,
  recordPlacement,
  transactionCommand,
  transactionPreview,
  transactionStatus,
} from "../../sieve-transactions-api";
import type {
  Placement,
  TransactionPreview,
  TransactionView,
} from "../../sieve-transactions-api";
import { formatNumber } from "../format";
import { describeError, sieveRefusalText } from "../status-labels";
import {
  Action,
  Busy,
  CheckField,
  Choice,
  Details,
  Empty,
  LanguageContext,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "../components";

export function SieveDeployPanel({
  account,
  synthetic = false,
  liveWrites,
  liveJob,
  onOpenWriteMode,
}: {
  account: Account;
  synthetic?: boolean;
  liveWrites: boolean;
  liveJob: AccountStatus["job"];
  onOpenWriteMode: () => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);
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
  const jobRunning = liveJob?.progress.state === "running";

  const load = useCallback(async () => {
    if (account.provider !== "mailbox_org") return;
    setError("");
    const [sieveRes, statusRes, txRes] = await Promise.allSettled([
      openSieve(account.id),
      sieveStatus(account.id),
      transactionStatus(account.id),
    ]);
    if (statusRes.status === "fulfilled") {
      setRunning(statusRes.value.running);
    }
    if (sieveRes.status === "fulfilled") {
      setDiscovery(sieveRes.value);
      setUnavailable(false);
    }
    if (txRes.status === "fulfilled") {
      const tx = txRes.value;
      setTxView(tx);
      setUnavailable(false);
      if (tx.placement) {
        setPlacementKind(tx.placement.kind);
        if (tx.placement.kind === "include")
          setIncludeScript(tx.placement.script);
      } else if (tx.recommendation?.recommended) {
        setPlacementKind(tx.recommendation.recommended);
      }
    }
    if (
      sieveRes.status === "rejected" &&
      txRes.status === "rejected" &&
      !running &&
      !jobRunning &&
      !discovery &&
      !txView
    ) {
      setUnavailable(true);
    }
  }, [account.id, account.provider, running, jobRunning]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!running && !jobRunning) return;
    const timer = setInterval(() => void load(), 1800);
    return () => clearInterval(timer);
  }, [running, jobRunning, load]);

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

  if (unavailable && !discovery && !txView) {
    return (
      <Empty
        title={t(
          "Live Sieve deployment unavailable",
          "Live-Sieve-Bereitstellung nicht verfügbar",
        )}
      >
        {synthetic
          ? t(
              "Demo accounts use invented mail and cannot connect to a live provider to deploy Sieve scripts. You can still build, simulate, and download rules.sieve locally.",
              "Demo-Konten nutzen fiktive Daten und können keine Live-Sieve-Skripte an einen Anbieter senden. Du kannst rules.sieve weiterhin lokal erstellen, prüfen und herunterladen.",
            )
          : t(
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
          "Filter inspection started (read-only). Results will appear automatically.",
          "Filterprüfung gestartet (nur lesend). Ergebnisse erscheinen automatisch.",
        ),
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not start filter inspection. Check connectivity and credentials.",
            "Filterprüfung konnte nicht starten. Prüfe Verbindung und Zugangsdaten.",
          ),
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not save placement choice. Check the included script name.",
            "Koexistenz-Strategie konnte nicht gespeichert werden. Prüfe den Skriptnamen.",
          ),
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not preview filter publishing. Save valid local rules, choose how to keep existing filters, and check connectivity.",
            "Filter-Vorschau fehlgeschlagen. Speichere gültige lokale Regeln, wähle eine Platzierung und prüfe die Verbindung.",
          ),
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
          "Filter publishing started. Status refreshes automatically to confirm what mailbox.org accepted.",
          "Filter-Veröffentlichung gestartet. Der Status aktualisiert sich automatisch zur Bestätigung.",
        ),
      );
      setTimeout(() => void load(), 1200);
    } catch (err) {
      setError(
        liveWrites
          ? describeError(
              err,
              t(
                "Transaction was refused or interrupted. Refresh and reconcile before retrying.",
                "Transaktion wurde abgelehnt oder unterbrochen. Aktualisiere und gleiche den Stand vor einem neuen Versuch ab.",
              ),
            )
          : t(
              "Live provider writes are locked in Read-only mode. Enable Live writes first.",
              "Änderungen beim Anbieter sind im Nur-Lese-Modus gesperrt. Aktiviere zuerst Live-Schreibzugriff.",
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
        <Notice tone="info">
          <ShieldAlert size={16} />
          <span>
            {t(
              "Read-only mode is active. Inspecting your current mailbox.org filters and previewing rule changes work right now; publishing filters requires Live writes.",
              "Der Nur-Lese-Modus ist aktiv. Filterprüfung und Änderungsvorschau funktionieren sofort; zum Veröffentlichen aktiviere Live-Schreibzugriff.",
            )}
          </span>
          <Action size="compact" tone="warning" onClick={onOpenWriteMode}>
            {t("Enable live writes", "Live-Schreibzugriff aktivieren")}
          </Action>
        </Notice>
      )}
      {running && (
        <Notice tone="info">
          <Busy>
            {t(
              "Inspecting mailbox.org filters…",
              "mailbox.org-Filter werden geprüft…",
            )}
          </Busy>
          <Action
            size="compact"
            onClick={() =>
              void cancelSieve(account.id)
                .then(load)
                .catch(() =>
                  setError(
                    t(
                      "Could not stop filter inspection.",
                      "Filterprüfung konnte nicht gestoppt werden.",
                    ),
                  ),
                )
            }
          >
            {t("Stop", "Stoppen")}
          </Action>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={16} />
          <span>{message}</span>
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "1. Inspect current mailbox.org filters (read-only)",
                "1. Aktuelle mailbox.org-Filter prüfen (nur lesend)",
              )}
            </h2>
            <p className="muted">
              {t(
                "Inspect active server scripts and your existing mailbox.org webmail rules without changing anything.",
                "Prüfe aktive Filterskripte und bestehende mailbox.org-Webmail-Regeln ohne Änderungen.",
              )}
            </p>
          </div>
          <div className="actions">
            <Action onClick={() => void load()} disabled={busy}>
              <RefreshCw size={15} />
              {t("Refresh", "Aktualisieren")}
            </Action>
            {!running && (
              <Action
                primary
                onClick={() => void runDiscovery()}
                disabled={busy || jobRunning}
              >
                <Server size={15} />
                {t("Inspect mailbox.org filters", "Filter bei mailbox.org prüfen")}
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
                value={formatNumber(baseline.names.length, language)}
              />
              <Stat
                label={t("Filter extensions", "Filter-Erweiterungen")}
                value={formatNumber(
                  baseline.capabilities.SIEVE
                    ? baseline.capabilities.SIEVE.split(/\s+/).filter(Boolean)
                        .length
                    : 0,
                  language,
                )}
              />
            </div>
            <Details
              title={t(
                "Server capabilities and observed scripts",
                "Server-Fähigkeiten und gelesene Skripte",
              )}
            >
              <p className="help">
                <strong>SIEVE:</strong> {baseline.capabilities.SIEVE ?? "—"}
              </p>
              {baseline.bodies.map((body) => (
                <div key={body.name}>
                  <h4>
                    {body.name} ({formatNumber(body.bytes.length, language)} B)
                  </h4>
                  <pre tabIndex={0}>{exactReview(body)}</pre>
                </div>
              ))}
            </Details>
          </>
        ) : (
          <p className="muted">
            {t(
              "No mailbox.org filter baseline observed yet. Run Inspect mailbox.org filters above.",
              "Noch keine Filter-Basis erfasst. Starte oben Filter bei mailbox.org prüfen.",
            )}
          </p>
        )}
      </section>

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>
              {t(
                "2. Keep existing mailbox.org filters working",
                "2. Bestehende mailbox.org-Filter beibehalten",
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
            <span className="badge info">
              {t("Recommended", "Empfohlen")}:{" "}
              {rec.recommended === "include"
                ? t("Separate script", "Eigenes Skript")
                : t("Append block", "Block anfügen")}
            </span>
          )}
        </div>
        {rec && <p className="help">{rec.reason}</p>}
        <div className="settings-form">
          <Choice
            fieldLabel={t("Placement mode", "Platzierungsmodus")}
            label={t("Placement mode", "Platzierungsmodus")}
            value={placementKind}
            onChange={(value) => {
              if (value === "block" || value === "include") {
                setPlacementKind(value);
                setPreview(null);
              }
            }}
            options={[
              {
                value: "block",
                label: t(
                  "Append — add Mailroom rules after existing filters",
                  "Anfügen — Mailroom-Regeln nach bestehenden Filtern",
                ),
              },
              {
                value: "include",
                label: t(
                  "Separate script — link Mailroom rules cleanly after existing filters",
                  "Eigenes Skript — Mailroom-Regeln sauber nach bestehenden Filtern einbinden",
                ),
              },
            ]}
          />
          {placementKind === "include" && (
            <label className="field">
              {t("Included script name", "Name des eingebundenen Skripts")}
              <input
                value={includeScript}
                onChange={(e) => {
                  setIncludeScript(e.target.value);
                  setPreview(null);
                }}
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
                "3. Preview and publish to mailbox.org",
                "3. Vorschau und Veröffentlichung bei mailbox.org",
              )}
            </h2>
            <p className="muted">
              {t(
                "Preview checks live mailbox.org filters and exact rule precedence before anything is uploaded. Syntax and live readback are verified automatically on publish.",
                "Die Vorschau prüft die aktuellen mailbox.org-Filter und die genaue Regelrangfolge vor jedem Upload. Syntax und Rücklesen werden beim Veröffentlichen automatisch geprüft.",
              )}
            </p>
          </div>
          <div className="actions">
            <Action
              primary
              onClick={() => void runPreview("deploy")}
              disabled={busy || jobRunning || !txView?.placement}
            >
              <Rocket size={15} />
              {t("Preview deploy", "Bereitstellung prüfen")}
            </Action>
            <Action
              tone="danger-outline"
              onClick={() => void runPreview("removal")}
              disabled={busy || jobRunning}
            >
              <Trash2 size={15} />
              {t("Preview removal", "Entfernung prüfen")}
            </Action>
          </div>
        </div>
        {preview && preview.outcome === "refused" && (
          <Notice error>
            <div>
              <strong>
                {t("Deployment refused", "Bereitstellung abgelehnt")}
              </strong>
              <p>{sieveRefusalText(preview.refusal.reason, t)}</p>
              {(preview.refusal.script ||
                preview.refusal.rule ||
                preview.refusal.detail) && (
                <p className="help">
                  {preview.refusal.script ? `${preview.refusal.script} ` : ""}
                  {preview.refusal.rule ? `(${preview.refusal.rule}) ` : ""}
                  {preview.refusal.line
                    ? `${t("line", "Zeile")} ${preview.refusal.line} `
                    : ""}
                  {preview.refusal.detail ?? ""}
                </p>
              )}
            </div>
          </Notice>
        )}
        {preview && preview.outcome === "ready" && (
          <div className="subpanel">
            <h3>
              {t("Ready transaction preview", "Bereite Transaktionsvorschau")} ·{" "}
              {preview.effects.join(", ")}
            </h3>
            {preview.precedence && (
              <>
                <div className="stats compact">
                  <Stat
                    label={t("Mailroom files", "Mailroom legt ab")}
                    value={formatNumber(preview.precedence.counts.mailroom, language)}
                  />
                  <Stat
                    label={t(
                      "Existing rules win",
                      "Bestehende Regeln gewinnen",
                    )}
                    value={formatNumber(preview.precedence.counts.existing_wins, language)}
                  />
                  <Stat
                    label={t("Conflicts", "Konflikte")}
                    value={formatNumber(preview.precedence.counts.conflicting, language)}
                  />
                  <Stat
                    label={t("Accounting copies", "Rechnungskopien")}
                    value={formatNumber(preview.precedence.counts.copies.copied, language)}
                  />
                </div>
                {preview.precedence.warnings.length > 0 && (
                  <Notice tone="warning">
                    <ul className="evidence-list list-flush">
                      {preview.precedence.warnings.map((w, i) => (
                        <li key={i}>{w}</li>
                      ))}
                    </ul>
                  </Notice>
                )}
                {preview.precedence.decisions.length > 0 && (
                  <Details
                    defaultOpen={preview.precedence.counts.conflicting > 0}
                    title={t(
                      `Existing-filter precedence details (${formatNumber(preview.precedence.decisions.length, language)})`,
                      `Rangfolgedetails zu bestehenden Filtern (${formatNumber(preview.precedence.decisions.length, language)})`,
                    )}
                  >
                    <div className="table-scroll">
                      <table>
                        <thead>
                          <tr>
                            <th>{t("Rule", "Regel")}</th>
                            <th>{t("Status", "Status")}</th>
                            <th>{t("Mailroom target", "Mailroom-Ziel")}</th>
                            <th>{t("Conflict", "Konflikt")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {preview.precedence.decisions
                            .slice(0, 50)
                            .map((d, i) => (
                              <tr key={`${d.id}-${i}`}>
                                <td>
                                  <code>{d.rule}</code>
                                </td>
                                <td>
                                  <StatusBadge status={d.status} />
                                </td>
                                <td>
                                  {d.mailroom.kind === "folder"
                                    ? d.mailroom.name
                                    : "INBOX"}
                                </td>
                                <td>
                                  {d.conflicting ? (
                                    <StatusBadge
                                      status="conflict"
                                      label={t("Yes", "Ja")}
                                    />
                                  ) : (
                                    t("No", "Nein")
                                  )}
                                </td>
                              </tr>
                            ))}
                        </tbody>
                      </table>
                    </div>
                  </Details>
                )}
              </>
            )}
            <Details
              title={t(
                `Target script contents (${formatNumber(preview.transaction.after.bodies.length, language)})`,
                `Ziel-Skriptinhalte (${formatNumber(preview.transaction.after.bodies.length, language)})`,
              )}
            >
              {preview.transaction.after.bodies.map((body) => (
                <div key={body.name}>
                  <h4>
                    {body.name} ({formatNumber(body.bytes.length, language)} B)
                  </h4>
                  <pre tabIndex={0}>{exactReview(body)}</pre>
                </div>
              ))}
            </Details>
            <CheckField checked={confirmed} onChange={setConfirmed}>
              {t(
                "I reviewed the filter order, script contents, and effects above and confirm publishing this update.",
                "Ich habe Filterrangfolge, Skriptinhalte und Auswirkungen oben geprüft und bestätige das Veröffentlichen.",
              )}
            </CheckField>
            <div className="actions">
              <Action onClick={() => setPreview(null)} disabled={busy}>
                {t("Cancel", "Abbrechen")}
              </Action>
              {!liveWrites ? (
                <Action tone="warning" onClick={onOpenWriteMode}>
                  <ShieldAlert size={15} />
                  {t(
                    "Enable live writes to apply",
                    "Live-Schreibzugriff aktivieren",
                  )}
                </Action>
              ) : (
                <Action
                  tone={previewKind === "removal" ? "danger" : "primary"}
                  disabled={!confirmed || busy}
                  onClick={() => void confirmTransaction()}
                >
                  <Rocket size={15} />
                  {previewKind === "removal"
                    ? t("Remove Mailroom rules", "Mailroom-Regeln entfernen")
                    : t("Apply transaction", "Transaktion ausführen")}
                </Action>
              )}
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
                        <StatusBadge status={tx.status} />
                      </td>
                      <td>
                        <div className="cell-actions">
                          {(tx.status === "uncertain" ||
                            tx.status === "stopped") && (
                            <Action
                              size="compact"
                              disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                setError("");
                                try {
                                  setTxView(
                                    await reconcileTransaction(
                                      account.id,
                                      tx.id,
                                    ),
                                  );
                                  setMessage(
                                    t(
                                      "Transaction reconciled against server.",
                                      "Transaktion mit dem Server abgeglichen.",
                                    ),
                                  );
                                } catch (err) {
                                  setError(
                                    describeError(
                                      err,
                                      t(
                                        "Reconciliation failed. Check connectivity and retry.",
                                        "Abgleich fehlgeschlagen. Prüfe die Verbindung.",
                                      ),
                                    ),
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
                              size="compact"
                              disabled={busy}
                              onClick={() =>
                                void runPreview({ compensates: tx.id })
                              }
                            >
                              <Undo2 size={14} />
                              {t("Preview rollback", "Rücknahme prüfen")}
                            </Action>
                          )}
                        </div>
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

