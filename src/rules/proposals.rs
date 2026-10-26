use super::{
    config::{self, Category, Condition, Overrides, Patch, Rule, Stage},
    proof,
    workspace::{self, ProofPreview},
};
use crate::{
    acquisition::Provider,
    census::{AccountState, CensusKeys},
    engine::Documents,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SEMANTICS: &str = "organisational-filing-v1";
pub const DECISION_LIMIT: usize = 512;
const CANDIDATE_LIMIT: usize = 64;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub history: Vec<Decision>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub candidate: String,
    pub generation: String,
    pub disposition: Disposition,
    pub target: String,
    pub as_of: i64,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Reject,
}
impl Disposition {
    fn purpose(self) -> &'static [u8] {
        match self {
            Self::Accept => b"mailroom/organisational-proposal-accept/v1",
            Self::Reject => b"mailroom/organisational-proposal-reject/v1",
        }
    }
}
impl Decisions {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        if self.history.len() > DECISION_LIMIT
            || self.history.iter().any(|d| {
                !hex_id(&d.candidate)
                    || !hex_id(&d.generation)
                    || config::identifier(&d.target).is_err()
                    || !seen.insert(&d.candidate)
            })
        {
            return Err("Invalid or full proposal decision history".into());
        }
        Ok(())
    }
}
fn hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identity(value: &impl Serialize) -> Result<String, String> {
    let bytes =
        zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| "Cannot bind proposals")?);
    Ok(data_encoding::HEXLOWER.encode(&Sha256::digest(&*bytes)))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Patch { patch: Patch },
    Addition { rule: Rule },
}
impl Change {
    fn target(&self) -> &str {
        match self {
            Self::Patch { patch } => &patch.id,
            Self::Addition { rule } => &rule.id,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub id: String,
    pub change: Change,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub account: String,
    pub generation: String,
    pub candidates: Vec<Selected>,
}
#[derive(Serialize)]
pub struct Observation {
    pub message: String,
    pub subject: String,
    pub folders: BTreeSet<String>,
}
#[derive(Serialize)]
pub struct Candidate {
    pub id: String,
    pub sender_claim: String,
    pub change: Change,
    pub observations: Vec<Observation>,
    pub rationale: String,
    pub uncertainty: &'static str,
    pub rejected: bool,
    pub prior_rejections: usize,
    pub impact: ProofPreview,
}
#[derive(Serialize)]
pub struct Unresolved {
    pub sender_claim: String,
    pub messages: usize,
    pub reason: String,
}
#[derive(Serialize)]
pub struct Generation {
    pub account: String,
    pub generation: String,
    pub semantics: &'static str,
    pub analysis: &'static str,
    pub revision: u64,
    pub candidates: Vec<Candidate>,
    pub unresolved: Vec<Unresolved>,
    pub current: ProofPreview,
    pub decisions: Decisions,
    pub limitations: &'static str,
}
const UNCERTAINTY: &str = "Sender and header claims are unauthenticated. No role, ownership, service, invoice approval or forwarding authority is inferred. Unknown or mixed recipients remain Inbox without copies. Local approval never deploys rules or acts on mail.";

fn sender_matches(rule: &Rule, sender: &str) -> bool {
    rule.stage == Stage::From
        && !rule.retain
        && !rule.mark_read
        && rule.when.iter().any(|c| {
            matches!(
                c,
                Condition::FromAddress { .. } | Condition::FromDomain { .. }
            )
        })
        && rule.when.iter().all(|c| match c {
            Condition::FromAddress { value } => value == sender,
            Condition::FromDomain { value } => sender
                .split_once('@')
                .is_some_and(|(_, domain)| domain == value),
            Condition::NonBulk => true,
            _ => false,
        })
}
fn resolved_id<'a>(preset: &'a config::Preset, id: &'a str) -> &'a str {
    let mut id = id;
    while let Some(next) = preset.renamed.get(id) {
        id = next;
    }
    id
}
fn applies(rule: &Rule, account: &str) -> bool {
    rule.accounts.is_empty() || rule.accounts.contains(account)
}
fn category_key(category: Category) -> String {
    serde_json::to_string(&category).expect("category serializes")
}
fn add_change(overrides: &mut Overrides, change: &Change) -> Result<(), String> {
    if overrides.patches.iter().any(|p| p.id == change.target())
        || overrides.additions.iter().any(|r| r.id == change.target())
    {
        return Err("Proposal conflicts with an existing owner patch or addition".into());
    }
    match change {
        Change::Patch { patch } => overrides.patches.push(patch.clone()),
        Change::Addition { rule } => overrides.additions.push(rule.clone()),
    }
    Ok(())
}
fn resulting(state: &AccountState, changes: &[Change]) -> Result<AccountState, String> {
    let mut result = state.clone();
    let workspace = result
        .workspace
        .as_mut()
        .ok_or("Save rules before generating proposals")?;
    let (_, mut overrides) = config::documents(
        &workspace.input.documents.preset_yaml,
        &workspace.input.documents.override_yaml,
    )?;
    let mut targets = BTreeSet::new();
    for change in changes {
        if !targets.insert(change.target()) {
            return Err("Conflicting proposal targets".into());
        }
        add_change(&mut overrides, change)?;
    }
    workspace.input.documents.override_yaml =
        serde_yaml_ng::to_string(&overrides).map_err(|_| "Cannot serialize resulting overrides")?;
    workspace.revision = workspace
        .revision
        .checked_add(1)
        .ok_or("Workspace revision exhausted")?;
    Ok(result)
}

fn normalized_observations(state: &AccountState) -> crate::census::Census {
    fn snapshot(s: &mut crate::census::acquisition::Snapshot) {
        s.attempt_id = "0".repeat(32);
        s.topology_observed_at = 0;
        s.completed_at = s.completed_at.map(|_| 0);
        s.folders.sort_by(|a, b| a.name.cmp(&b.name));
        for folder in &mut s.folders {
            if let crate::census::acquisition::CountObservation::Observed { observed_at, .. } =
                &mut folder.count
            {
                *observed_at = 0;
            }
        }
    }
    let mut census = (*state.census).clone();
    census.last_completed_at = census.last_completed_at.map(|_| 0);
    if let Some(attempt) = &mut census.acquisition.latest {
        attempt.id = "0".repeat(32);
        attempt.started_at = 0;
        attempt.finished_at = attempt.finished_at.map(|_| 0);
        if let Some(session) = &mut attempt.session {
            session.observed_at = 0;
        }
        if let Some(checkpoint) = &mut attempt.checkpoint {
            snapshot(checkpoint);
        }
    }
    if let Some(success) = &mut census.acquisition.last_successful {
        snapshot(success);
    }
    census
}

pub fn generate(
    state: &AccountState,
    context: &workspace::WorkspaceContext,
    keys: &CensusKeys,
) -> Result<Generation, String> {
    let provider = context.provider();
    state.proposals.validate()?;
    let workspace = state
        .workspace
        .as_ref()
        .ok_or("Save rules before generating proposals")?;
    let input = &workspace.input;
    let (preset, overrides) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml)?;
    let current = workspace::preview(state, context, keys)?;
    let account = state.census.account.as_str();
    let generation = identity(&(
        SEMANTICS,
        crate::census::report::ANALYSIS_VERSION,
        super::program::SEMANTICS,
        proof::SEMANTICS,
        account,
        provider,
        normalized_observations(state),
        state.census.acquisition.view(input.as_of).current,
        input,
        &state.ledger,
        if input.baseline == proof::Baseline::Previous {
            state.exports.last()
        } else {
            None
        },
    ))?;
    let mut groups: BTreeMap<String, Vec<proof::InputMessage<'_>>> = BTreeMap::new();
    let mut unresolved = Vec::new();
    for message in proof::messages(&state.census, provider == Provider::Gmail) {
        let from = &message.record.headers.from;
        if from.len() != 1 || !config::mailbox(&from[0].address) {
            unresolved.push(Unresolved {
                sender_claim: "Ambiguous or missing raw From".into(),
                messages: 1,
                reason: "No unique valid sender claim".into(),
            });
        } else {
            groups
                .entry(from[0].address.clone())
                .or_default()
                .push(message);
        }
    }
    let mut candidates = Vec::new();
    for (sender, messages) in groups {
        let unresolved_reason = |reason: &str| Unresolved {
            sender_claim: sender.clone(),
            messages: messages.len(),
            reason: reason.into(),
        };
        if !messages.iter().all(|m| {
            matches!(
                crate::census::report::classify(&m.record.headers, false),
                crate::census::report::SenderClass::Bulk
                    | crate::census::report::SenderClass::Platform
            )
        }) {
            unresolved.push(unresolved_reason("Insufficient or mixed organisational evidence. No sender identity is inferred from infrastructure."));
            continue;
        }
        if overrides
            .additions
            .iter()
            .any(|r| r.enabled && applies(r, account) && sender_matches(r, &sender))
        {
            unresolved.push(unresolved_reason(
                "Existing owner sender mapping retained. No replacement proposed.",
            ));
            continue;
        }
        let known: Vec<_> = preset
            .rules
            .iter()
            .filter(|r| applies(r, account) && sender_matches(r, &sender))
            .collect();
        if !known.is_empty() {
            if known.len() != 1
                || known[0].enabled
                || overrides
                    .patches
                    .iter()
                    .any(|p| resolved_id(&preset, &p.id) == known[0].id)
            {
                unresolved.push(unresolved_reason(
                    "Known preset sender or owner patch retained. No overwrite proposed.",
                ));
                continue;
            }
        }
        let mut support: BTreeMap<String, (Category, Option<String>, String)> = BTreeMap::new();
        let mut keywords: BTreeMap<String, (Category, Vec<&Rule>)> = BTreeMap::new();
        for rule in &preset.rules {
            if rule.enabled
                && !rule.retain
                && rule.stage == Stage::Subject
                && applies(rule, account)
                && !overrides
                    .patches
                    .iter()
                    .any(|p| resolved_id(&preset, &p.id) == rule.id)
                && !rule.when.is_empty()
                && rule
                    .when
                    .iter()
                    .all(|c| matches!(c, Condition::SubjectContains { .. }))
            {
                keywords
                    .entry(format!(
                        "{}/{}",
                        category_key(rule.category),
                        rule.folder.as_deref().unwrap_or("")
                    ))
                    .or_insert_with(|| (rule.category, Vec::new()))
                    .1
                    .push(rule);
            }
        }
        for (key, (category, rules)) in keywords {
            let count = messages
                .iter()
                .filter(|m| {
                    rules.iter().any(|r| {
                        r.when.iter().all(|c| match c {
                            Condition::SubjectContains { value } => m
                                .record
                                .headers
                                .subject
                                .to_ascii_lowercase()
                                .contains(&value.to_ascii_lowercase()),
                            _ => false,
                        })
                    })
                })
                .count();
            if count * 2 > messages.len() {
                let folder = rules[0].folder.clone();
                let key = if folder.is_none() {
                    category_key(category)
                } else {
                    key
                };
                support.insert(key, (category, folder, format!("Strict subject majority {count}/{} supports the generic preset category", messages.len())));
            }
        }
        let placements: BTreeSet<_> = messages
            .iter()
            .flat_map(|m| m.folders.iter())
            .filter(|f| !f.eq_ignore_ascii_case("inbox"))
            .cloned()
            .collect();
        if placements.len() == 1
            && messages
                .iter()
                .all(|m| m.folders.iter().any(|f| placements.contains(f)))
        {
            let placement = placements.first().expect("one folder");
            for effective in current.proof.artifact.effective() {
                let super::program::EffectiveDefinition::Filing { rule } = &effective.definition
                else {
                    continue;
                };
                if !rule.enabled || rule.retain || rule.mark_read || !applies(rule, account) {
                    continue;
                }
                let name = rule
                    .folder
                    .as_deref()
                    .unwrap_or_else(|| rule.category.label(overrides.layout.language));
                if [
                    format!("{}/{name}", overrides.layout.personal_root),
                    format!("{}/{name}", overrides.layout.business_root),
                ]
                .contains(placement)
                {
                    support.insert(
                        if rule.folder.is_none() { category_key(rule.category) } else { format!("{}/{}", category_key(rule.category), name) },
                        (
                            rule.category,
                            rule.folder.clone(),
                            "All observations share a filing folder mapped by the preset or existing owner rules".into(),
                        ),
                    );
                }
            }
        }
        if support.len() != 1 {
            unresolved.push(unresolved_reason("No unique supported preset category or approved folder mapping. Owner classification is required."));
            continue;
        }
        let (category, folder, rationale) = support.into_values().next().expect("one category");
        let change = if let Some(rule) = known.first() {
            if category_key(rule.category) != category_key(category) {
                unresolved.push(unresolved_reason(
                    "Preset sender category conflicts with observed category",
                ));
                continue;
            }
            Change::Patch {
                patch: Patch {
                    id: rule.id.clone(),
                    enabled: Some(true),
                    folder: None,
                    when: None,
                    extend: vec![Condition::NonBulk],
                    accounts: None,
                    mark_read: None,
                },
            }
        } else {
            let sender_id = identity(&(account, &sender))?;
            Change::Addition {
                rule: Rule {
                    id: format!("user.organisation.{}", &sender_id[..32]),
                    category,
                    stage: Stage::From,
                    when: vec![
                        Condition::FromAddress {
                            value: sender.clone(),
                        },
                        Condition::NonBulk,
                    ],
                    folder,
                    enabled: true,
                    accounts: BTreeSet::from([account.into()]),
                    retain: false,
                    mark_read: false,
                },
            }
        };
        let result = match resulting(state, std::slice::from_ref(&change)) {
            Ok(v) => v,
            Err(_) => {
                unresolved.push(unresolved_reason("Candidate conflicts with existing owner rules or cannot compile. Existing configuration retained."));
                continue;
            }
        };
        if candidates.len() >= CANDIDATE_LIMIT {
            unresolved.push(unresolved_reason(
                "Candidate limit reached. No additional candidate generated.",
            ));
            continue;
        }
        let impact = match workspace::preview(&result, context, keys) {
            Ok(proof) => proof,
            Err(_) => {
                unresolved.push(unresolved_reason("Candidate does not compile with current owner rules. Existing configuration retained."));
                continue;
            }
        };
        let id = identity(&(&generation, &change, &sender))?;
        let observations = messages
            .iter()
            .map(|m| Observation {
                message: m.id.clone(),
                subject: m.record.headers.subject.clone(),
                folders: m.folders.clone(),
            })
            .collect();
        let rejected = state
            .proposals
            .history
            .iter()
            .any(|d| d.candidate == id && d.disposition == Disposition::Reject);
        let prior_rejections = state
            .proposals
            .history
            .iter()
            .filter(|d| {
                d.disposition == Disposition::Reject
                    && d.target == change.target()
                    && d.candidate != id
            })
            .count();
        candidates.push(Candidate {
            id,
            sender_claim: sender,
            change,
            observations,
            rationale,
            uncertainty: UNCERTAINTY,
            rejected,
            prior_rejections,
            impact,
        });
    }
    Ok(Generation {
        account: account.into(),
        generation,
        semantics: SEMANTICS,
        analysis: crate::census::report::ANALYSIS_VERSION,
        revision: state.revision,
        candidates,
        unresolved,
        current,
        decisions: state.proposals.clone(),
        limitations: UNCERTAINTY,
    })
}
#[derive(Serialize)]
pub struct Review {
    pub selection: Selection,
    pub disposition: Disposition,
    pub changes: Vec<Change>,
    pub original_documents: Documents,
    pub resulting_documents: Documents,
    pub document_diff: String,
    pub current: ProofPreview,
    pub resulting: ProofPreview,
    pub receipt: String,
    pub acceptance_available: bool,
    pub effects: &'static str,
}
fn document_diff(before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let before: Vec<_> = before.split_inclusive('\n').collect();
    let after: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = String::from("--- override.yaml\n+++ override.yaml\n");
    for line in &before[prefix..before.len() - suffix] {
        result.push('-');
        result.push_str(line);
        if !line.ends_with('\n') {
            result.push('\n');
        }
    }
    for line in &after[prefix..after.len() - suffix] {
        result.push('+');
        result.push_str(line);
        if !line.ends_with('\n') {
            result.push('\n');
        }
    }
    result
}
pub fn review(
    state: &AccountState,
    context: &workspace::WorkspaceContext,
    keys: &CensusKeys,
    selection: &Selection,
    disposition: Disposition,
) -> Result<Review, String> {
    let provider = context.provider();
    let generation = generate(state, context, keys)?;
    if selection.account != generation.account
        || selection.generation != generation.generation
        || selection.candidates.is_empty()
        || selection.candidates.len() > CANDIDATE_LIMIT
    {
        return Err("Empty, stale or wrong-account proposal selection".into());
    }
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    for selected in &selection.candidates {
        if !seen.insert(&selected.id) {
            return Err("Duplicate proposal selection".into());
        }
        let candidate = generation
            .candidates
            .iter()
            .find(|c| c.id == selected.id)
            .ok_or("Unknown proposal candidate")?;
        if state
            .proposals
            .history
            .iter()
            .any(|d| d.candidate == candidate.id)
            || serde_json::to_value(&candidate.change).map_err(|_| "Invalid change")?
                != serde_json::to_value(&selected.change).map_err(|_| "Invalid change")?
        {
            return Err("Rejected or altered proposal candidate".into());
        }
        changes.push(candidate.change.clone());
    }
    if state.proposals.history.len() + changes.len() > DECISION_LIMIT {
        return Err("Proposal decision history is full. No history was removed".into());
    }
    let result = if disposition == Disposition::Accept {
        resulting(state, &changes)?
    } else {
        state.clone()
    };
    let current = generation.current;
    let resulting = workspace::preview(&result, context, keys)?;
    let original_documents = state
        .workspace
        .as_ref()
        .ok_or("Missing workspace")?
        .input
        .documents
        .clone();
    let resulting_documents = result
        .workspace
        .as_ref()
        .ok_or("Missing workspace")?
        .input
        .documents
        .clone();
    let document_diff = document_diff(
        &original_documents.override_yaml,
        &resulting_documents.override_yaml,
    );
    let effects = if disposition == Disposition::Accept {
        "Save exactly the selected local filing rules and decisions. No export, ledger-file change, provider operation or mail action. Run fresh proof after saving."
    } else {
        "Record only the selected exact rejections. Rule documents remain unchanged."
    };
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(
            SEMANTICS,
            provider,
            state,
            selection,
            disposition,
            &changes,
            &current,
            &resulting,
            &resulting_documents,
            &document_diff,
            effects,
        ))
        .map_err(|_| "Cannot bind proposal review")?,
    );
    let receipt = keys.binding_for(disposition.purpose(), &payload);
    let acceptance_available =
        disposition == Disposition::Reject || resulting.proof.gate != "blocked";
    Ok(Review {
        selection: selection.clone(),
        disposition,
        changes,
        original_documents,
        resulting_documents,
        document_diff,
        current,
        resulting,
        receipt,
        acceptance_available,
        effects,
    })
}
pub fn confirm(
    state: &mut AccountState,
    context: &workspace::WorkspaceContext,
    keys: &CensusKeys,
    selection: &Selection,
    disposition: Disposition,
    receipt: &str,
) -> Result<(), String> {
    use subtle::ConstantTimeEq;
    let review = review(state, context, keys, selection, disposition)?;
    if !review.acceptance_available
        || !bool::from(review.receipt.as_bytes().ct_eq(receipt.as_bytes()))
    {
        return Err("Proposal review changed or is blocked. Review a fresh exact batch".into());
    }
    let mut result = if disposition == Disposition::Accept {
        resulting(state, &review.changes)?
    } else {
        state.clone()
    };
    let as_of = state
        .workspace
        .as_ref()
        .ok_or("Missing workspace")?
        .input
        .as_of;
    for selected in &selection.candidates {
        result.proposals.history.push(Decision {
            candidate: selected.id.clone(),
            generation: selection.generation.clone(),
            disposition,
            target: selected.change.target().into(),
            as_of,
        });
    }
    result.proposals.validate()?;
    *state = result;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
