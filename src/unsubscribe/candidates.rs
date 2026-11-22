use super::*;
use crate::census::{
    Direction, HeaderRecord, MessageRecord, ScanState,
    report::{Counts, SenderClass},
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
enum ListIdentity {
    Absent,
    Exact(String),
    Ambiguous(String),
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct SubscriptionKey {
    sender: String,
    recipient: String,
    recipient_uncertainty: Option<String>,
    list: ListIdentity,
}
#[derive(Serialize)]
pub struct Candidate {
    pub id: String,
    pub account: String,
    pub sender: String,
    pub domain: Option<String>,
    pub recipient: String,
    pub recipient_evidence: String,
    pub list_id: Option<String>,
    pub list_evidence: String,
    pub class: SenderClass,
    pub count_12_months: usize,
    pub read_share: Option<f64>,
    pub newest_dated_seen_message: Option<i64>,
    pub unread_volume: usize,
    pub rank: usize,
    pub recommended: bool,
    pub flags_known: bool,
    pub authentication: String,
    pub original_header_survives: bool,
    pub relay_alias_initially_selected: bool,
    pub methods: Vec<Method>,
    pub role: String,
    pub source: String,
    pub header_digest: String,
    pub source_ids: Vec<String>,
    pub straggler: String,
}
#[derive(Serialize)]
pub struct Report {
    pub version: String,
    pub complete: bool,
    pub revision: u64,
    pub as_of: i64,
    pub last_completed_at: Option<i64>,
    pub census_age_seconds: Option<i64>,
    pub rows: Vec<Candidate>,
    pub warnings: Vec<String>,
}
/// Where a review's authority and effects come from: the synthetic fixture, or
/// the wired real adapters behind the write policy.
pub(crate) trait Adapters {
    /// Whether recipient evidence is trusted. Only synthetic fixtures establish it so far.
    fn trusted_recipients(&self) -> bool;
    fn authentication(&self, h: &HeaderRecord) -> String;
    fn one_click_authorized(&self, authentication: &str) -> bool;
    fn original_survives(&self, h: &HeaderRecord) -> bool;
    /// The host and payload digest a one-click POST to `raw` is bound to.
    fn bind_post(&self, raw: &str) -> Result<(String, String), String>;
    /// Whether effects go to the wired real adapters.
    fn wired(&self) -> bool;
}
impl Adapters for InstalledSynthetic {
    fn trusted_recipients(&self) -> bool {
        true
    }
    fn authentication(&self, h: &HeaderRecord) -> String {
        InstalledSynthetic::authentication(self, h)
    }
    fn one_click_authorized(&self, authentication: &str) -> bool {
        authentication == "synthetic_provider_pass_both_same_signature"
    }
    fn original_survives(&self, h: &HeaderRecord) -> bool {
        InstalledSynthetic::original_survives(self, h)
    }
    fn bind_post(&self, raw: &str) -> Result<(String, String), String> {
        let post = self.post(raw)?;
        Ok((post.host().into(), post.request_digest()))
    }
    fn wired(&self) -> bool {
        false
    }
}
/// The wired adapters. One-click authority comes from the census's recorded
/// provider Authentication-Results under `authserv`.
pub(crate) struct Wired<'a> {
    pub trusted_recipients: bool,
    pub authserv: Option<&'a str>,
}
impl Adapters for Wired<'_> {
    fn trusted_recipients(&self) -> bool {
        self.trusted_recipients
    }
    fn authentication(&self, h: &HeaderRecord) -> String {
        super::authority::one_click(h, self.authserv).into()
    }
    fn one_click_authorized(&self, authentication: &str) -> bool {
        authentication == super::authority::PASS
    }
    fn original_survives(&self, h: &HeaderRecord) -> bool {
        !h.evidence
            .keys()
            .any(|k| k.starts_with("x-simplelogin-") || k.starts_with("x-anonaddy-"))
    }
    fn bind_post(&self, raw: &str) -> Result<(String, String), String> {
        Ok((safety::Endpoint::parse(raw)?.host, digest(raw.as_bytes())))
    }
    fn wired(&self) -> bool {
        true
    }
}

