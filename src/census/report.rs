use super::{
    AddressCandidate, Census, CensusError, CensusKeys, Direction, HeaderRecord, MessageRecord,
    ScanState,
};
use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EspObservation {
    AmazonSesFeedbackIdClaim,
    AmazonSesReturnPathClaim,
    MandrillReturnPathClaim,
    KlaviyoReturnPathClaim,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportLimitation {
    UnknownAuthentication,
    CurrentFlagsAndDates,
    GmailTopology,
    SubjectTokens,
    Classification,
}
impl ReportLimitation {
    fn text(self) -> &'static str {
        match self {
            Self::UnknownAuthentication => {
                "Header observations are unauthenticated. Relay originals are claims, not verified identities. DKIM unsubscribe coverage remains unknown."
            }
            Self::CurrentFlagsAndDates => {
                "Monthly volume uses header Date. Read rate uses current Seen flags. Latest read message date is the message Date, not the time it was read. Invalid and future dates are counted separately and excluded from windows."
            }
            Self::GmailTopology => {
                "Gmail arrivals use INTERNALDATE for received canonical messages only. Full Gmail label topology and excluded Spam/Trash counts are unavailable."
            }
            Self::SubjectTokens => {
                "Subject keywords are local tokens occurring in at least three distinct canonical received messages per sender, with digits masked. Summary keywords, folders, labels and List-Ids always use keyed placeholders."
            }
            Self::Classification => {
                "A discussion List-Post with List-Id suggests a list with people. A complete one-click pair suggests bulk. ESP or Auto-Submitted observations suggest platform. Confirmed Sent correspondence without bulk/list markers suggests person. Mixed or insufficient evidence stays unknown. No class grants authentication, routing or mutation permission."
            }
        }
    }
}

