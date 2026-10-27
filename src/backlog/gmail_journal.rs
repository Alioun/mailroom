use super::{
    Operation, Options, Plan,
    gmail_label::{INBOX, LabelBatch, Outcome, StepKind},
    imap_move::Purpose,
    protocol::{Closure, SavedPlan, Status},
    retention,
};
use crate::{
    census::{AccountId, AccountState, CensusError, CensusKeys},
    rules::proof::Proof,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const HISTORY: usize = super::protocol::HISTORY;
pub const BATCHES: usize = super::protocol::BATCHES;
pub const SEMANTICS: &str = "gmail-label-readback/v1";
pub const LIMITATIONS: &str = "Explicit synthetic loopback Gmail X-GM-EXT-1 source only. Saved workspace evidence is installed separately. Execution resolves each message by X-GM-MSGID under the discovered All Mail UIDVALIDITY, adds the destination label, reads it back, then separately removes only Inbox and reads back. It never replaces the label set, changes flags or Seen, expunges, or creates labels. Gmail offers no conditional label store, so another client can change labels or flags after the final read. Local locks do not exclude other clients. A label store whose acknowledgment is lost stays uncertain and is never retried. Undo reverses only this run's applied label deltas and keeps later labels and flags; it is not snapshot rollback. Google label semantics are unverified fixture assumptions. Restored accounts remain held.";

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summaries: Vec<retention::Summary>,
}
impl Journal {
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty() && self.summaries.is_empty()
    }
    /// Q27 retention, as for MOVE runs in `protocol::Journal::compact`.
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
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reverse {
    pub forward: usize,
    pub batch: LabelBatch,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub receipt: String,
    pub plan: SavedPlan,
    pub status: Status,
    pub stop_requested: bool,
    pub batches: Vec<LabelBatch>,
    pub reverse: Vec<Reverse>,
    pub undo_receipt: Option<String>,
    pub undo_conflicts: usize,
    /// See `protocol::Run::completed_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
    /// See `protocol::Run::closed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closure>,
}
impl Run {
    pub fn stamp(&mut self, now: u64) {
        if self.completed_at.is_none() && matches!(self.status, Status::Complete | Status::Undone) {
            self.completed_at = Some(now);
        }
    }
    fn summary(&self, completed_at: u64, now: u64) -> retention::Summary {
        let count = |entries: Vec<&LabelBatch>| {
            entries
                .iter()
                .filter(|b| b.verified())
                .map(|b| b.intent().messages().len())
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
            applied: count(self.entries(false)),
            restored: count(self.entries(true)),
            folders: retention::folders(&self.plan.filing.changes),
        }
    }
    /// Complete, or undone with no forward effect left unknown. Observed
    /// conflicts that Undo has addressed no longer block a new run.
    pub fn finished(&self) -> bool {
        (self.status == Status::Undone
            || self.status == Status::Complete && self.undo_receipt.is_none())
            && self
                .batches
                .iter()
                .all(|b| b.all_outcomes().all(|o| o != Outcome::Uncertain))
    }
    /// See `protocol::Run::settled`.
    pub fn settled(&self) -> bool {
        self.finished() || self.closed.is_some()
    }
    /// See `protocol::Run::closable`.
    pub fn closable(&self) -> bool {
        !self.settled()
            && matches!(self.status, Status::Conflict | Status::Undone)
            && self.observed_without_uncertainty()
    }
    /// See `protocol::Run::observed_without_uncertainty`.
    pub fn observed_without_uncertainty(&self) -> bool {
        self.batches
            .iter()
            .chain(self.reverse.iter().map(|r| &r.batch))
            .all(|b| b.observed() && b.all_outcomes().all(|o| o != Outcome::Uncertain))
    }
    pub fn entries(&self, reverse: bool) -> Vec<&LabelBatch> {
        if reverse {
            self.reverse.iter().map(|r| &r.batch).collect()
        } else {
            self.batches.iter().collect()
        }
    }
    pub fn entries_mut(&mut self, reverse: bool) -> Vec<&mut LabelBatch> {
        if reverse {
            self.reverse.iter_mut().map(|r| &mut r.batch).collect()
        } else {
            self.batches.iter_mut().collect()
        }
    }
}
#[derive(Serialize)]
pub struct Preview {
    pub operation: Run,
    pub receipt: String,
    pub saved: crate::rules::workspace::ProofPreview,
    pub execution: Proof,
    /// Destinations excluded as `missing_label`. Label provisioning creates
    /// those that are not blocked; no label store creates labels.
    pub missing_labels: Vec<MissingLabel>,
    pub limitations: &'static str,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MissingLabel {
    pub name: String,
    pub messages: usize,
    /// Why creating it is refused, or none when label provisioning can.
    pub blocked: Option<crate::census::folder_provisioning::Failure>,
}
#[derive(Serialize)]
pub struct UndoPreview {
    pub run: String,
    pub reverse: Vec<Reverse>,
    pub conflicts: usize,
    pub receipt: String,
    pub limitations: &'static str,
}

