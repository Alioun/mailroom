mod render;
pub use render::{Disclosure, ExportPreview};
#[cfg(test)]
pub(crate) mod synthetic;
#[cfg(test)]
mod tests;
use crate::{
    acquisition::Provider,
    census::{
        AccountState, CensusKeys, Direction, MessageRecord,
        report::{Counts, SenderClass, header_date},
    },
    ledger::Snapshot,
};
use chrono::{DateTime, Months, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SEMANTICS: &str = "mailroom/winddown/v1";
pub const EFFECTS: &str = "Local bookkeeping only. Dismissal removes a sender from waiting, not from unresolved evidence. Undismissal restores evidence-derived status. No retirement, forwarding, rule deployment, unsubscribe, alias disabling or sending is authorized.";

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionEvidence {
    pub complete_since: Option<i64>,
    pub complete_through: Option<i64>,
    pub spam_count: Option<usize>,
    pub deliveries: BTreeMap<String, Delivery>,
}
impl std::fmt::Debug for AcquisitionEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcquisitionEvidence([PRIVATE])")
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Delivery {
    Direct { address: String },
    Forwarded,
    Ambiguous,
}
pub fn message_identity(message: &MessageRecord) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(
        serde_json::to_vec(&(
            &message.headers,
            &message.internal_date,
            &message.gmail_message_id,
        ))
        .expect("message serialization"),
    ))
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub winding_down: bool,
    pub comparisons: BTreeMap<String, BTreeSet<String>>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub settings: Settings,
    pub dismissed: BTreeSet<String>,
    pub known: BTreeMap<String, Row>,
    pub history: Vec<DecisionRecord>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    #[serde(default)]
    pub receipt: String,
    pub input_version: String,
    pub changed: Change,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Settings {
        settings: Settings,
    },
    Dismiss {
        rows: BTreeSet<String>,
        dismissed: bool,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    People,
    Service,
    Bulk,
    Unresolved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Waiting,
    Moved,
    Dismissed,
    Quiet,
    Bulk,
    Unresolved,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressObservation {
    pub account: String,
    pub field: String,
    pub address: String,
    pub raw_values: Vec<String>,
    pub message_date: Option<i64>,
    pub delivery_authority: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    #[serde(default)]
    pub message_identity: String,
    pub account: String,
    pub address: String,
    pub message_date: i64,
    pub evidence: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub sender: Option<String>,
    pub domain: Option<String>,
    pub group: Group,
    pub status: Status,
    pub messages: usize,
    pub recent: usize,
    pub first_date: Option<i64>,
    pub last_date: Option<i64>,
    pub invalid_dates: usize,
    pub future_dates: usize,
    pub wrote_to_sender: bool,
    pub observations: Vec<AddressObservation>,
    pub migration: Vec<Migration>,
    pub reasons: BTreeSet<String>,
    pub settings_link: Option<String>,
    pub service_associations: BTreeSet<String>,
    pub historical: bool,
}
#[derive(Clone, Serialize)]
pub struct SenderData {
    pub version: String,
    pub reviewed: BTreeMap<String, SenderEntry>,
}
#[derive(Clone, Serialize)]
pub struct SenderEntry {
    pub transactional: bool,
    pub settings_link: Option<String>,
}
impl Default for SenderData {
    fn default() -> Self {
        Self {
            version: "sender-data-v1/empty-reviewed-catalog".into(),
            reviewed: BTreeMap::new(),
        }
    }
}
#[derive(Serialize)]
pub struct SourceVersion {
    pub account: String,
    pub revision: u64,
    pub census_version: String,
    pub complete: bool,
    pub owned_addresses: BTreeSet<String>,
    pub spam_count: Option<usize>,
    pub coverage_since: Option<i64>,
    pub coverage_through: Option<i64>,
}
pub struct Source<'a> {
    pub state: &'a AccountState,
    pub provider: Provider,
    pub keys: &'a CensusKeys,
}
#[derive(Serialize)]
pub struct Report {
    pub version: String,
    pub semantics: &'static str,
    pub census_analysis: &'static str,
    pub sender_data_version: String,
    pub ledger_version: String,
    pub account: String,
    pub as_of: i64,
    pub boundary: i64,
    pub settings: Settings,
    pub sources: Vec<SourceVersion>,
    pub rows: Vec<Row>,
    pub counts: BTreeMap<String, usize>,
    pub blockers: BTreeSet<String>,
    pub verdict: &'static str,
    pub effects: &'static str,
}
fn error<T: std::fmt::Display>(value: T) -> String {
    value.to_string()
}
fn token<T: Serialize>(keys: &CensusKeys, purpose: &[u8], value: &T) -> String {
    keys.binding_for(
        purpose,
        &serde_json::to_vec(value).expect("report serialization"),
    )
}
fn date(value: Option<i64>) -> String {
    value
        .and_then(|v| DateTime::<Utc>::from_timestamp(v, 0))
        .map(|v| v.to_rfc3339())
        .unwrap_or_else(|| "unknown".into())
}
fn safe_link(link: &str) -> bool {
    link.parse::<axum::http::Uri>().is_ok_and(|u| {
        u.scheme_str() == Some("https")
            && u.authority()
                .is_some_and(|a| !a.as_str().contains('@') && !a.host().is_empty())
    }) && !link.contains(['?', '#'])
}
fn evidence_issues(
    source: &Source<'_>,
    complete: bool,
    boundary: i64,
    as_of: i64,
) -> BTreeSet<String> {
    let e = &source.state.census.winddown_evidence;
    let mut issues = BTreeSet::new();
    if !complete {
        issues.insert("incomplete_acquisition".into());
    }
    if e.spam_count.is_none() {
        issues.insert("spam_count_unknown".into());
    }
    if !e.complete_since.is_some_and(|v| v <= boundary) {
        issues.insert("historical_coverage_unknown".into());
    }
    if !e.complete_through.is_some_and(|v| v >= as_of) {
        issues.insert("source_stale_or_coverage_unknown".into());
    }
    if source.state.census.confirmed_own_addresses.is_empty() {
        issues.insert("ownership_unconfirmed".into());
    }
    issues
}
fn delivery_fact(
    census: &crate::census::Census,
    message: &MessageRecord,
    gmail: bool,
) -> Option<Delivery> {
    let identity = message_identity(message);
    let first = census.winddown_evidence.deliveries.get(&identity)?;
    if gmail {
        if let Some(id) = &message.gmail_message_id {
            for other in census
                .folders
                .values()
                .flat_map(|f| f.messages.values())
                .filter(|m| m.gmail_message_id.as_ref() == Some(id))
            {
                if message_identity(other) != identity
                    || census
                        .winddown_evidence
                        .deliveries
                        .get(&message_identity(other))
                        != Some(first)
                {
                    return Some(Delivery::Ambiguous);
                }
            }
        }
    }
    Some(first.clone())
}
fn valid_addresses(
    source: &Source<'_>,
    comparison: &Source<'_>,
    addresses: &BTreeSet<String>,
) -> bool {
    comparison.state.census.account != source.state.census.account
        && !addresses.is_empty()
        && addresses.is_subset(&comparison.state.census.confirmed_own_addresses)
        && addresses.is_disjoint(&source.state.census.confirmed_own_addresses)
}
pub fn derive(
    source: &Source<'_>,
    comparisons: &[Source<'_>],
    ledger: &Snapshot,
    data: &SenderData,
    as_of: i64,
) -> Result<Report, String> {
    let now = DateTime::<Utc>::from_timestamp(as_of, 0).ok_or("Invalid as-of")?;
    let boundary = now
        .checked_sub_months(Months::new(13))
        .ok_or("Invalid calendar boundary")?
        .timestamp();
    let saved = source.state;
    let local = saved
        .census
        .report(source.keys, as_of, source.provider == Provider::Gmail)
        .map_err(error)?;
    let mut sources = Vec::new();
    let mut blockers = BTreeSet::new();
    if !saved.winddown.settings.winding_down {
        blockers.insert("account_not_designated".into());
    }
    if saved.winddown.settings.comparisons.is_empty() {
        blockers.insert("comparison_accounts_absent".into());
    }
    for s in std::iter::once(source).chain(comparisons.iter()) {
        let r = s
            .state
            .census
            .report(s.keys, as_of, s.provider == Provider::Gmail)
            .map_err(error)?;
        blockers.extend(evidence_issues(s, r.complete, boundary, as_of));
        if r.received.invalid_dates + r.received.future_dates > 0 {
            blockers.insert("invalid_future_or_missing_message_dates".into());
        }
        sources.push(SourceVersion {
            account: s.state.census.account.as_str().into(),
            revision: s.state.revision,
            census_version: r.version,
            complete: r.complete,
            owned_addresses: s.state.census.confirmed_own_addresses.clone(),
            spam_count: s.state.census.winddown_evidence.spam_count,
            coverage_since: s.state.census.winddown_evidence.complete_since,
            coverage_through: s.state.census.winddown_evidence.complete_through,
        });
    }
    for (id, addresses) in &saved.winddown.settings.comparisons {
        match comparisons
            .iter()
            .find(|s| s.state.census.account.as_str() == id)
        {
            None => {
                blockers.insert("selected_comparison_unavailable".into());
            }
            Some(s) if !valid_addresses(source, s, addresses) => {
                blockers.insert("comparison_ownership_changed".into());
            }
            _ => {}
        }
    }
    if comparisons.iter().any(|s| {
        !saved
            .winddown
            .settings
            .comparisons
            .contains_key(s.state.census.account.as_str())
    }) {
        return Err("Unexpected comparison source".into());
    }
    let messages = saved
        .census
        .canonical_messages(source.provider == Provider::Gmail);
    let source_message_ids: BTreeSet<_> = messages
        .values()
        .filter_map(|(m, _)| m.headers.message_id_hash.clone())
        .collect();
    let mut rows = BTreeMap::new();
    for sender in local.senders {
        let id = token(source.keys, b"winddown-row/v1", &sender.sender);
        let group = if sender.relay_claims.is_empty()
            && sender.classes.len() == 1
            && sender.raw_from.len() == 1
        {
            match sender.class {
                SenderClass::Person => Group::People,
                SenderClass::Bulk => Group::Bulk,
                SenderClass::Platform | SenderClass::Unknown
                    if sender
                        .domain
                        .as_ref()
                        .and_then(|d| data.reviewed.get(d))
                        .is_some_and(|d| d.transactional) =>
                {
                    Group::Service
                }
                _ => Group::Unresolved,
            }
        } else {
            Group::Unresolved
        };
        let Counts {
            messages: count,
            first_date,
            last_date,
            last_13_months: recent,
            invalid_dates,
            future_dates,
            ..
        } = sender.counts;
        let mut row = Row {
            id: id.clone(),
            sender: sender.sender.clone(),
            domain: sender.domain.clone(),
            group,
            status: Status::Waiting,
            messages: count,
            recent,
            first_date,
            last_date,
            invalid_dates,
            future_dates,
            wrote_to_sender: sender.wrote_to_sender,
            observations: vec![],
            migration: vec![],
            reasons: BTreeSet::new(),
            settings_link: sender
                .domain
                .as_ref()
                .and_then(|d| data.reviewed.get(d))
                .and_then(|s| s.settings_link.clone())
                .filter(|s| safe_link(s)),
            service_associations: BTreeSet::new(),
            historical: false,
        };
        if group == Group::Unresolved {
            row.reasons
                .insert("sender_identity_or_class_unresolved".into());
        }
        if invalid_dates + future_dates > 0 {
            row.reasons
                .insert("invalid_future_or_missing_message_dates".into());
        }
        for (m, _) in messages.values().filter(|(m, _)| {
            m.headers.direction == Direction::Received
                && m.headers.effective_sender.as_ref().map(|s| &s.address) == sender.sender.as_ref()
        }) {
            for (field, addresses) in [
                ("delivered-to", &m.headers.delivered_to),
                ("x-original-to", &m.headers.original_to),
                ("to", &m.headers.to),
                ("cc", &m.headers.cc),
            ] {
                for address in addresses {
                    row.observations.push(AddressObservation {
                        account: saved.census.account.as_str().into(),
                        field: field.into(),
                        address: address.address.clone(),
                        raw_values: m.headers.evidence.get(field).cloned().unwrap_or_default(),
                        message_date: header_date(&m.headers).filter(|d| *d <= as_of),
                        delivery_authority: "unverified_header_observation".into(),
                    });
                }
            }
        }
        for entry in &ledger.document.entries {
            if entry.association.as_ref().is_some_and(|a| {
                row.domain
                    .as_ref()
                    .is_some_and(|d| a.sender_domains.contains(d))
            }) {
                row.service_associations.insert(entry.service.clone());
            }
        }
        if row.sender.is_some() && group != Group::Unresolved && group != Group::Bulk {
            for s in comparisons {
                let selected =
                    &saved.winddown.settings.comparisons[s.state.census.account.as_str()];
                if !valid_addresses(source, s, selected) {
                    row.reasons.insert("comparison_ownership_changed".into());
                    continue;
                }
                let mut uncertain = false;
                for (m, _) in s
                    .state
                    .census
                    .canonical_messages(s.provider == Provider::Gmail)
                    .values()
                    .filter(|(m, _)| {
                        m.headers.direction == Direction::Received
                            && m.headers.effective_sender.as_ref().map(|a| &a.address)
                                == row.sender.as_ref()
                    })
                {
                    let Some(d) = header_date(&m.headers).filter(|d| *d >= boundary && *d <= as_of)
                    else {
                        uncertain = true;
                        continue;
                    };
                    let fact = delivery_fact(&s.state.census, m, s.provider == Provider::Gmail);
                    match fact {
                        Some(Delivery::Direct { address })
                            if selected.contains(&address)
                                && m.headers.from.len() == 1
                                && m.headers.from[0].address
                                    == *row.sender.as_ref().expect("sender")
                                && !m
                                    .headers
                                    .message_id_hash
                                    .as_ref()
                                    .is_some_and(|id| source_message_ids.contains(id)) =>
                        {
                            row.migration.push(Migration {
                                message_identity: message_identity(m),
                                account: s.state.census.account.as_str().into(),
                                address: address.clone(),
                                message_date: d,
                                evidence: "installed_direct_delivery_fact; distinct_message".into(),
                            });
                        }
                        _ => uncertain = true,
                    }
                }
                if uncertain {
                    row.reasons
                        .insert("comparison_forwarded_ambiguous_duplicate_or_untrusted".into());
                }
            }
        }
        row.status = if saved.winddown.dismissed.contains(&id) {
            Status::Dismissed
        } else if group == Group::Bulk {
            Status::Bulk
        } else if group == Group::Unresolved || invalid_dates + future_dates > 0 {
            Status::Unresolved
        } else if !row.migration.is_empty() {
            Status::Moved
        } else if recent == 0
            && last_date.is_some_and(|d| d < boundary)
            && evidence_issues(source, local.complete, boundary, as_of).is_empty()
        {
            Status::Quiet
        } else {
            if recent == 0 {
                row.reasons.insert("quietness_not_proven".into());
            }
            Status::Waiting
        };
        rows.insert(id, row);
    }
    for (id, known) in &saved.winddown.known {
        if !rows.contains_key(id) {
            let mut row = known.clone();
            row.historical = true;
            row.migration.clear();
            row.status = if saved.winddown.dismissed.contains(id) {
                Status::Dismissed
            } else {
                Status::Unresolved
            };
            row.reasons
                .insert("known_row_absent_from_current_census".into());
            rows.insert(id.clone(), row);
        }
    }
    let mut counts = BTreeMap::new();
    for row in rows.values() {
        let status = serde_json::to_value(row.status)
            .expect("status")
            .as_str()
            .expect("string")
            .to_owned();
        *counts.entry(status).or_default() += 1;
        if row.group == Group::Unresolved
            || row.invalid_dates + row.future_dates > 0
            || row.historical
        {
            blockers.insert("unresolved_rows_or_dates".into());
        }
        if row.status == Status::Waiting {
            blockers.insert("senders_waiting".into());
        }
        if row.group == Group::Bulk {
            blockers.insert("bulk_requires_separate_unsubscribe_review".into());
        }
    }
    let version = token(
        source.keys,
        b"winddown-input-set/v1",
        &(
            SEMANTICS,
            &sources,
            &saved.winddown,
            &saved.census,
            comparisons.iter().map(|s| s.state).collect::<Vec<_>>(),
            ledger,
            data,
            as_of,
            &rows,
        ),
    );
    Ok(Report {
        version,
        semantics: SEMANTICS,
        census_analysis: crate::census::report::ANALYSIS_VERSION,
        sender_data_version: data.version.clone(),
        ledger_version: ledger.content_version.clone(),
        account: saved.census.account.as_str().into(),
        as_of,
        boundary,
        settings: saved.winddown.settings.clone(),
        sources,
        rows: rows.into_values().collect(),
        counts,
        verdict: if blockers.is_empty() {
            "evidence_supports_owner_retirement_review"
        } else {
            "insufficient_evidence_or_waiting"
        },
        blockers,
        effects: EFFECTS,
    })
}
#[derive(Serialize)]
pub struct DecisionReview {
    pub resulting_report: Report,
    pub receipt: String,
    pub report: Report,
    pub change: Change,
    pub consequences: &'static str,
}
pub fn review(
    source: &Source<'_>,
    comparisons: &[Source<'_>],
    next_comparisons: &[Source<'_>],
    ledger: &Snapshot,
    data: &SenderData,
    as_of: i64,
    version: &str,
    change: &Change,
) -> Result<DecisionReview, String> {
    let report = derive(source, comparisons, ledger, data, as_of)?;
    if report.version != version {
        return Err("Report changed. Reload and review again".into());
    }
    match change {
        Change::Settings { settings } => {
            if settings.comparisons.len() > 16 || settings == &source.state.winddown.settings {
                return Err("Settings unchanged or too many comparison sources".into());
            }
            for (id, addresses) in &settings.comparisons {
                let comparison = next_comparisons
                    .iter()
                    .find(|s| s.state.census.account.as_str() == id)
                    .ok_or("Selected comparison unavailable")?;
                if !valid_addresses(source, comparison, addresses) {
                    return Err("Select distinct explicitly reviewed newer owned addresses".into());
                }
            }
        }
        Change::Dismiss { rows, dismissed } => {
            if rows.is_empty()
                || rows.len() > 4000
                || rows.iter().any(|id| {
                    !report.rows.iter().any(|r| {
                        &r.id == id
                            && (!*dismissed || matches!(r.group, Group::People | Group::Service))
                    }) || source.state.winddown.dismissed.contains(id) == *dismissed
                })
            {
                return Err("Select exact changed people or service rows".into());
            }
        }
    }
    if source.state.winddown.history.len() >= 4000 {
        return Err("Local decision history is full; preserve it for recovery".into());
    }
    let mut next = source.state.clone();
    match change {
        Change::Settings { settings } => next.winddown.settings = settings.clone(),
        Change::Dismiss { rows, dismissed } => {
            for id in rows {
                if *dismissed {
                    next.winddown.dismissed.insert(id.clone());
                } else {
                    next.winddown.dismissed.remove(id);
                }
            }
        }
    }
    let resulting_report = derive(
        &Source {
            state: &next,
            provider: source.provider,
            keys: source.keys,
        },
        next_comparisons,
        ledger,
        data,
        as_of,
    )?;
    let receipt = token(
        source.keys,
        b"winddown-owner-decision/v1",
        &(&report, &resulting_report, change, EFFECTS),
    );
    Ok(DecisionReview {
        resulting_report,
        receipt,
        report,
        change: change.clone(),
        consequences: EFFECTS,
    })
}
pub fn apply(state: &mut AccountState, review: &DecisionReview) {
    for row in &review.report.rows {
        state.winddown.known.insert(row.id.clone(), row.clone());
    }
    match &review.change {
        Change::Settings { settings } => state.winddown.settings = settings.clone(),
        Change::Dismiss { rows, dismissed } => {
            for row in rows {
                if *dismissed {
                    state.winddown.dismissed.insert(row.clone());
                } else {
                    state.winddown.dismissed.remove(row);
                }
            }
        }
    }
    state.winddown.history.push(DecisionRecord {
        receipt: review.receipt.clone(),
        input_version: review.report.version.clone(),
        changed: review.change.clone(),
    });
}
