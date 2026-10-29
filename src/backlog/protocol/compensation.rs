use super::*;
use crate::backlog::imap_copy::{CompensationEvidence, CompensationResolution as R};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compensation {
    pub forward: usize,
    pub evidence: CompensationEvidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_clear_approvals: Vec<DependentCleanupApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependentCleanupApproval {
    pub clear: usize,
    pub receipt: String,
    pub binding: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CompensationCounts {
    pub restored: usize,
    pub copies_removed: usize,
    pub deleted_cleared: usize,
    pub pending: usize,
    pub uncertain: usize,
    pub conflicts: usize,
}

impl Run {
    pub fn compensation_counts(&self) -> CompensationCounts {
        let mut counts = CompensationCounts::default();
        for resolution in self
            .compensation
            .iter()
            .flat_map(|c| c.evidence.resolutions().into_values())
        {
            match resolution {
                R::Restored { .. } => counts.restored += 1,
                R::ExtraCopyRemoved => counts.copies_removed += 1,
                R::DeletedCleared => counts.deleted_cleared += 1,
                R::Unattempted | R::Pending => counts.pending += 1,
                R::Uncertain => counts.uncertain += 1,
                R::Conflict => counts.conflicts += 1,
            }
        }
        counts
    }

    pub fn history_view(&self) -> serde_json::Value {
        let Run {
            id,
            receipt,
            plan,
            status,
            stop_requested,
            batches,
            reverse,
            compensation,
            undo_receipt,
            undo_conflicts,
            completed_at,
            closed,
        } = self;
        let mut value = serde_json::json!({
            "id": id, "receipt": receipt, "plan": plan, "status": status,
            "stop_requested": stop_requested, "batches": batches, "reverse": reverse,
            "undo_receipt": undo_receipt, "undo_conflicts": undo_conflicts,
        });
        if !compensation.is_empty() {
            value["compensation"] = serde_json::json!(self.compensation_counts());
        }
        if let Some(at) = completed_at {
            value["completed_at"] = serde_json::json!(at);
        }
        if let Some(closure) = closed {
            value["closed"] = serde_json::json!(closure);
        }
        value
    }
}

impl Journal {
    pub fn history_view(&self) -> serde_json::Value {
        let Journal { runs, summaries } = self;
        let mut value = serde_json::json!({
            "runs": runs.iter().map(Run::history_view).collect::<Vec<_>>()
        });
        if !summaries.is_empty() {
            value["summaries"] = serde_json::json!(summaries);
        }
        value
    }
}

pub(super) fn outcome(run: &Run) -> Status {
    let counts = run.compensation_counts();
    if counts.uncertain > 0 {
        Status::Uncertain
    } else if counts.conflicts > 0 {
        Status::Conflict
    } else if run.batches.iter().enumerate().any(|(index, batch)| {
        batch.attempted() && !run.compensation.iter().any(|c| c.forward == index)
    }) {
        Status::Conflict
    } else if !run.compensation.is_empty()
        && run.compensation.iter().all(|c| c.evidence.fully_reversed())
    {
        Status::Undone
    } else {
        Status::Stopped
    }
}

pub(super) fn validate(run: &Run) -> Result<(), CensusError> {
    let mut indices = BTreeSet::new();
    for entry in &run.compensation {
        if !indices.insert(entry.forward)
            || run.batches.get(entry.forward).and_then(Batch::as_copy)
                != Some(entry.evidence.forward())
        {
            return Err(CensusError::Storage);
        }
        let mut clears = BTreeSet::new();
        for approval in &entry.after_clear_approvals {
            if !clears.insert(approval.clear)
                || entry.evidence.dependent_plan(approval.clear).is_none()
                || !digest(&approval.receipt)
                || !digest(&approval.binding)
            {
                return Err(CensusError::Storage);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
