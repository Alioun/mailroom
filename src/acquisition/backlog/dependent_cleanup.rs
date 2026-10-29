use super::compensation_review::CopyCompensationActions;
use super::*;
use crate::backlog::{
    imap_copy::{CompensationEvidence, Readback},
    protocol::{Compensation, DependentCleanupApproval},
};
use serde::Serialize;

const REVIEW: &[u8] = b"mailroom/imap-copy-dependent-cleanup-review/v1";
const PLAN: &[u8] = b"mailroom/imap-copy-dependent-cleanup-plan/v1";

#[derive(Clone, Serialize)]
pub struct DependentCleanupRequest {
    pub run: String,
    pub forward: usize,
    pub clear: usize,
    pub topology: Inventory,
    pub source: Readback,
    pub destination: Readback,
}

#[derive(Serialize)]
pub struct DependentCleanupReview {
    receipt: String,
    actions: CopyCompensationActions,
    evidence: CompensationEvidence,
}

impl DependentCleanupReview {
    pub fn receipt(&self) -> &str {
        &self.receipt
    }
    pub fn actions(&self) -> CopyCompensationActions {
        self.actions
    }
    pub fn evidence(&self) -> &CompensationEvidence {
        &self.evidence
    }
}

fn plan_binding(
    store: &EncryptedStore,
    run: &Run,
    entry: &Compensation,
    clear: usize,
    receipt: &str,
) -> Result<String, AcquisitionError> {
    let plan = entry.evidence.dependent_plan(clear).ok_or_else(invalid)?;
    Ok(store.keys().binding_for(
        PLAN,
        &crate::census::binding::bound(|| {
            serde_json::to_vec(&(
                receipt,
                &run.id,
                &run.receipt,
                &run.undo_receipt,
                &run.plan,
                entry.forward,
                entry.evidence.forward(),
                entry.evidence.preview(),
                plan,
            ))
        })
        .map_err(failure)?,
    ))
}

pub(super) fn approved_dependencies(
    store: &EncryptedStore,
    run: &Run,
) -> Result<(), AcquisitionError> {
    for entry in &run.compensation {
        if entry.evidence.dependent_clears().count() != entry.after_clear_approvals.len() {
            return Err(invalid());
        }
        for approval in &entry.after_clear_approvals {
            let expected = plan_binding(store, run, entry, approval.clear, &approval.receipt)?;
            if !bool::from(expected.as_bytes().ct_eq(approval.binding.as_bytes())) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

impl Accounts {
    /// Reviews new local cleanup plans without executing them or loading credentials.
    pub fn review_dependent_cleanup(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        request: &DependentCleanupRequest,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<DependentCleanupReview, AcquisitionError> {
        saved.protocol_backlog.validate(&account.id)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .find(|run| run.id == request.run)
            .ok_or_else(invalid)?;
        if !matches!(run.status, Status::Stopped | Status::Conflict)
            || run.plan.filing.semantics != model::COPY_SEMANTICS
            || run.plan.topology != request.topology
            || run.undo_receipt.is_none()
            || run.closed.is_some()
            || model::outcome(run, true) == Status::Uncertain
            || !crate::backlog::retention::undo_open(run.completed_at, self.now())
        {
            return Err(invalid());
        }
        self.validate_copy_authority(account, store, saved, &run.plan.binding, &fresh)?;
        if !store.checkpoint_matches(saved, crate::census::ProtocolStateView::Persisted)? {
            return Err(invalid());
        }
        approved_dependencies(store, run)?;
        let entry = run
            .compensation
            .iter()
            .find(|entry| entry.forward == request.forward)
            .ok_or_else(invalid)?;
        let mut evidence = entry.evidence.clone();
        evidence
            .prepare_after_clear(
                request.clear,
                request.source.clone(),
                request.destination.clone(),
            )
            .map_err(failure)?;
        let plan = evidence.dependent_plan(request.clear).ok_or_else(invalid)?;
        let actions = CopyCompensationActions {
            remove_copy: plan.plans().iter().map(|plan| plan.originals().len()).sum(),
            conflicts: plan.conflicts().len(),
            ..Default::default()
        };
        let receipt = store.keys().binding_for(
            REVIEW,
            &crate::census::binding::bound(|| {
                serde_json::to_vec(&(saved, request, &evidence, actions))
            })
            .map_err(failure)?,
        );
        Ok(DependentCleanupReview {
            receipt,
            actions,
            evidence,
        })
    }

    /// Saves only the confirmed dependency and its approval. It changes no mail.
    pub fn confirm_dependent_cleanup(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        request: &DependentCleanupRequest,
        receipt: &str,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let review = self.review_dependent_cleanup(account, store, saved, request, fresh)?;
        if review.actions.remove_copy == 0
            || !bool::from(review.receipt.as_bytes().ct_eq(receipt.as_bytes()))
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
        let index = run
            .compensation
            .iter()
            .position(|entry| entry.forward == request.forward)
            .ok_or_else(invalid)?;
        run.compensation[index].evidence = review.evidence;
        let binding = plan_binding(
            store,
            run,
            &run.compensation[index],
            request.clear,
            &review.receipt,
        )?;
        run.compensation[index]
            .after_clear_approvals
            .push(DependentCleanupApproval {
                clear: request.clear,
                receipt: review.receipt,
                binding,
            });
        run.status = model::outcome(run, true);
        run.stop_requested = true;
        run.stamp(self.now());
        store.save_state(&mut staged)?;
        *saved = staged;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
