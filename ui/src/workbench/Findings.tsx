import { useContext, useMemo, useState } from "react";
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
  LanguageContext,
  Modal,
  Notice,
  Stat,
  StatusBadge,
  useCopy,
} from "./components";
import {
  formatDateTime,
  formatNumber,
  localeFor,
  pluralCount,
} from "./format";

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
  const language = useContext(LanguageContext);
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
  const date = formatDateTime(report.as_of, language);
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
          {pluralCount(
            report.folders.length,
            language,
            "folder",
            "folders",
            "Ordner",
            "Ordner",
          )}{" "}
          · {t("Analysis updated", "Analyse aktualisiert")} {date}
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
          value={formatNumber(report.canonical_messages, language)}
          detail={t(
            "Unique saved messages",
            "Eindeutige gespeicherte Nachrichten",
          )}
        />
        <Stat
          label={t("Received", "Empfangen")}
          value={formatNumber(report.received.messages, language)}
          detail={`${formatNumber(report.received.seen, language)} ${t("currently marked read", "aktuell als gelesen markiert")}`}
        />
        <Stat
          label={t("Sent", "Gesendet")}
          value={formatNumber(report.sent, language)}
          detail={t(
            "Saved sent correspondence",
            "Gespeicherte gesendete Korrespondenz",
          )}
        />
        <Stat
          label={t("Senders", "Absender")}
          value={formatNumber(report.senders.length, language)}
          detail={pluralCount(
            bulk,
            language,
            "bulk message",
            "bulk messages",
            "Massenmail",
            "Massenmails",
          )}
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
                        {formatNumber(count, language)}
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
                        {formatNumber(count, language)}
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
                          localeFor(language),
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
                          <td className="numeric">
                            {formatNumber(count, language)}
                          </td>
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
                  {formatNumber(sender.counts.messages, language)}
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
            ? formatDateTime(scannedAt, language)
            : t("Time unavailable", "Zeitpunkt nicht verfügbar")}
        </p>
        <p className="help">
          {t("Current Spam count", "Aktuelle Spam-Anzahl")} ·{" "}
          {report.acquisition.spam.kind === "observed"
            ? formatNumber(report.acquisition.spam.messages, language)
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
                  <td className="numeric">
                    {formatNumber(folder.saved, language)}
                  </td>
                  <td className="numeric">
                    {formatNumber(folder.inventory, language)}
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
  onGoRules,
}: {
  report: CensusReport;
  onRecommend: () => void;
  onGoRules?: () => void;
}) {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const classLabel = useClassLabel();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [sort, setSort] = useState("volume");
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Sender | null>(null);
  const [checkedKeys, setCheckedKeys] = useState<Set<string>>(() => new Set());
  const senderKey = (sender: Sender, index: number) =>
    `${sender.sender ?? ""}::${sender.domain ?? ""}::${index}`;
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
  const visibleKeys = visible.map((sender, i) =>
    senderKey(sender, current * 20 + i),
  );
  const allVisibleChecked =
    visibleKeys.length > 0 && visibleKeys.every((key) => checkedKeys.has(key));

  function toggleOne(key: string) {
    setCheckedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  function toggleVisible() {
    setCheckedKeys((prev) => {
      const next = new Set(prev);
      if (allVisibleChecked) {
        for (const key of visibleKeys) next.delete(key);
      } else {
        for (const key of visibleKeys) next.add(key);
      }
      return next;
    });
  }

  return (
    <div className="page-stack">
      <div className="toolbar">
        <label className="search">
          <Search size={18} />
          <input
            id="senders-search-input"
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
            <span className="count">{formatNumber(results.length, language)}</span>
          </h2>
          <span className="help">
            {t(
              "Classification is evidence, not authentication.",
              "Zuordnung ist ein Hinweis, kein Identitätsnachweis.",
            )}
          </span>
        </div>
        {checkedKeys.size > 0 && (
          <div className="bulk-bar" role="region" aria-label={t("Selected senders", "Ausgewählte Absender")}>
            <strong>
              {pluralCount(
                checkedKeys.size,
                language,
                "sender selected",
                "senders selected",
                "Absender ausgewählt",
                "Absender ausgewählt",
              )}
            </strong>
            <div className="bulk-bar-actions">
              <Action
                size="compact"
                primary
                onClick={() => {
                  setCheckedKeys(new Set());
                  onRecommend();
                }}
              >
                {t("Ask for recommendations", "Vorschläge anfordern")}
              </Action>
              {onGoRules && (
                <Action
                  size="compact"
                  onClick={() => {
                    setCheckedKeys(new Set());
                    onGoRules();
                  }}
                >
                  {t("Open Rules & deploy", "Regeln & Bereitstellung")}
                </Action>
              )}
              <Action size="compact" onClick={() => setCheckedKeys(new Set())}>
                {t("Clear selection", "Auswahl aufheben")}
              </Action>
            </div>
          </div>
        )}
        {visible.length ? (
          <>
            <div className="table-scroll">
              <table className="senders-table">
                <thead>
                  <tr>
                    <th>
                      <div className="sender-check-cell">
                        <input
                          type="checkbox"
                          className="row-check"
                          checked={allVisibleChecked}
                          onChange={toggleVisible}
                          aria-label={t(
                            "Select all senders on this page",
                            "Alle Absender auf dieser Seite auswählen",
                          )}
                        />
                        <span>{t("Sender", "Absender")}</span>
                      </div>
                    </th>
                    <th>{t("Type", "Typ")}</th>
                    <th className="numeric">{t("Messages", "Nachrichten")}</th>
                    <th className="numeric">
                      {t("Read / total", "Gelesen / gesamt")}
                    </th>
                    <th>{t("Details", "Details")}</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((sender, i) => {
                    const key = senderKey(sender, current * 20 + i);
                    const senderLabel =
                      sender.sender ??
                      t("Unknown sender", "Unbekannter Absender");
                    return (
                      <tr key={key}>
                        <td>
                          <div className="sender-check-cell">
                            <input
                              type="checkbox"
                              className="row-check"
                              checked={checkedKeys.has(key)}
                              onChange={() => toggleOne(key)}
                              aria-label={`${t("Select", "Auswählen")} ${senderLabel}`}
                            />
                            <div>
                              <strong>{senderLabel}</strong>
                              <small className="muted">
                                {sender.domain ??
                                  t("Unknown domain", "Unbekannte Domain")}
                              </small>
                            </div>
                          </div>
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
                          {formatNumber(sender.counts.messages, language)}
                        </td>
                        <td
                          className="numeric"
                          data-label={t("Read / total", "Gelesen / gesamt")}
                        >
                          {formatNumber(sender.counts.seen, language)} /{" "}
                          {formatNumber(sender.counts.messages, language)}
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
                    );
                  })}
                </tbody>
              </table>
            </div>
            <div className="pagination">
              <span className="muted">
                {formatNumber(current * 20 + 1, language)}–
                {formatNumber(
                  Math.min(results.length, (current + 1) * 20),
                  language,
                )}{" "}
                / {formatNumber(results.length, language)}
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
                  {formatNumber(current + 1, language)} /{" "}
                  {formatNumber(pages, language)}
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
                value={formatNumber(selected.counts.messages, language)}
              />
              <Stat
                label={t("Read", "Gelesen")}
                value={formatNumber(selected.counts.seen, language)}
              />
              <Stat
                label={t("Answered", "Beantwortet")}
                value={formatNumber(selected.counts.answered, language)}
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
                {pluralCount(
                  selected.one_click_pair,
                  language,
                  "message includes a complete one-click unsubscribe pair.",
                  "messages include a complete one-click unsubscribe pair.",
                  "Nachricht enthält ein vollständiges Ein-Klick-Abmeldepaar.",
                  "Nachrichten enthalten ein vollständiges Ein-Klick-Abmeldepaar.",
                )}
              </li>
              <li>
                {pluralCount(
                  selected.list_unsubscribe,
                  language,
                  "message includes an unsubscribe header.",
                  "messages include an unsubscribe header.",
                  "Nachricht enthält einen Abmelde-Header.",
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
