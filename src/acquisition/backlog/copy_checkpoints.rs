use super::*;
use crate::{
    acquisition::folder_provisioning::transport::message_move::CopyCheckpoints,
    backlog::imap_copy::{CopyEvidence, NextAction},
    census::folder_provisioning::Failure,
};

#[derive(Clone, Copy)]
pub(in crate::acquisition) enum CopySlot<'a> {
    Filing { run: &'a str, batch: usize },
    Reconcile { run: &'a str, batch: usize },
}

pub(in crate::acquisition) struct SavedCopyCheckpoints<'a> {
    repo: &'a Accounts,
    account: &'a Account,
    store: &'a EncryptedStore,
    saved: &'a mut AccountState,
    run: usize,
    batch: usize,
    filing: bool,
    approval: String,
    previous: CopyEvidence,
    authorized: Option<CopyEvidence>,
    fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
}

fn approval(store: &EncryptedStore, run: &Run) -> Result<String, AcquisitionError> {
    Ok(store.keys().binding_for(
        b"mailroom/imap-copy-checkpoint-plan/v1",
        &crate::census::binding::bound(|| serde_json::to_vec(&(&run.id, &run.receipt, &run.plan)))
            .map_err(failure)?,
    ))
}

impl Accounts {
    pub(in crate::acquisition) fn copy_checkpoints<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        slot: CopySlot<'_>,
        topology: &Inventory,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<SavedCopyCheckpoints<'a>, AcquisitionError> {
        let (id, batch, filing) = match slot {
            CopySlot::Filing { run, batch } => (run, batch, true),
            CopySlot::Reconcile { run, batch } => (run, batch, false),
        };
        saved.protocol_backlog.validate(&account.id)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        let current = &saved.protocol_backlog.runs[run];
        if current.plan.filing.semantics != model::COPY_SEMANTICS
            || current.plan.topology != *topology
            || current.undo_receipt.is_some()
            || current.closed.is_some()
            || (filing && (current.status != Status::Running || current.stop_requested))
            || (!filing
                && !matches!(
                    current.status,
                    Status::Uncertain | Status::Stopped | Status::Conflict
                ))
        {
            return Err(invalid());
        }
        let previous = current
            .batches
            .get(batch)
            .and_then(model::Batch::as_copy)
            .ok_or_else(invalid)?
            .clone();
        if filing && !current.batches[batch].resumable() {
            return Err(invalid());
        }
        let approval = approval(store, current)?;
        let checkpoints = SavedCopyCheckpoints {
            repo: self,
            account,
            store,
            saved,
            run,
            batch,
            filing,
            approval,
            previous,
            authorized: None,
            fresh,
        };
        checkpoints.validate_authority()?;
        Ok(checkpoints)
    }
}

impl SavedCopyCheckpoints<'_> {
    fn validate_authority(&self) -> Result<(), AcquisitionError> {
        let run = &self.saved.protocol_backlog.runs[self.run];
        if approval(self.store, run)? != self.approval {
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

    fn same_slot(&self, evidence: &CopyEvidence) -> Result<(), Failure> {
        let current = &self.saved.protocol_backlog.runs[self.run];
        if current.batches[self.batch].as_copy() != Some(&self.previous)
            || evidence.intent() != self.previous.intent()
            || evidence.before() != self.previous.before()
            || approval(self.store, current).map_err(|_| Failure::Unstable)? != self.approval
        {
            return Err(Failure::Unstable);
        }
        evidence
            .continues(&self.previous)
            .map_err(|_| Failure::Unstable)?;
        Ok(())
    }
}

impl Accounts {
    pub(super) fn validate_copy_authority(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        binding: &Binding,
        fresh: &dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        if saved.census.account != account.id || store.recovery_pending() {
            return Err(invalid());
        }
        let durable = store.load_state()?;
        if durable.census.account != account.id || durable.revision != saved.revision {
            return Err(invalid());
        }
        self.require_ready(&account.id)?;
        if serde_json::to_vec(&self.load(&account.id)?).map_err(failure)?
            != serde_json::to_vec(account).map_err(failure)?
        {
            return Err(invalid());
        }
        let source = self.backlog_source(account)?;
        if source.profile().map_err(failure)? != Profile::Copy
            || !source.address.ip().is_loopback()
            || source.epoch.is_empty()
            || self.backlog_binding(account, store, saved)? != *binding
        {
            return Err(invalid());
        }
        fresh(saved).map_err(failure)
    }
}

impl CopyCheckpoints for SavedCopyCheckpoints<'_> {
    fn validate_intent(&mut self, evidence: &CopyEvidence) -> Result<(), Failure> {
        self.same_slot(evidence)?;
        let run = &self.saved.protocol_backlog.runs[self.run];
        if !self.filing
            || self.authorized.is_some()
            || run.status != Status::Running
            || run.stop_requested
            || run.undo_receipt.is_some()
            || run.closed.is_some()
        {
            return Err(Failure::Unsupported);
        }
        let transition = match (self.previous.next_action(), evidence.next_action()) {
            (NextAction::Copy, NextAction::ObserveCopy) => true,
            (NextAction::PrepareDeletion, NextAction::ObserveStore(i)) => {
                i == self.previous.deletions().count() && evidence.deletions().count() == i + 1
            }
            (NextAction::Expunge(a), NextAction::ObserveExpunge(b)) => a == b,
            _ => false,
        };
        if !transition {
            return Err(Failure::Unstable);
        }
        self.validate_authority().map_err(|_| Failure::Unstable)?;
        self.authorized = Some(evidence.clone());
        Ok(())
    }

    fn save(&mut self, evidence: &CopyEvidence) -> Result<(), Failure> {
        self.same_slot(evidence)?;
        if self.authorized.as_ref().is_some_and(|e| e != evidence) {
            return Err(Failure::Unstable);
        }
        let starts_effect = !self.previous.copy().attempted() && evidence.copy().attempted()
            || evidence.deletions().count() > self.previous.deletions().count()
            || self
                .previous
                .deletions()
                .zip(evidence.deletions())
                .any(|(a, b)| a.expunge_before().is_none() && b.expunge_before().is_some());
        if starts_effect && self.authorized.as_ref() != Some(evidence) {
            return Err(Failure::Unsupported);
        }
        let mut staged = self.saved.clone();
        let run = &mut staged.protocol_backlog.runs[self.run];
        run.batches[self.batch] = model::Batch::copy(evidence.clone());
        if !self.filing {
            run.status = model::outcome(run, false);
        }
        self.store
            .save_state(&mut staged)
            .map_err(|_| Failure::Limit)?;
        *self.saved = staged;
        self.previous = evidence.clone();
        self.authorized = None;
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::acquisition) mod tests;
