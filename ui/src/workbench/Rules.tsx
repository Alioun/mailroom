import { useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  Download,
  FolderPlus,
  Play,
  Rocket,
  Scale,
  Settings2,
  SlidersHorizontal,
  Sparkles,
} from "lucide-react";
import {
  confirmProposals,
  downloadSieve,
  openProposals,
  openWorkspace,
  previewWorkspace,
  reviewProposals,
  saveWorkspace,
} from "../api";
import type {
  Account,
  AccountView,
  ProofPreview,
  ProposalGeneration,
  ProposalReview,
  WorkspaceView,
} from "../api";
import { createLocalRules } from "./rules-workspace";
import { FoldersPanel, PlacementPanel, SieveDeployPanel } from "./Deploy";
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
type RulesTab = "local" | "folders" | "deploy" | "placement";

export function Rules({
  account,
  view,
  workspace,
  liveWrites,
  onWorkspace,
  onGoSettings,
}: {
  account: Account;
  view: AccountView | null;
  workspace: WorkspaceView | null;
  liveWrites: boolean;
  onWorkspace: (value: WorkspaceView) => void;
  onGoSettings: () => void;
}) {
  const t = useCopy();
  const [tab, setTab] = useState<RulesTab>("local");
  const [draft, setDraft] = useState<Input | null>(null);
  const [base, setBase] = useState("");
  const [proof, setProof] = useState<ProofPreview | null>(null);
  const [proposals, setProposals] = useState<ProposalGeneration | null>(null);
  const [proposalReview, setProposalReview] = useState<ProposalReview | null>(
    null,
  );
  const [decisionSearch, setDecisionSearch] = useState("");
  const [decisionFilter, setDecisionFilter] = useState<
    "all" | "inbox" | "review" | "copies"
  >("all");
  const [decisionPage, setDecisionPage] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
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
    void previewWorkspace(account.id)
      .then((value) => {
        if (active) setProof(value);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [workspace, account.id]);

  async function initialize() {
    if (!workspace) return;
    setBusy(true);
    setError("");
    try {
      onWorkspace(await createLocalRules(account.id, workspace));
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
      setProof(await previewWorkspace(account.id));
      setDecisionPage(0);
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

  async function loadDeterministicProposals() {
    setBusy(true);
    setError("");
    try {
      setProposals(await openProposals(account.id));
      setProposalReview(null);
    } catch {
      setError(
        t(
          "Could not load deterministic proposals. Save local rules and scan your mailbox first.",
          "Deterministische Vorschläge konnten nicht geladen werden. Speichere lokale Regeln und erfasse das Postfach.",
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
      const next = await saveWorkspace(account.id, workspace.revision, draft);
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

  async function exportSieve() {
    if (!proof) return;
    setBusy(true);
    setError("");
    try {
      const result = await downloadSieve(account.id, proof.receipt);
      onWorkspace(result.workspace);
      const blob = new Blob([result.sieve], {
        type: "text/plain;charset=utf-8",
      });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = result.filename;
      a.click();
      URL.revokeObjectURL(url);
      setMessage(
        t(
          "Compiled rules.sieve downloaded and baseline recorded.",
          "Kompilierte rules.sieve heruntergeladen und als Basis gespeichert.",
        ),
      );
    } catch {
      setError(
        t(
          "Download refused. Re-run Check saved rules and verify no pins are blocked.",
          "Download abgelehnt. Prüfe die Regeln erneut und stelle sicher, dass kein Pin blockiert ist.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  function reload() {
    void openWorkspace(account.id)
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

  const filteredDecisions = (proof?.proof.decisions ?? []).filter((d) => {
    if (decisionFilter === "inbox" && d.destination.kind !== "inbox")
      return false;
    if (decisionFilter === "review" && !d.review) return false;
    if (decisionFilter === "copies" && d.copies.length === 0) return false;
    if (!decisionSearch.trim()) return true;
    const q = decisionSearch.trim().toLowerCase();
    return (
      d.subject.toLowerCase().includes(q) ||
      d.from.toLowerCase().includes(q) ||
      (d.destination.kind === "folder"
        ? d.destination.name.toLowerCase().includes(q)
        : "inbox".includes(q))
    );
  });
  const perPage = 50;
  const pageDecisions = filteredDecisions.slice(
    decisionPage * perPage,
    (decisionPage + 1) * perPage,
  );

  return (
    <div className="page-stack">
      <div
        className="subtabs"
        role="tablist"
        aria-label={t("Rules and deploy sections", "Bereiche für Regeln und Bereitstellung")}
      >
        <button
          role="tab"
          aria-selected={tab === "local"}
          className={`subtab ${tab === "local" ? "active" : ""}`}
          onClick={() => setTab("local")}
        >
          <SlidersHorizontal size={15} />
          {t("Local rules & proof", "Lokale Regeln & Prüfung")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "folders"}
          className={`subtab ${tab === "folders" ? "active" : ""}`}
          onClick={() => setTab("folders")}
        >
          <FolderPlus size={15} />
          {t("Folders", "Ordner")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "deploy"}
          className={`subtab ${tab === "deploy" ? "active" : ""}`}
          onClick={() => setTab("deploy")}
        >
          <Rocket size={15} />
          {t("Sieve deploy", "Sieve-Bereitstellung")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "placement"}
          className={`subtab ${tab === "placement" ? "active" : ""}`}
          onClick={() => setTab("placement")}
        >
          <Scale size={15} />
          {t("Placement check", "Ablagevergleich")}
        </button>
      </div>

      {tab === "folders" && (
        <FoldersPanel
          account={account}
          liveWrites={liveWrites}
          onGoSettings={onGoSettings}
        />
      )}

      {tab === "deploy" && (
        <SieveDeployPanel
          account={account}
          liveWrites={liveWrites}
          onGoSettings={onGoSettings}
        />
      )}

      {tab === "placement" && <PlacementPanel view={view} />}

      {tab === "local" && (
        <>
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
              <span className="badge subtle">
                {t("Local only", "Nur lokal")}
              </span>
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
                  <Action
                    primary
                    onClick={() => void initialize()}
                    disabled={busy}
                  >
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
                      {t("Revision", "Revision")}{" "}
                      {workspace.workspace.revision} ·{" "}
                      {workspace.engine === "mailbox_sieve"
                        ? "mailbox.org"
                        : "Gmail"}
                    </p>
                  </div>
                  <div className="actions">
                    <Action
                      onClick={() => void loadDeterministicProposals()}
                      disabled={busy || dirty}
                    >
                      <Sparkles size={15} />
                      {t("Rule proposals", "Regelvorschläge")}
                    </Action>
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

          {proposals && (
            <section className="panel">
              <div className="section-heading">
                <h2>
                  {t(
                    "Deterministic rule proposals",
                    "Deterministische Regelvorschläge",
                  )}
                </h2>
                <span className="badge">
                  {proposals.candidates.length} {t("candidates", "Kandidaten")}
                </span>
              </div>
              {proposals.candidates.length === 0 ? (
                <p className="muted">
                  {t(
                    "No deterministic rule proposals for current unmatched senders.",
                    "Keine deterministischen Regelvorschläge für aktuelle nicht zugeordnete Absender.",
                  )}
                </p>
              ) : (
                <div className="page-stack">
                  {proposals.candidates.slice(0, 20).map((cand) => (
                    <div className="proposal-card" key={cand.id}>
                      <div>
                        <strong>{cand.sender_claim}</strong>
                        <p className="help">{cand.uncertainty}</p>
                      </div>
                      <div className="actions">
                        <Action
                          disabled={busy}
                          onClick={async () => {
                            setBusy(true);
                            try {
                              setProposalReview(
                                await reviewProposals(
                                  account.id,
                                  {
                                    account: account.id,
                                    generation: proposals.generation,
                                    candidates: [
                                      { id: cand.id, change: cand.change },
                                    ],
                                  },
                                  "accept",
                                ),
                              );
                            } finally {
                              setBusy(false);
                            }
                          }}
                        >
                          {t("Preview accept", "Annahme prüfen")}
                        </Action>
                        <Action
                          disabled={busy}
                          onClick={async () => {
                            setBusy(true);
                            try {
                              const rev = await reviewProposals(
                                account.id,
                                {
                                  account: account.id,
                                  generation: proposals.generation,
                                  candidates: [
                                    { id: cand.id, change: cand.change },
                                  ],
                                },
                                "reject",
                              );
                              setProposals(
                                await confirmProposals(account.id, rev),
                              );
                            } finally {
                              setBusy(false);
                            }
                          }}
                        >
                          {t("Reject", "Ablehnen")}
                        </Action>
                      </div>
                    </div>
                  ))}
                </div>
              )}
              {proposalReview && (
                <div className="subpanel">
                  <p>{proposalReview.effects}</p>
                  <pre tabIndex={0}>{proposalReview.document_diff}</pre>
                  <div className="actions">
                    <Action
                      primary
                      disabled={!proposalReview.acceptance_available || busy}
                      onClick={async () => {
                        setBusy(true);
                        try {
                          setProposals(
                            await confirmProposals(account.id, proposalReview),
                          );
                          setProposalReview(null);
                          reload();
                          setMessage(
                            t(
                              "Proposal applied to local rules.",
                              "Vorschlag in lokale Regeln übernommen.",
                            ),
                          );
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      {t("Apply proposal", "Vorschlag anwenden")}
                    </Action>
                    <Action onClick={() => setProposalReview(null)}>
                      {t("Cancel", "Abbrechen")}
                    </Action>
                  </div>
                </div>
              )}
            </section>
          )}

          {proof && (
            <section className="panel">
              <div className="section-heading">
                <h2>
                  {t(
                    "Simulation on saved mail",
                    "Simulation mit gespeicherten Nachrichten",
                  )}
                </h2>
                <div className="actions">
                  <span className="badge">
                    {t("No mail changed", "Keine Nachrichten verändert")}
                  </span>
                  {proof.download_available && (
                    <Action onClick={() => void exportSieve()} disabled={busy}>
                      <Download size={15} />
                      {t("Download rules.sieve", "rules.sieve herunterladen")}
                    </Action>
                  )}
                </div>
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
                              ? t(
                                  "No saved matches",
                                  "Keine gespeicherten Treffer",
                                )
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
              <Details
                title={`${t("Inspect message decisions", "Nachrichtenentscheidungen prüfen")} · ${proof.proof.decisions.length}`}
              >
                <div className="toolbar">
                  <input
                    type="search"
                    value={decisionSearch}
                    onChange={(e) => {
                      setDecisionSearch(e.target.value);
                      setDecisionPage(0);
                    }}
                    placeholder={t(
                      "Filter subject, sender, destination…",
                      "Betreff, Absender, Ziel filtern…",
                    )}
                  />
                  <div className="actions">
                    {(["all", "inbox", "review", "copies"] as const).map(
                      (f) => (
                        <Action
                          key={f}
                          primary={decisionFilter === f}
                          onClick={() => {
                            setDecisionFilter(f);
                            setDecisionPage(0);
                          }}
                        >
                          {f === "all"
                            ? t("All", "Alle")
                            : f === "inbox"
                              ? t("Inbox", "Posteingang")
                              : f === "review"
                                ? t("Review", "Prüfung")
                                : t("Copies", "Kopien")}
                        </Action>
                      ),
                    )}
                  </div>
                </div>
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("From", "Absender")}</th>
                        <th>{t("Subject", "Betreff")}</th>
                        <th>{t("Destination", "Ziel")}</th>
                        <th>{t("Rule", "Regel")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pageDecisions.map((d) => (
                        <tr key={d.id}>
                          <td>{d.from}</td>
                          <td>{d.subject}</td>
                          <td>
                            {d.destination.kind === "folder"
                              ? d.destination.name
                              : "INBOX"}
                          </td>
                          <td>{d.matched_rule ?? "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {filteredDecisions.length > perPage && (
                  <div className="actions">
                    <Action
                      disabled={decisionPage === 0}
                      onClick={() =>
                        setDecisionPage((p) => Math.max(0, p - 1))
                      }
                    >
                      {t("Previous", "Zurück")}
                    </Action>
                    <span className="help">
                      {decisionPage * perPage + 1}–
                      {Math.min(
                        (decisionPage + 1) * perPage,
                        filteredDecisions.length,
                      )}{" "}
                      / {filteredDecisions.length}
                    </span>
                    <Action
                      disabled={
                        (decisionPage + 1) * perPage >= filteredDecisions.length
                      }
                      onClick={() => setDecisionPage((p) => p + 1)}
                    >
                      {t("Next", "Weiter")}
                    </Action>
                  </div>
                )}
              </Details>
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
        </>
      )}
    </div>
  );
}
