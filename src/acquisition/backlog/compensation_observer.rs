use super::compensation_review::{
    CopyCompensationObservation, CopyCompensationRequest, CopyCompensationReview,
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
            if !batch.attempted() {
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
        let request = CopyCompensationRequest {
            run: id.into(),
            topology,
            observations,
        };
        let review = self.review_copy_compensation(account, store, saved, &request, fresh)?;
        Ok(ObservedCopyCompensation { request, review })
    }
}
