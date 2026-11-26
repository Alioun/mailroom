import { useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  CheckCircle2,
  FolderPlus,
  Play,
  Rocket,
  Scale,
  Settings2,
  SlidersHorizontal,
  Sparkles,
} from "lucide-react";
import {
  downloadSieve,
  openProposals,
  openWorkspace,
  previewWorkspace,
  saveWorkspace,
} from "../api";
import type {
  Account,
  AccountStatus,
  AccountView,
  Pin,
  ProofPreview,
  ProposalGeneration,
  ProposalReview,
  WorkspaceView,
} from "../api";
import { createLocalRules } from "./rules-workspace";
import { FoldersPanel, PlacementPanel, SieveDeployPanel } from "./Deploy";
import { ProposalsSection } from "./rules/ProposalsSection";
import { ProofSimulationSection } from "./rules/ProofSimulationSection";
import { describeError } from "./status-labels";
import {
  Action,
  Busy,
  Details,
  Empty,
  Notice,
  StatusBadge,
  SubTabs,
  useCopy,
} from "./components";
import type { SubTabItem } from "./components";

type Input = NonNullable<WorkspaceView["workspace"]>["input"];
type RulesTab = "local" | "folders" | "deploy" | "placement";

export function Rules({
  account,
  view,
  workspace,
  liveWrites,
  liveJob,
  synthetic = false,
  onWorkspace,
  onOpenWriteMode,
  onGoBacklog,
  onGoAddresses,
}: {
  account: Account;
  view: AccountView | null;
  workspace: WorkspaceView | null;
  liveWrites: boolean;
  liveJob: AccountStatus["job"];
  synthetic?: boolean;
  onWorkspace: (value: WorkspaceView) => void;
  onOpenWriteMode: () => void;
  onGoBacklog: () => void;
  onGoAddresses: () => void;
}) {
  const t = useCopy();
  const isGmail = account.provider === "gmail";
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
    if (isGmail && (tab === "folders" || tab === "deploy")) {
      setTab("local");
    }
  }, [isGmail, tab]);

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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not create local rules. Refresh saved state and try again.",
            "Lokale Regeln ließen sich nicht anlegen. Aktualisiere den gespeicherten Stand und versuche es erneut.",
          ),
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "The rules could not be checked. Refresh saved state and try again.",
            "Die Regeln konnten nicht geprüft werden. Aktualisiere den gespeicherten Stand und versuche es erneut.",
          ),
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not load deterministic proposals. Save local rules and scan your mailbox first.",
            "Deterministische Vorschläge konnten nicht geladen werden. Speichere lokale Regeln und erfasse das Postfach.",
          ),
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
      setProposalReview(null);
      onWorkspace(next);
      setMessage(t("Local rules saved.", "Lokale Regeln gespeichert."));
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Save refused. The rules may be invalid or the saved revision changed. Your draft is retained.",
            "Speichern abgelehnt. Die Regeln sind möglicherweise ungültig oder der gespeicherte Stand hat sich geändert. Dein Entwurf bleibt erhalten.",
          ),
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
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Download refused. Re-run Check saved rules and verify no pins are blocked.",
            "Download abgelehnt. Prüfe die Regeln erneut und stelle sicher, dass kein Pin blockiert ist.",
          ),
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
        setProposalReview(null);
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

  function addPin(messageIdHash: string, expected: Pin["expected"]) {
    if (!draft) return;
    if (draft.pins.some((p) => p.message_id_hash === messageIdHash)) return;
    setDraft({
      ...draft,
      pins: [
        ...draft.pins,
        {
          account: account.id,
          message_id_hash: messageIdHash,
          expected,
        },
      ],
    });
    setProof(null);
    setProposalReview(null);
    setMessage(
      t(
        "Pin added to draft. Save local rules to enforce this expectation in future checks.",
        "Pin zum Entwurf hinzugefügt. Speichere lokale Regeln, um diese Erwartung künftig zu prüfen.",
      ),
    );
  }

  function removePin(messageIdHash: string) {
    if (!draft) return;
    setDraft({
      ...draft,
      pins: draft.pins.filter((p) => p.message_id_hash !== messageIdHash),
    });
    setProof(null);
    setProposalReview(null);
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

  const localContent = (
    <div className="page-stack">
      {isGmail && (
        <Notice tone="info">
          <span>
            {t(
              "Gmail does not support ManageSieve. Use these local rules to sort existing Gmail mail via labels in Backlog, and configure Gmail forwarding to mailbox.org in Addresses & wind-down.",
              "Gmail unterstützt kein ManageSieve. Nutze diese lokalen Regeln für Gmail-Labels im Bereich Bestand sortieren und richte die Weiterleitung unter Adressen & Abbau ein.",
            )}
          </span>
          <div className="actions">
            <Action size="compact" onClick={onGoBacklog}>
              {t("Backlog", "Bestand sortieren")} <ArrowRight size={13} />
            </Action>
            <Action size="compact" onClick={onGoAddresses}>
              {t("Gmail forwarding", "Gmail-Weiterleitung")}{" "}
              <ArrowRight size={13} />
            </Action>
          </div>
        </Notice>
      )}
      {error && <Notice error>{error}</Notice>}
      {message && (
        <Notice>
          <CheckCircle2 size={18} />
          <span>{message}</span>
        </Notice>
      )}
      {conflict && (
        <Notice tone="warning">
          <span>
            {t(
              "The saved analysis or rules changed while your draft was open. Your draft is retained. Copy it before loading saved rules.",
              "Die gespeicherte Analyse oder die Regeln haben sich geändert, während dein Entwurf geöffnet war. Er bleibt erhalten. Kopiere ihn, bevor du gespeicherte Regeln lädst.",
            )}
          </span>
          <Action size="compact" onClick={reload}>
            {t("Reload saved rules", "Gespeicherte Regeln laden")}
          </Action>
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
                  {t("Revision", "Revision")} {workspace.workspace.revision} ·{" "}
                  {workspace.engine === "mailbox_sieve"
                    ? "mailbox.org"
                    : "Gmail"}{" "}
                  · {draft?.pins.length ?? 0} {t("pins", "Pins")}
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
              <Notice tone="info">
                <span>
                  {t(
                    "You have unsaved draft edits. Save local rules to re-run simulation and proposals.",
                    "Du hast ungespeicherte Entwurfsänderungen. Speichere lokale Regeln, um Simulation und Vorschläge neu zu prüfen.",
                  )}
                </span>
                <Action
                  primary
                  size="compact"
                  disabled={busy || conflict}
                  onClick={() => void save()}
                >
                  {t("Save local rules", "Lokale Regeln speichern")}
                </Action>
              </Notice>
            )}
          </>
        )}
      </section>

      {proposals && (
        <ProposalsSection
          account={account}
          proposals={proposals}
          proposalReview={proposalReview}
          busy={busy}
          dirty={dirty}
          onBusy={setBusy}
          onError={setError}
          onMessage={setMessage}
          onProposals={setProposals}
          onProposalReview={setProposalReview}
          onReload={reload}
        />
      )}

      {proof && (
        <ProofSimulationSection
          proof={proof}
          draft={draft}
          busy={busy}
          decisionSearch={decisionSearch}
          decisionFilter={decisionFilter}
          decisionPage={decisionPage}
          perPage={perPage}
          filteredDecisions={filteredDecisions}
          pageDecisions={pageDecisions}
          onDecisionSearch={(val) => {
            setDecisionSearch(val);
            setDecisionPage(0);
          }}
          onDecisionFilter={(f) => {
            setDecisionFilter(f);
            setDecisionPage(0);
          }}
          onDecisionPage={setDecisionPage}
          onExportSieve={() => void exportSieve()}
          onAddPin={addPin}
          onRemovePin={removePin}
        />
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
              className="code-editor"
              wrap="off"
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
                setProposalReview(null);
              }}
            />
          </label>
          <label className="field">
            {t("Override YAML", "Anpassungs-YAML")}
            <textarea
              className="code-editor"
              wrap="off"
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
                setProposalReview(null);
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
            <Action disabled={busy || !dirty} onClick={reload}>
              {t("Discard draft", "Entwurf verwerfen")}
            </Action>
          </div>
        </Details>
      )}
    </div>
  );

  const items: SubTabItem<RulesTab>[] = [
    {
      value: "local",
      label: t("Local rules & proof", "Lokale Regeln & Prüfung"),
      icon: <SlidersHorizontal size={15} />,
      content: localContent,
      keepMounted: true,
    },
    ...(!isGmail
      ? ([
          {
            value: "folders",
            label: t("Folders", "Ordner"),
            icon: <FolderPlus size={15} />,
            content: (
              <FoldersPanel
                account={account}
                liveWrites={liveWrites}
                liveJob={liveJob}
                onOpenWriteMode={onOpenWriteMode}
              />
            ),
            keepMounted: true,
          },
          {
            value: "deploy",
            label: t("Sieve deploy", "Sieve-Bereitstellung"),
            icon: <Rocket size={15} />,
            content: (
              <SieveDeployPanel
                account={account}
                liveWrites={liveWrites}
                liveJob={liveJob}
                synthetic={synthetic}
                onOpenWriteMode={onOpenWriteMode}
              />
            ),
            keepMounted: true,
          },
        ] satisfies SubTabItem<RulesTab>[])
      : []),
    {
      value: "placement",
      label: t("Placement check", "Ablagevergleich"),
      icon: <Scale size={15} />,
      content: <PlacementPanel view={view} />,
      keepMounted: true,
    },
  ];

  return (
    <SubTabs
      value={tab}
      onChange={setTab}
      label={t(
        "Rules and deploy sections",
        "Bereiche für Regeln und Bereitstellung",
      )}
      items={items}
    />
  );
}
