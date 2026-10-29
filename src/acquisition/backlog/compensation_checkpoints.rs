use super::*;
use crate::{
    backlog::imap_copy::{CheckpointChange, CompensationEvidence},
    census::folder_provisioning::Failure,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyCompensationWork {
    Execute,
    Reconcile,
}

pub struct CopyCompensationSlot<'a> {
    pub run: &'a str,
    pub forward: usize,
    pub work: CopyCompensationWork,
}

/// Proof of one saved intent, not a provider permit or native confirmation.
pub struct DurableCompensationIntent {
    account: AccountId,
    run: String,
    forward: usize,
    revision: u64,
    evidence: CompensationEvidence,
}

impl DurableCompensationIntent {
    pub fn location(&self) -> (&AccountId, &str, usize, u64) {
        (&self.account, &self.run, self.forward, self.revision)
    }
    pub fn evidence(&self) -> &CompensationEvidence {
        &self.evidence
    }
}

pub struct SavedCopyCompensationCheckpoints<'a> {
    repo: &'a Accounts,
    account: &'a Account,
    store: &'a EncryptedStore,
    saved: &'a mut AccountState,
    run: usize,
    entry: usize,
    work: CopyCompensationWork,
    approval: String,
    previous: CompensationEvidence,
    fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
}

fn approval(store: &EncryptedStore, run: &Run) -> Result<String, AcquisitionError> {
    super::dependent_cleanup::approved_dependencies(store, run)?;
    let plans: Vec<_> = run
        .compensation
        .iter()
        .map(|entry| {
            (
                entry.forward,
                entry.evidence.forward(),
                entry.evidence.preview(),
                &entry.after_clear_approvals,
            )
        })
        .collect();
    Ok(store.keys().binding_for(
        b"mailroom/imap-copy-compensation-checkpoint-plan/v1",
        &crate::census::binding::bound(|| {
            serde_json::to_vec(&(&run.id, &run.receipt, &run.plan, &run.undo_receipt, plans))
        })
        .map_err(failure)?,
    ))
}

impl Accounts {
    pub fn copy_compensation_checkpoints<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        slot: CopyCompensationSlot<'_>,
        topology: &Inventory,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<SavedCopyCompensationCheckpoints<'a>, AcquisitionError> {
        saved.protocol_backlog.validate(&account.id)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == slot.run)
            .ok_or_else(invalid)?;
        let current = &saved.protocol_backlog.runs[run];
        let entry = current
            .compensation
            .iter()
            .position(|c| c.forward == slot.forward)
            .ok_or_else(invalid)?;
        if current.plan.filing.semantics != model::COPY_SEMANTICS
            || current.plan.topology != *topology
            || current.undo_receipt.is_none()
            || current.closed.is_some()
        {
            return Err(invalid());
        }
        match slot.work {
            CopyCompensationWork::Execute
                if current.status != Status::UndoRunning || current.stop_requested =>
            {
                return Err(invalid());
            }
            CopyCompensationWork::Reconcile
                if !matches!(
                    current.status,
                    Status::Uncertain | Status::Stopped | Status::Conflict
                ) =>
            {
                return Err(invalid());
            }
            _ => (),
        }
        let approval = approval(store, current)?;
        let previous = current.compensation[entry].evidence.clone();
        let checkpoints = SavedCopyCompensationCheckpoints {
            repo: self,
            account,
            store,
            saved,
            run,
            entry,
            work: slot.work,
            approval,
            previous,
            fresh,
        };
        checkpoints.validate_authority()?;
        checkpoints.same_slot()?;
        Ok(checkpoints)
    }
}

impl SavedCopyCompensationCheckpoints<'_> {
    fn same_slot(&self) -> Result<(), AcquisitionError> {
        self.saved.protocol_backlog.validate(&self.account.id)?;
        let run = &self.saved.protocol_backlog.runs[self.run];
        if run.compensation[self.entry].evidence != self.previous
            || approval(self.store, run)? != self.approval
            || self.store.recovery_pending()
            || !self.store.checkpoint_matches(
                self.saved,
                match self.work {
                    CopyCompensationWork::Execute => crate::census::ProtocolStateView::Persisted,
                    CopyCompensationWork::Reconcile => {
                        crate::census::ProtocolStateView::Interrupted
                    }
                },
            )?
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_authority(&self) -> Result<(), AcquisitionError> {
        let run = &self.saved.protocol_backlog.runs[self.run];
        if approval(self.store, run)? != self.approval {
            return Err(invalid());
        }
        if self.work == CopyCompensationWork::Execute
            && (run.status != Status::UndoRunning
                || run.stop_requested
                || !crate::backlog::retention::undo_open(run.completed_at, self.repo.now()))
        {
            return Err(invalid());
        }
        self.repo.validate_copy_authority(
            self.account,
            self.store,
            self.saved,
            &run.plan.binding,
            self.fresh,
        )
    }

    pub fn persist_intent(
        &mut self,
        evidence: &CompensationEvidence,
    ) -> Result<DurableCompensationIntent, Failure> {
        if self.work != CopyCompensationWork::Execute
            || evidence
                .checkpoint_change(&self.previous)
                .map_err(|_| Failure::Unstable)?
                != CheckpointChange::Intent
        {
            return Err(Failure::Unsupported);
        }
        self.validate_authority().map_err(|_| Failure::Unstable)?;
        self.commit(evidence)?;
        let run = &self.saved.protocol_backlog.runs[self.run];
        Ok(DurableCompensationIntent {
            account: self.account.id.clone(),
            run: run.id.clone(),
            forward: run.compensation[self.entry].forward,
            revision: self.saved.revision,
            evidence: evidence.clone(),
        })
    }

    pub fn observe(&mut self, evidence: &CompensationEvidence) -> Result<(), Failure> {
        if evidence
            .checkpoint_change(&self.previous)
            .map_err(|_| Failure::Unstable)?
            != CheckpointChange::Observation
        {
            return Err(Failure::Unsupported);
        }
        self.commit(evidence)
    }

    fn commit(&mut self, evidence: &CompensationEvidence) -> Result<(), Failure> {
        self.same_slot().map_err(|_| Failure::Unstable)?;
        if evidence == &self.previous {
            return Ok(());
        }
        let mut staged = self.saved.clone();
        let run = &mut staged.protocol_backlog.runs[self.run];
        run.compensation[self.entry].evidence = evidence.clone();
        if self.work == CopyCompensationWork::Reconcile {
            run.status = model::outcome(run, true);
            run.stamp(self.repo.now());
        }
        self.store
            .save_state(&mut staged)
            .map_err(|_| Failure::Limit)?;
        *self.saved = staged;
        self.previous = evidence.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
