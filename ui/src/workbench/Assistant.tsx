import { useContext, useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  ArrowUpRight,
  Ban,
  CheckCircle2,
  Sparkles,
  Send,
  Settings,
  ShieldCheck,
  RefreshCw,
} from "lucide-react";
import { confirmAgentEdits, previewAgentEdits, reviewSummary } from "../api";
import type {
  AgentEditReview,
  CensusReport,
  SummaryPreview,
  WorkspaceView,
} from "../api";
import type { AgentProposal } from "../agent-exchange";
import { createLocalRules } from "./rules-workspace";
import { assistantAction } from "./assistant-api";
import type { AssistantInbox, AssistantRequest } from "./assistant-api";
import type { SenderFocusScope } from "./Findings";
import { formatDateTime, formatNumber, pluralCount } from "./format";
import { describeError } from "./status-labels";
import {
  Action,
  Busy,
  CheckField,
  Details,
  Empty,
  LanguageContext,
  Modal,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "./components";

function friendlyProvider(provider: string): string {
  switch (provider) {
    case "codex_home_v2":
    case "codex":
      return "Codex";
    case "claude_code":
    case "claudeAgent":
    case "claudeAgent_home":
      return "Claude Code";
    case "antigravity":
      return "Antigravity";
    default:
      return provider;
  }
}

type SelectedProposal = {
  request: AssistantRequest;
  id: string;
  proposal: AgentProposal;
};

export function Assistant({
  account,
  active,
  report,
  workspace,
  focusedSenders = null,
  onClearFocusedSenders,
  onWorkspace,
  onInbox,
  onGoSettings,
  onGoRules,
}: {
  account: string;
  active: boolean;
  report: CensusReport | null;
  workspace: WorkspaceView | null;
  focusedSenders?: SenderFocusScope | null;
  onClearFocusedSenders?: () => void;
  onWorkspace: (value: WorkspaceView) => void;
  onInbox: (count: number) => void;
  onGoSettings: () => void;
  onGoRules: () => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const [inbox, setInbox] = useState<AssistantInbox>({ requests: [] });
  const [sharing, setSharing] = useState(false);
  const [approved, setApproved] = useState<string[]>([]);
  const [summary, setSummary] = useState<SummaryPreview | null>(null);
  const [summaryUpdating, setSummaryUpdating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [pollError, setPollError] = useState("");
  const [message, setMessage] = useState("");
  const [savedRulesNotice, setSavedRulesNotice] = useState(false);
  const [selection, setSelection] = useState<SelectedProposal | null>(null);
  const [review, setReview] = useState<AgentEditReview | null>(null);
  const [reviewBusy, setReviewBusy] = useState(false);
  const [reviewError, setReviewError] = useState("");
  const reviewHeading = useRef<HTMLHeadingElement>(null);

  const receive = (value: AssistantInbox) => {
    setInbox(value);
    setPollError("");
    onInbox(
      value.requests.reduce(
        (count, r) => count + r.proposals.filter((p) => !p.dismissed).length,
        0,
      ),
    );
  };

  const hasInFlightLaunch = inbox.requests.some(
    (r) => r.phase === "launching" || r.phase === "launched",
  );

  useEffect(() => {
    let mounted = true;
    const refresh = () =>
      assistantAction(account, { action: "status" }, { passive: true })
        .then((value) => {
          if (mounted) receive(value);
        })
        .catch(() => {
          if (mounted)
            setPollError(
              t(
                "Recommendations could not refresh. Retry below.",
                "Vorschläge konnten nicht aktualisiert werden. Versuche es unten erneut.",
              ),
            );
        });
    void refresh();
    if (!active && !hasInFlightLaunch) {
      return () => {
        mounted = false;
      };
    }
    const timer = setInterval(() => void refresh(), 3000);
    return () => {
      mounted = false;
      clearInterval(timer);
    };
  }, [account, active, hasInFlightLaunch]);

  useEffect(() => {
    if (!sharing || !report) return;
    let active = true;
    setSummaryUpdating(true);
    setError("");
    void reviewSummary(account, report, approved)
      .then((value) => {
        if (active) setSummary(value);
      })
      .catch(() => {
        if (active)
          setError(
            t(
              "The saved analysis changed. Close this preview and refresh your findings.",
              "Die gespeicherte Analyse hat sich geändert. Schließe die Vorschau und aktualisiere die Ergebnisse.",
            ),
          );
      })
      .finally(() => {
        if (active) setSummaryUpdating(false);
      });
    return () => {
      active = false;
    };
  }, [sharing, report, approved, account]);

  useEffect(() => {
    if (selection && review) {
      setReview(null);
      setReviewError(
        t(
          "The analysis refreshed while this review was open. Close and reopen the proposal to check it against the saved state.",
          "Die Analyse wurde während der Prüfung aktualisiert. Schließe und öffne den Vorschlag erneut, um ihn mit dem gespeicherten Stand zu prüfen.",
        ),
      );
    }
  }, [report]);

  async function launch(request: AssistantRequest, forceNew = false) {
    setBusy(true);
    setError("");
    try {
      receive(
        await assistantAction(account, {
          action: "launch",
          request_id: request.id,
          ...(forceNew ? { force_new: true } : {}),
        }),
      );
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "T3 could not start. Your request is saved. Retry the request below.",
            "T3 konnte nicht starten. Deine Anfrage ist gespeichert. Wiederhole sie unten.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function share() {
    if (!summary || !report) return;
    setBusy(true);
    setError("");
    try {
      const binding = {
        as_of: report.as_of,
        version: report.version,
        approved,
        receipt: summary.receipt,
      };
      const next = await assistantAction(account, {
        action: "prepare",
        summary: binding,
        confirmed: true,
      });
      receive(next);
      const prepared = next.requests.find(
        (request) => request.summary.receipt === binding.receipt,
      );
      if (!prepared) throw new Error("Request unavailable");
      setSharing(false);
      await launch(prepared);
    } catch (err) {
      setError(
        describeError(
          err,
          t(
            "Could not share this summary. Your mailbox is unchanged. Refresh the analysis and retry.",
            "Diese Zusammenfassung konnte nicht geteilt werden. Dein Postfach ist unverändert. Aktualisiere die Analyse und versuche es erneut.",
          ),
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function dismiss(selected: SelectedProposal) {
    setBusy(true);
    setError("");
    try {
      receive(
        await assistantAction(account, {
          action: "dismiss",
          request_id: selected.request.id,
          proposal_id: selected.id,
        }),
      );
      setSelection(null);
      setReview(null);
    } catch {
      setError(
        t(
          "Could not dismiss the proposal. Try again.",
          "Der Vorschlag ließ sich nicht ausblenden. Versuche es erneut.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function inspect(selected: SelectedProposal) {
    setSelection(selected);
    setReview(null);
    setReviewError("");
    setMessage("");
    setSavedRulesNotice(false);
    if (!selected.proposal.rule_edits || !workspace?.workspace) return;
    setReviewBusy(true);
    try {
      setReview(
        await previewAgentEdits(
          account,
          selected.request.summary,
          selected.proposal,
        ),
      );
      requestAnimationFrame(() => reviewHeading.current?.focus());
    } catch (err) {
      setReviewError(
        describeError(
          err,
          t(
            "These edits cannot be saved against the current analysis and local rules. Keep this proposal for reference, or request fresh recommendations.",
            "Diese Änderungen passen nicht zum aktuellen Stand der Analyse und lokalen Regeln. Bewahre den Vorschlag als Referenz auf oder fordere neue Vorschläge an.",
          ),
        ),
      );
    } finally {
      setReviewBusy(false);
    }
  }

  async function initializeForReview() {
    if (!workspace || !selection) return;
    setReviewBusy(true);
    setReviewError("");
    try {
      onWorkspace(await createLocalRules(account, workspace));
      setReview(
        await previewAgentEdits(
          account,
          selection.request.summary,
          selection.proposal,
        ),
      );
      requestAnimationFrame(() => reviewHeading.current?.focus());
    } catch {
      setReviewError(
        t(
          "Could not prepare or check local rules. Your proposal is retained. Close and reopen it to retry.",
          "Lokale Regeln konnten nicht vorbereitet oder geprüft werden. Dein Vorschlag bleibt erhalten. Schließe und öffne ihn erneut.",
        ),
      );
    } finally {
      setReviewBusy(false);
    }
  }

  async function decide(disposition: "accept" | "reject") {
    if (!selection || !review) return;
    const receipt =
      disposition === "accept" ? review.accept_receipt : review.reject_receipt;
    if (!receipt) return;
    setReviewBusy(true);
    setReviewError("");
    try {
      const saved = await confirmAgentEdits(
        account,
        selection.request.summary,
        selection.proposal,
        disposition,
        receipt,
      );
      onWorkspace(saved.workspace);
      setReview(null);
      setSelection(null);
      if (disposition === "accept") {
        setSavedRulesNotice(true);
        setMessage(
          t(
            "Local rules saved. Open Rules & deploy to inspect simulation or deploy.",
            "Lokale Regeln gespeichert. Öffne Regeln & Bereitstellung, um Simulation oder Bereitstellung zu prüfen.",
          ),
        );
      } else {
        setSavedRulesNotice(false);
        setMessage(
          t(
            "Rule edits permanently rejected and recorded in rejection history.",
            "Regeländerungen dauerhaft abgelehnt und im Ablehnungsverlauf vermerkt.",
          ),
        );
      }
      await assistantAction(account, {
        action: "dismiss",
        request_id: selection.request.id,
        proposal_id: selection.id,
      })
        .then(receive)
        .catch(() =>
          setError(
            t(
              "Your decision was saved, but the proposal could not be marked complete. Refresh to retry dismissal.",
              "Deine Entscheidung wurde gespeichert, aber der Vorschlag konnte nicht abgeschlossen werden. Aktualisiere den Eingang.",
            ),
          ),
        );
    } catch (err) {
      setReviewError(
        describeError(
          err,
          t(
            "The saved state changed. Reopen the proposal to check it again. Your proposal is retained.",
            "Der gespeicherte Stand hat sich geändert. Öffne den Vorschlag erneut zur Prüfung. Er bleibt erhalten.",
          ),
        ),
      );
      setReview(null);
    } finally {
      setReviewBusy(false);
    }
  }

  const requests = inbox.requests;
  const activeConfig = inbox.config;
  const canShare = Boolean(report && report.canonical_messages > 0);
  const candidateTotal = report?.clear_candidates.length ?? 0;
  const hasFocus = Boolean(
    focusedSenders &&
      (focusedSenders.senders.length > 0 || focusedSenders.domains.length > 0),
  );
  const matchingCandidateIds = (() => {
    if (!focusedSenders || !report) return [] as string[];
    const domains = new Set([
      ...focusedSenders.domains.map((d) => d.toLowerCase()),
      ...focusedSenders.senders
        .map((s) => s.split("@")[1]?.toLowerCase() ?? "")
        .filter(Boolean),
    ]);
    return report.clear_candidates
      .filter((c) => domains.has(c.domain.toLowerCase()))
      .map((c) => c.id);
  })();

  function openShareModal(preselectedDomains: string[]) {
    setApproved(preselectedDomains);
    setSummary(null);
    setSharing(true);
    setMessage("");
  }

  return (
    <div className="page-stack">
      {hasFocus && focusedSenders && (
        <Notice tone="info">
          <span>
            {pluralCount(
              focusedSenders.senders.length || focusedSenders.domains.length,
              language,
              "sender",
              "senders",
              "Absender",
              "Absender",
            )}{" "}
            {t("selected from Senders", "aus Absender übernommen")} (
            {(focusedSenders.senders.length
              ? focusedSenders.senders
              : focusedSenders.domains
            )
              .slice(0, 3)
              .join(", ")}
            {(focusedSenders.senders.length || focusedSenders.domains.length) >
            3
              ? "…"
              : ""}
            )
          </span>
          <div className="actions">
            <Action
              size="compact"
              primary
              disabled={!canShare || busy}
              onClick={() => openShareModal(matchingCandidateIds)}
            >
              {t("Review focused summary", "Auswahl-Zusammenfassung prüfen")}
            </Action>
            {onClearFocusedSenders && (
              <Action size="compact" onClick={onClearFocusedSenders}>
                {t("Clear focus", "Auswahl verwerfen")}
              </Action>
            )}
          </div>
        </Notice>
      )}
      {message && (
        <Notice>
          <CheckCircle2 size={18} />
          <span>{message}</span>
          {savedRulesNotice && (
            <Action size="compact" onClick={onGoRules}>
              {t("Open Rules & deploy", "Regeln & Bereitstellung")}
              <ArrowRight size={14} />
            </Action>
          )}
        </Notice>
      )}
      {(error || pollError) && (
        <Notice error>
          <span>{error || pollError}</span>
          <Action
            size="compact"
            onClick={() => {
              setError("");
              setPollError("");
              void assistantAction(account, { action: "status" })
                .then(receive)
                .catch(() =>
                  setPollError(
                    t(
                      "Still unavailable. Try again shortly.",
                      "Weiterhin nicht verfügbar. Versuche es gleich erneut.",
                    ),
                  ),
                );
            }}
          >
            {t("Retry", "Erneut versuchen")}
          </Action>
        </Notice>
      )}
      <section className="assistant-intro panel">
        <div className="sparkle-icon">
          <Sparkles size={25} />
        </div>
        <div>
          <h2>
            {t(
              "A useful next step for your mailbox",
              "Ein sinnvoller nächster Schritt für dein Postfach",
            )}
          </h2>
          <p className="muted">
            {t(
              "Your local T3 assistant reviews a masked mailbox summary and sends rule ideas back here. You review every change before saving.",
              "Dein lokaler T3-Assistent prüft eine maskierte Zusammenfassung und sendet Regelvorschläge hierher zurück. Du prüfst jede Änderung vor dem Speichern.",
            )}
          </p>
          <div className="inline-note">
            <ShieldCheck size={16} />
            <span>
              {t(
                "You choose what to share. Raw mail and passwords never leave Mailroom.",
                "Du bestimmst, was geteilt wird. Unveränderte Mails und Passwörter verlassen Mailroom nie.",
              )}
            </span>
          </div>
          {activeConfig && (
            <div className="inline-note mt-xs">
              <span>
                {t("Assistant model", "Assistentenmodell")}:{" "}
                <strong>
                  {friendlyProvider(activeConfig.provider)} · {activeConfig.model}
                </strong>
              </span>
              <Action size="compact" onClick={onGoSettings}>
                <Settings size={13} />
                {t("Change in Settings", "In Einstellungen ändern")}
              </Action>
            </div>
          )}
        </div>
        <Action
          primary
          disabled={!canShare || busy}
          onClick={() => openShareModal([])}
        >
          <Sparkles size={16} />
          {t("Get recommendations", "Vorschläge erhalten")}
        </Action>
      </section>
      {!canShare && (
        <Notice tone="info">
          {t(
            "Scan your mailbox or import a Google Takeout .mbox file first so there is a saved summary to review.",
            "Erfasse zuerst dein Postfach oder importiere eine Google-Takeout-.mbox-Datei, damit eine Zusammenfassung erstellt werden kann.",
          )}
        </Notice>
      )}
      {requests.length === 0 ? (
        <Empty
          title={t(
            "Your recommendations will appear here",
            "Deine Vorschläge erscheinen hier",
          )}
        >
          {t(
            "Share a summary with T3. Requests and returned proposals remain available after a refresh.",
            "Teile eine Zusammenfassung mit T3. Anfragen und Vorschläge bleiben nach dem Neuladen verfügbar.",
          )}
        </Empty>
      ) : (
        requests.map((request) => (
          <section className="panel request" key={request.id}>
            <div className="section-heading">
              <div>
                <StatusBadge status={request.phase} />
                <h2>{t("Mailbox recommendations", "Postfachvorschläge")}</h2>
                <p className="help">
                  {formatDateTime(request.summary.as_of, language)} ·{" "}
                  {request.summary.approved.length
                    ? pluralCount(
                        request.summary.approved.length,
                        language,
                        "selected domain shared",
                        "selected domains shared",
                        "ausgewählte Domain geteilt",
                        "ausgewählte Domains geteilt",
                      )
                    : t("Masked identifiers only", "Nur maskierte Kennungen")}
                </p>
              </div>
              {request.thread_link &&
                /^t3-thread:\/\/|^https?:\/\//.test(request.thread_link) && (
                  <a className="button" href={request.thread_link}>
                    {t("Open T3", "T3 öffnen")}
                    <ArrowUpRight size={15} />
                  </a>
                )}
            </div>
            {request.error && (
              <Notice error>
                <span>{request.error}</span>
              </Notice>
            )}
            {(request.phase === "prepared" || request.phase === "failed") && (
              <div className="actions">
                <Action disabled={busy} onClick={() => void launch(request)}>
                  <RefreshCw size={15} />
                  {t("Retry / check existing thread", "Erneut prüfen / starten")}
                </Action>
                {request.phase === "failed" && (
                  <Action
                    disabled={busy}
                    tone="warning"
                    onClick={() => void launch(request, true)}
                  >
                    <Send size={15} />
                    {t("Start fresh T3 thread", "Neuen T3-Thread starten")}
                  </Action>
                )}
              </div>
            )}
            {(request.phase === "launched" ||
              request.phase === "connected") && (
              <Details title={t("Waiting for T3?", "Wartest du auf T3?")}>
                <p>
                  {t(
                    "If work stopped, continue the existing thread in T3. Search for this title. Mailroom receives submitted proposals automatically.",
                    "Falls die Arbeit angehalten wurde, setze den bestehenden Thread in T3 fort. Suche nach diesem Titel. Mailroom empfängt eingereichte Vorschläge automatisch.",
                  )}
                </p>
                <p>
                  <strong>{`Mailroom review ${request.id}`}</strong>
                </p>
                {request.thread_id && (
                  <p className="help">
                    {t("Thread ID", "Thread-ID")}{" "}
                    <code>{request.thread_id}</code>
                  </p>
                )}
              </Details>
            )}
            {request.proposals.map((proposal) => (
              <div
                className={`proposal-card ${proposal.dismissed ? "dismissed" : ""}`}
                key={proposal.id}
              >
                <div>
                  <div className="actions mb-xs">
                    <strong>
                      {proposal.proposal.recommendations[0]?.title}
                    </strong>
                    {proposal.dismissed && <StatusBadge status="dismissed" />}
                  </div>
                  <p className="muted">
                    {pluralCount(
                      proposal.proposal.recommendations.length,
                      language,
                      "recommendation",
                      "recommendations",
                      "Vorschlag",
                      "Vorschläge",
                    )}
                    {proposal.proposal.rule_edits
                      ? ` · ${t("Includes local rule edits", "Enthält lokale Regeländerungen")}`
                      : ""}
                  </p>
                </div>
                <Action
                  onClick={() =>
                    void inspect({
                      request,
                      id: proposal.id,
                      proposal: proposal.proposal,
                    })
                  }
                >
                  {proposal.dismissed
                    ? t(
                        "View saved proposal",
                        "Gespeicherten Vorschlag ansehen",
                      )
                    : t("Review proposal", "Vorschlag prüfen")}
                </Action>
              </div>
            ))}
          </section>
        ))
      )}
      <Modal
        open={sharing}
        onOpenChange={(open) => {
          if (!busy) setSharing(open);
        }}
        title={t("Review what T3 will receive", "Prüfe, was T3 erhält")}
        description={t(
          "Only this summary is shared with a new T3 recommendation request. Account passwords and raw mail stay in Mailroom.",
          "Nur diese Zusammenfassung wird mit einer neuen T3-Anfrage geteilt. Kontopasswörter und unveränderte Nachrichten bleiben in Mailroom.",
        )}
        wide
      >
        {activeConfig && (
          <p className="help mb-sm">
            {t("Will launch with", "Startet mit")}:{" "}
            <strong>
              {friendlyProvider(activeConfig.provider)} · {activeConfig.model}
            </strong>
          </p>
        )}
        {candidateTotal > 0 && (
          <Details
            title={t(
              `Optionally reveal selected organization domains (${approved.length} of ${candidateTotal} selected)`,
              `Optional ausgewählte Organisationsdomains offenlegen (${approved.length} von ${candidateTotal} gewählt)`,
            )}
          >
            <p className="help">
              {t(
                "Personal identifiers stay masked. Select only organization domains you want T3 to see.",
                "Persönliche Kennungen bleiben maskiert. Wähle nur Organisationsdomains, die T3 sehen soll.",
              )}
            </p>
            <div className="domain-options">
              {report?.clear_candidates.map((candidate) => (
                <CheckField
                  key={candidate.id}
                  checked={approved.includes(candidate.id)}
                  onChange={(checked) =>
                    setApproved((values) =>
                      checked
                        ? [...values, candidate.id]
                        : values.filter((value) => value !== candidate.id),
                    )
                  }
                >
                  {candidate.domain}
                </CheckField>
              ))}
            </div>
          </Details>
        )}
        {summaryUpdating && summary && (
          <p className="help mt-sm">
            <Busy>{t("Updating preview…", "Vorschau wird aktualisiert…")}</Busy>
          </p>
        )}
        {summary ? (
          <pre
            className="summary-preview"
            tabIndex={0}
            aria-label={t(
              "Exact summary shared with T3",
              "Genaue Zusammenfassung für T3",
            )}
          >
            {summary.summary}
          </pre>
        ) : (
          <p role="status">
            <Busy>{t("Preparing preview…", "Vorschau wird erstellt…")}</Busy>
          </p>
        )}
        {error && <Notice error>{error}</Notice>}
        <div className="dialog-actions">
          <Action onClick={() => setSharing(false)} disabled={busy}>
            {t("Cancel", "Abbrechen")}
          </Action>
          <Action
            primary
            disabled={busy || summaryUpdating || !summary}
            onClick={() => void share()}
          >
            <Send size={16} />
            {busy
              ? t("Sending…", "Wird gesendet…")
              : t("Share and open T3", "Teilen und T3 öffnen")}
          </Action>
        </div>
      </Modal>
      <Modal
        open={selection !== null}
        onOpenChange={(open) => {
          if (!reviewBusy && !busy && !open) {
            setSelection(null);
            setReview(null);
          }
        }}
        title={t("Review recommendations", "Vorschläge prüfen")}
        wide
      >
        {selection && (
          <>
            <div className="recommendation-list">
              {selection.proposal.recommendations.map((recommendation, i) => (
                <article key={i}>
                  <small className="muted">
                    {t("Recommendation", "Vorschlag")}{" "}
                    {formatNumber(i + 1, language)}
                  </small>
                  <h3>{recommendation.title}</h3>
                  <p>{recommendation.reason}</p>
                  <div className="proposed-change">
                    {recommendation.suggested_change}
                  </div>
                </article>
              ))}
            </div>
            {reviewBusy && (
              <p role="status">
                <Busy>
                  {t("Checking local rules…", "Lokale Regeln werden geprüft…")}
                </Busy>
              </p>
            )}
            {selection.proposal.rule_edits && !workspace?.workspace && (
              <Notice tone="info">
                <span>
                  {t(
                    "Create local rules once to check and save this proposal. The proposal stays open.",
                    "Lege lokale Regeln einmalig an, um diesen Vorschlag zu prüfen und zu speichern. Der Vorschlag bleibt geöffnet.",
                  )}
                </span>
                <Action
                  primary
                  disabled={reviewBusy || !workspace}
                  onClick={() => void initializeForReview()}
                >
                  {t("Create local rules", "Lokale Regeln anlegen")}
                </Action>
              </Notice>
            )}
            {reviewError && <Notice error>{reviewError}</Notice>}
            {review && (
              <>
                <h3 ref={reviewHeading} tabIndex={-1}>
                  {t(
                    "Effect on saved mail",
                    "Auswirkung auf gespeicherte Nachrichten",
                  )}
                </h3>
                <p className="help">
                  {t(
                    "Simulation only. Saving changes local rules; it does not move messages or deploy provider rules.",
                    "Nur eine Simulation. Speichern ändert lokale Regeln; es verschiebt keine Nachrichten und aktiviert keine Regeln beim Anbieter.",
                  )}
                </p>
                <div className="stats compact">
                  <Stat
                    label={t(
                      "Filed before → after",
                      "Abgelegt vorher → nachher",
                    )}
                    value={`${formatNumber(review.current.proof.counts.filed, language)} → ${formatNumber(review.resulting.proof.counts.filed, language)}`}
                  />
                  <Stat
                    label={t(
                      "Inbox before → after",
                      "Posteingang vorher → nachher",
                    )}
                    value={`${formatNumber(review.current.proof.counts.inbox, language)} → ${formatNumber(review.resulting.proof.counts.inbox, language)}`}
                  />
                  <Stat
                    label={t("Needs review", "Prüfung erforderlich")}
                    value={formatNumber(review.resulting.proof.counts.review, language)}
                  />
                  <Stat
                    label={t("Destination changes", "Zieländerungen")}
                    value={formatNumber(review.resulting.proof.diff.length, language)}
                  />
                </div>
                <ul className="evidence-list">
                  {review.edits.map((edit) => (
                    <li key={edit.digest}>
                      {edit.kind === "addition"
                        ? t("Add local rule", "Lokale Regel hinzufügen")
                        : t("Update local rule", "Lokale Regel ändern")}{" "}
                      <code>{edit.target}</code>
                    </li>
                  ))}
                </ul>
                {review.resulting.proof.diff.length > 0 && (
                  <Details
                    defaultOpen
                    title={t(
                      `Changed message destinations (${review.resulting.proof.diff.length})`,
                      `Geänderte Nachrichtenziele (${review.resulting.proof.diff.length})`,
                    )}
                  >
                    <div className="table-scroll">
                      <table>
                        <thead>
                          <tr>
                            <th>{t("Before", "Vorher")}</th>
                            <th>{t("After", "Nachher")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {review.resulting.proof.diff.slice(0, 25).map((d) => (
                            <tr key={d.id}>
                              <td>{d.before.join(", ") || "INBOX"}</td>
                              <td>{d.after}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </Details>
                )}
                {!review.accept_receipt && (
                  <Notice tone="warning">
                    {t(
                      "The local simulation blocks saving these changes. You can reject or dismiss the proposal or ask for revised recommendations.",
                      "Die lokale Simulation sperrt das Speichern dieser Änderungen. Du kannst den Vorschlag ablehnen, ausblenden oder überarbeitete Vorschläge anfordern.",
                    )}
                  </Notice>
                )}
                <Details
                  defaultOpen
                  title={t(
                    "Exact rule diff and validation",
                    "Genaue Regeländerungen und Prüfung",
                  )}
                >
                  <h4>{t("Changes", "Änderungen")}</h4>
                  <pre tabIndex={0}>{review.document_diff}</pre>
                  <p>
                    {t("Validation status", "Prüfstatus")}:{" "}
                    <StatusBadge status={review.resulting.proof.gate} />
                  </p>
                  <Details
                    title={t("Full YAML documents", "Vollständige YAML-Dokumente")}
                  >
                    <h4>{t("Current rules", "Aktuelle Regeln")}</h4>
                    <pre tabIndex={0}>
                      {review.original_documents.override_yaml}
                    </pre>
                    <h4>{t("Proposed rules", "Vorgeschlagene Regeln")}</h4>
                    <pre tabIndex={0}>
                      {review.resulting_documents.override_yaml}
                    </pre>
                  </Details>
                </Details>
              </>
            )}
            <div className="dialog-actions">
              <Action onClick={() => setSelection(null)} disabled={reviewBusy}>
                {t("Later", "Später")}
              </Action>
              <Action
                disabled={busy || reviewBusy}
                onClick={() => void dismiss(selection)}
              >
                {t("Dismiss", "Ausblenden")}
              </Action>
              {selection.proposal.rule_edits && review?.reject_receipt && (
                <Action
                  tone="danger-outline"
                  disabled={busy || reviewBusy}
                  onClick={() => void decide("reject")}
                >
                  <Ban size={15} />
                  {t("Reject edits", "Änderungen ablehnen")}
                </Action>
              )}
              {selection.proposal.rule_edits && (
                <Action
                  primary
                  onClick={() => void decide("accept")}
                  disabled={busy || reviewBusy || !review?.accept_receipt}
                >
                  {t("Save local rules", "Lokale Regeln speichern")}
                </Action>
              )}
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
