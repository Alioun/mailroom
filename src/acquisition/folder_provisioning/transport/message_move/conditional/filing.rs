use super::*;
use crate::backlog::imap_copy::NextAction;

fn cancelled(cancel: &Cancellation) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn guarded<'a>(
    checkpoints: &'a mut impl CopyCheckpoints,
    cancel: &'a Cancellation,
) -> impl FnMut(&CopyEvidence) -> Result<(), Error> + 'a {
    let mut first = true;
    move |evidence| {
        let intent = std::mem::take(&mut first);
        if intent {
            cancelled(cancel)?;
            checkpoints.validate_intent(evidence)?;
        }
        checkpoints.save(evidence)?;
        if intent {
            cancelled(cancel)?;
        }
        Ok(())
    }
}

impl<I: Commands> Conditional<'_, I> {
    pub(super) fn file(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        loop {
            cancelled(cancel)?;
            match evidence.next_action() {
                NextAction::Copy => {
                    self.copy(permit, evidence, guarded(checkpoints, cancel))?;
                    cancelled(cancel)?;
                    self.observe_copy(permit, evidence, |e| checkpoints.save(e))?;
                }
                NextAction::PrepareDeletion => {
                    let prepared = self
                        .pending_deletions(evidence)?
                        .into_iter()
                        .next()
                        .ok_or(Error::Unstable)?;
                    let index =
                        self.store(permit, evidence, prepared, guarded(checkpoints, cancel))?;
                    cancelled(cancel)?;
                    self.observe_store(permit, evidence, index, |e| checkpoints.save(e))?;
                }
                NextAction::Expunge(index) => {
                    self.expunge(permit, evidence, index, guarded(checkpoints, cancel))?;
                    cancelled(cancel)?;
                    self.observe_expunge(permit, evidence, index, |e| checkpoints.save(e))?;
                }
                NextAction::ObserveCopy
                | NextAction::ObserveStore(_)
                | NextAction::ObserveExpunge(_) => return Err(Error::Unsupported),
                NextAction::Complete => return Ok(()),
                NextAction::Conflict => return Err(Error::Unstable),
            }
        }
    }

    pub(super) fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        loop {
            cancelled(cancel)?;
            match evidence.next_action() {
                NextAction::ObserveCopy => {
                    self.observe_copy(permit, evidence, |e| checkpoints.save(e))?
                }
                NextAction::ObserveStore(index) => {
                    self.observe_store(permit, evidence, index, |e| checkpoints.save(e))?
                }
                NextAction::ObserveExpunge(index) => {
                    self.observe_expunge(permit, evidence, index, |e| checkpoints.save(e))?
                }
                _ => return Ok(()),
            }
        }
    }
}

impl Connection {
    pub(in crate::acquisition) fn file_copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .file(permit, evidence, cancel, checkpoints)
    }

    pub(in crate::acquisition) fn reconcile_copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .reconcile(permit, evidence, cancel, checkpoints)
    }
}
