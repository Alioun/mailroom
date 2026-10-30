use super::{
    CompiledArtifact, Engine,
    sieve::{Action, MessageView, Truth},
};
use crate::census::{Census, Direction, MessageRecord, ScanState};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SEMANTICS: &str = "parsed-emitted-sieve-subset-v6";

pub mod diagnostics;
use diagnostics::{Diagnostics, Notice, RuntimeCheck};

#[cfg(test)]
#[path = "proof/generated_tests.rs"]
mod generated_tests;

/// Lowercase MIME part content types per proof message ID, from acquired originals.
/// A message without an entry has unknown structure.
pub type MimeEvidence = BTreeMap<String, BTreeSet<String>>;

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub account: String,
    pub message_id_hash: String,
    pub expected: Destination,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Baseline {
    Current,
    Previous,
}
#[derive(Clone, Serialize)]
pub struct Proof {
    pub source: &'static str,
    pub semantics: &'static str,
    pub artifact: CompiledArtifact,
    pub counts: Counts,
    pub decisions: Vec<Decision>,
    pub baseline: Baseline,
    pub diff: Vec<FolderDiff>,
    pub rules: Vec<RuleCount>,
    pub generated_rules: Vec<RuleCount>,
    pub months: Vec<String>,
    pub unmatched_senders: BTreeMap<String, usize>,
    pub pins: Vec<PinResult>,
    pub gate: &'static str,
    pub diagnostics: Diagnostics,
    pub warnings: Vec<String>,
    pub runtime_cross_check: &'static str,
}
#[derive(Clone, Serialize)]
pub struct Counts {
    pub messages: usize,
    pub filed: usize,
    pub inbox: usize,
    pub copies: usize,
    pub copied_messages: usize,
    pub review: usize,
}
#[derive(Clone, Serialize)]
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
    pub delivery_status: &'static str,
    pub keyword_additions: Vec<String>,
    pub observed_flags: Option<BTreeSet<String>>,
    pub delivery_tags: Vec<TagDecision>,
}
#[derive(Clone, Serialize)]
pub struct TagDecision {
    pub rule: String,
    pub status: &'static str,
    pub reason: &'static str,
}
#[derive(Clone, Serialize)]
pub struct CopyDecision {
    pub rule: String,
    pub recipient: String,
}
#[derive(Clone, Serialize)]
pub struct CopyEligibility {
    pub rule: String,
    pub status: &'static str,
    pub reason: &'static str,
}
#[derive(Clone, Serialize)]
pub struct FolderDiff {
    pub id: String,
    pub before: BTreeSet<String>,
    pub after: String,
}
#[derive(Clone, Serialize)]
pub struct RuleCount {
    pub id: String,
    pub matched: usize,
    pub candidates: usize,
    pub monthly: BTreeMap<String, usize>,
    pub zero_match: bool,
    pub fully_shadowed: bool,
}
#[derive(Clone, Serialize)]
pub struct PinResult {
    pub account: String,
    pub message_id_hash: String,
    pub status: &'static str,
    pub expected: Destination,
    pub actual: Vec<Destination>,
}
pub(super) struct InputMessage<'a> {
    pub(super) id: String,
    pub(super) record: &'a MessageRecord,
    pub(super) folders: BTreeSet<String>,
    sent: bool,
    media_types: Option<&'a BTreeSet<String>>,
}

pub(super) fn messages(census: &Census, gmail: bool) -> Vec<InputMessage<'_>> {
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
                media_types: None,
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
pub(super) fn view<'a>(artifact: &CompiledArtifact, m: &InputMessage<'a>) -> MessageView<'a> {
    let installed = match &artifact.context().recipient_evidence {
        super::EvidenceSource::SyntheticEnvelope { source } => Some(source),
        _ => None,
    };
    let binding = crate::census::delivery::DeliveryEvidence::message_binding(
        &m.id,
        m.record.headers.message_id_hash.as_deref(),
    );
    MessageView {
        headers: &m.record.headers,
        envelope: m.record.delivery.resolve(installed, &binding).0,
        media_types: m.media_types,
    }
}

