import { confirmProposals, reviewProposals } from "../../api";
import type {
  Account,
  ProposalGeneration,
  ProposalReview,
} from "../../api";
import { describeError } from "../status-labels";
import { Action, useCopy } from "../components";

export function ProposalsSection({
  account,
  proposals,
  proposalReview,
  busy,
  dirty,
  onBusy,
  onError,
  onMessage,
  onProposals,
  onProposalReview,
  onReload,
}: {
  account: Account;
  proposals: ProposalGeneration;
  proposalReview: ProposalReview | null;
  busy: boolean;
  dirty: boolean;
  onBusy: (value: boolean) => void;
  onError: (value: string) => void;
  onMessage: (value: string) => void;
  onProposals: (value: ProposalGeneration) => void;
  onProposalReview: (value: ProposalReview | null) => void;
  onReload: () => void;
}) {
  const t = useCopy();

  return (
    <section className="panel">
      <div className="section-heading">
        <h2>
          {t(
            "Deterministic rule proposals",
            "Deterministische Regelvorschläge",
          )}
        </h2>
        <span className="badge info">
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
                  disabled={busy || dirty}
                  onClick={async () => {
                    onBusy(true);
                    onError("");
                    try {
                      onProposalReview(
                        await reviewProposals(
                          account.id,
                          {
                            account: account.id,
                            generation: proposals.generation,
                            candidates: [{ id: cand.id, change: cand.change }],
                          },
                          "accept",
                        ),
                      );
                    } catch (err) {
                      onError(
                        describeError(
                          err,
                          t(
                            "Could not preview this proposal. Refresh proposals and try again.",
                            "Vorschlag konnte nicht geprüft werden. Lade die Vorschläge neu.",
                          ),
                        ),
                      );
                    } finally {
                      onBusy(false);
                    }
                  }}
                >
                  {t("Preview accept", "Annahme prüfen")}
                </Action>
                <Action
                  tone="danger-outline"
                  disabled={busy || dirty}
                  onClick={async () => {
                    onBusy(true);
                    onError("");
                    try {
                      const rev = await reviewProposals(
                        account.id,
                        {
                          account: account.id,
                          generation: proposals.generation,
                          candidates: [{ id: cand.id, change: cand.change }],
                        },
                        "reject",
                      );
                      onProposals(await confirmProposals(account.id, rev));
                      onMessage(
                        t("Proposal rejected.", "Regelvorschlag abgelehnt."),
                      );
                    } catch (err) {
                      onError(
                        describeError(
                          err,
                          t(
                            "Could not reject this proposal. Refresh proposals and try again.",
                            "Vorschlag konnte nicht abgelehnt werden. Lade die Vorschläge neu.",
                          ),
                        ),
                      );
                    } finally {
                      onBusy(false);
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
            <Action onClick={() => onProposalReview(null)}>
              {t("Cancel", "Abbrechen")}
            </Action>
            <Action
              primary
              disabled={!proposalReview.acceptance_available || busy || dirty}
              onClick={async () => {
                onBusy(true);
                onError("");
                try {
                  onProposals(
                    await confirmProposals(account.id, proposalReview),
                  );
                  onProposalReview(null);
                  onReload();
                  onMessage(
                    t(
                      "Proposal applied to local rules.",
                      "Vorschlag in lokale Regeln übernommen.",
                    ),
                  );
                } catch (err) {
                  onError(
                    describeError(
                      err,
                      t(
                        "Could not apply proposal. Reload rules and retry.",
                        "Vorschlag konnte nicht angewendet werden. Lade Regeln neu.",
                      ),
                    ),
                  );
                } finally {
                  onBusy(false);
                }
              }}
            >
              {t("Apply proposal", "Vorschlag anwenden")}
            </Action>
          </div>
        </div>
      )}
    </section>
  );
}
