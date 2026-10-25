use super::{Options, Plan, imap_move::*};
use crate::{
    census::{AccountId, AccountState, CensusError, CensusKeys, folder_provisioning::Inventory},
    rules::proof::Proof,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const HISTORY: usize = 32;
pub const BATCHES: usize = 128;
pub const LIMITATIONS: &str = "Explicit synthetic loopback MOVE/UIDPLUS source only. Saved workspace evidence is installed separately. Filing ignores predicted copies, read changes and keywords. No COPY fallback, Gmail protocol labels or automatic folder creation. MOVE cannot guard atomically against another client changing flags after the final read. Local locks do not exclude other clients. Undo preserves current flags and assigns new UIDs; it is not snapshot rollback. Missing mapping after possible transmission stays uncertain and cannot be retried. Restored accounts remain held.";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub runs: Vec<Run>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub account: AccountId,
    pub source: String,
    pub authority: String,
    pub workspace: String,
    pub material: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPlan {
    pub filing: Plan,
    pub binding: Binding,
    pub topology: Inventory,
    pub saved_proof: String,
    pub execution_proof: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Prepared,
    Running,
    Stopped,
    Complete,
    Uncertain,
    Conflict,
    UndoRunning,
    Undone,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reverse {
    pub forward: usize,
    pub evidence: MoveEvidence,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub receipt: String,
    pub plan: SavedPlan,
    pub status: Status,
    pub stop_requested: bool,
    pub batches: Vec<MoveEvidence>,
    pub reverse: Vec<Reverse>,
    pub undo_receipt: Option<String>,
    #[serde(default)]
    pub undo_conflicts: usize,
}
impl Run {
    pub fn finished(&self) -> bool {
        self.status == Status::Undone
            || self.status == Status::Complete && self.undo_receipt.is_none()
    }
}
#[derive(Serialize)]
pub struct Preview {
    pub operation: Run,
    pub receipt: String,
    pub saved: crate::rules::workspace::ProofPreview,
    pub execution: Proof,
    pub limitations: &'static str,
}
#[derive(Serialize)]
pub struct UndoPreview {
    pub run: String,
    pub reverse: Vec<Reverse>,
    pub conflicts: usize,
    pub receipt: String,
    pub limitations: &'static str,
}

pub fn material(state: &AccountState, keys: &CensusKeys) -> Result<String, String> {
    let mut value = serde_json::to_value(state).map_err(|_| "Cannot bind material state")?;
    value.as_object_mut().unwrap().remove("revision");
    value.as_object_mut().unwrap().remove("protocol_backlog");
    Ok(keys.binding_for(
        b"mailroom/imap-backlog-material/v1",
        &serde_json::to_vec(&value).map_err(|_| "Cannot bind material state")?,
    ))
}
pub fn same_binding(a: &Binding, b: &Binding) -> bool {
    a.account == b.account
        && a.source == b.source
        && a.authority == b.authority
        && a.workspace == b.workspace
        && a.material == b.material
}
pub fn verified(e: &MoveEvidence) -> bool {
    e.attempted()
        && e.resolutions()
            .values()
            .all(|r| matches!(r, Resolution::Verified { .. }))
}
pub fn outcome(run: &Run, reverse: bool) -> Status {
    let entries: Vec<_> = if reverse {
        run.reverse.iter().map(|r| &r.evidence).collect()
    } else {
        run.batches.iter().collect()
    };
    if entries
        .iter()
        .any(|e| e.attempted() && e.resolutions().values().any(|r| *r == Resolution::Conflict))
    {
        Status::Conflict
    } else if entries.iter().any(|e| e.attempted() && !verified(e)) {
        Status::Uncertain
    } else if !entries.is_empty() && entries.iter().all(|e| verified(e)) {
        if reverse {
            Status::Undone
        } else {
            Status::Complete
        }
    } else {
        Status::Stopped
    }
}
impl Journal {
    pub fn interrupted(&mut self) {
        for run in &mut self.runs {
            if matches!(run.status, Status::Running | Status::UndoRunning) {
                run.status = outcome(run, run.status == Status::UndoRunning);
                run.stop_requested = true;
            }
        }
    }
    pub fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        let invalid = || CensusError::Storage;
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.runs.len() > HISTORY
            || self.runs.iter().filter(|r| !r.finished()).count() > 1
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 4 * 1024 * 1024
        {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for r in &self.runs {
            if !ids.insert(&r.id)
                || !digest(&r.id)
                || !digest(&r.receipt)
                || r.plan.binding.account != *account
                || r.plan.filing.account != account.as_str()
                || r.plan.filing.provider != crate::acquisition::Provider::MailboxOrg
                || r.plan.filing.semantics != "imap-move-mapping-readback/v1"
                || r.plan.filing.changes.iter().any(|c| {
                    c.operation
                        != super::Operation::Imap {
                            method: super::MoveMethod::UidMove,
                        }
                })
                || r.plan.filing.artifact.context().account != account.as_str()
                || r.batches.is_empty()
                || r.batches.len() > BATCHES
                || r.reverse.len() > r.batches.len()
                || [
                    &r.plan.binding.source,
                    &r.plan.binding.authority,
                    &r.plan.binding.workspace,
                    &r.plan.binding.material,
                    &r.plan.saved_proof,
                    &r.plan.execution_proof,
                ]
                .iter()
                .any(|s| !digest(s))
                || (r.undo_receipt.is_none() && !r.reverse.is_empty())
                || (r.undo_receipt.is_some() && r.reverse.is_empty())
                || (r.undo_receipt.is_none() && r.undo_conflicts != 0)
                || r.undo_conflicts > r.plan.filing.changes.len()
                || r.undo_receipt.as_ref().is_some_and(|s| !digest(s))
            {
                return Err(invalid());
            }
            r.plan.topology.validate().map_err(|_| invalid())?;
            let mut members = BTreeSet::new();
            let mut count = 0;
            for b in &r.batches {
                if b.command().purpose()
                    != (Purpose::Filing {
                        protect_flagged: r.plan.filing.options.protect_flagged,
                    })
                {
                    return Err(invalid());
                }
                for uid in b.command().messages().keys() {
                    if !members.insert((
                        b.command().source().name.decoded.clone(),
                        b.command().source().uidvalidity.get(),
                        uid.get(),
                    )) {
                        return Err(invalid());
                    }
                    count += 1;
                    if !r.plan.filing.changes.iter().any(|c| {
                        c.source.folder == b.command().source().name.decoded
                            && c.source.uidvalidity == b.command().source().uidvalidity.get()
                            && c.source.uid == uid.get()
                            && c.destination == b.command().destination().name.decoded
                            && c.source.message_id_hash
                                == b.command().messages()[uid].message_id_hash
                            && c.flags == b.command().messages()[uid].flags
                    }) {
                        return Err(invalid());
                    }
                }
            }
            if count != r.plan.filing.changes.len() {
                return Err(invalid());
            }
            let mut reversed = BTreeSet::new();
            for b in &r.reverse {
                let f = r.batches.get(b.forward).ok_or_else(invalid)?;
                if !reversed.insert(b.forward)
                    || !verified(f)
                    || b.evidence.command().purpose() != Purpose::ConditionalUndo
                    || b.evidence.command().source() != f.command().destination()
                    || b.evidence.command().destination() != f.command().source()
                    || f.mapping().is_none_or(|m| {
                        b.evidence
                            .command()
                            .messages()
                            .iter()
                            .any(|(uid, current)| {
                                m.pairs()
                                    .iter()
                                    .find(|(_, mapped)| *mapped == uid)
                                    .is_none_or(|(original, _)| {
                                        let original = &f.command().messages()[original];
                                        original.message_id_hash != current.message_id_hash
                                            || original.size != current.size
                                            || original.internal_date != current.internal_date
                                    })
                            })
                    })
                {
                    return Err(invalid());
                }
            }
            if r.status == Status::Prepared
                && (r.batches.iter().any(MoveEvidence::attempted) || r.undo_receipt.is_some())
                || matches!(
                    r.status,
                    Status::Prepared | Status::Running | Status::UndoRunning
                ) && r.stop_requested
                || r.status == Status::Running && r.undo_receipt.is_some()
                || r.status == Status::UndoRunning
                    && r.batches.iter().any(|b| b.attempted() && !verified(b))
                || r.status == Status::Complete && !r.batches.iter().all(verified)
                || r.status == Status::Undone
                    && (r.reverse.is_empty() || !r.reverse.iter().all(|b| verified(&b.evidence)))
                || r.status == Status::UndoRunning && r.undo_receipt.is_none()
                || r.status == Status::Complete && r.reverse.iter().any(|b| b.evidence.attempted())
                || r.status == Status::Uncertain
                    && outcome(r, r.undo_receipt.is_some()) != Status::Uncertain
                || r.status == Status::Stopped
                    && outcome(r, r.undo_receipt.is_some()) != Status::Stopped
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
pub(crate) fn project(
    state: &AccountState,
    census: &crate::census::Census,
    proof: &Proof,
    options: Options,
) -> Result<Plan, String> {
    let mut plan = Plan {
        account: census.account.as_str().into(),
        provider: crate::acquisition::Provider::MailboxOrg,
        source: "explicit-synthetic-loopback-imap-move; separate installed workspace evidence"
            .into(),
        state_revision: state.revision,
        ledger_version: state.ledger.source_version.clone(),
        mailbox_revision: None,
        semantics: "imap-move-mapping-readback/v1".into(),
        artifact: proof.artifact.clone(),
        proof_gate: proof.gate.into(),
        options,
        changes: vec![],
        excluded: Default::default(),
        unmatched: 0,
        review: 0,
        groups: Default::default(),
        stopped_folders: Default::default(),
        diagnostics: proof.warnings.clone(),
    };
    super::project(&mut plan, census, proof, None)?;
    Ok(plan)
}