/// Per-step outcomes of every run for display: run id, then batches, then
/// steps, then UID to outcome.
pub fn outcomes(journal: &Journal) -> serde_json::Value {
    let batches = |batches: Vec<&LabelBatch>| {
        batches
            .iter()
            .map(|b| {
                (0..b.steps().len())
                    .map(|i| b.outcomes(i))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    journal
        .runs
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                serde_json::json!({"forward": batches(r.entries(false)), "reverse": batches(r.entries(true))}),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

const EXCLUDED: &[&str] = &[
    "revision",
    "protocol_backlog",
    "gmail_backlog",
    "label_provisioning",
];
pub(crate) const MATERIAL_V1: &[u8] = b"mailroom/gmail-backlog-material/v1";
/// See `protocol::material`: v3 leaves out the census.
pub fn material(state: &AccountState, keys: &CensusKeys) -> Result<String, String> {
    super::protocol::material_as(
        state,
        keys,
        &[EXCLUDED, &["census"]].concat(),
        b"mailroom/gmail-backlog-material/v3",
    )
}
/// See `protocol::material_legacy`.
pub(crate) fn material_legacy(state: &AccountState, keys: &CensusKeys) -> Vec<String> {
    [
        b"mailroom/gmail-backlog-material/v2".as_slice(),
        MATERIAL_V1,
    ]
    .iter()
    .filter_map(|purpose| super::protocol::material_as(state, keys, EXCLUDED, purpose).ok())
    .collect()
}

pub fn outcome(run: &Run, reverse: bool) -> Status {
    let entries = run.entries(reverse);
    if entries.iter().any(|b| {
        b.all_outcomes()
            .any(|o| matches!(o, Outcome::Conflict | Outcome::Changed | Outcome::Unapplied))
    }) {
        Status::Conflict
    } else if entries
        .iter()
        .any(|b| b.all_outcomes().any(|o| o == Outcome::Uncertain))
    {
        Status::Uncertain
    } else if !entries.is_empty() && entries.iter().all(|b| b.verified()) {
        if reverse {
            Status::Undone
        } else {
            Status::Complete
        }
    } else {
        Status::Stopped
    }
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
            if !ids.insert(&r.id) || !valid_run(r, account) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

fn valid_run(r: &Run, account: &AccountId) -> bool {
    let filing = &r.plan.filing;
    if !digest(&r.id)
        || !digest(&r.receipt)
        || r.plan.binding.account != *account
        || filing.account != account.as_str()
        || filing.provider != crate::acquisition::Provider::Gmail
        || filing.semantics != SEMANTICS
        || filing.artifact.context().account != account.as_str()
        || filing
            .changes
            .iter()
            .any(|c| !matches!(c.operation, Operation::Gmail { .. }))
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
        || r.undo_receipt.is_none() != r.reverse.is_empty()
        || (r.undo_receipt.is_none() && r.undo_conflicts != 0)
        || r.undo_conflicts > filing.changes.len()
        || r.undo_receipt.as_ref().is_some_and(|s| !digest(s))
        || r.closed
            .as_ref()
            .is_some_and(|c| !c.valid(r.status) || !r.observed_without_uncertainty())
        || r.plan.topology.validate().is_err()
    {
        return false;
    }
    let changes: std::collections::BTreeMap<_, _> = filing
        .changes
        .iter()
        .map(|c| {
            (
                (c.source.folder.as_str(), c.source.uidvalidity, c.source.uid),
                c,
            )
        })
        .collect();
    if changes.len() != filing.changes.len() {
        return false;
    }
    let mut members = BTreeSet::new();
    let mut gmail_ids = BTreeSet::new();
    let mut count = 0;
    for b in &r.batches {
        let i = b.intent();
        if i.purpose()
            != (Purpose::Filing {
                protect_flagged: filing.options.protect_flagged,
            })
            || r.plan
                .topology
                .get(&i.context().name.decoded)
                .is_none_or(|m| m.name != i.context().name || !m.attributes.contains("\\ALL"))
            || r.plan
                .topology
                .get(&i.destination().decoded)
                .is_none_or(|m| m.name != *i.destination())
        {
            return false;
        }
        for (uid, m) in i.messages() {
            if !members.insert((i.context().uidvalidity.get(), uid.get()))
                || !gmail_ids.insert(m.gmail_id.clone())
            {
                return false;
            }
            count += 1;
            let label = &i.destination().decoded;
            let add: BTreeSet<String> = if m.labels.contains(label) {
                BTreeSet::new()
            } else {
                [label.clone()].into()
            };
            let remove: BTreeSet<String> = m
                .labels
                .iter()
                .filter(|l| l.eq_ignore_ascii_case(INBOX))
                .cloned()
                .collect();
            if !changes
                .get(&(
                    i.context().name.decoded.as_str(),
                    i.context().uidvalidity.get(),
                    uid.get(),
                ))
                .is_some_and(|c| {
                    c.source.gmail_id.as_deref() == Some(m.gmail_id.as_str())
                        && c.source.message_id_hash == m.message_id_hash
                        && c.flags == m.flags
                        && c.original_labels == m.labels
                        && c.destination == *label
                        && c.operation
                            == Operation::Gmail {
                                add: add.clone(),
                                remove: remove.clone(),
                            }
                })
            {
                return false;
            }
        }
    }
    if count != filing.changes.len() {
        return false;
    }
    let mut reversed = BTreeSet::new();
    for rev in &r.reverse {
        let Some(f) = r.batches.get(rev.forward) else {
            return false;
        };
        let i = rev.batch.intent();
        if !reversed.insert(rev.forward)
            || i.purpose() != Purpose::ConditionalUndo
            || i.context() != f.intent().context()
            || i.destination() != f.intent().destination()
        {
            return false;
        }
        let applied = |kind: StepKind| -> BTreeSet<_> {
            f.steps()
                .iter()
                .position(|s| s.kind == kind)
                .map(|at| {
                    f.outcomes(at)
                        .into_iter()
                        .filter(|(_, o)| o.applied())
                        .map(|(uid, _)| uid)
                        .collect()
                })
                .unwrap_or_default()
        };
        for (uid, m) in i.messages() {
            if f.intent()
                .messages()
                .get(uid)
                .is_none_or(|original| !original.same_message(m))
            {
                return false;
            }
        }
        for step in rev.batch.steps() {
            let forward = match step.kind {
                StepKind::RestoreInbox => StepKind::RemoveInbox,
                StepKind::RemoveLabel => StepKind::AddLabel,
                _ => return false,
            };
            let applied = applied(forward);
            if step.uids.iter().any(|u| !applied.contains(u)) {
                return false;
            }
        }
    }
    let reverse_phase = r.undo_receipt.is_some();
    !(r.status == Status::Prepared
        && (r.batches.iter().any(LabelBatch::attempted) || reverse_phase)
        || matches!(
            r.status,
            Status::Prepared | Status::Running | Status::UndoRunning
        ) && r.stop_requested
        || r.status == Status::Running && reverse_phase
        || reverse_phase && r.batches.iter().any(|b| !b.observed())
        || r.status == Status::Complete && !r.batches.iter().all(LabelBatch::verified)
        || r.status == Status::Undone
            && (r.reverse.is_empty() || !r.reverse.iter().all(|b| b.batch.verified()))
        || r.status == Status::UndoRunning && !reverse_phase
        || r.status == Status::Complete && r.reverse.iter().any(|b| b.batch.attempted())
        || r.status == Status::Uncertain && outcome(r, reverse_phase) != Status::Uncertain
        || r.status == Status::Stopped && outcome(r, reverse_phase) != Status::Stopped)
}

pub(crate) fn project(
    state: &AccountState,
    census: &crate::census::Census,
    proof: &Proof,
    options: Options,
) -> Result<Plan, String> {
    let mut plan = Plan {
        account: census.account.as_str().into(),
        provider: crate::acquisition::Provider::Gmail,
        source: "explicit-synthetic-loopback-gmail-labels; separate installed workspace evidence"
            .into(),
        state_revision: state.revision,
        ledger_version: state.ledger.source_version.clone(),
        mailbox_revision: None,
        semantics: SEMANTICS.into(),
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
