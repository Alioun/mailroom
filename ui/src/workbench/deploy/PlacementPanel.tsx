import { useContext, useState } from "react";
import type { AccountView } from "../../api";
import { Action, Details, Empty, LanguageContext, Stat, StatusBadge, useCopy } from "../components";
import { formatNumber } from "../format";

export function PlacementPanel({ view }: { view: AccountView | null }) {
  const t = useCopy();
  const language = useContext(LanguageContext);
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
          "Once you publish rules to mailbox.org and run a fresh mailbox scan, Mailroom compares observed folders against your published rules here.",
          "Sobald du Regeln bei mailbox.org veröffentlichst und das Postfach erneut erfasst, vergleicht Mailroom hier die tatsächlichen Ordner mit deinen veröffentlichten Regeln.",
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
            value={formatNumber(landing.counts.matches, language)}
          />
          <Stat
            label={t("Elsewhere", "Woanders")}
            value={formatNumber(landing.counts.elsewhere, language)}
          />
          <Stat
            label={t("Additional folders", "Zusätzliche Ordner")}
            value={formatNumber(landing.counts.additional_folders, language)}
          />
          <Stat
            label={t("Uncheckable", "Nicht prüfbar")}
            value={formatNumber(landing.counts.uncheckable, language)}
          />
          <Stat
            label={t("Total messages", "Nachrichten gesamt")}
            value={formatNumber(rows.length, language)}
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
                      <StatusBadge status={status} />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        {rows.length > perPage && (
          <div className="pagination">
            <span className="muted">
              {page * perPage + 1}–{Math.min((page + 1) * perPage, rows.length)}{" "}
              / {rows.length}
            </span>
            <div>
              <Action
                size="compact"
                disabled={page === 0}
                onClick={() => setPage((p) => Math.max(0, p - 1))}
              >
                {t("Previous", "Zurück")}
              </Action>
              <Action
                size="compact"
                disabled={(page + 1) * perPage >= rows.length}
                onClick={() => setPage((p) => p + 1)}
              >
                {t("Next", "Weiter")}
              </Action>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
