use super::compensation_review::{
    CopyCompensationObservation, CopyCompensationRequest, CopyCompensationReview,
};
use super::dependent_cleanup::{
    DependentCleanupRequest, DependentCleanupReview, DependentCleanupTarget,
};
use super::*;
use crate::{
    acquisition::folder_provisioning::transport::message_move::BacklogPermit,
    backlog::imap_copy::{CopyEvidence, Readback},
    census::folder_provisioning::Failure,
};

pub(in crate::acquisition) trait CompensationReadTransport {
    fn inventory(&mut self) -> Result<Inventory, Failure>;
    fn observe(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Failure>;
}

impl CompensationReadTransport for Connection {
    fn inventory(&mut self) -> Result<Inventory, Failure> {
        Connection::inventory(self)
    }

    fn observe(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Failure> {
        self.observe_for_compensation(permit, forward)
    }
}

pub(in crate::acquisition) struct ObservedCopyCompensation {
    request: CopyCompensationRequest,
    review: CopyCompensationReview,
}

pub(in crate::acquisition) struct ObservedDependentCleanup {
    request: DependentCleanupRequest,
    review: DependentCleanupReview,
}

impl ObservedDependentCleanup {
    pub(in crate::acquisition) fn review(&self) -> &DependentCleanupReview {
        &self.review
    }

    pub(in crate::acquisition) fn confirm(
        self,
        repo: &Accounts,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        receipt: &str,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        if !bool::from(self.review.receipt().as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        repo.confirm_dependent_cleanup(account, store, saved, &self.request, receipt, fresh)
    }
}

impl ObservedCopyCompensation {
    pub(in crate::acquisition) fn review(&self) -> &CopyCompensationReview {
        &self.review
    }

    pub(in crate::acquisition) fn confirm(
        self,
        repo: &Accounts,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        receipt: &str,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        if !bool::from(self.review.receipt().as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        repo.confirm_copy_compensation(account, store, saved, &self.request, receipt, fresh)
    }
}

impl Accounts {
    pub(in crate::acquisition) fn observe_copy_compensation(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        id: &str,
        transport: &mut impl CompensationReadTransport,
        permit: &BacklogPermit<'_>,
        cancel: &Cancellation,
        fresh: &dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<ObservedCopyCompensation, AcquisitionError> {
        let run = self.copy_compensation_admit(account, store, saved, id, fresh)?;
        let request = collect_observations(run, None, transport, permit, cancel)?;
        let review = self.review_copy_compensation(account, store, saved, &request, fresh)?;
        Ok(ObservedCopyCompensation { request, review })
    }

    pub(in crate::acquisition) fn observe_dependent_cleanup(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        target: DependentCleanupTarget<'_>,
        transport: &mut impl CompensationReadTransport,
        permit: &BacklogPermit<'_>,
        cancel: &Cancellation,
        fresh: &dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<ObservedDependentCleanup, AcquisitionError> {
        let (run, _) = self.dependent_cleanup_admit(account, store, saved, &target, fresh)?;
        let mut observations =
            collect_observations(run, Some(target.forward), transport, permit, cancel)?;
        let observation = observations.observations.pop().ok_or_else(invalid)?;
        let request = DependentCleanupRequest {
            run: observations.run,
            forward: observation.forward,
            clear: target.clear,
            topology: observations.topology,
            source: observation.source,
            destination: observation.destination,
        };
        let review = self.review_dependent_cleanup(account, store, saved, &request, fresh)?;
        Ok(ObservedDependentCleanup { request, review })
    }
}

fn collect_observations(
    run: &Run,
    only: Option<usize>,
    transport: &mut impl CompensationReadTransport,
    permit: &BacklogPermit<'_>,
    cancel: &Cancellation,
) -> Result<CopyCompensationRequest, AcquisitionError> {
    if permit.source().identity() != run.plan.binding.source {
        return Err(invalid());
    }
    let running = || {
        if cancel.is_cancelled() {
            Err(AcquisitionError::Cancelled)
        } else {
            Ok(())
        }
    };
    running()?;
    let topology = transport.inventory().map_err(failure)?;
    if topology != run.plan.topology {
        return Err(invalid());
    }
    let mut observations = Vec::new();
    for (forward, batch) in run.batches.iter().enumerate() {
        if !batch.attempted() || only.is_some_and(|selected| selected != forward) {
            continue;
        }
        running()?;
        let (source, destination) = transport
            .observe(permit, batch.as_copy().ok_or_else(invalid)?)
            .map_err(|error| {
                if error == Failure::Cancelled {
                    AcquisitionError::Cancelled
                } else {
                    failure(error)
                }
            })?;
        observations.push(CopyCompensationObservation {
            forward,
            source,
            destination,
        });
    }
    running()?;
    if transport.inventory().map_err(failure)? != topology {
        return Err(invalid());
    }
    running()?;
    Ok(CopyCompensationRequest {
        run: run.id.clone(),
        topology,
        observations,
    })
}