/// Whether `test` is unknown only because the message's MIME structure is unknown:
/// assuming any known structure decides it.
fn attachment_unknown(
    test: &super::sieve::Test,
    view: &MessageView<'_>,
    duplicates: &BTreeSet<String>,
) -> bool {
    if view.media_types.is_some()
        || !test.uses_mime()
        || test.evaluate(view, duplicates) != Truth::Unknown
    {
        return false;
    }
    let none = BTreeSet::new();
    let every: BTreeSet<String> = super::program::SUPPORTED_INVOICE_TYPES
        .iter()
        .map(|t| t.to_string())
        .collect();
    [&none, &every].into_iter().all(|types| {
        test.evaluate(
            &MessageView {
                headers: view.headers,
                envelope: view.envelope,
                media_types: Some(types),
            },
            duplicates,
        ) != Truth::Unknown
    })
}

type Candidates = BTreeMap<String, (usize, BTreeMap<String, usize>)>;
struct Execution {
    decisions: Vec<Decision>,
    candidates: Candidates,
    /// Candidates of the messages marked in `window`, as a separate pass over
    /// only those messages would count them. Valid only while no copy entered
    /// duplicate state, since that state would differ in such a pass.
    window_candidates: Candidates,
    duplicate_state_used: bool,
}
fn execute(
    artifact: &CompiledArtifact,
    input: &[InputMessage<'_>],
    window: Option<&[bool]>,
) -> Result<Execution, String> {
    let statements = artifact.statements()?;
    let mut duplicates = BTreeSet::new();
    let mut decisions = Vec::new();
    let mut candidates = Candidates::new();
    let mut window_candidates = Candidates::new();
    for (index, m) in input.iter().enumerate() {
        let in_window = window.is_some_and(|w| w[index]);
        let h = &m.record.headers;
        let installed = match &artifact.context().recipient_evidence {
            super::EvidenceSource::SyntheticEnvelope { source } => Some(source),
            _ => None,
        };
        let binding = crate::census::delivery::DeliveryEvidence::message_binding(
            &m.id,
            h.message_id_hash.as_deref(),
        );
        let (envelope, delivery_status) = m.record.delivery.resolve(installed, &binding);
        let view = MessageView {
            headers: h,
            envelope,
            media_types: m.media_types,
        };
        let review = statements
            .iter()
            .find(|s| s.id == "system.role_safety")
            .ok_or("Missing role safety gate")?
            .test
            .evaluate(&view, &duplicates)
            != Truth::NoMatch;
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
            delivery_status,
            keyword_additions: Vec::new(),
            observed_flags: m.record.flags_observed.then(|| m.record.flags.clone()),
            delivery_tags: Vec::new(),
        };
        if !review {
            let mut matched_ids = BTreeSet::new();
            for s in &statements {
                if s.id == "system.role_safety"
                    || !s.test.evaluate(&view, &duplicates).matched()
                    || !matched_ids.insert(&s.id)
                {
                    continue;
                }
                let month = timestamp(m.record)
                    .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
                    .map(|date| date.format("%Y-%m").to_string());
                for (map, counted) in [(&mut candidates, true), (&mut window_candidates, in_window)]
                {
                    if !counted {
                        continue;
                    }
                    let row = map.entry(s.id.clone()).or_default();
                    row.0 += 1;
                    if let Some(month) = &month {
                        *row.1.entry(month.clone()).or_default() += 1;
                    }
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
            } else if statement.is_some_and(|s| s.test.evaluate(&view, &duplicates).matched()) {
                ("eligible", "Compiled forward matches with duplicate guard")
            } else if statement.is_some_and(|s| attachment_unknown(&s.test, &view, &duplicates)) {
                (
                    "unknown",
                    "MIME structure is unknown; the engine decides from the delivered message",
                )
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
            let truth = s.test.evaluate(&view, &duplicates);
            // Copies never change filing, so a copy that only MIME structure leaves open
            // predicts no copy without holding the message.
            if truth == Truth::Unknown
                && s.actions.iter().all(|a| matches!(a, Action::Copy(_)))
                && attachment_unknown(&s.test, &view, &duplicates)
            {
                continue;
            }
            if truth == Truth::Unknown {
                if review {
                    d.matched_rule = Some("system.role_safety".into());
                }
                d.review = true;
                d.destination = Destination::Inbox;
                d.copies.clear();
                d.keyword_additions.clear();
                d.mark_read = false;
                pending.clear();
                break;
            }
            if !truth.matched() {
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
                            pending.push(format!("{}/{hash}", super::program::copy_handle(target)));
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
                    Action::AddKeyword(k) => {
                        if !d
                            .keyword_additions
                            .iter()
                            .any(|v| v.eq_ignore_ascii_case(k.as_str()))
                        {
                            d.keyword_additions.push(k.as_str().into());
                        }
                    }
                    Action::Stop => break 'script,
                }
            }
        }
        for tag in artifact
            .effective()
            .iter()
            .filter(|r| r.origin == "override delivery tag")
        {
            let omitted = artifact.omissions().iter().any(|o| o.rule == tag.id);
            let result = statements
                .iter()
                .find(|s| s.id == tag.id)
                .map(|s| s.test.evaluate(&view, &duplicates));
            let added = match &tag.definition {
                super::program::EffectiveDefinition::DeliveryTag { rule } => d
                    .keyword_additions
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(rule.add_keyword.as_str())),
                _ => false,
            };
            let (status, reason) = if omitted {
                (
                    "omitted",
                    "Complete tag omitted by engine, scope, evidence or capabilities",
                )
            } else if result == Some(Truth::Unknown) {
                (
                    "unknown",
                    "Current delivery evidence is unavailable or incompatible",
                )
            } else if result == Some(Truth::Match) && !added {
                (
                    "unknown",
                    "Later recipient evidence is unknown; no keyword addition is predicted",
                )
            } else if result == Some(Truth::Match) && d.review {
                (
                    "held_tagged",
                    "Verified envelope matches; keyword added while recipient review keeps Inbox and read status. Ingress only: no role, filing or accounting eligibility",
                )
            } else if result == Some(Truth::Match) {
                (
                    "match",
                    "Parsed envelope predicate matches; additive keyword planned",
                )
            } else {
                (
                    "no_match",
                    "Parsed current delivery recipient does not match",
                )
            };
            d.delivery_tags.push(TagDecision {
                rule: tag.id.clone(),
                status,
                reason,
            });
        }
        duplicates.extend(pending);
        decisions.push(d);
    }
    Ok(Execution {
        decisions,
        candidates,
        window_candidates,
        duplicate_state_used: !duplicates.is_empty(),
    })
}

