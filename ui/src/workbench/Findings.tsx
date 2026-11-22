import { useMemo, useState } from "react";
import {
  ArrowRight,
  Search,
  Users,
  MailCheck,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import type { CensusReport } from "../api";
import {
  Action,
  Choice,
  Details,
  Empty,
  Modal,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "./components";

type Sender = CensusReport["senders"][number];
function useClassLabel() {
  const t = useCopy();
  return (kind: Sender["class"]) =>
    ({
      bulk: t("Bulk mail", "Massenmail"),
      platform: t("Service updates", "Dienstmeldungen"),
      person: t("Correspondence", "Korrespondenz"),
      list_with_people: t("Discussion list", "Diskussionsliste"),
      unknown: t("Unclassified", "Nicht zugeordnet"),
    })[kind];
}
export function Overview({
  report,
  onSenders,
  onRecommend,
}: {
  report: CensusReport;
  onSenders: () => void;
  onRecommend: () => void;
}) {
  const t = useCopy();
  const classLabel = useClassLabel();
  const top = [...report.senders]
    .sort((a, b) => b.counts.messages - a.counts.messages)
    .slice(0, 5);
  const monthly = Object.entries(report.received.monthly)
    .sort(([a], [b]) => a.localeCompare(b))
    .slice(-12);
  const max = Math.max(1, ...monthly.map(([, value]) => value));
  const bulk = report.senders
    .filter((sender) => sender.class === "bulk")
    .reduce((total, sender) => total + sender.counts.messages, 0);
  const date = new Date(report.as_of * 1000).toLocaleString();
  const historical =
    report.acquisition.last_successful !== null && !report.acquisition.current;
  const scannedAt =
    report.acquisition.last_successful?.completed_at ??
    report.acquisition.latest?.finished_at;

  return (
    <div className="page-stack">
      <div className="coverage">
        <span
          className={`status-dot ${report.complete && !historical ? "" : "amber"}`}
        />
        <strong>
          {historical
            ? t(
                "Historical saved findings",
                "Historische gespeicherte Ergebnisse",
              )
            : report.complete
              ? t("Scan complete", "Erfassung vollständig")
              : t("Partial findings", "Teilweise erfasst")}
        </strong>
        <span className="muted">
          {report.folders.length} {t("folders", "Ordner")} ·{" "}
          {t("Analysis updated", "Analyse aktualisiert")} {date}
        </span>
      </div>
      {historical && (
        <Notice tone="warning">
          {t(
            "These saved results have no refreshed provider observations. Scan your mailbox to update them.",
            "Für diese gespeicherten Ergebnisse fehlen erneuerte Anbieterbeobachtungen. Erfasse dein Postfach, um sie zu aktualisieren.",
          )}
        </Notice>
      )}
      <div className="stats">
        <Stat
          label={t("Messages", "Nachrichten")}
          value={report.canonical_messages.toLocaleString()}
          detail={t(
            "Unique saved messages",
            "Eindeutige gespeicherte Nachrichten",
          )}
        />
        <Stat
          label={t("Received", "Empfangen")}
          value={report.received.messages.toLocaleString()}
          detail={`${report.received.seen.toLocaleString()} ${t("currently marked read", "aktuell als gelesen markiert")}`}
        />
        <Stat
          label={t("Sent", "Gesendet")}
          value={report.sent.toLocaleString()}
          detail={t(
            "Saved sent correspondence",
            "Gespeicherte gesendete Korrespondenz",
          )}
        />
        <Stat
          label={t("Senders", "Absender")}
          value={report.senders.length.toLocaleString()}
          detail={`${bulk.toLocaleString()} ${t("bulk messages", "Massenmails")}`}
        />
      </div>
      <div className="overview-grid">
        <section className="panel">
          <div className="section-heading">
            <div>
              <h2>{t("Mailbox activity", "Postfachaktivität")}</h2>
              <p className="muted">
                {t(
                  "Received messages by month",
                  "Empfangene Nachrichten pro Monat",
                )}
              </p>
            </div>
            <span className="badge subtle">
              {t("Saved headers", "Gespeicherte Kopfzeilen")}
            </span>
          </div>
          {monthly.length ? (
            <>
              {monthly.length < 4 ? (
                <div
                  className="compact-months"
                  role="img"
                  aria-label={t(
                    "Monthly received message volume. Exact counts are in the table below.",
                    "Empfangene Nachrichten pro Monat. Genaue Zahlen stehen in der Tabelle darunter.",
                  )}
                >
                  {monthly.map(([month, count]) => (
                    <div className="compact-month-row" key={month}>
                      <span>{month}</span>
                      <div className="compact-month-track">
                        <div
                          className="compact-month-bar"
                          style={{
                            width: `${Math.max(4, (count / max) * 100)}%`,
                          }}
                        />
                      </div>
                      <strong className="numeric">
                        {count.toLocaleString()}
                      </strong>
                    </div>
                  ))}
                </div>
              ) : (
                <div
                  className="chart"
                  role="img"
                  aria-label={t(
                    "Monthly received message volume. Exact counts are in the table below.",
                    "Empfangene Nachrichten pro Monat. Genaue Zahlen stehen in der Tabelle darunter.",
                  )}
                >
                  {monthly.map(([month, count]) => (
                    <div className="chart-column" key={month}>
                      <span className="chart-value">
                        {count.toLocaleString()}
                      </span>
                      <div className="chart-track">
                        <div
                          className="chart-bar"
                          style={{
                            height: `${Math.max(2, (count / max) * 100)}%`,
                          }}
                        />
                      </div>
                      <span className="chart-month">
                        {new Date(`${month}-01T12:00:00`).toLocaleString(
                          undefined,
                          { month: "short" },
                        )}
                      </span>
                    </div>
                  ))}
                </div>
              )}
              <p className="help">
                {t(
                  "Uses message dates, which senders can set. Read counts use current flags.",
                  "Verwendet das Nachrichtendatum, das Absender festlegen können. Gelesen-Zahlen beruhen auf aktuellen Markierungen.",
                )}
              </p>
              <Details
                title={t("View monthly counts", "Monatszahlen anzeigen")}
              >
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>{t("Month", "Monat")}</th>
                        <th className="numeric">{t("Received", "Empfangen")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {monthly.map(([month, count]) => (
                        <tr key={month}>
                          <th>{month}</th>
                          <td className="numeric">{count.toLocaleString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </Details>
            </>
          ) : (
            <p className="muted">
              {t(
                "No dated messages are saved yet.",
                "Noch keine datierten Nachrichten gespeichert.",
              )}
            </p>
          )}
        </section>
        <section className="panel">
          <div className="section-heading">
            <div>
              <h2>{t("Most active senders", "Aktivste Absender")}</h2>
              <p className="muted">
                {t(
                  "Where your mail comes from",
                  "Woher deine Nachrichten kommen",
                )}
              </p>
            </div>
            <Users size={20} className="muted" />
          </div>
          <div className="sender-ranking">
            {top.map((sender, i) => (
              <div className="rank-row" key={sender.sender ?? i}>
                <div className="sender-avatar">
                  {(sender.domain ?? sender.sender ?? "?")
                    .replace(/^.*@/, "")
                    .slice(0, 1)
                    .toUpperCase()}
                </div>
                <div className="rank-name">
                  <strong>
                    {sender.sender ??
                      t("Unknown sender", "Unbekannter Absender")}
                  </strong>
                  <small className="muted">{classLabel(sender.class)}</small>
                </div>
                <strong className="numeric">
                  {sender.counts.messages.toLocaleString()}
                </strong>
              </div>
            ))}
          </div>
          <Action className="full" onClick={onSenders}>
            {t("Explore all senders", "Alle Absender ansehen")}
            <ArrowRight size={16} />
          </Action>
        </section>
      </div>
      <section className="recommend-banner">
        <div className="banner-icon">
          <MailCheck size={25} />
        </div>
        <div>
          <h2>
            {t(
              "Make room for the mail that matters",
              "Mehr Platz für wichtige Nachrichten",
            )}
          </h2>
          <p>
            {t(
              "Ask T3 to suggest useful local rules from a summary you review first.",
              "Lass T3 lokale Regeln aus einer Zusammenfassung vorschlagen, die du vorher prüfst.",
            )}
          </p>
        </div>
        <Action primary onClick={onRecommend}>
          {t("Get recommendations", "Vorschläge erhalten")}
          <ArrowRight size={16} />
        </Action>
      </section>
      <Details
        title={t(
          "Scan coverage and evidence",
          "Erfassungsumfang und Nachweise",
        )}
      >
        <p>
          {t(
            "Saved records can include the same message in several folders. The message total above is deduplicated.",
            "Gespeicherte Datensätze können dieselbe Nachricht in mehreren Ordnern enthalten. Die Nachrichtenzahl oben zählt jede Nachricht einmal.",
          )}
        </p>
        <p className="help">
          {t("Last successful scan", "Letzte erfolgreiche Erfassung")} ·{" "}
          {scannedAt
            ? new Date(scannedAt * 1000).toLocaleString()
            : t("Time unavailable", "Zeitpunkt nicht verfügbar")}
        </p>
        <p className="help">
          {t("Current Spam count", "Aktuelle Spam-Anzahl")} ·{" "}
          {report.acquisition.spam.kind === "observed"
            ? report.acquisition.spam.messages.toLocaleString()
            : t("Unknown", "Unbekannt")}
        </p>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>{t("Folder", "Ordner")}</th>
                <th className="numeric">{t("Saved", "Gespeichert")}</th>
                <th className="numeric">{t("Inventory", "Bestand")}</th>
                <th>{t("State", "Status")}</th>
              </tr>
            </thead>
            <tbody>
              {report.folders.map((folder) => (
                <tr key={folder.folder}>
                  <th>{folder.folder}</th>
                  <td className="numeric">{folder.saved.toLocaleString()}</td>
                  <td className="numeric">
                    {folder.inventory.toLocaleString()}
                  </td>
                  <td>
                    <StatusBadge status={folder.state} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p className="help">
          {t(
            "Header observations do not authenticate senders. Current folder coverage does not prove historical Spam coverage.",
            "Kopfzeilen bestätigen keine Absenderidentität. Aktuelle Ordnerdaten belegen keine historische Spam-Abdeckung.",
          )}
        </p>
      </Details>
    </div>
  );
}

export function Senders({
  report,
  onRecommend,
}: {
  report: CensusReport;
  onRecommend: () => void;
}) {
  const t = useCopy();
  const classLabel = useClassLabel();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [sort, setSort] = useState("volume");
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Sender | null>(null);
  const results = useMemo(
    () =>
      report.senders
        .filter(
          (sender) =>
            (filter === "all" || sender.class === filter) &&
            `${sender.sender ?? ""} ${sender.domain ?? ""}`
              .toLowerCase()
              .includes(query.toLowerCase().trim()),
        )
        .sort((a, b) =>
          sort === "name"
            ? (a.sender ?? "").localeCompare(b.sender ?? "")
            : sort === "unread"
              ? b.counts.messages -
                b.counts.seen -
                (a.counts.messages - a.counts.seen)
              : b.counts.messages - a.counts.messages,
        ),
    [report, query, filter, sort],
  );
  const pages = Math.max(1, Math.ceil(results.length / 20));
  const current = Math.min(page, pages - 1);
  const visible = results.slice(current * 20, current * 20 + 20);
  return (
    <div className="page-stack">
      <div className="toolbar">
        <label className="search">
          <Search size={18} />
          <input
            aria-label={t("Search senders", "Absender suchen")}
            placeholder={t(
              "Search senders or domains…",
              "Absender oder Domains suchen…",
            )}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setPage(0);
            }}
          />
        </label>
        <Choice
          label={t("Filter by type", "Nach Typ filtern")}
          value={filter}
          onChange={(value) => {
            setFilter(value);
            setPage(0);
          }}
          options={[
            { value: "all", label: t("All types", "Alle Typen") },
            ...(
              [
                "bulk",
                "platform",
                "person",
                "list_with_people",
                "unknown",
              ] satisfies Sender["class"][]
            ).map((kind) => ({ value: kind, label: classLabel(kind) })),
          ]}
        />
        <Choice
          label={t("Sort senders", "Absender sortieren")}
          value={sort}
          onChange={(value) => {
            setSort(value);
            setPage(0);
          }}
          options={[
            {
              value: "volume",
              label: t("Most messages", "Meiste Nachrichten"),
            },
            { value: "unread", label: t("Most unread", "Meiste ungelesene") },
            { value: "name", label: t("Name A–Z", "Name A–Z") },
          ]}
        />
      </div>
      <div className="panel table-panel">
        <div className="section-heading">
          <h2>
            {t("Senders", "Absender")}{" "}
            <span className="count">{results.length.toLocaleString()}</span>
          </h2>
          <span className="help">
            {t(
              "Classification is evidence, not authentication.",
              "Zuordnung ist ein Hinweis, kein Identitätsnachweis.",
            )}
          </span>
        </div>
        {visible.length ? (
          <>
            <div className="table-scroll">
              <table className="senders-table">
                <thead>
                  <tr>
                    <th>{t("Sender", "Absender")}</th>
                    <th>{t("Type", "Typ")}</th>
                    <th className="numeric">{t("Messages", "Nachrichten")}</th>
                    <th className="numeric">
                      {t("Read / total", "Gelesen / gesamt")}
                    </th>
                    <th>{t("Details", "Details")}</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((sender, i) => (
                    <tr key={`${sender.sender}-${i}`}>
                      <td>
                        <strong>
                          {sender.sender ??
                            t("Unknown sender", "Unbekannter Absender")}
                        </strong>
                        <small className="muted">
                          {sender.domain ??
                            t("Unknown domain", "Unbekannte Domain")}
                        </small>
                      </td>
                      <td>
                        <span className={`badge class-${sender.class}`}>
                          {classLabel(sender.class)}
                        </span>
                      </td>
                      <td
                        className="numeric"
                        data-label={t("messages", "Nachrichten")}
                      >
                        {sender.counts.messages.toLocaleString()}
                      </td>
                      <td
                        className="numeric"
                        data-label={t("Read / total", "Gelesen / gesamt")}
                      >
                        {sender.counts.seen.toLocaleString()} /{" "}
                        {sender.counts.messages.toLocaleString()}
                      </td>
                      <td>
                        <Action
                          onClick={() => setSelected(sender)}
                          aria-label={`${t("Inspect", "Ansehen")} ${sender.sender ?? ""}`}
                        >
                          {t("Inspect", "Ansehen")}
                          <ArrowRight size={14} />
                        </Action>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="pagination">
              <span className="muted">
                {current * 20 + 1}–
                {Math.min(results.length, (current + 1) * 20)} /{" "}
                {results.length}
              </span>
              <div>
                <Action
                  aria-label={t("Previous page", "Vorherige Seite")}
                  disabled={current === 0}
                  onClick={() => setPage(current - 1)}
                >
                  <ChevronLeft size={16} />
                </Action>
                <span>
                  {current + 1} / {pages}
                </span>
                <Action
                  aria-label={t("Next page", "Nächste Seite")}
                  disabled={current + 1 >= pages}
                  onClick={() => setPage(current + 1)}
                >
                  <ChevronRight size={16} />
                </Action>
              </div>
            </div>
          </>
        ) : (
          <Empty title={t("No matching senders", "Keine passenden Absender")}>
            {t(
              "Try a different search or choose all types.",
              "Ändere die Suche oder wähle alle Typen.",
            )}
          </Empty>
        )}
      </div>
      <Modal
        open={selected !== null}
        onOpenChange={(open) => {
          if (!open) setSelected(null);
        }}
        title={selected?.sender ?? t("Sender details", "Absenderdetails")}
        wide
      >
        {selected && (
          <>
            <span className={`badge class-${selected.class}`}>
              {classLabel(selected.class)}
            </span>
            <div className="stats compact">
              <Stat
                label={t("Messages", "Nachrichten")}
                value={selected.counts.messages.toLocaleString()}
              />
              <Stat
                label={t("Read", "Gelesen")}
                value={selected.counts.seen.toLocaleString()}
              />
              <Stat
                label={t("Answered", "Beantwortet")}
                value={selected.counts.answered.toLocaleString()}
              />
            </div>
            <h3>{t("What we observed", "Unsere Beobachtungen")}</h3>
            <ul className="evidence-list">
              <li>
                {selected.wrote_to_sender
                  ? t(
                      "You have sent mail to this sender.",
                      "Du hast diesem Absender Nachrichten gesendet.",
                    )
                  : t(
                      "No saved sent correspondence with this sender.",
                      "Keine gespeicherte gesendete Korrespondenz mit diesem Absender.",
                    )}
              </li>
              <li>
                {selected.one_click_pair.toLocaleString()}{" "}
                {t(
                  "messages include a complete one-click unsubscribe pair.",
                  "Nachrichten enthalten ein vollständiges Ein-Klick-Abmeldepaar.",
                )}
              </li>
              <li>
                {selected.list_unsubscribe.toLocaleString()}{" "}
                {t(
                  "messages include an unsubscribe header.",
                  "Nachrichten enthalten einen Abmelde-Header.",
                )}
              </li>
              <li>
                {t(
                  "Sender authentication remains unknown.",
                  "Die Absenderauthentifizierung bleibt unbekannt.",
                )}
              </li>
            </ul>
            <Details
              title={t(
                "Subject keywords and folders",
                "Betreffschlüsselwörter und Ordner",
              )}
            >
              <div className="chips">
                {Object.entries(selected.subject_keywords).map(
                  ([word, count]) => (
                    <span className="badge subtle" key={word}>
                      {word} · {count}
                    </span>
                  ),
                )}
              </div>
              <ul>
                {selected.folders.map((folder) => (
                  <li key={folder}>{folder}</li>
                ))}
              </ul>
            </Details>
            <div className="dialog-actions">
              <Action onClick={() => setSelected(null)}>
                {t("Close", "Schließen")}
              </Action>
              <Action
                primary
                onClick={() => {
                  setSelected(null);
                  onRecommend();
                }}
              >
                {t("Ask for recommendations", "Vorschläge anfordern")}
              </Action>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
