import { useEffect, useRef, useState } from "react";
import { CheckCircle2, Play, Settings2 } from "lucide-react";
import { openWorkspace, previewWorkspace, saveWorkspace } from "../api";
import { createLocalRules } from "./rules-workspace";
import type { ProofPreview, WorkspaceView } from "../api";
import {
  Action,
  Busy,
  Details,
  Empty,
  Notice,
  Stat,
  useCopy,
} from "./components";

type Input = NonNullable<WorkspaceView["workspace"]>["input"];
export function Rules({
  account,
  workspace,
  onWorkspace,
}: {
  account: string;
  workspace: WorkspaceView | null;
  onWorkspace: (value: WorkspaceView) => void;
}) {
  const t = useCopy(),
    [draft, setDraft] = useState<Input | null>(null),
    [base, setBase] = useState(""),
    [proof, setProof] = useState<ProofPreview | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [message, setMessage] = useState("");
  const [conflict, setConflict] = useState(false);
  const baseRevision = useRef<string | null>(null);
  const dirty = draft !== null && JSON.stringify(draft) !== base;
  useEffect(() => {
    setProof(null);
    if (!workspace?.workspace) return;
    const revision = `${workspace.revision}/${workspace.workspace.revision}`;
    if (dirty) {
      setConflict(baseRevision.current !== revision);
      return;
    }
    baseRevision.current = revision;
    const next = workspace.workspace.input;
    setDraft(next);
    setBase(JSON.stringify(next));
    setConflict(false);
    let active = true;
    void previewWorkspace(account)
      .then((value) => {
        if (active) setProof(value);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [workspace, account]);
  async function initialize() {
    if (!workspace) return;
    setBusy(true);
    setError("");
    try {
      onWorkspace(await createLocalRules(account, workspace));
      setMessage(
        t(
          "Local rules created. Check their effect on your saved mail.",
          "Lokale Regeln angelegt. Prüfe ihre Wirkung auf gespeicherte Nachrichten.",
        ),
      );
    } catch {
      setError(
        t(
          "Could not create local rules. Refresh saved state and try again.",
          "Lokale Regeln ließen sich nicht anlegen. Aktualisiere den gespeicherten Stand und versuche es erneut.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }
  async function run() {
    setBusy(true);
    setError("");
    try {
      setProof(await previewWorkspace(account));
    } catch {
      setError(
        t(
          "The rules could not be checked. Refresh saved state and try again.",
          "Die Regeln konnten nicht geprüft werden. Aktualisiere den gespeicherten Stand und versuche es erneut.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }
  async function save() {
    if (!workspace || !draft || conflict) return;
    setBusy(true);
    setError("");
    try {
      const next = await saveWorkspace(account, workspace.revision, draft);
      baseRevision.current = `${next.revision}/${next.workspace?.revision}`;
      setBase(JSON.stringify(draft));
      setConflict(false);
      onWorkspace(next);
      setMessage(t("Local rules saved.", "Lokale Regeln gespeichert."));
    } catch {
      setError(
        t(
          "Save refused. The rules may be invalid or the saved revision changed. Your draft is retained.",
          "Speichern abgelehnt. Die Regeln sind möglicherweise ungültig oder der gespeicherte Stand hat sich geändert. Dein Entwurf bleibt erhalten.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }
  function reload() {
    void openWorkspace(account)
      .then((value) => {
        baseRevision.current = `${value.revision}/${value.workspace?.revision}`;
        setDraft(value.workspace?.input ?? null);
        setBase(JSON.stringify(value.workspace?.input ?? null));
        setConflict(false);
        onWorkspace(value);
      })
      .catch(() =>
        setError(
          t(
            "Saved state is unavailable. Try again.",
            "Der gespeicherte Stand ist nicht verfügbar. Versuche es erneut.",
          ),
        ),
      );
  }
  return (
    <div className="page-stack">
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={18} />
          {message}
        </Notice>
      )}
      {conflict && (
        <Notice>
          {t(
            "The saved analysis or rules changed while your draft was open. Your draft is retained. Copy it before loading saved rules.",
            "Die gespeicherte Analyse oder die Regeln haben sich geändert, während dein Entwurf geöffnet war. Er bleibt erhalten. Kopiere ihn, bevor du gespeicherte Regeln lädst.",
          )}
        </Notice>
      )}
      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>{t("Your local rules", "Deine lokalen Regeln")}</h2>
            <p className="muted">
              {t(
                "Check how rules would sort your saved mail before changing anything at your provider.",
                "Prüfe, wie Regeln gespeicherte Nachrichten sortieren würden, bevor beim Anbieter etwas geändert wird.",
              )}
            </p>
          </div>
          <span className="badge subtle">{t("Local only", "Nur lokal")}</span>
        </div>
        {!workspace ? (
          <Action onClick={reload}>
            {t("Load local rules", "Lokale Regeln laden")}
          </Action>
        ) : !workspace.workspace ? (
          <Empty
            title={t(
              "Start with the Mailroom presets",
              "Mit Mailroom-Vorlagen beginnen",
            )}
            action={
              <Action primary onClick={() => void initialize()} disabled={busy}>
                {t("Create local rules", "Lokale Regeln anlegen")}
              </Action>
            }
          >
            {t(
              "Create a local rule workspace. You can review and adjust it before any provider deployment.",
              "Lege einen lokalen Regelarbeitsstand an. Du kannst ihn vor jeder Aktivierung beim Anbieter prüfen und anpassen.",
            )}
          </Empty>
        ) : (
          <>
            <div className="rules-status">
              <Settings2 size={22} />
              <div>
                <strong>
                  {t(
                    "Saved local workspace",
                    "Gespeicherter lokaler Arbeitsstand",
                  )}
                </strong>
                <p className="help">
                  {t("Revision", "Revision")} {workspace.workspace.revision} ·{" "}
                  {workspace.engine === "mailbox_sieve"
                    ? "mailbox.org"
                    : "Gmail"}
                </p>
              </div>
              <Action
                primary
                onClick={() => void run()}
                disabled={busy || dirty}
              >
                {busy ? (
                  <Busy>{t("Checking…", "Wird geprüft…")}</Busy>
                ) : (
                  <>
                    <Play size={15} />
                    {t("Check saved rules", "Gespeicherte Regeln prüfen")}
                  </>
                )}
              </Action>
            </div>
            {dirty && (
              <p className="help">
                {t(
                  "Save your draft to check its effect.",
                  "Speichere deinen Entwurf, um seine Wirkung zu prüfen.",
                )}
              </p>
            )}
          </>
        )}
      </section>
      {proof && (
        <section className="panel">
          <div className="section-heading">
            <h2>
              {t(
                "Simulation on saved mail",
                "Simulation mit gespeicherten Nachrichten",
              )}
            </h2>
            <span className="badge">
              {t("No mail changed", "Keine Nachrichten verändert")}
            </span>
          </div>
          <div className="stats compact">
            <Stat
              label={t("Messages checked", "Geprüfte Nachrichten")}
              value={proof.proof.counts.messages}
            />
            <Stat
              label={t("Would be filed", "Würden abgelegt")}
              value={proof.proof.counts.filed}
            />
            <Stat
              label={t("Kept in Inbox", "Bleiben im Posteingang")}
              value={proof.proof.counts.inbox}
            />
            <Stat
              label={t("Need review", "Benötigen Prüfung")}
              value={proof.proof.counts.review}
            />
          </div>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Rule", "Regel")}</th>
                  <th className="numeric">{t("Matches", "Treffer")}</th>
                  <th>{t("Status", "Status")}</th>
                </tr>
              </thead>
              <tbody>
                {proof.proof.rules.map((rule) => (
                  <tr key={rule.id}>
                    <th>{rule.id}</th>
                    <td className="numeric">{rule.matched}</td>
                    <td>
                      {rule.fully_shadowed
                        ? t(
                            "Covered by earlier rules",
                            "Von früheren Regeln abgedeckt",
                          )
                        : rule.zero_match
                          ? t("No saved matches", "Keine gespeicherten Treffer")
                          : t(
                              "Matches saved mail",
                              "Passt auf gespeicherte Nachrichten",
                            )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Details title={t("Validation details", "Prüfdetails")}>
            <p>
              {t("Validation status", "Prüfstatus")} · {proof.proof.gate}
            </p>
            <ul>
              {proof.proof.artifact.omissions.map((item, i) => (
                <li key={i}>
                  {item.rule} · {item.reason}
                </li>
              ))}
            </ul>
          </Details>
        </section>
      )}
      {draft && (
        <Details
          title={t(
            "Edit exact rule documents",
            "Genaue Regeldokumente bearbeiten",
          )}
        >
          <p className="help">
            {t(
              "Advanced editing. Invalid or stale documents cannot be saved. Saving affects local rules only.",
              "Erweiterte Bearbeitung. Ungültige oder veraltete Dokumente lassen sich nicht speichern. Speichern ändert nur lokale Regeln.",
            )}
          </p>
          <label className="field">
            {t("Preset YAML", "Vorlagen-YAML")}
            <textarea
              aria-label={t("Preset YAML", "Vorlagen-YAML")}
              spellCheck={false}
              rows={12}
              value={draft.documents.preset_yaml}
              onChange={(e) => {
                setDraft({
                  ...draft,
                  documents: {
                    ...draft.documents,
                    preset_yaml: e.target.value,
                  },
                });
                setProof(null);
              }}
            />
          </label>
          <label className="field">
            {t("Override YAML", "Anpassungs-YAML")}
            <textarea
              aria-label={t("Override YAML", "Anpassungs-YAML")}
              spellCheck={false}
              rows={10}
              value={draft.documents.override_yaml}
              onChange={(e) => {
                setDraft({
                  ...draft,
                  documents: {
                    ...draft.documents,
                    override_yaml: e.target.value,
                  },
                });
                setProof(null);
              }}
            />
          </label>
          <div className="actions">
            <Action
              primary
              disabled={busy || !dirty || conflict}
              onClick={() => void save()}
            >
              {t("Save local rules", "Lokale Regeln speichern")}
            </Action>
            <Action disabled={busy} onClick={reload}>
              {t("Load saved rules", "Gespeicherte Regeln laden")}
            </Action>
          </div>
        </Details>
      )}
    </div>
  );
}