/// `prove` over a loaded census, remembered for the exact inputs: the census
/// digest and the serialized artifact, date, pins and baselines. A preview and
/// the confirmation that repeats it then evaluate a large census once.
pub fn prove_shared(
    artifact: CompiledArtifact,
    census: &crate::census::SharedCensus,
    as_of: i64,
    pins: &[Pin],
    baseline: Baseline,
    previous: Option<&CompiledArtifact>,
) -> Result<Proof, String> {
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    static PROOFS: Mutex<Vec<([u8; 32], Arc<Proof>)>> = Mutex::new(Vec::new());
    let inputs = serde_json::to_vec(&(&artifact, as_of, pins, baseline, previous))
        .map_err(|_| "Cannot key proof inputs")?;
    let key: [u8; 32] = Sha256::new()
        .chain_update(census.digest())
        .chain_update(&inputs)
        .finalize()
        .into();
    if let Some((_, proof)) = PROOFS
        .lock()
        .expect("proofs")
        .iter()
        .find(|(k, _)| *k == key)
    {
        return Ok((**proof).clone());
    }
    let proof = prove(artifact, census, as_of, pins, baseline, previous)?;
    let mut cache = PROOFS.lock().expect("proofs");
    if cache.len() >= 2 {
        cache.remove(0);
    }
    cache.push((key, Arc::new(proof.clone())));
    Ok(proof)
}

