use super::{
    catalog,
    config::{self, Patch, Stage},
    proposals::{document_diff, hex_id},
    workspace::{self, ProofPreview, WorkspaceContext},
};
use crate::{
    agent::{Proposal, RuleEdits},
    census::{AccountState, CensusKeys},
    engine::Documents,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const SEMANTICS: &str = "mailroom-agent-rule-edits-v1";
pub const REJECTION_LIMIT: usize = 512;
const EFFECTS: Effects = Effects {
    accept: "Save exactly these rule edits to the local override document. No export, deployment, provider operation or mail action. Run fresh proof after saving.",
    reject: "Record these exact edits as rejected so they cannot be imported again. Rule documents remain unchanged.",
};

/// A rejected agent edit, kept so the same edit is refused when proposed again.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejection {
    pub edit: String,
    pub target: String,
    pub summary: String,
    pub as_of: i64,
}

pub(crate) fn validate_rejections(rejections: &[Rejection]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    if rejections.len() > REJECTION_LIMIT
        || rejections.iter().any(|r| {
            !hex_id(&r.edit)
                || !hex_id(&r.summary)
                || config::identifier(&r.target).is_err()
                || !seen.insert(&r.edit)
        })
    {
        return Err("Invalid or full agent rejection history".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Reject,
}
impl Disposition {
    fn purpose(self) -> &'static [u8] {
        match self {
            Self::Accept => b"mailroom/agent-rule-edits-accept/v1",
            Self::Reject => b"mailroom/agent-rule-edits-reject/v1",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    Patch,
    Addition,
}
/// One validated edit. `target` is the current rule ID after the preset's
/// rename chain; `digest` identifies the edit for rejection memory.
#[derive(Clone, Serialize)]
pub struct ReviewedEdit {
    pub kind: EditKind,
    pub requested: String,
    pub target: String,
    pub digest: String,
}
#[derive(Clone, Copy, Serialize)]
pub struct Effects {
    pub accept: &'static str,
    pub reject: &'static str,
}
#[derive(Serialize)]
pub struct Review {
    pub semantics: &'static str,
    pub account: String,
    pub revision: u64,
    pub workspace_revision: u64,
    pub summary_sha256: String,
    pub edits: Vec<ReviewedEdit>,
    pub changes: RuleEdits,
    pub original_documents: Documents,
    pub resulting_documents: Documents,
    pub document_diff: String,
    pub current: ProofPreview,
    pub resulting: ProofPreview,
    pub accept_receipt: Option<String>,
    pub reject_receipt: Option<String>,
    pub effects: Effects,
}

fn identity(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Cannot identify agent edit")?;
    Ok(data_encoding::HEXLOWER.encode(&Sha256::digest(&bytes)))
}

fn resolve<'a>(preset: &'a config::Preset, mut id: &'a str) -> Result<&'a str, String> {
    let mut seen = BTreeSet::new();
    while let Some(next) = preset.renamed.get(id) {
        if !seen.insert(id) {
            return Err("Cyclic rule rename chain".into());
        }
        id = next;
    }
    Ok(id)
}

fn changes_nothing(patch: &Patch) -> bool {
    patch.enabled.is_none()
        && patch.folder.is_none()
        && patch.when.is_none()
        && patch.extend.is_empty()
        && patch.accounts.is_none()
        && patch.mark_read.is_none()
}

/// Validates the edits against the saved documents and returns them with the
/// state they would produce. Patches are written under their current IDs.
fn apply(
    state: &AccountState,
    context: &WorkspaceContext,
    edits: &RuleEdits,
) -> Result<(Vec<ReviewedEdit>, AccountState), String> {
    let saved = state
        .workspace
        .as_ref()
        .ok_or("Save local rules before importing agent edits")?;
    let (preset, mut overrides) = config::documents(
        &saved.input.documents.preset_yaml,
        &saved.input.documents.override_yaml,
    )?;
    let rules = catalog::load(&preset)?.preset.rules;
    let mut claimed = BTreeSet::new();
    for patch in &overrides.patches {
        claimed.insert(resolve(&preset, &patch.id)?.to_owned());
    }
    for rule in &overrides.additions {
        claimed.insert(rule.id.clone());
    }
    let mut reviewed = Vec::new();
    for patch in &edits.patches {
        config::identifier(&patch.id)?;
        let target = resolve(&preset, &patch.id)?.to_owned();
        let rule = rules
            .iter()
            .find(|r| r.id == target)
            .ok_or("Agent patch targets no existing preset rule")?;
        if rule.stage == Stage::Guard {
            return Err("Agent edits cannot change guard rules".into());
        }
        if changes_nothing(patch) {
            return Err("Agent patch changes no value".into());
        }
        if !claimed.insert(target.clone()) {
            return Err("Agent edit claims a rule that already has an edit".into());
        }
        let canonical = Patch {
            id: target.clone(),
            ..patch.clone()
        };
        reviewed.push(ReviewedEdit {
            kind: EditKind::Patch,
            requested: patch.id.clone(),
            digest: identity(&("patch", &canonical))?,
            target,
        });
        overrides.patches.push(canonical);
    }
    for rule in &edits.additions {
        config::validate_rule(rule)?;
        if !rule.id.starts_with("user.") || rule.stage == Stage::Guard {
            return Err("Agent additions need a user.* filing rule".into());
        }
        if rules.iter().any(|r| r.id == rule.id)
            || preset.renamed.contains_key(&rule.id)
            || !claimed.insert(rule.id.clone())
        {
            return Err("Agent addition reuses an existing rule ID".into());
        }
        reviewed.push(ReviewedEdit {
            kind: EditKind::Addition,
            requested: rule.id.clone(),
            target: rule.id.clone(),
            digest: identity(&("addition", rule))?,
        });
        overrides.additions.push(rule.clone());
    }
    let rejected: BTreeSet<_> = state
        .proposals
        .agent_rejections
        .iter()
        .map(|r| &r.edit)
        .collect();
    if reviewed.iter().any(|e| rejected.contains(&e.digest)) {
        return Err("Agent proposal repeats a rejected rule edit".into());
    }
    let mut result = state.clone();
    let workspace = result.workspace.as_mut().ok_or("Missing workspace")?;
    workspace.input.documents.override_yaml =
        serde_yaml_ng::to_string(&overrides).map_err(|_| "Cannot prepare resulting rules")?;
    workspace.revision = workspace
        .revision
        .checked_add(1)
        .ok_or("Workspace revision exhausted")?;
    crate::ledger::validate_workspace_edit(state, &workspace.input)?;
    workspace::validate(&workspace.input, state, context)?;
    Ok((reviewed, result))
}

fn prepare(
    state: &AccountState,
    context: &WorkspaceContext,
    keys: &CensusKeys,
    proposal: &Proposal,
    summary: &str,
) -> Result<(Review, AccountState), String> {
    proposal.validate(summary)?;
    let changes = proposal
        .rule_edits
        .clone()
        .ok_or("Proposal contains no rule edits")?;
    let (edits, result) = apply(state, context, &changes)?;
    let current = workspace::preview(state, context, keys)?;
    let resulting = workspace::preview(&result, context, keys)?;
    let original_documents = state
        .workspace
        .as_ref()
        .ok_or("Missing workspace")?
        .input
        .documents
        .clone();
    let after = result.workspace.as_ref().ok_or("Missing workspace")?;
    let resulting_documents = after.input.documents.clone();
    let document_diff = document_diff(
        &original_documents.override_yaml,
        &resulting_documents.override_yaml,
    );
    let payload = zeroize::Zeroizing::new(
        crate::census::binding::bound(|| {
            serde_json::to_vec(&(
                SEMANTICS,
                context,
                state,
                proposal,
                &edits,
                &original_documents,
                &resulting_documents,
                &document_diff,
                &current,
                &resulting,
                EFFECTS,
            ))
        })
        .map_err(|_| "Cannot bind agent edit review")?,
    );
    let accept_receipt = (resulting.proof.gate != "blocked")
        .then(|| keys.binding_for(Disposition::Accept.purpose(), &payload));
    let reject_receipt = (state.proposals.agent_rejections.len() + edits.len() <= REJECTION_LIMIT)
        .then(|| keys.binding_for(Disposition::Reject.purpose(), &payload));
    let review = Review {
        semantics: SEMANTICS,
        account: state.census.account.as_str().into(),
        revision: state.revision,
        workspace_revision: after.revision,
        summary_sha256: proposal.summary_sha256.clone(),
        edits,
        changes,
        original_documents,
        resulting_documents,
        document_diff,
        current,
        resulting,
        accept_receipt,
        reject_receipt,
        effects: EFFECTS,
    };
    Ok((review, result))
}

pub(crate) fn review(
    state: &AccountState,
    context: &WorkspaceContext,
    keys: &CensusKeys,
    proposal: &Proposal,
    summary: &str,
) -> Result<Review, String> {
    Ok(prepare(state, context, keys, proposal, summary)?.0)
}

pub(crate) fn confirm(
    state: &mut AccountState,
    context: &WorkspaceContext,
    keys: &CensusKeys,
    proposal: &Proposal,
    summary: &str,
    disposition: Disposition,
    receipt: &str,
) -> Result<(), String> {
    use subtle::ConstantTimeEq;
    let (review, result) = prepare(state, context, keys, proposal, summary)?;
    let expected = match disposition {
        Disposition::Accept => review.accept_receipt,
        Disposition::Reject => review.reject_receipt,
    }
    .ok_or("This decision is blocked for the reviewed edits")?;
    if !bool::from(expected.as_bytes().ct_eq(receipt.as_bytes())) {
        return Err("Agent edit review changed or is stale. Review the proposal again".into());
    }
    match disposition {
        Disposition::Accept => *state = result,
        Disposition::Reject => {
            let as_of = state
                .workspace
                .as_ref()
                .ok_or("Missing workspace")?
                .input
                .as_of;
            let mut decisions = state.proposals.clone();
            decisions
                .agent_rejections
                .extend(review.edits.into_iter().map(|e| Rejection {
                    edit: e.digest,
                    target: e.target,
                    summary: review.summary_sha256.clone(),
                    as_of,
                }));
            decisions.validate()?;
            state.proposals = decisions;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
