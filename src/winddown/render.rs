use super::*;

impl Report {
    pub fn terminal(&self) -> String {
        let mut out = format!(
            "Wind-down report\nAccount {}\nAs of {}\n13-month boundary {}\nVerdict {}\nCounts {}\nBlockers {}\n{}\n\nGroup\tStatus\tSender\tCount\tRecent\tFirst message date\tLast message date\n",
            self.account,
            date(Some(self.as_of)),
            date(Some(self.boundary)),
            self.verdict,
            serde_json::to_string(&self.counts).expect("counts"),
            self.blockers.iter().cloned().collect::<Vec<_>>().join(", "),
            self.effects
        );
        for r in &self.rows {
            out.push_str(&format!(
                "{:?}\t{:?}\t{}\t{}\t{}\t{}\t{}\n",
                r.group,
                r.status,
                serde_json::to_string(&r.sender).expect("sender"),
                r.messages,
                r.recent,
                date(r.first_date),
                date(r.last_date)
            ));
            out.push_str(&format!(
                "Evidence {}\n",
                serde_json::to_string(r).expect("row")
            ));
        }
        out.push_str(&format!(
            "Sources {}\nAnalysis {} {}\nLedger {}\nInput {}\n",
            serde_json::to_string(&self.sources).expect("sources"),
            self.census_analysis,
            self.sender_data_version,
            self.ledger_version,
            self.version
        ));
        out
    }
    pub fn summary(&self, keys: &CensusKeys) -> String {
        let mut out = format!(
            "Wind-down summary\nAs of {}\nVerdict {}\nCounts {}\nBlockers {}\n{}\nRaw addresses, service associations, settings links and notes withheld.\n",
            date(Some(self.as_of)),
            self.verdict,
            serde_json::to_string(&self.counts).expect("counts"),
            self.blockers.iter().cloned().collect::<Vec<_>>().join(", "),
            self.effects
        );
        out.push_str(&format!(
            "Input {}\nAnalysis {} {} {}\n",
            self.version, self.semantics, self.census_analysis, self.sender_data_version
        ));
        for source in &self.sources {
            out.push_str(&format!(
                "Source {} version {} complete {} Spam {} coverage {} / {}\n",
                keys.placeholder("account", &source.account),
                source.census_version,
                source.complete,
                source
                    .spam_count
                    .map_or_else(|| "unknown".into(), |v| v.to_string()),
                date(source.coverage_since),
                date(source.coverage_through)
            ));
        }
        for source in &self.sources {
            if let Some(crate::census::acquisition::CountObservation::Observed {
                observed_at,
                ..
            }) = &source.spam_observation
            {
                out.push_str(&format!("Spam count observation {} via STATUS MESSAGES. Current mailbox total only; historical Spam coverage remains unknown.\n", date(Some(*observed_at))));
            }
        }
        let mut tail = 0;
        for row in &self.rows {
            if row.messages < 3 {
                tail += row.messages;
                continue;
            }
            out.push_str(&format!(
                "{} {:?} {:?} count {} recent {} first {} last {} reasons {}\n",
                keys.placeholder("sender", &row.id),
                row.group,
                row.status,
                row.messages,
                row.recent,
                date(row.first_date),
                date(row.last_date),
                row.reasons.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        out.push_str(&format!(
            "Long tail {tail} messages. Spam and coverage uncertainty remains as listed above.\n"
        ));
        out
    }
    pub fn export_preview(&self, keys: &CensusKeys, disclosure: Disclosure) -> ExportPreview {
        let text = match disclosure {
            Disclosure::LocalFile => self.terminal(),
            Disclosure::AgentSummary => self.summary(keys),
        };
        let receipt = token(
            keys,
            b"winddown-exact-disclosure/v1",
            &(&self, disclosure, text.as_bytes()),
        );
        ExportPreview {
            receipt,
            disclosure,
            text,
            filename: match disclosure {
                Disclosure::LocalFile => "winddown-local.txt",
                Disclosure::AgentSummary => "winddown-summary.txt",
            },
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disclosure {
    LocalFile,
    AgentSummary,
}
#[derive(Serialize)]
pub struct ExportPreview {
    pub receipt: String,
    pub disclosure: Disclosure,
    pub text: String,
    pub filename: &'static str,
}
