use super::*;
use crate::backlog::{
    imap_copy::{CompensationEvidence, Readback},
    protocol::Compensation,
};
use serde::Serialize;

const PURPOSE: &[u8] = b"mailroom/imap-copy-compensation-approval/v1";

#[derive(Clone, Serialize)]
pub struct CopyCompensationObservation {
    pub forward: usize,
    pub source: Readback,
    pub destination: Readback,
}

/// Local evidence input. These observations do not establish provider authority.
#[derive(Clone, Serialize)]
pub struct CopyCompensationRequest {
    pub run: String,
    pub topology: Inventory,
    pub observations: Vec<CopyCompensationObservation>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CopyCompensationActions {
    pub restore: usize,
    pub remove_copy: usize,
    pub clear_deleted: usize,
    pub conflicts: usize,
}

#[derive(Serialize)]
pub struct CopyCompensationReview {
    account: AccountId,
    revision: u64,
    run: String,
    receipt: String,
    actions: CopyCompensationActions,
    compensation: Vec<Compensation>,
}

impl CopyCompensationReview {
    pub fn receipt(&self) -> &str {
        &self.receipt
    }
    pub fn actions(&self) -> CopyCompensationActions {
        self.actions
    }
    pub fn compensation(&self) -> &[Compensation] {
        &self.compensation
    }
}

impl Accounts {
    /// Derives a local exact review. It neither connects nor loads a password.
    pub fn review_copy_compensation(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        request: &CopyCompensationRequest,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<CopyCompensationReview, AcquisitionError> {
        saved.protocol_backlog.validate(&account.id)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .find(|run| run.id == request.run)
            .ok_or_else(invalid)?;
        if !matches!(
            run.status,
            Status::Complete | Status::Stopped | Status::Conflict
        ) || run.plan.filing.semantics != model::COPY_SEMANTICS
            || run.batches.iter().any(|batch| !batch.is_copy())
            || run.undo_receipt.is_some()
            || run.closed.is_some()
            || request.topology != run.plan.topology
            || request.observations.is_empty()
            || request.observations.len() > model::BATCHES
            || !crate::backlog::retention::undo_open(run.completed_at, self.now())
        {
            return Err(invalid());
        }
        self.validate_copy_authority(account, store, saved, &run.plan.binding, &fresh)?;
        let durable = store.load_state()?;
        let encode = |value: &AccountState| {
            crate::census::binding::bound(|| serde_json::to_vec(value)).map_err(failure)
        };
        if encode(saved)? != encode(&durable)? {
            return Err(invalid());
        }
        let attempted: Vec<_> = run
            .batches
            .iter()
            .enumerate()
            .filter(|(_, batch)| batch.attempted())
            .collect();
        if attempted.len() != request.observations.len() {
            return Err(invalid());
        }
        let mut compensation = Vec::with_capacity(attempted.len());
        let mut actions = CopyCompensationActions::default();
        for ((forward, batch), observation) in attempted.iter().zip(&request.observations) {
            if *forward != observation.forward {
                return Err(invalid());
            }
            let evidence = batch.as_copy().ok_or_else(invalid)?;
            let preview = evidence
                .compensation_preview(observation.source.clone(), observation.destination.clone())
                .map_err(failure)?;
            actions.restore += preview.restore().map_or(0, |e| e.intent().messages().len());
            actions.remove_copy += preview
                .cleanup()
                .iter()
                .map(|p| p.originals().len())
                .sum::<usize>();
            actions.clear_deleted += preview
                .clear()
                .iter()
                .map(|p| p.clear().before().messages().len())
                .sum::<usize>();
            actions.conflicts += preview.conflicts().len();
            compensation.push(Compensation {
                forward: *forward,
                evidence: CompensationEvidence::new(evidence.clone(), preview).map_err(failure)?,
            });
        }
        let receipt = store.keys().binding_for(
            PURPOSE,
            &crate::census::binding::bound(|| {
                serde_json::to_vec(&(saved, request, &compensation, actions))
            })
            .map_err(failure)?,
        );
        Ok(CopyCompensationReview {
            account: account.id.clone(),
            revision: saved.revision,
            run: request.run.clone(),
            receipt,
            actions,
            compensation,
        })
    }

    /// Persists the exact initial compensation intent. It changes no mail.
    pub fn confirm_copy_compensation(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        request: &CopyCompensationRequest,
        receipt: &str,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let review = self.review_copy_compensation(account, store, saved, request, fresh)?;
        if !bool::from(review.receipt.as_bytes().ct_eq(receipt.as_bytes()))
            || review.actions.restore + review.actions.remove_copy + review.actions.clear_deleted
                == 0
        {
            return Err(invalid());
        }
        let mut staged = saved.clone();
        let run = staged
            .protocol_backlog
            .runs
            .iter_mut()
            .find(|run| run.id == request.run)
            .ok_or_else(invalid)?;
        run.compensation = review.compensation;
        run.undo_receipt = Some(review.receipt);
        run.undo_conflicts = review.actions.conflicts;
        run.status = model::outcome(run, true);
        staged.protocol_backlog.validate(&account.id)?;
        store.save_state(&mut staged)?;
        *saved = staged;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
