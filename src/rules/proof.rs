use super::{CompiledArtifact, Engine, sieve::Action};
use crate::census::{Census, Direction, MessageRecord, ScanState};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Destination {
    Inbox,
    Folder { name: String },
}
impl Destination {
    pub fn label(&self) -> &str {
        match self {
            Self::Inbox => "INBOX",
            Self::Folder { name } => name,
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub account: String,
    pub message_id_hash: String,
    pub expected: Destination,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Baseline {
    Current,
    Previous,
}
#[derive(Serialize)]
pub struct Proof {
    pub source: &'static str,
    pub semantics: &'static str,
    pub artifact: CompiledArtifact,
    pub counts: Counts,
    pub decisions: Vec<Decision>,
    pub baseline: Baseline,
    pub diff: Vec<FolderDiff>,
    pub rules: Vec<RuleCount>,
    pub months: Vec<String>,
    pub unmatched_senders: BTreeMap<String, usize>,
    pub pins: Vec<PinResult>,
    pub gate: &'static str,
    pub warnings: Vec<String>,
    pub runtime_cross_check: &'static str,
}
#[derive(Serialize)]
pub struct Counts {
    pub messages: usize,
    pub filed: usize,
    pub inbox: usize,
    pub copies: usize,
    pub copied_messages: usize,
    pub review: usize,
}
#[derive(Serialize)]
pub struct Decision {
    pub id: String,
    pub message_id_hash: Option<String>,
    pub subject: String,
    pub from: String,
    pub destination: Destination,
    pub matched_rule: Option<String>,
    pub review: bool,
    pub mark_read: bool,
    pub copies: Vec<CopyDecision>,
    pub copy_eligibility: Vec<CopyEligibility>,
    pub current_folders: BTreeSet<String>,
}
#[derive(Serialize)]
pub struct CopyDecision {
    pub rule: String,
    pub recipient: String,
}
#[derive(Serialize)]
pub struct CopyEligibility {
    pub rule: String,
    pub status: &'static str,
    pub reason: &'static str,
}
#[derive(Serialize)]
pub struct FolderDiff {
    pub id: String,
    pub before: BTreeSet<String>,
    pub after: String,
}
#[derive(Serialize)]
pub struct RuleCount {
    pub id: String,
    pub matched: usize,
    pub candidates: usize,
    pub monthly: BTreeMap<String, usize>,
    pub zero_match: bool,
    pub fully_shadowed: bool,
}
#[derive(Serialize)]
pub struct PinResult {
    pub account: String,
    pub message_id_hash: String,
    pub status: &'static str,
    pub expected: Destination,
    pub actual: Vec<Destination>,
}
struct InputMessage<'a> {
    id: String,
    record: &'a MessageRecord,
    folders: BTreeSet<String>,
    sent: bool,
}

fn messages(census: &Census, gmail: bool) -> Vec<InputMessage<'_>> {
    let mut result: BTreeMap<String, InputMessage<'_>> = BTreeMap::new();
    for (folder, f) in &census.folders {
        for m in f.messages.values() {
            let id = if gmail {
                m.gmail_message_id.clone().map(|v| format!("gmail/{v}"))
            } else {
                None
            }
            .unwrap_or_else(|| format!("{folder}/{}/{}", f.uidvalidity, m.uid));
            let entry = result.entry(id.clone()).or_insert_with(|| InputMessage {
                id,
                record: m,
                folders: BTreeSet::new(),
                sent: false,
            });
            entry.sent |= m.headers.direction == Direction::Sent;
            if gmail {
                entry
                    .folders
                    .extend(m.gmail_labels.iter().filter_map(|label| {
                        if label.eq_ignore_ascii_case("\\Inbox") {
                            Some("INBOX".to_owned())
                        } else if label.starts_with('\\') {
                            None
                        } else {
                            Some(label.clone())
                        }
                    }));
            }
            if !gmail || folder.eq_ignore_ascii_case("inbox") {
                entry.folders.insert(folder.clone());
            }
        }
    }
    let mut result: Vec<_> = result.into_values().filter(|m| !m.sent).collect();
    result.sort_by_key(|m| {
        (
            timestamp(m.record).unwrap_or(i64::MIN),
            m.record.uid,
            m.id.clone(),
        )
    });
    result
}
fn timestamp(m: &MessageRecord) -> Option<i64> {
    m.headers
        .date
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc2822(s).ok())
        .map(|d| d.timestamp())
}
fn months(as_of: i64) -> Result<Vec<String>, String> {
    let date = DateTime::<Utc>::from_timestamp(as_of, 0).ok_or("Invalid proof as-of time")?;
    let last = date.year() * 12 + date.month0() as i32;
    Ok((last - 12..=last)
        .map(|m| format!("{:04}-{:02}", m.div_euclid(12), m.rem_euclid(12) + 1))
        .collect())
}
fn execute(
    artifact: &CompiledArtifact,
    input: &[InputMessage<'_>],
) -> Result<
    (
        Vec<Decision>,
        BTreeMap<String, (usize, BTreeMap<String, usize>)>,
    ),
    String,
> {
    let statements = artifact.statements()?;
    let mut duplicates = BTreeSet::new();
    let mut decisions = Vec::new();
    let mut candidates: BTreeMap<String, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for m in input {
        let h = &m.record.headers;
        let review = statements
            .first()
            .ok_or("Missing role safety gate")?
            .test
            .evaluate(h, &duplicates);
        let mut d = Decision {
            id: m.id.clone(),
            message_id_hash: h.message_id_hash.clone(),
            subject: h.subject.clone(),
            from: h
                .from
                .iter()
                .map(|a| a.address.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            destination: Destination::Inbox,
            matched_rule: None,
            review,
            mark_read: false,
            copies: Vec::new(),
            copy_eligibility: Vec::new(),
            current_folders: m.folders.clone(),
        };
        if !review {
            let mut matched_ids = BTreeSet::new();
            for s in &statements {
                if s.id.starts_with("system.")
                    || !s.test.evaluate(h, &duplicates)
                    || !matched_ids.insert(&s.id)
                {
                    continue;
                }
                let row = candidates.entry(s.id.clone()).or_default();
                row.0 += 1;
                if let Some(date) =
                    timestamp(m.record).and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
                {
                    *row.1.entry(date.format("%Y-%m").to_string()).or_default() += 1;
                }
            }
        }
        for f in artifact.effective().iter().filter(|r| {
            r.origin == "override forward"
                && r.enabled
                && (r.accounts.is_empty() || r.accounts.contains(&artifact.context().account))
        }) {
            let statement = statements.iter().find(|s| s.id == f.id);
            let (status, reason) = if review {
                ("unknown", "Recipient role safety held the message")
            } else if artifact.omissions().iter().any(|o| o.rule == f.id) {
                (
                    "omitted",
                    "Required engine capability or evidence is unavailable",
                )
            } else if h.message_id_hash.is_none() {
                ("unknown", "Message-ID evidence is missing; no copy")
            } else if statement.is_some_and(|s| s.test.evaluate(h, &duplicates)) {
                ("eligible", "Compiled forward matches with duplicate guard")
            } else {
                (
                    "ineligible",
                    "Compiled forward does not match or is a duplicate",
                )
            };
            d.copy_eligibility.push(CopyEligibility {
                rule: f.id.clone(),
                status,
                reason,
            });
        }
        let mut pending = Vec::new();
        'script: for s in &statements {
            if !s.test.evaluate(h, &duplicates) {
                continue;
            }
            for action in &s.actions {
                match action {
                    Action::Copy(target) => {
                        d.copies.push(CopyDecision {
                            rule: s.id.clone(),
                            recipient: target.clone(),
                        });
                        if let Some(hash) = &h.message_id_hash {
                            pending.push(format!("mailroom.{}/{hash}", s.id));
                        }
                    }
                    Action::File(name) => {
                        d.destination = Destination::Folder { name: name.clone() };
                        d.matched_rule = Some(s.id.clone());
                    }
                    Action::Keep => {
                        d.destination = Destination::Inbox;
                        d.matched_rule = Some(s.id.clone());
                    }
                    Action::Seen => d.mark_read = true,
                    Action::Stop => break 'script,
                }
            }
        }
        duplicates.extend(pending);
        decisions.push(d);
    }
    Ok((decisions, candidates))
}

pub fn prove(
    artifact: CompiledArtifact,
    census: &Census,
    as_of: i64,
    pins: &[Pin],
    baseline: Baseline,
    previous: Option<&CompiledArtifact>,
) -> Result<Proof, String> {
    if artifact.context().account != census.account.as_str() {
        return Err("Compiled account does not match census".into());
    }
    if pins.len() > 200
        || pins.iter().any(|p| {
            p.message_id_hash.len() != 64
                || !p.message_id_hash.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err("Pins require census Message-ID SHA-256 references".into());
    }
    let months = months(as_of)?;
    if let Some(previous) = previous {
        if previous.context().account != artifact.context().account
            || previous.context().engine != artifact.context().engine
        {
            return Err("Previous artifact has a different account or engine".into());
        }
    }
    let input = messages(census, artifact.context().engine == Engine::GmailBacklog);
    let (decisions, _) = execute(&artifact, &input)?;
    let window_input: Vec<_> = input
        .iter()
        .filter(|m| {
            timestamp(m.record).is_some_and(|t| {
                t <= as_of
                    && DateTime::<Utc>::from_timestamp(t, 0)
                        .is_some_and(|d| months.contains(&d.format("%Y-%m").to_string()))
            })
        })
        .map(|m| InputMessage {
            id: m.id.clone(),
            record: m.record,
            folders: m.folders.clone(),
            sent: m.sent,
        })
        .collect();
    // Candidate diagnostics use the same compiled tests; selected counts use displayed decisions.
    let (_, candidates) = execute(&artifact, &window_input)?;
    let window_ids: BTreeSet<_> = window_input.iter().map(|m| &m.id).collect();
    let rules = artifact
        .effective()
        .iter()
        .filter(|r| {
            r.enabled && (r.accounts.is_empty() || r.accounts.contains(&artifact.context().account))
        })
        .map(|r| {
            let selected: Vec<_> = decisions
                .iter()
                .filter(|d| {
                    window_ids.contains(&d.id)
                        && (d.matched_rule.as_ref() == Some(&r.id)
                            || d.copies.iter().any(|c| c.rule == r.id))
                })
                .collect();
            let mut monthly: BTreeMap<_, _> = months.iter().map(|m| (m.clone(), 0)).collect();
            for d in &selected {
                if let Some(m) = input
                    .iter()
                    .find(|m| m.id == d.id)
                    .and_then(|m| timestamp(m.record))
                    .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
                {
                    *monthly.get_mut(&m.format("%Y-%m").to_string()).unwrap() += 1;
                }
            }
            let count = candidates.get(&r.id).map(|r| r.0).unwrap_or(0);
            RuleCount {
                id: r.id.clone(),
                matched: selected.len(),
                candidates: count,
                monthly,
                zero_match: selected.is_empty(),
                fully_shadowed: count > 0 && selected.is_empty(),
            }
        })
        .collect::<Vec<_>>();
    let mut warnings = vec!["Recipient safety accepts exactly one verified synthetic delivery recipient. Multiple recipients retain Inbox for review, including same-role recipients. Real provider provenance remains unknown.".into(), "Header census proves no attachment presence or accounting acceptance. Duplicate state starts empty for each proof and has no historical provider cache.".into(), "Folder paths use a logical slash separator. Actual provider separator, existing folders and rule coexistence remain unverified.".into()];
    if census.acquisition_state != ScanState::Complete
        || census
            .folders
            .values()
            .any(|f| f.state != ScanState::Complete)
    {
        warnings.push("Census is partial; counts cover saved records only".into());
    }
    if input
        .iter()
        .any(|m| timestamp(m.record).is_none_or(|t| t > as_of))
    {
        warnings.push(
            "Missing, invalid or future Date values are excluded from 13-month counts".into(),
        );
    }
    if rules.iter().any(|r| r.zero_match) {
        warnings.push("Some enabled rules match no selected messages in 13 months".into());
    }
    if rules.iter().any(|r| r.fully_shadowed) {
        warnings.push("Some rules are fully shadowed by earlier compiled rules".into());
    }
    let mut unmatched_senders = BTreeMap::new();
    for d in decisions
        .iter()
        .filter(|d| d.destination == Destination::Inbox)
    {
        *unmatched_senders.entry(d.from.clone()).or_default() += 1;
    }
    let pins: Vec<_> = pins
        .iter()
        .map(|p| {
            let actual: Vec<_> = if p.account == census.account.as_str() {
                decisions
                    .iter()
                    .filter(|d| d.message_id_hash.as_ref() == Some(&p.message_id_hash))
                    .map(|d| d.destination.clone())
                    .collect()
            } else {
                Vec::new()
            };
            let status = if actual.is_empty() {
                "stale"
            } else if actual.iter().all(|d| d == &p.expected) {
                "pass"
            } else {
                "fail"
            };
            PinResult {
                account: p.account.clone(),
                message_id_hash: p.message_id_hash.clone(),
                status,
                expected: p.expected.clone(),
                actual,
            }
        })
        .collect();
    if pins.iter().any(|p| p.status == "stale") {
        warnings.push("Some census pins are stale".into());
    }
    let previous_decisions = match (baseline, previous) {
        (Baseline::Previous, Some(previous)) => Some(execute(previous, &input)?.0),
        (Baseline::Previous, None) => {
            return Err("Previous comparison needs a previous compiled configuration".into());
        }
        _ => None,
    };
    let diff = decisions
        .iter()
        .filter_map(|d| {
            let before = previous_decisions
                .as_ref()
                .and_then(|p| p.iter().find(|p| p.id == d.id))
                .map(|p| BTreeSet::from([p.destination.label().to_owned()]))
                .unwrap_or_else(|| d.current_folders.clone());
            if before == BTreeSet::from([d.destination.label().to_owned()]) {
                None
            } else {
                Some(FolderDiff {
                    id: d.id.clone(),
                    before,
                    after: d.destination.label().into(),
                })
            }
        })
        .collect();
    let filed = decisions
        .iter()
        .filter(|d| matches!(d.destination, Destination::Folder { .. }))
        .count();
    let counts = Counts {
        messages: decisions.len(),
        filed,
        inbox: decisions.len() - filed,
        copies: decisions.iter().map(|d| d.copies.len()).sum(),
        copied_messages: decisions.iter().filter(|d| !d.copies.is_empty()).count(),
        review: decisions.iter().filter(|d| d.review).count(),
    };
    let gate = if pins.iter().any(|p| p.status == "fail") {
        "blocked"
    } else {
        "warnings"
    };
    Ok(Proof {
        source: "local_census",
        semantics: "parsed-emitted-sieve-subset-v2",
        artifact,
        counts,
        decisions,
        baseline,
        diff,
        rules,
        months,
        unmatched_senders,
        pins,
        gate,
        warnings,
        runtime_cross_check: "Not run by this proof. Run scripts/verify-sieve.sh on synthetic artifacts. This does not establish provider behavior.",
    })
}
