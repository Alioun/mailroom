use crate::{
    census::{AccountState, CensusKeys, Direction, ScanState},
    rules::{
        config::{self, Category, LedgerEntry, LedgerStatus},
        workspace,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) mod file;
pub use file::InstalledLedger;
#[cfg(test)]
mod tests;

pub const SEMANTICS: &str = "mailroom/private-address-ledger/v1/psl-2.1.245";
const PURPOSE: &[u8] = b"mailroom/exact-ledger-batch/v1";

// Private owner data deliberately has no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub address: String,
    pub service: String,
    pub folder: String,
    pub first_seen: String,
    pub status: LedgerStatus,
    pub note: String,
    #[serde(default = "default_category")]
    pub category: Category,
    #[serde(default)]
    pub invoice: bool,
    #[serde(default)]
    pub association: Option<Association>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
fn default_category() -> Category {
    Category::Orders
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Association {
    pub sender_domains: BTreeSet<String>,
    pub approved_evidence: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub enabled: bool,
    pub entries: Vec<Entry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            entries: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub source_version: String,
    pub document: Option<Document>,
    pub managed: Vec<LedgerEntry>,
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub content_version: String,
    pub document: Document,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub content_version: String,
    pub revision: u64,
    pub enabled: bool,
    pub entries: Vec<Entry>,
}
#[derive(Serialize)]
pub struct Observation {
    pub account: String,
    pub folder: String,
    pub uidvalidity: u32,
    pub uid: u32,
    pub recipient_header: String,
    pub delivery_authority: &'static str,
    pub sender_domain: Option<String>,
    pub message_date: Option<String>,
}
#[derive(Serialize)]
pub struct Proposal {
    pub address: String,
    pub first_seen_proposal: String,
    pub first_seen_meaning: &'static str,
    pub ownership: &'static str,
    pub role: &'static str,
    pub service: Option<String>,
    pub evidence: Vec<Observation>,
}
#[derive(Serialize)]
pub struct Leak {
    pub address: String,
    pub service: String,
    pub approved: Association,
    pub observation: Observation,
    pub signal: &'static str,
}
#[derive(Serialize)]
pub struct View {
    pub snapshot: Snapshot,
    pub revision: u64,
    pub census_state: ScanState,
    pub proposals: Vec<Proposal>,
    pub leaks: Vec<Leak>,
    pub unknown_sender_observations: usize,
    pub projection_changed: bool,
    pub semantics: &'static str,
    pub limitation: &'static str,
}
#[derive(Serialize)]
pub struct Review {
    pub receipt: String,
    pub batch: Batch,
    pub resulting_document: Document,
    pub resulting_overrides: String,
    pub proof: workspace::ProofPreview,
    pub effects: &'static str,
}
const EFFECTS: &str = "Private local bookkeeping and derived filing rules only. This authoritative file and its policy switch are shared across installed accounts; each account reconciles before proof and actions. Ownership, roles and invoice eligibility are separate approvals. Retirement neither disables an alias nor unsubscribes. Existing accounting copies remain independent. Fresh proof is required. Provider recipient provenance and production routing remain unverified.";

fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_vec(a).ok() == serde_json::to_vec(b).ok()
}
pub fn validate(document: &Document) -> Result<(), String> {
    if document.version != 1 || document.entries.len() > 4000 {
        return Err("Unsupported ledger version or size".into());
    }
    let mut addresses = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for e in &document.entries {
        // Census parsing already folds local parts. Preserve owner spelling and reject ambiguous collisions.
        if !config::mailbox(&e.address)
            || !addresses.insert(&e.address)
            || !folded.insert(e.address.to_ascii_lowercase())
        {
            return Err("Invalid or duplicate ledger address".into());
        }
        config::segment(&e.folder)?;
        for value in [&e.service, &e.first_seen, &e.note] {
            if value.len() > 4096 || value.chars().any(|c| c.is_control() && c != '\n') {
                return Err("Invalid ledger text".into());
            }
        }
        if e.service.trim().is_empty() || e.first_seen.trim().is_empty() {
            return Err("Service and first_seen require explicit values".into());
        }
        if let Some(a) = &e.association {
            if a.sender_domains.is_empty()
                || a.approved_evidence.trim().is_empty()
                || a.approved_evidence.len() > 4096
                || a.sender_domains
                    .iter()
                    .any(|d| !config::domain(d) || psl::domain_str(d) != Some(d.as_str()))
            {
                return Err("Service association needs explicit organisational domains and approval evidence".into());
            }
        }
    }
    Ok(())
}
fn projected(d: &Document) -> Vec<LedgerEntry> {
    if !d.enabled {
        return Vec::new();
    }
    d.entries
        .iter()
        .map(|e| LedgerEntry {
            address: e.address.clone(),
            status: e.status,
            category: e.category,
            folder: Some(e.folder.clone()),
            invoice: e.invoice,
        })
        .collect()
}
pub fn reconcile(
    state: &mut AccountState,
    snapshot: &Snapshot,
    context: &workspace::WorkspaceContext,
) -> Result<bool, String> {
    validate(&snapshot.document)?;
    let old = &state.ledger;
    let expected = projected(&snapshot.document);
    let mut next = state
        .workspace
        .clone()
        .ok_or("Save a rule workspace before loading the ledger")?;
    let (_, mut overrides) = config::documents(
        &next.input.documents.preset_yaml,
        &next.input.documents.override_yaml,
    )?;
    for managed in &old.managed {
        let Some(i) = overrides
            .ledger
            .iter()
            .position(|e| e.address == managed.address)
        else {
            return Err(
                "Managed ledger projection was edited. Restore it before reconciliation".into(),
            );
        };
        if !same(&overrides.ledger[i], managed) {
            return Err("Managed ledger projection conflicts with independent edits".into());
        }
        overrides.ledger.remove(i);
    }
    for e in &expected {
        if overrides
            .ledger
            .iter()
            .any(|other| other.address.eq_ignore_ascii_case(&e.address))
        {
            return Err(
                "Independent ledger entry conflicts with the private file. Resolve explicitly"
                    .into(),
            );
        }
    }
    overrides.ledger.extend(expected.clone());
    let changed = old.source_version != snapshot.content_version || !same(&old.managed, &expected);
    if !changed {
        return Ok(false);
    }
    if !same(&old.managed, &expected) {
        next.input.documents.override_yaml =
            serde_yaml_ng::to_string(&overrides).map_err(|_| "Cannot encode projection")?;
    }
    workspace::validate(&next.input, state, context)?;
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or("Workspace revision exhausted")?;
    state.workspace = Some(next);
    state.ledger = Projection {
        source_version: snapshot.content_version.clone(),
        document: Some(snapshot.document.clone()),
        managed: expected,
    };
    Ok(true)
}
pub fn validate_workspace_edit(
    state: &AccountState,
    input: &workspace::WorkspaceInput,
) -> Result<(), String> {
    let (_, overrides) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml)?;
    for managed in &state.ledger.managed {
        let matches: Vec<_> = overrides
            .ledger
            .iter()
            .filter(|e| e.address.eq_ignore_ascii_case(&managed.address))
            .collect();
        if matches.len() != 1 || !same(matches[0], managed) {
            return Err("Edit managed entries in the authoritative ledger".into());
        }
    }
    Ok(())
}
fn sender_domain(address: &str) -> Option<String> {
    let (_, host) = address.rsplit_once('@')?;
    let host = idna::domain_to_ascii(host.trim_end_matches('.'))
        .ok()?
        .to_ascii_lowercase();
    if !config::domain(&host) {
        return None;
    }
    psl::domain_str(&host).map(str::to_owned)
}
pub fn view(state: &AccountState, snapshot: Snapshot, changed: bool) -> View {
    let now = chrono::DateTime::from_timestamp(now() as i64, 0)
        .expect("valid current time")
        .to_rfc3339();
    let mut proposals: BTreeMap<String, Proposal> = BTreeMap::new();
    let mut leaks = Vec::new();
    let mut unknown = 0;
    for (folder_name, folder) in &state.census.folders {
        for m in folder
            .messages
            .values()
            .filter(|m| m.headers.direction == Direction::Received)
        {
            let h = &m.headers;
            let domain = if h.from.len() == 1 {
                sender_domain(&h.from[0].address)
            } else {
                None
            };
            for (header, recipients) in [
                ("delivered-to", &h.delivered_to),
                ("x-original-to", &h.original_to),
                ("to", &h.to),
                ("cc", &h.cc),
            ] {
                for recipient in recipients.iter().filter(|r| config::mailbox(&r.address)) {
                    let observation = Observation {
                        account: state.census.account.as_str().into(),
                        folder: folder_name.clone(),
                        uidvalidity: folder.uidvalidity,
                        uid: m.uid,
                        recipient_header: header.into(),
                        delivery_authority: "unknown; discovery hint only",
                        sender_domain: domain.clone(),
                        message_date: h.date.clone(),
                    };
                    match snapshot.document.entries.iter().find(|e| e.address.eq_ignore_ascii_case(&recipient.address)) {
                        None => proposals.entry(recipient.address.clone()).or_insert_with(|| Proposal { address: recipient.address.clone(), first_seen_proposal: now.clone(), first_seen_meaning: "Time this review generated the proposal, not first receipt or Message Date. Earlier census observation time is unknown.", ownership: "unknown", role: "unknown", service: None, evidence: Vec::new() }).evidence.push(observation),
                        Some(entry) => match (&entry.association, &domain) {
                            (Some(approved), Some(observed)) if !approved.sender_domains.contains(observed) => leaks.push(Leak { address: entry.address.clone(), service: entry.service.clone(), approved: approved.clone(), observation, signal: "Different observed From organisational domain. Review signal only, not proof of abuse or authenticated sender identity." }),
                            (None, _) | (_, None) => unknown += 1,
                            _ => {},
                        }
                    }
                }
            }
        }
    }
    View {
        snapshot,
        revision: state.revision,
        census_state: state.census.acquisition_state,
        proposals: proposals.into_values().collect(),
        leaks,
        unknown_sender_observations: unknown,
        projection_changed: changed,
        semantics: SEMANTICS,
        limitation: EFFECTS,
    }
}
pub fn review(
    state: &AccountState,
    source: &Snapshot,
    context: &workspace::WorkspaceContext,
    keys: &CensusKeys,
    batch: &Batch,
) -> Result<Review, String> {
    if batch.content_version != source.content_version || batch.revision != state.revision {
        return Err("Ledger or account changed. Reload and review again".into());
    }
    let mut document = source.document.clone();
    // Upsert only. Deletion is not an alias lifecycle action; retire explicitly.
    for entry in &batch.entries {
        if let Some(i) = document
            .entries
            .iter()
            .position(|e| e.address.eq_ignore_ascii_case(&entry.address))
        {
            let old = &document.entries[i];
            if old.address != entry.address
                || old.first_seen != entry.first_seen
                || old.extra != entry.extra
                || old.invoice != entry.invoice
                || !same(&old.category, &entry.category)
            {
                return Err("Preserve existing address identity, first_seen, category, invoice approval and unknown fields".into());
            }
            if !same(&old.association, &entry.association) && old.association.is_some() {
                return Err(
                    "Original approved service association evidence must be preserved".into(),
                );
            }
            document.entries[i] = entry.clone();
        } else {
            if entry.invoice {
                return Err("Discovery does not approve invoice eligibility".into());
            }
            document.entries.push(entry.clone());
        }
    }
    let mut batch_addresses = BTreeSet::new();
    if batch
        .entries
        .iter()
        .any(|e| !batch_addresses.insert(e.address.to_ascii_lowercase()))
    {
        return Err("Duplicate address in batch".into());
    }
    document.enabled = batch.enabled;
    validate(&document)?;
    let bytes = encode(&document)?;
    let candidate = Snapshot {
        content_version: digest(&bytes),
        document: document.clone(),
    };
    let mut next = state.clone();
    reconcile(&mut next, &candidate, context)?;
    let proof = workspace::preview(&next, context, keys)?;
    let resulting_overrides = next
        .workspace
        .as_ref()
        .ok_or("Missing workspace")?
        .input
        .documents
        .override_yaml
        .clone();
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(
            SEMANTICS,
            state,
            &source.content_version,
            batch,
            &document,
            &resulting_overrides,
            &proof,
            EFFECTS,
        ))
        .map_err(|_| "Cannot bind ledger review")?,
    );
    let receipt = keys.binding_for(PURPOSE, &payload);
    Ok(Review {
        receipt,
        batch: batch.clone(),
        resulting_document: document,
        resulting_overrides,
        proof,
        effects: EFFECTS,
    })
}
pub(crate) fn encode(document: &Document) -> Result<Vec<u8>, String> {
    let bytes = serde_yaml_ng::to_string(document)
        .map_err(|_| "Cannot encode private ledger")?
        .into_bytes();
    if bytes.len() > 1024 * 1024 {
        return Err("Ledger exceeds 1 MiB".into());
    }
    Ok(bytes)
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("current time")
        .as_secs()
}