pub const ANALYSIS_VERSION: &str = "census-report-v1/psl-2.1.245";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderClass {
    Bulk,
    Platform,
    ListWithPeople,
    Person,
    Unknown,
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Counts {
    pub messages: usize,
    pub seen: usize,
    pub flagged: usize,
    pub answered: usize,
    pub monthly: BTreeMap<String, usize>,
    pub invalid_dates: usize,
    pub future_dates: usize,
    pub latest_read_message_date: Option<i64>,
    pub first_date: Option<i64>,
    pub last_date: Option<i64>,
    pub last_12_months: usize,
    pub read_last_6_months: usize,
    pub last_13_months: usize,
}
impl Counts {
    fn add(&mut self, message: &MessageRecord, as_of: i64) {
        self.messages += 1;
        let seen = message
            .flags
            .iter()
            .any(|f| f.eq_ignore_ascii_case("\\Seen"));
        self.seen += usize::from(seen);
        self.flagged += usize::from(
            message
                .flags
                .iter()
                .any(|f| f.eq_ignore_ascii_case("\\Flagged")),
        );
        self.answered += usize::from(
            message
                .flags
                .iter()
                .any(|f| f.eq_ignore_ascii_case("\\Answered")),
        );
        match header_date(&message.headers) {
            Some(date) if date <= as_of => {
                *self.monthly.entry(month(date)).or_default() += 1;
                self.first_date = Some(self.first_date.map_or(date, |old| old.min(date)));
                self.last_date = Some(self.last_date.map_or(date, |old| old.max(date)));
                if seen {
                    self.latest_read_message_date = Some(
                        self.latest_read_message_date
                            .map_or(date, |old| old.max(date)),
                    );
                }
                let now = DateTime::<Utc>::from_timestamp(as_of, 0).expect("validated as-of");
                for (months, count) in [
                    (12, &mut self.last_12_months),
                    (13, &mut self.last_13_months),
                ] {
                    if now
                        .checked_sub_months(chrono::Months::new(months))
                        .is_some_and(|start| date >= start.timestamp())
                    {
                        *count += 1;
                    }
                }
                if seen
                    && now
                        .checked_sub_months(chrono::Months::new(6))
                        .is_some_and(|start| date >= start.timestamp())
                {
                    self.read_last_6_months += 1;
                }
            }
            Some(_) => self.future_dates += 1,
            None => self.invalid_dates += 1,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct SenderReport {
    pub sender: Option<String>,
    pub domain: Option<String>,
    pub class: SenderClass,
    pub wrote_to_sender: bool,
    pub counts: Counts,
    pub raw_from: BTreeSet<String>,
    pub relay_claims: BTreeSet<String>,
    pub one_click_pair: usize,
    pub list_unsubscribe: usize,
    pub list_id_messages: usize,
    pub list_ids: BTreeSet<String>,
    pub esp_observations: BTreeSet<EspObservation>,
    pub folders: BTreeSet<String>,
    pub gmail_labels: BTreeSet<String>,
    pub recipient_addresses: BTreeSet<String>,
    pub subject_keywords: BTreeMap<String, usize>,
    #[serde(skip)]
    classes: BTreeSet<SenderClass>,
}
#[derive(Clone, Debug, Serialize)]
pub struct FolderReport {
    pub folder: String,
    pub state: ScanState,
    pub inventory: usize,
    pub saved: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct ClearCandidate {
    pub id: String,
    pub domain: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    #[serde(skip)]
    account: super::AccountId,
    pub version: String,
    pub analysis_version: &'static str,
    pub as_of: i64,
    pub state: ScanState,
    pub complete: bool,
    pub folders: Vec<FolderReport>,
    pub saved_records: usize,
    pub canonical_messages: usize,
    pub received: Counts,
    pub sent: usize,
    pub own_addresses: Vec<AddressCandidate>,
    pub own_domains: BTreeSet<String>,
    pub senders: Vec<SenderReport>,
    pub domains: BTreeMap<String, Counts>,
    pub gmail_arrivals: BTreeMap<String, usize>,
    pub invalid_arrival_dates: usize,
    pub future_arrival_dates: usize,
    pub clear_candidates: Vec<ClearCandidate>,
    pub limitations: Vec<ReportLimitation>,
}

fn header_date(h: &HeaderRecord) -> Option<i64> {
    DateTime::parse_from_rfc2822(h.date.as_deref()?)
        .ok()
        .filter(|d| (1970..=2100).contains(&d.year()))
        .map(|d| d.timestamp())
}
fn month(date: i64) -> String {
    DateTime::<Utc>::from_timestamp(date, 0)
        .expect("validated date")
        .format("%Y-%m")
        .to_string()
}
fn domain(address: &str) -> Option<String> {
    let (_, host) = address.rsplit_once('@')?;
    let host = idna::domain_to_ascii(host.trim_end_matches('.'))
        .ok()?
        .to_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok()
        || host.is_empty()
        || host.len() > 253
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return None;
    }
    psl::domain_str(&host).map(str::to_owned)
}
fn values<'a>(h: &'a HeaderRecord, name: &str) -> impl Iterator<Item = &'a str> {
    h.evidence
        .get(name)
        .into_iter()
        .flatten()
        .map(String::as_str)
}
fn has(h: &HeaderRecord, name: &str) -> bool {
    values(h, name).any(|v| !v.trim().is_empty())
}
fn one_click(h: &HeaderRecord) -> bool {
    values(h, "list-unsubscribe-post").count() == 1
        && values(h, "list-unsubscribe-post")
            .any(|v| v.trim().eq_ignore_ascii_case("List-Unsubscribe=One-Click"))
        && values(h, "list-unsubscribe").any(|v| {
            v.split('<')
                .skip(1)
                .filter_map(|s| s.split_once('>').map(|(uri, _)| uri))
                .any(|uri| {
                    uri.parse::<axum::http::Uri>().ok().is_some_and(|u| {
                        u.scheme_str() == Some("https")
                            && u.authority()
                                .is_some_and(|a| !a.host().is_empty() && !a.as_str().contains('@'))
                    })
                })
        })
}
fn esp(h: &HeaderRecord) -> BTreeSet<EspObservation> {
    let mut found = BTreeSet::new();
    if values(h, "feedback-id").any(|v| v.trim().ends_with(":AmazonSES")) {
        found.insert(EspObservation::AmazonSesFeedbackIdClaim);
    }
    if let Some(host) = &h.return_path_domain {
        for (suffix, label) in [
            ("amazonses.com", EspObservation::AmazonSesReturnPathClaim),
            ("mandrillapp.com", EspObservation::MandrillReturnPathClaim),
            ("klaviyomail.com", EspObservation::KlaviyoReturnPathClaim),
        ] {
            if host == suffix || host.ends_with(&format!(".{suffix}")) {
                found.insert(label);
            }
        }
    }
    found
}
fn classify(h: &HeaderRecord, wrote: bool) -> SenderClass {
    if has(h, "list-id")
        && values(h, "list-post").any(|v| v.to_ascii_lowercase().contains("<mailto:"))
    {
        SenderClass::ListWithPeople
    } else if one_click(h) {
        SenderClass::Bulk
    } else if !esp(h).is_empty()
        || values(h, "auto-submitted").any(|v| {
            v.trim()
                .split(';')
                .next()
                .is_some_and(|s| !s.eq_ignore_ascii_case("no") && !s.is_empty())
        })
    {
        SenderClass::Platform
    } else if wrote && !has(h, "list-id") && !has(h, "list-unsubscribe") && !has(h, "precedence") {
        SenderClass::Person
    } else {
        SenderClass::Unknown
    }
}
fn keywords(subject: &str) -> BTreeSet<String> {
    subject
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.chars().count() >= 3)
        .map(|s| {
            s.to_lowercase()
                .chars()
                .map(|c| if c.is_numeric() { '#' } else { c })
                .collect()
        })
        .collect()
}

impl Census {
    pub fn report(
        &self,
        keys: &CensusKeys,
        as_of: i64,
        gmail: bool,
    ) -> Result<Report, CensusError> {
        if !keys.belongs_to(&self.account) {
            return Err(CensusError::AccountMismatch);
        }
        if !DateTime::<Utc>::from_timestamp(as_of, 0)
            .is_some_and(|d| (1970..=2100).contains(&d.year()))
        {
            return Err(CensusError::InvalidCheckpoint);
        }
        let snapshot = serde_json::to_vec(self).map_err(|_| CensusError::Storage)?;
        let version = keys.binding(
            &[
                ANALYSIS_VERSION.as_bytes(),
                &snapshot,
                &as_of.to_be_bytes(),
                &[u8::from(gmail)],
            ]
            .concat(),
        );
        let mut messages: BTreeMap<String, (MessageRecord, BTreeSet<String>)> = BTreeMap::new();
        let mut folders = Vec::new();
        for (name, folder) in &self.folders {
            folders.push(FolderReport {
                folder: name.clone(),
                state: folder.state,
                inventory: folder.live_uids.len(),
                saved: folder.messages.len(),
            });
            for message in folder.messages.values() {
                let key = if gmail {
                    message
                        .gmail_message_id
                        .as_ref()
                        .map(|id| format!("gmail:{id}"))
                } else {
                    None
                }
                .unwrap_or_else(|| {
                    format!(
                        "{}:{name}:{}:{}",
                        name.len(),
                        folder.uidvalidity,
                        message.uid
                    )
                });
                let entry = messages
                    .entry(key)
                    .or_insert_with(|| (message.clone(), BTreeSet::new()));
                entry.1.insert(name.clone());
                entry.0.gmail_labels.extend(message.gmail_labels.clone());
                entry.0.flags.extend(message.flags.clone());
                if message.headers.direction == Direction::Sent {
                    entry.0.headers.direction = Direction::Sent;
                }
            }
        }
        let wrote: BTreeSet<String> = messages
            .values()
            .filter(|(m, _)| {
                m.headers.direction == Direction::Sent
                    && m.headers
                        .from
                        .iter()
                        .any(|a| self.confirmed_own_addresses.contains(&a.address))
            })
            .flat_map(|(m, _)| {
                m.headers
                    .to
                    .iter()
                    .chain(&m.headers.cc)
                    .chain(&m.headers.bcc)
                    .map(|a| a.address.clone())
            })
            .collect();
        let mut report = Report {
            account: self.account.clone(),
            version,
            analysis_version: ANALYSIS_VERSION,
            as_of,
            state: self.acquisition_state,
            complete: self.acquisition_state == ScanState::Complete
                && self.folders.values().all(|f| {
                    f.state == ScanState::Complete && f.live_uids.len() == f.messages.len()
                }),
            saved_records: folders.iter().map(|f| f.saved).sum(),
            folders,
            canonical_messages: messages.len(),
            received: Counts::default(),
            sent: 0,
            own_addresses: self.own_address_candidates(),
            own_domains: self
                .confirmed_own_addresses
                .iter()
                .filter_map(|a| domain(a))
                .collect(),
            senders: vec![],
            domains: BTreeMap::new(),
            gmail_arrivals: BTreeMap::new(),
            invalid_arrival_dates: 0,
            future_arrival_dates: 0,
            clear_candidates: vec![],
            limitations: vec![
                ReportLimitation::UnknownAuthentication,
                ReportLimitation::CurrentFlagsAndDates,
                ReportLimitation::GmailTopology,
                ReportLimitation::SubjectTokens,
                ReportLimitation::Classification,
            ],
        };
        let mut senders = BTreeMap::<Option<String>, SenderReport>::new();
        for (m, folders) in messages.values() {
            if m.headers.direction == Direction::Sent {
                report.sent += 1;
                continue;
            }
            let h = &m.headers;
            report.received.add(m, as_of);
            if gmail {
                let date = DateTime::parse_from_str(&m.internal_date, "%d-%b-%Y %H:%M:%S %z")
                    .ok()
                    .filter(|d| (1970..=2100).contains(&d.year()));
                match date {
                    Some(d) if d.timestamp() <= as_of => {
                        *report
                            .gmail_arrivals
                            .entry(month(d.timestamp()))
                            .or_default() += 1
                    }
                    Some(_) => report.future_arrival_dates += 1,
                    None => report.invalid_arrival_dates += 1,
                }
            }
            let sender = h.effective_sender.as_ref().map(|s| s.address.clone());
            let wrote_to_sender = sender.as_ref().is_some_and(|s| wrote.contains(s));
            let sender_domain = sender.as_deref().and_then(domain);
            let row = senders
                .entry(sender.clone())
                .or_insert_with(|| SenderReport {
                    sender,
                    domain: sender_domain.clone(),
                    class: SenderClass::Unknown,
                    wrote_to_sender,
                    counts: Counts::default(),
                    raw_from: BTreeSet::new(),
                    relay_claims: BTreeSet::new(),
                    one_click_pair: 0,
                    list_unsubscribe: 0,
                    list_id_messages: 0,
                    list_ids: BTreeSet::new(),
                    esp_observations: BTreeSet::new(),
                    folders: BTreeSet::new(),
                    gmail_labels: BTreeSet::new(),
                    recipient_addresses: BTreeSet::new(),
                    subject_keywords: BTreeMap::new(),
                    classes: BTreeSet::new(),
                });
            row.counts.add(m, as_of);
            row.classes.insert(classify(h, wrote_to_sender));
            row.raw_from
                .extend(h.from.iter().map(|a| a.address.clone()));
            for name in [
                "x-simplelogin-original-from",
                "x-anonaddy-original-from-header",
                "x-anonaddy-original-sender",
            ] {
                row.relay_claims.extend(values(h, name).map(str::to_owned));
            }
            row.one_click_pair += usize::from(one_click(h));
            row.list_unsubscribe += usize::from(has(h, "list-unsubscribe"));
            row.list_id_messages += usize::from(has(h, "list-id"));
            row.list_ids.extend(values(h, "list-id").map(str::to_owned));
            row.esp_observations.extend(esp(h));
            row.folders.extend(folders.clone());
            row.gmail_labels.extend(m.gmail_labels.clone());
            row.recipient_addresses.extend(
                h.delivered_to
                    .iter()
                    .chain(&h.original_to)
                    .chain(&h.to)
                    .chain(&h.cc)
                    .map(|a| a.address.clone()),
            );
            for keyword in keywords(&h.subject) {
                *row.subject_keywords.entry(keyword).or_default() += 1;
            }
            report
                .domains
                .entry(sender_domain.unwrap_or_else(|| "unknown".into()))
                .or_default()
                .add(m, as_of);
        }
        for mut row in senders.into_values() {
            if row.classes.len() == 1 {
                row.class = *row.classes.first().expect("one observed class");
            }
            row.subject_keywords.retain(|_, count| *count >= 3);
            report.senders.push(row);
        }
        let protected: BTreeSet<String> = report
            .own_addresses
            .iter()
            .filter_map(|a| domain(&a.address))
            .chain(
                report
                    .senders
                    .iter()
                    .filter(|s| !matches!(s.class, SenderClass::Bulk | SenderClass::Platform))
                    .filter_map(|s| s.domain.clone()),
            )
            .collect();
        let candidates: BTreeSet<String> = report
            .senders
            .iter()
            .filter(|s| {
                s.counts.messages >= 3
                    && matches!(s.class, SenderClass::Bulk | SenderClass::Platform)
                    && s.relay_claims.is_empty()
            })
            .filter_map(|s| s.domain.clone())
            .filter(|d| !protected.contains(d))
            .collect();
        report.clear_candidates = candidates
            .into_iter()
            .map(|domain| ClearCandidate {
                id: keys.binding(format!("clear-domain:{domain}").as_bytes()),
                domain,
            })
            .collect();
        Ok(report)
    }
}

#[derive(Serialize)]
pub struct SummaryPreview {
    pub summary: String,
    pub receipt: String,
}
impl Report {
    pub fn summary_preview(
        &self,
        keys: &CensusKeys,
        approved: &BTreeSet<String>,
    ) -> Result<SummaryPreview, CensusError> {
        if !keys.belongs_to(&self.account) {
            return Err(CensusError::AccountMismatch);
        }
        if !approved
            .iter()
            .all(|id| self.clear_candidates.iter().any(|c| &c.id == id))
        {
            return Err(CensusError::InvalidCheckpoint);
        }
        let reveal = |domain: &str| {
            self.clear_candidates
                .iter()
                .any(|c| c.domain == domain && approved.contains(&c.id))
        };
        let mut text = format!(
            "# Census summary\n\nAnalysis {}. As of {} UTC seconds. Acquisition {:?}. Complete {}.\n\n{} canonical messages, {} received, {} sent, {} saved records, {} scanned folders.\n\n",
            ANALYSIS_VERSION,
            self.as_of,
            self.state,
            self.complete,
            self.canonical_messages,
            self.received.messages,
            self.sent,
            self.saved_records,
            self.folders.len()
        );
        text.push_str("Confirmed own addresses\n\n");
        for a in self.own_addresses.iter().filter(|a| a.confirmed) {
            if let Some((local, host)) = a.address.rsplit_once('@') {
                text.push_str(&format!(
                    "- {}@{}\n",
                    keys.placeholder("alias", local),
                    keys.placeholder("own-domain", host)
                ));
            }
        }
        text.push_str(&format!("\n{} own-address candidates. {} confirmed registrable own domains.\n\nCurrent folders\n\n", self.own_addresses.len(), self.own_domains.len()));
        for f in &self.folders {
            text.push_str(&format!(
                "- {} saved {} / inventory {}, state {:?}\n",
                keys.placeholder("folder", &f.folder),
                f.saved,
                f.inventory,
                f.state
            ));
        }
        text.push_str("\nSenders with at least three messages\n\n");
        let mut tail = 0;
        for row in &self.senders {
            if row.counts.messages < 3 {
                tail += row.counts.messages;
                continue;
            }
            let sender = keys.placeholder("person", row.sender.as_deref().unwrap_or("unknown"));
            let host = row
                .domain
                .as_ref()
                .map(|d| {
                    if reveal(d)
                        && matches!(row.class, SenderClass::Bulk | SenderClass::Platform)
                        && row.relay_claims.is_empty()
                    {
                        d.clone()
                    } else {
                        keys.placeholder("domain", d)
                    }
                })
                .unwrap_or_else(|| "unknown".into());
            text.push_str(&format!("- {sender} / {host}. {:?}. Wrote to sender {}. {} messages, {} seen / {}, {} flagged, {} answered. One-click pair {}, List-Unsubscribe {}, List-Id {}. Latest read message date {:?}. Monthly {:?}. 12 months {}, 13 months {}, read in 6 months {}. Invalid dates {}, future dates {}.\n", row.class, row.wrote_to_sender, row.counts.messages, row.counts.seen, row.counts.messages, row.counts.flagged, row.counts.answered, row.one_click_pair, row.list_unsubscribe, row.list_id_messages, row.counts.latest_read_message_date, row.counts.monthly, row.counts.last_12_months, row.counts.last_13_months, row.counts.read_last_6_months, row.counts.invalid_dates, row.counts.future_dates));
            for (kind, values) in [
                ("folder", &row.folders),
                ("label", &row.gmail_labels),
                ("list", &row.list_ids),
            ] {
                for value in values {
                    text.push_str(&format!("  {}\n", keys.placeholder(kind, value)));
                }
            }
            for (word, count) in &row.subject_keywords {
                text.push_str(&format!(
                    "  {} {count}\n",
                    keys.placeholder("keyword", word)
                ));
            }
            for observation in &row.esp_observations {
                text.push_str(&format!("  {observation:?}\n"));
            }
            text.push_str(&format!(
                "  {} raw From values, {} relay claims. Authentication unknown.\n",
                row.raw_from.len(),
                row.relay_claims.len()
            ));
        }
        text.push_str(&format!(
            "\nLong tail {tail} messages.\n\nDomain volumes\n\n"
        ));
        for (d, counts) in &self.domains {
            let label = if reveal(d) {
                d.clone()
            } else {
                keys.placeholder("domain", d)
            };
            text.push_str(&format!(
                "- {label} {} messages, monthly {:?}\n",
                counts.messages, counts.monthly
            ));
        }
        text.push_str(&format!(
            "\nGmail arrivals {:?}. Invalid arrival dates {}, future arrival dates {}.\n\n",
            self.gmail_arrivals, self.invalid_arrival_dates, self.future_arrival_dates
        ));
        for limitation in &self.limitations {
            text.push_str(limitation.text());
            text.push('\n');
        }
        let receipt = keys.binding(
            &[
                self.version.as_bytes(),
                serde_json::to_string(approved)
                    .map_err(|_| CensusError::Storage)?
                    .as_bytes(),
                text.as_bytes(),
            ]
            .concat(),
        );
        Ok(SummaryPreview {
            summary: text,
            receipt,
        })
    }
}
