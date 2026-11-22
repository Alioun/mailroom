import { useContext, useState } from "react";
import { Download, ExternalLink, ShieldAlert } from "lucide-react";
import { gmailRoutePlan, gmailRouteText } from "../../gmail-route";
import { Action, Details, LanguageContext, Notice, useCopy } from "../components";

export function GmailForwardingPanel() {
  const t = useCopy();
  const language = useContext(LanguageContext);
  const copy = gmailRoutePlan[language];

  function downloadPlan() {
    const text = gmailRouteText(language);
    const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `gmail-forwarding-plan-${language}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  return (
    <section className="panel">
      <div className="section-heading">
        <div>
          <h2>{copy.title}</h2>
          <p className="muted">{copy.summary}</p>
        </div>
        <Action primary onClick={downloadPlan}>
          <Download size={15} />
          {t("Download plan", "Plan herunterladen")}
        </Action>
      </div>
      <Notice tone="warning">
        <ShieldAlert size={16} />
        <span>{copy.boundary}</span>
      </Notice>
      <p className="help">{copy.private}</p>
      <ol className="guide-steps">
        {gmailRoutePlan.stages.map((stage, i) => (
          <li key={i}>
            <div>
              <strong>{stage[language].title}</strong>
              <p className="muted">{stage[language].body}</p>
            </div>
          </li>
        ))}
      </ol>
      <p className="help">{copy.next}</p>
      {gmailRoutePlan.sources.length > 0 && (
        <Details title={t("Official references", "Offizielle Quellen")}>
          <ul className="evidence-list">
            {gmailRoutePlan.sources.map((src) => (
              <li key={src.url}>
                <a href={src.url} target="_blank" rel="noreferrer">
                  {src.label} <ExternalLink size={12} />
                </a>
              </li>
            ))}
          </ul>
        </Details>
      )}
    </section>
  );
}

