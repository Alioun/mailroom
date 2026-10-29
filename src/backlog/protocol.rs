use super::{Options, Plan, imap_move::*, retention};
use crate::{
    census::{AccountId, AccountState, CensusError, CensusKeys, folder_provisioning::Inventory},
    rules::proof::Proof,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Retained runs with a full per-message journal. Q27 compacts finished runs
/// 90 days after completion, so this bounds runs started within 90 days.
pub const HISTORY: usize = 512;
/// Batches per run: 512,000 messages at the 500-message batch limit.
pub const BATCHES: usize = 1024;
pub const LIMITATIONS: &str = "Explicit synthetic loopback MOVE/UIDPLUS source only. Saved workspace evidence is installed separately. Filing ignores predicted copies, read changes and keywords. No COPY fallback, Gmail protocol labels or automatic folder creation. MOVE cannot guard atomically against another client changing flags after the final read. Local locks do not exclude other clients. Undo preserves current flags and assigns new UIDs; it is not snapshot rollback. Missing mapping after possible transmission stays uncertain and cannot be retried. Restored accounts remain held.";

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summaries: Vec<retention::Summary>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub account: AccountId,
    pub source: String,
    pub authority: String,
    pub workspace: String,
    pub material: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reverse {
    pub forward: usize,
    pub evidence: MoveEvidence,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    /// When every forward batch was first verified. Undo is offered for 90
    /// days from here. Runs saved before schema 19 lack it until the first
    /// save under the new rule stamps them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
    /// Set when the owner closed a run that ended in conflict with nothing
    /// left to undo or observe. Since schema 21.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closure>,
}
/// The owner's confirmed acknowledgment of a conflicted run. It changes no
/// mail and keeps the whole journal. A closed run no longer blocks new runs,
/// and it is never compacted (Q27 keeps conflicted runs whole).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Closure {
    pub at: u64,
    pub receipt: String,
}
impl Closure {
    pub(crate) fn valid(&self, status: Status) -> bool {
        matches!(status, Status::Conflict | Status::Undone)
            && self.receipt.len() == 64
            && self
                .receipt
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
/// Why the owner may close a conflicted run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// A fresh Undo preview would reverse nothing.
    NothingToUndo,
    /// The run's Undo already ran and ended in conflict.
    UndoEnded,
    /// Undo is not offered: the approval inputs changed or the window closed.
    UndoUnavailable,
}
#[derive(Debug, Serialize)]
pub struct ClosePreview {
    pub run: String,
    pub reason: CloseReason,
    pub receipt: String,
}
impl Run {
    pub fn finished(&self) -> bool {
        (self.status == Status::Undone
            || self.status == Status::Complete && self.undo_receipt.is_none())
            && self.batches.iter().all(|b| !b.attempted() || verified(b))
    }
    /// Finished, or closed by the owner. Only an unsettled run blocks a new one.
    pub fn settled(&self) -> bool {
        self.finished() || self.closed.is_some()
    }
    /// Whether the run is stuck in a state only closing can settle: a
    /// conflict, or an Undo that finished beside forward conflicts, with every
    /// attempt observed and nothing uncertain.
    pub fn closable(&self) -> bool {
        !self.settled()
            && matches!(self.status, Status::Conflict | Status::Undone)
            && self.observed_without_uncertainty()
    }
    /// Every attempted batch, forward and reverse, has saved readback and no
    /// message is uncertain. Only then can a conflicted run be closed.
    pub fn observed_without_uncertainty(&self) -> bool {
        self.batches
            .iter()
            .chain(self.reverse.iter().map(|r| &r.evidence))
            .all(|e| {
                !e.attempted()
                    || e.observed()
                        && e.resolutions()
                            .values()
                            .all(|r| *r != Resolution::Uncertain)
            })
    }
    /// Records the first completion. Called wherever a run's status is set.
    pub fn stamp(&mut self, now: u64) {
        if self.completed_at.is_none() && matches!(self.status, Status::Complete | Status::Undone) {
            self.completed_at = Some(now);
        }
    }
    fn summary(&self, completed_at: u64, now: u64) -> retention::Summary {
        let count = |entries: &mut dyn Iterator<Item = &MoveEvidence>| {
            entries
                .map(|e| {
                    e.resolutions()
                        .values()
                        .filter(|r| matches!(r, Resolution::Verified { .. }))
                        .count()
                })
                .sum()
        };
        retention::Summary {
            id: self.id.clone(),
            receipt: self.receipt.clone(),
            status: self.status,
            completed_at,
            compacted_at: now,
            messages: self.plan.filing.changes.len(),
            batches: self.batches.len(),
            applied: count(&mut self.batches.iter()),
            restored: count(&mut self.reverse.iter().map(|r| &r.evidence)),
            folders: retention::folders(&self.plan.filing.changes),
        }
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

/// Fields no material form covers: the revision, the journals runs write
/// themselves, and Gmail label provisioning, which only adds labels a run can
/// file into.
pub(crate) const UNBOUND: &[&str] = &["revision", "protocol_backlog", "label_provisioning"];
/// Digest of the account state a run may rely on: the workspace, ledger,
/// exports and every other aggregate, but not the census. The census is a
/// cached observation. Every effect, Resume and Undo reads the messages it
/// touches fresh and refuses any that differ from the approved snapshot, so a
/// read-only refresh changes nothing a run relies on.
pub fn material(state: &AccountState, keys: &CensusKeys) -> Result<String, String> {
    material_as(state, keys, &[UNBOUND, &["census"]].concat(), MATERIAL_V3)
}
/// v1 serialized the census whole and v2 binds it by digest. A run saved with
/// either keeps being checked in that form, so a refresh still invalidates it.
pub(crate) const MATERIAL_V1: &[u8] = b"mailroom/imap-backlog-material/v1";
const MATERIAL_V2: &[u8] = b"mailroom/imap-backlog-material/v2";
const MATERIAL_V3: &[u8] = b"mailroom/imap-backlog-material/v3";
pub(crate) fn material_legacy(state: &AccountState, keys: &CensusKeys) -> Vec<String> {
    [MATERIAL_V2, MATERIAL_V1]
        .iter()
        .filter_map(|purpose| material_as(state, keys, UNBOUND, purpose).ok())
        .collect()
}
pub(crate) fn material_as(
    state: &AccountState,
    keys: &CensusKeys,
    excluded: &[&str],
    purpose: &[u8],
) -> Result<String, String> {
    let legacy = purpose.ends_with(b"/v1");
    let encode = || -> Result<Vec<u8>, String> {
        let mut value = serde_json::to_value(state).map_err(|_| "Cannot bind material state")?;
        for field in excluded {
            value.as_object_mut().unwrap().remove(*field);
        }
        serde_json::to_vec(&value).map_err(|_| "Cannot bind material state".into())
    };
    let bytes = if legacy {
        encode()?
    } else {
        crate::census::binding::bound(encode)?
    };
    Ok(keys.binding_for(purpose, &bytes))
}
/// Whether a freshly computed binding still matches a saved run's. A saved
/// material that differs from the current form is recomputed in the legacy
/// forms it may have been written in.
pub fn same_binding(a: &Binding, b: &Binding, legacy: impl FnOnce() -> Vec<String>) -> bool {
    a.account == b.account
        && a.source == b.source
        && a.authority == b.authority
        && a.workspace == b.workspace
        && (a.material == b.material || legacy().contains(&b.material))
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
    /// Applies Q27: stamps finished runs saved before the rule, then replaces
    /// each finished run whose undo window has closed with its summary.
    /// Returns whether anything changed, so the caller saves once.
    pub fn compact(&mut self, now: u64) -> bool {
        let mut changed = false;
        for run in &mut self.runs {
            if run.completed_at.is_none() && run.finished() {
                run.completed_at = Some(now);
                changed = true;
            }
        }
        let (expired, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.runs).into_iter().partition(|r| {
                r.finished()
                    && r.completed_at
                        .is_some_and(|c| !retention::undo_open(Some(c), now))
            });
        self.runs = kept;
        for run in expired {
            self.summaries
                .push(run.summary(run.completed_at.expect("expired run"), now));
            changed = true;
        }
        changed
    }
    pub fn deadlines(&self, now: u64) -> Vec<retention::Deadline> {
        self.runs
            .iter()
            .map(|r| retention::Deadline::of(&r.id, r.completed_at, now))
            .collect()
    }
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
            || self.runs.iter().filter(|r| !r.settled()).count() > 1
            || self.summaries.len() > retention::SUMMARIES
        {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for s in &self.summaries {
            if !ids.insert(&s.id) || !s.valid() {
                return Err(invalid());
            }
        }
        for r in &self.runs {
            let changes: std::collections::BTreeMap<_, _> = r
                .plan
                .filing
                .changes
                .iter()
                .map(|c| {
                    (
                        (c.source.folder.as_str(), c.source.uidvalidity, c.source.uid),
                        c,
                    )
                })
                .collect();
            if !ids.insert(&r.id)
                || changes.len() != r.plan.filing.changes.len()
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
                || r.closed
                    .as_ref()
                    .is_some_and(|c| !c.valid(r.status) || !r.observed_without_uncertainty())
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
                    if changes
                        .get(&(
                            b.command().source().name.decoded.as_str(),
                            b.command().source().uidvalidity.get(),
                            uid.get(),
                        ))
                        .is_none_or(|c| {
                            c.destination != b.command().destination().name.decoded
                                || c.source.message_id_hash
                                    != b.command().messages()[uid].message_id_hash
                                || c.flags != b.command().messages()[uid].flags
                        })
                    {
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
                let resolutions = f.resolutions();
                if !reversed.insert(b.forward)
                    || b.evidence.command().purpose() != Purpose::ConditionalUndo
                    || b.evidence.command().source() != f.command().destination()
                    || b.evidence.command().destination() != f.command().source()
                    || f.mapping().is_none_or(|m| {
                        let before: std::collections::BTreeMap<_, _> =
                            m.pairs().iter().map(|(b, a)| (a, b)).collect();
                        b.evidence
                            .command()
                            .messages()
                            .iter()
                            .any(|(uid, current)| {
                                before.get(uid).is_none_or(|uid_before| {
                                    let original = &f.command().messages()[uid_before];
                                    resolutions[uid_before]
                                        != (Resolution::Verified { destination: *uid })
                                        || original.message_id_hash != current.message_id_hash
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
                || r.undo_receipt.is_some()
                    && r.batches.iter().any(|b| b.attempted() && !b.observed())
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
    super::project(
        &mut plan,
        census,
        proof,
        None,
        state
            .workspace
            .as_ref()
            .ok_or("Workspace required")?
            .input
            .as_of,
    )?;
    Ok(plan)
}