pub(crate) fn uris(h: &HeaderRecord) -> Vec<String> {
    let Some(values) = h.evidence.get("list-unsubscribe").filter(|v| v.len() == 1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in values[0].split(',') {
        let raw = part.trim();
        let Some(uri) = raw.strip_prefix('<').and_then(|s| s.strip_suffix('>')) else {
            return Vec::new();
        };
        if uri.contains(['<', '>']) {
            return Vec::new();
        }
        out.push(uri.into());
    }
    out
}
pub(crate) fn source<'a>(state: &'a AccountState, id: &str) -> Option<&'a MessageRecord> {
    state.census.folders.iter().find_map(|(name, f)| {
        f.messages
            .values()
            .find(|m| source_id(name, f.uidvalidity, m.uid) == id)
    })
}
fn source_id(folder: &str, validity: u32, uid: u32) -> String {
    digest(format!("{folder}/{validity}/{uid}").as_bytes())
}
fn arrival_identity(provider: Provider, source: &str, m: &MessageRecord) -> String {
    m.gmail_message_id
        .as_ref()
        .filter(|_| provider == Provider::Gmail)
        .map(|v| digest(format!("gmail/{v}").as_bytes()))
        .or_else(|| m.headers.message_id_hash.clone())
        .unwrap_or_else(|| source.into())
}
fn date(m: &MessageRecord) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(m.headers.date.as_deref()?)
        .ok()
        .map(|d| d.timestamp())
}
fn seen(m: &MessageRecord) -> bool {
    m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"))
}
fn arrival(m: &MessageRecord) -> Option<i64> {
    chrono::DateTime::parse_from_str(&m.internal_date, "%d-%b-%Y %H:%M:%S %z")
        .ok()
        .map(|d| d.timestamp())
}
pub(crate) fn report<A: Adapters + ?Sized>(
    state: &AccountState,
    provider: Provider,
    keys: &CensusKeys,
    as_of: i64,
    installed: Option<&A>,
) -> Result<Report, String> {
    let census = &state.census;
    let base = census
        .report(keys, as_of, provider == Provider::Gmail)
        .map_err(|_| "Cannot derive unsubscribe census")?;
    let config = state
        .workspace
        .as_ref()
        .and_then(|w| {
            crate::rules::config::documents(
                &w.input.documents.preset_yaml,
                &w.input.documents.override_yaml,
            )
            .ok()
        })
        .map(|(_, c)| c);
    let mut canonical: BTreeMap<String, (String, MessageRecord)> = BTreeMap::new();
    for (folder, f) in &census.folders {
        for m in f
            .messages
            .values()
            .filter(|m| m.headers.direction == Direction::Received)
        {
            let source = source_id(folder, f.uidvalidity, m.uid);
            let id = if provider == Provider::Gmail {
                m.gmail_message_id
                    .as_ref()
                    .map(|v| format!("gmail/{v}"))
                    .unwrap_or(source.clone())
            } else {
                source.clone()
            };
            if let Some((_, prior)) = canonical.get_mut(&id) {
                prior.flags.extend(m.flags.clone());
                prior.flags_observed &= m.flags_observed && prior.headers == m.headers;
            } else {
                canonical.insert(id, (source, m.clone()));
            }
        }
    }
    let mut groups: BTreeMap<SubscriptionKey, Vec<(String, &MessageRecord)>> = BTreeMap::new();
    for (source, m) in canonical.values() {
        let h = &m.headers;
        let Some(sender) = h.effective_sender.as_ref() else {
            continue;
        };
        let Some(class) = base
            .senders
            .iter()
            .find(|s| s.sender.as_deref() == Some(&sender.address))
        else {
            continue;
        };
        if class.class == SenderClass::Person {
            continue;
        }
        let recipients = if !h.delivered_to.is_empty() {
            &h.delivered_to
        } else if !h.original_to.is_empty() {
            &h.original_to
        } else {
            &h.to
        };
        let recipient = if recipients.len() == 1 {
            recipients[0].address.clone()
        } else {
            String::new()
        };
        let list = match h.evidence.get("list-id") {
            None => ListIdentity::Absent,
            Some(v)
                if v.len() == 1
                    && !v[0].trim().is_empty()
                    && !v[0].chars().any(char::is_control) =>
            {
                ListIdentity::Exact(v[0].trim().to_ascii_lowercase())
            }
            Some(v) => ListIdentity::Ambiguous(digest(
                &serde_json::to_vec(v).map_err(|_| "List evidence encoding")?,
            )),
        };
        let recipient_uncertainty = recipient.is_empty().then(|| {
            digest(
                &serde_json::to_vec(&(&h.delivered_to, &h.original_to, &h.to))
                    .expect("Recipient evidence encoding"),
            )
        });
        groups
            .entry(SubscriptionKey {
                sender: sender.address.clone(),
                recipient,
                recipient_uncertainty,
                list,
            })
            .or_default()
            .push((source.clone(), m));
    }
    let now = chrono::DateTime::from_timestamp(as_of, 0).ok_or("Invalid census date")?;
    let start = now
        .checked_sub_months(chrono::Months::new(12))
        .ok_or("Invalid census window")?
        .timestamp();
    let complete = base.complete && census.acquisition_state == ScanState::Complete;
    let mut rows = Vec::new();
    for (key, mut messages) in groups {
        let sender = key.sender.clone();
        let recipient = key.recipient.clone();
        let list_id = match &key.list {
            ListIdentity::Exact(v) => Some(v.clone()),
            _ => None,
        };
        let list_known = !matches!(key.list, ListIdentity::Ambiguous(_));
        let list_evidence = match key.list {
            ListIdentity::Absent => "absent_exact_from_fallback",
            ListIdentity::Exact(_) => "exact_full_header_ascii_casemap",
            ListIdentity::Ambiguous(_) => "ambiguous_no_from_fallback",
        }
        .to_owned();
        messages.sort_by_key(|(id, m)| (date(m).filter(|d| *d <= as_of), id.clone()));
        let (source, newest) = messages.last().ok_or("Empty subscription")?;
        let h = &newest.headers;
        let classification = base
            .senders
            .iter()
            .find(|s| s.sender.as_deref() == Some(&sender))
            .ok_or("Sender missing")?;
        let mut counts = Counts::default();
        for (_, m) in &messages {
            counts.add(m, as_of);
        }
        let flags_known = messages.iter().all(|(_, m)| m.flags_observed);
        let read_12 = messages
            .iter()
            .filter(|(_, m)| seen(m) && date(m).is_some_and(|d| d >= start && d <= as_of))
            .count();
        let id = keys.identity_binding_for(
            b"mailroom/subscription/v1",
            &serde_json::to_vec(&(census.account.as_str(), &key))
                .map_err(|_| "Subscription encoding")?,
        );
        let header_digest = digest(&serde_json::to_vec(h).map_err(|_| "Header encoding")?);
        let authentication = installed
            .map(|a| a.authentication(h))
            .unwrap_or_else(|| "unknown_provider_provenance".into());
        let originals = !h
            .evidence
            .keys()
            .any(|k| k.starts_with("x-simplelogin-") || k.starts_with("x-anonaddy-"))
            || installed.is_some_and(|a| a.original_survives(h));
        let recipient_known =
            installed.is_some_and(|a| a.trusted_recipients()) && h.delivered_to.len() == 1;
        let mut methods = vec![Method::Keep];
        let urls = uris(h);
        let valid_https = urls
            .iter()
            .filter(|s| s.starts_with("https://"))
            .collect::<Vec<_>>();
        if valid_https.len() == 1 && safety::Endpoint::parse(valid_https[0]).is_ok() {
            methods.push(Method::BrowserPreparation);
            if recipient_known
                && list_known
                && installed.is_some_and(|a| a.one_click_authorized(&authentication))
                && originals
                && h.evidence
                    .get("list-unsubscribe-post")
                    .is_some_and(|v| v == &["List-Unsubscribe=One-Click"])
            {
                methods.push(Method::OneClick);
            }
        }
        if urls.iter().filter(|s| s.starts_with("mailto:")).count() == 1
            && urls
                .iter()
                .any(|s| safety::Mail::parse(s, &recipient).is_ok())
        {
            methods.push(Method::MailAppPreparation);
            if recipient_known && originals && list_known {
                methods.push(Method::LocalMail);
            }
        }
        let role = config
            .as_ref()
            .map(|c| {
                c.roles
                    .addresses
                    .get(&recipient)
                    .or_else(|| {
                        recipient
                            .split_once('@')
                            .and_then(|(_, d)| c.roles.domains.get(d))
                    })
                    .copied()
                    .unwrap_or(crate::rules::config::Role::Unknown)
            })
            .unwrap_or(crate::rules::config::Role::Unknown);
        let role = match role {
            crate::rules::config::Role::Personal => "personal",
            crate::rules::config::Role::Business => "business",
            _ => "unknown_or_mixed",
        }
        .to_owned();
        if recipient_known
            && role != "unknown_or_mixed"
            && !recipient.is_empty()
            && h.from.len() == 1
            && list_known
        {
            methods.push(Method::Mute);
        }
        if config
            .as_ref()
            .is_some_and(|c| c.ledger.iter().any(|e| e.address == recipient))
        {
            methods.push(Method::RetirementPreparation);
        }
        methods.sort_by_key(|m| match m {
            Method::OneClick => 0,
            Method::BrowserPreparation => 1,
            Method::MailAppPreparation => 2,
            Method::LocalMail => 3,
            Method::RetirementPreparation => 4,
            Method::Mute => 5,
            Method::Keep => 6,
            Method::RemoveMute => 7,
        });
        let recommended = complete
            && flags_known
            && classification.class == SenderClass::Bulk
            && counts.last_12_months >= 5
            && counts.read_last_6_months == 0
            && counts.invalid_dates == 0
            && counts.future_dates == 0
            && recipient_known
            && originals
            && list_known;
        let source_ids: Vec<_> = messages
            .iter()
            .map(|(id, m)| arrival_identity(provider, id, m))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut straggler = "no_recorded_request";
        if let Some(entry) = state.unsubscribe.history.iter().rev().find(|e| {
            e.subscription == id && matches!(e.method, Method::OneClick | Method::LocalMail)
        }) {
            straggler = if !matches!(entry.status, Status::HttpObserved | Status::MailHandoff) {
                "unknown_request_observation"
            } else if as_of < entry.time + 14 * 86400 {
                "wait_14_days"
            } else if messages.iter().any(|(id, m)| {
                !entry
                    .source_ids
                    .contains(&arrival_identity(provider, id, m))
                    && arrival(m).is_some_and(|d| d > entry.time && d <= as_of)
            }) {
                "propose_mute_new_arrival"
            } else if messages.iter().any(|(id, m)| {
                !entry
                    .source_ids
                    .contains(&arrival_identity(provider, id, m))
                    && arrival(m).is_none_or(|d| d > as_of)
            }) {
                "unknown_arrival_evidence"
            } else {
                "no_new_arrivals_old_mail_is_not_a_straggler"
            };
        }
        rows.push(Candidate {
            id,
            account: census.account.as_str().into(),
            sender,
            domain: classification.domain.clone(),
            recipient,
            recipient_evidence: if recipient_known {
                if installed.is_some_and(|a| a.wired()) {
                    "provider_delivered_to"
                } else {
                    "synthetic_delivered_to"
                }
            } else {
                "unknown_or_header_claim"
            }
            .into(),
            list_id,
            list_evidence,
            class: classification.class,
            count_12_months: counts.last_12_months,
            read_share: (flags_known && counts.last_12_months > 0)
                .then_some(read_12 as f64 / counts.last_12_months.max(1) as f64),
            newest_dated_seen_message: counts.latest_read_message_date,
            unread_volume: if flags_known {
                counts.last_12_months - read_12
            } else {
                0
            },
            rank: 0,
            recommended,
            flags_known,
            authentication,
            original_header_survives: originals,
            relay_alias_initially_selected: false,
            methods,
            role,
            source: source.clone(),
            header_digest,
            source_ids,
            straggler: straggler.into(),
        });
    }
    rows.sort_by(|a, b| b.unread_volume.cmp(&a.unread_volume).then(a.id.cmp(&b.id)));
    for (i, row) in rows.iter_mut().enumerate() {
        row.rank = i + 1;
    }
    Ok(Report { version: base.version, complete, revision: state.revision, as_of, last_completed_at: census.last_completed_at, census_age_seconds: census.last_completed_at.filter(|t| *t <= as_of).map(|t| as_of-t), rows, warnings: vec!["Date of a currently Seen message is not the time the owner read it. Missing flag evidence prevents recommendations.".into(), "Real DKIM/provider/recipient provenance remains unknown. Header syntax grants no send authority. Partial or old census needs explicit refresh.".into(), "Approve each account, receiving address and mailing list separately. Sender grouping grants no authority. No automatic alias disabling.".into()] })
}
