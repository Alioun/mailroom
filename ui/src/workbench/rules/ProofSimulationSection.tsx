import { useContext } from "react";
import { Download, Pin as PinIcon, X } from "lucide-react";
import type { Pin, ProofPreview, WorkspaceView } from "../../api";
import {
  Action,
  Details,
  LanguageContext,
  Stat,
  StatusBadge,
  useCopy,
} from "../components";
import { formatNumber } from "../format";

type Input = NonNullable<WorkspaceView["workspace"]>["input"];

export function ProofSimulationSection({
  proof,
  draft,
  busy,
  decisionSearch,
  decisionFilter,
  decisionPage,
  perPage,
  filteredDecisions,
  pageDecisions,
  onDecisionSearch,
  onDecisionFilter,
  onDecisionPage,
  onExportSieve,
  onAddPin,
  onRemovePin,
}: {
  proof: ProofPreview;
  draft: Input | null;
  busy: boolean;
  decisionSearch: string;
  decisionFilter: "all" | "inbox" | "review" | "copies";
  decisionPage: number;
  perPage: number;
  filteredDecisions: ProofPreview["proof"]["decisions"];
  pageDecisions: ProofPreview["proof"]["decisions"];
  onDecisionSearch: (value: string) => void;
  onDecisionFilter: (value: "all" | "inbox" | "review" | "copies") => void;
  onDecisionPage: (updater: (page: number) => number) => void;
  onExportSieve: () => void;
  onAddPin: (messageIdHash: string, expected: Pin["expected"]) => void;
  onRemovePin: (messageIdHash: string) => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);

  return (
    <section className="panel">
      <div className="section-heading">
        <h2>
          {t(
            "Simulation on saved mail",
            "Simulation mit gespeicherten Nachrichten",
          )}
        </h2>
        <div className="actions">
          <StatusBadge status={proof.proof.gate} />
          <span className="badge subtle">
            {t("No mail changed", "Keine Nachrichten verändert")}
          </span>
          {proof.download_available && (
            <Action onClick={onExportSieve} disabled={busy}>
              <Download size={15} />
              {t("Download rules.sieve", "rules.sieve herunterladen")}
            </Action>
          )}
        </div>
      </div>
      <div className="stats compact">
        <Stat
          label={t("Messages checked", "Geprüfte Nachrichten")}
          value={formatNumber(proof.proof.counts.messages, language)}
        />
        <Stat
          label={t("Would be filed", "Würden abgelegt")}
          value={formatNumber(proof.proof.counts.filed, language)}
        />
        <Stat
          label={t("Kept in Inbox", "Bleiben im Posteingang")}
          value={formatNumber(proof.proof.counts.inbox, language)}
        />
        <Stat
          label={t("Need review", "Benötigen Prüfung")}
          value={formatNumber(proof.proof.counts.review, language)}
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
                <td className="numeric">{formatNumber(rule.matched, language)}</td>
                <td>
                  {rule.fully_shadowed ? (
                    <StatusBadge
                      status="warning"
                      label={t(
                        "Covered by earlier rules",
                        "Von früheren Regeln abgedeckt",
                      )}
                    />
                  ) : rule.zero_match ? (
                    <StatusBadge
                      status="subtle"
                      tone="subtle"
                      label={t(
                        "No saved matches",
                        "Keine gespeicherten Treffer",
                      )}
                    />
                  ) : (
                    <StatusBadge
                      status="pass"
                      label={t(
                        "Matches saved mail",
                        "Passt auf gespeicherte Nachrichten",
                      )}
                    />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {draft && draft.pins.length > 0 && (
        <Details
          title={`${t("Pinned destination expectations", "Fixierte Zielerwartungen")} · ${draft.pins.length}`}
        >
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("Message hash", "Nachrichten-Hash")}</th>
                  <th>{t("Expected", "Erwartet")}</th>
                  <th>{t("Status", "Status")}</th>
                  <th>{t("Action", "Aktion")}</th>
                </tr>
              </thead>
              <tbody>
                {draft.pins.map((pin) => {
                  const evaluated = proof.proof.pins.find(
                    (p) => p.message_id_hash === pin.message_id_hash,
                  );
                  return (
                    <tr key={pin.message_id_hash}>
                      <td>
                        <code>{pin.message_id_hash.slice(0, 12)}</code>
                      </td>
                      <td>
                        {pin.expected.kind === "folder"
                          ? pin.expected.name
                          : "INBOX"}
                      </td>
                      <td>
                        <StatusBadge status={evaluated?.status ?? "pending"} />
                      </td>
                      <td>
                        <div className="cell-actions">
                          <Action
                            size="compact"
                            onClick={() => onRemovePin(pin.message_id_hash)}
                          >
                            <X size={13} />
                            {t("Remove", "Entfernen")}
                          </Action>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </Details>
      )}
      <Details
        title={`${t("Inspect message decisions", "Nachrichtenentscheidungen prüfen")} · ${formatNumber(proof.proof.decisions.length, language)}`}
      >
        <div className="toolbar">
          <input
            type="search"
            aria-label={t(
              "Filter message decisions",
              "Nachrichtenentscheidungen filtern",
            )}
            value={decisionSearch}
            onChange={(e) => onDecisionSearch(e.target.value)}
            placeholder={t(
              "Filter subject, sender, destination…",
              "Betreff, Absender, Ziel filtern…",
            )}
          />
          <div
            className="actions"
            role="group"
            aria-label={t("Decision filter", "Entscheidungsfilter")}
          >
            {(["all", "inbox", "review", "copies"] as const).map((f) => (
              <Action
                key={f}
                size="compact"
                primary={decisionFilter === f}
                aria-pressed={decisionFilter === f}
                onClick={() => onDecisionFilter(f)}
              >
                {f === "all"
                  ? t("All", "Alle")
                  : f === "inbox"
                    ? t("Inbox", "Posteingang")
                    : f === "review"
                      ? t("Review", "Prüfung")
                      : t("Copies", "Kopien")}
              </Action>
            ))}
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
                <th>{t("Pin", "Pin")}</th>
              </tr>
            </thead>
            <tbody>
              {pageDecisions.map((d) => {
                const alreadyPinned = Boolean(
                  d.message_id_hash &&
                    draft?.pins.some(
                      (p) => p.message_id_hash === d.message_id_hash,
                    ),
                );
                return (
                  <tr key={d.id}>
                    <td>{d.from}</td>
                    <td>{d.subject}</td>
                    <td>
                      {d.destination.kind === "folder"
                        ? d.destination.name
                        : "INBOX"}
                    </td>
                    <td>{d.matched_rule ?? "—"}</td>
                    <td>
                      {d.message_id_hash ? (
                        <div className="cell-actions">
                          <Action
                            size="compact"
                            disabled={alreadyPinned}
                            onClick={() =>
                              onAddPin(d.message_id_hash!, d.destination)
                            }
                          >
                            <PinIcon size={13} />
                            {alreadyPinned
                              ? t("Pinned", "Fixiert")
                              : t("Pin", "Fixieren")}
                          </Action>
                        </div>
                      ) : (
                        "—"
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        {filteredDecisions.length > perPage && (
          <div className="pagination">
            <span className="muted">
              {decisionPage * perPage + 1}–
              {Math.min(
                (decisionPage + 1) * perPage,
                filteredDecisions.length,
              )}{" "}
              / {filteredDecisions.length}
            </span>
            <div>
              <Action
                size="compact"
                disabled={decisionPage === 0}
                onClick={() => onDecisionPage((p) => Math.max(0, p - 1))}
              >
                {t("Previous", "Zurück")}
              </Action>
              <Action
                size="compact"
                disabled={
                  (decisionPage + 1) * perPage >= filteredDecisions.length
                }
                onClick={() => onDecisionPage((p) => p + 1)}
              >
                {t("Next", "Weiter")}
              </Action>
            </div>
          </div>
        )}
      </Details>
      <Details title={t("Validation details", "Prüfdetails")}>
        <p>
          {t("Validation status", "Prüfstatus")}:{" "}
          <StatusBadge status={proof.proof.gate} />
        </p>
        {proof.proof.warnings.length > 0 && (
          <ul className="evidence-list">
            {proof.proof.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        )}
        {proof.proof.artifact.omissions.length > 0 && (
          <ul className="evidence-list">
            {proof.proof.artifact.omissions.map((item, i) => (
              <li key={i}>
                <code>{item.rule}</code> · {item.reason}
              </li>
            ))}
          </ul>
        )}
      </Details>
    </section>
  );
}