pub fn prove(
    artifact: CompiledArtifact,
    census: &Census,
    as_of: i64,
    pins: &[Pin],
    baseline: Baseline,
    previous: Option<&CompiledArtifact>,
) -> Result<Proof, String> {
    prove_with_evidence(
        artifact,
        census,
        as_of,
        pins,
        baseline,
        previous,
        &MimeEvidence::new(),
    )
}

/// `prove` with the MIME structure known for some messages.
pub fn prove_with_evidence(
    artifact: CompiledArtifact,
    census: &Census,
    as_of: i64,
    pins: &[Pin],
    baseline: Baseline,
    previous: Option<&CompiledArtifact>,
    evidence: &MimeEvidence,
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
    let mut input = messages(census, artifact.context().engine == Engine::GmailBacklog);
    for m in &mut input {
        m.media_types = evidence.get(&m.id);
    }
    let in_window: Vec<bool> = input
        .iter()
        .map(|m| {
            timestamp(m.record).is_some_and(|t| {
                t <= as_of
                    && DateTime::<Utc>::from_timestamp(t, 0)
                        .is_some_and(|d| months.contains(&d.format("%Y-%m").to_string()))
            })
        })
        .collect();
    let full = execute(&artifact, &input, Some(&in_window))?;
    let decisions = full.decisions;
    // Candidate diagnostics use the same compiled tests; selected counts use displayed decisions.
    // A separate pass over the window, in the input's order, evaluates every
    // message as the full pass did unless copies entered duplicate state.
    // (Running it on another core was slower: both passes contend for the
    // allocator.)
    let candidates = if full.duplicate_state_used {
        let window_input: Vec<_> = input
            .iter()
            .zip(&in_window)
            .filter(|(_, w)| **w)
            .map(|(m, _)| InputMessage {
                id: m.id.clone(),
                record: m.record,
                folders: m.folders.clone(),
                sent: m.sent,
                media_types: m.media_types,
            })
            .collect();
        execute(&artifact, &window_input, None)?.candidates
    } else {
        full.window_candidates
    };
    let window_ids: BTreeSet<_> = input
        .iter()
        .zip(&in_window)
        .filter(|(_, w)| **w)
        .map(|(m, _)| &m.id)
        .collect();
    let months_by_id: BTreeMap<&String, String> = input
        .iter()
        .filter_map(|m| {
            timestamp(m.record)
                .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
                .map(|d| (&m.id, d.format("%Y-%m").to_string()))
        })
        .collect();
    let mut generated = BTreeSet::new();
    let generated_ids = artifact
        .statements()?
        .into_iter()
        .filter(|s| s.id.starts_with("system.") && s.id != "system.role_safety")
        .filter_map(|s| generated.insert(s.id.clone()).then_some(s.id));
    let (rules, generated_rules): (Vec<_>, Vec<_>) = artifact
        .effective()
        .iter()
        .filter(|r| {
            r.enabled && (r.accounts.is_empty() || r.accounts.contains(&artifact.context().account))
        })
        .map(|r| r.id.clone())
        .chain(generated_ids)
        .map(|id| {
            let selected: Vec<_> = decisions
                .iter()
                .filter(|d| {
                    window_ids.contains(&d.id)
                        && (d.matched_rule.as_ref() == Some(&id)
                            || d.copies.iter().any(|c| c.rule == id)
                            || d.delivery_tags
                                .iter()
                                .any(|t| t.rule == id && t.status == "match"))
                })
                .collect();
            let mut monthly: BTreeMap<_, _> = months.iter().map(|m| (m.clone(), 0)).collect();
            for d in &selected {
                if let Some(month) = months_by_id.get(&d.id) {
                    *monthly.get_mut(month).unwrap() += 1;
                }
            }
            let count = candidates.get(&id).map(|r| r.0).unwrap_or(0);
            RuleCount {
                id,
                matched: selected.len(),
                candidates: count,
                monthly,
                zero_match: selected.is_empty(),
                fully_shadowed: count > 0 && selected.is_empty(),
            }
        })
        .partition(|r| !r.id.starts_with("system."));
    let mut notices = vec![
        Notice::DeliveryTagLimits,
        Notice::SyntheticRecipientLimits,
        Notice::AttachmentAndDuplicateLimits,
        Notice::FolderTopologyUnverified,
    ];
    if census.acquisition_state != ScanState::Complete
        || census
            .folders
            .values()
            .any(|f| f.state != ScanState::Complete)
    {
        notices.push(Notice::PartialCensus);
    }
    if input
        .iter()
        .any(|m| timestamp(m.record).is_none_or(|t| t > as_of))
    {
        notices.push(Notice::InvalidDates);
    }
    if rules.iter().any(|r| r.zero_match) {
        notices.push(Notice::RulesWithoutSelections);
    }
    if rules.iter().any(|r| r.fully_shadowed) {
        notices.push(Notice::ShadowedRules);
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
            } else if decisions.iter().any(|d| {
                d.message_id_hash.as_ref() == Some(&p.message_id_hash)
                    && d.review
                    && matches!(
                        artifact.context().recipient_evidence,
                        super::EvidenceSource::SyntheticEnvelope { .. }
                    )
                    && d.delivery_status != "known"
            }) {
                "unknown"
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
        notices.push(Notice::StalePins);
    }
    let previous_decisions = match (baseline, previous) {
        (Baseline::Previous, Some(previous)) => Some(execute(previous, &input, None)?.decisions),
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
    let gate = if pins.iter().any(|p| matches!(p.status, "fail" | "unknown")) {
        "blocked"
    } else {
        "warnings"
    };
    let diagnostics = Diagnostics {
        notices,
        runtime_cross_check: RuntimeCheck::NotRun,
    };
    Ok(Proof {
        source: "local_census",
        semantics: SEMANTICS,
        artifact,
        counts,
        decisions,
        baseline,
        diff,
        rules,
        generated_rules,
        months,
        unmatched_senders,
        pins,
        gate,
        warnings: diagnostics
            .notices
            .iter()
            .map(|notice| notice.message().into())
            .collect(),
        runtime_cross_check: diagnostics.runtime_cross_check.message(),
        diagnostics,
    })
}

#[cfg(test)]
mod window_tests {
    use super::*;

    /// Window candidates counted during the full pass equal a separate pass
    /// over only the window's messages, with and without copies entering
    /// duplicate state, for windows that hold all, some and none of them.
    #[test]
    fn window_candidates_from_the_full_pass_equal_a_separate_window_pass() {
        let mut input: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
        let census = crate::engine::synthetic_census(&input).unwrap();
        let messages = messages(&census, false);
        let with_forwards = crate::engine::artifact(&input.documents, &input.context).unwrap();
        let cut = input.documents.override_yaml.find("forwards:").unwrap();
        input.documents.override_yaml.truncate(cut);
        let without = crate::engine::artifact(&input.documents, &input.context).unwrap();
        let mut seen_copies = false;
        let mut compared = 0;
        for (artifact, every) in [&with_forwards, &without]
            .into_iter()
            .flat_map(|a| [1, 2, 3, usize::MAX].map(|e| (a, e)))
        {
            let window: Vec<bool> = (0..messages.len()).map(|i| i % every == 0).collect();
            let full = execute(&artifact, &messages, Some(&window)).unwrap();
            let only: Vec<_> = messages
                .iter()
                .zip(&window)
                .filter(|(_, w)| **w)
                .map(|(m, _)| InputMessage {
                    id: m.id.clone(),
                    record: m.record,
                    folders: m.folders.clone(),
                    sent: m.sent,
                    media_types: m.media_types,
                })
                .collect();
            let separate = execute(&artifact, &only, None).unwrap();
            seen_copies |= full.duplicate_state_used;
            if !full.duplicate_state_used {
                assert_eq!(full.window_candidates, separate.candidates, "every {every}");
                assert!(full.candidates.values().map(|c| c.0).sum::<usize>() > 0);
                compared += 1;
            }
        }
        assert!(seen_copies, "the sample's forwards enter duplicate state");
        assert_eq!(compared, 4, "every window of the artifact without forwards");
        let proof = prove(
            with_forwards,
            &census,
            input.as_of,
            &input.pins,
            input.baseline,
            None,
        )
        .unwrap();
        assert!(proof.rules.iter().any(|r| r.candidates > 0));
    }
}
