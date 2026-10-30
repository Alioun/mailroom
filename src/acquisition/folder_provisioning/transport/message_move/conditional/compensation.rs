use super::*;
use crate::backlog::imap_copy::{
    CheckpointChange, CleanupNext, CleanupSlot, ClearNext, CompensationEvidence, NextAction,
};

mod flags;
mod restore;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Execute,
    Reconcile,
}

fn running(cancel: &Cancellation) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn persist<C: CompensationCheckpoints>(
    checkpoints: &mut C,
    evidence: &CompensationEvidence,
    source: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    running(cancel)?;
    let token = checkpoints.persist_intent(evidence)?;
    running(cancel)?;
    checkpoints.consume_intent(token, source)?;
    running(cancel)
}

impl<I: Commands> Conditional<'_, I> {
    pub(super) fn observe_for_compensation(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Error> {
        self.authorize(permit)?;
        if forward.next_action().needs_observation() || !forward.observed() {
            return Err(Error::Unsupported);
        }
        let mapping = forward.copy().mapping().ok_or(Error::Unsupported)?;
        self.pair(
            forward.intent().source(),
            &forward.intent().messages().keys().copied().collect(),
            forward.intent().destination(),
            &mapping.pairs().values().copied().collect(),
        )
    }

    pub(super) fn compensate(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
        mode: Mode,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        running(cancel)?;
        let mut conflict = false;
        if let Some(restore) = evidence.restore() {
            if restore.next_action() == NextAction::Conflict {
                conflict = true;
            } else {
                self.restore(permit, evidence, cancel, checkpoints, mode)?;
            }
        }
        let slots: Vec<_> = evidence.cleanup_slots().collect();
        for slot in slots {
            loop {
                running(cancel)?;
                match evidence.cleanup_next(slot).map_err(|_| Error::Unstable)? {
                    CleanupNext::Start if mode == Mode::Execute => {
                        self.cleanup_store(permit, evidence, slot, cancel, checkpoints)?;
                        running(cancel)?;
                        self.observe_cleanup_store(permit, evidence, slot, checkpoints)?;
                    }
                    CleanupNext::Expunge if mode == Mode::Execute => {
                        self.cleanup_expunge(permit, evidence, slot, cancel, checkpoints)?;
                        running(cancel)?;
                        self.observe_cleanup_expunge(permit, evidence, slot, checkpoints)?;
                    }
                    CleanupNext::ObserveStore if mode == Mode::Reconcile => {
                        self.observe_cleanup_store(permit, evidence, slot, checkpoints)?;
                    }
                    CleanupNext::ObserveExpunge if mode == Mode::Reconcile => {
                        self.observe_cleanup_expunge(permit, evidence, slot, checkpoints)?;
                    }
                    CleanupNext::ObserveStore | CleanupNext::ObserveExpunge => {
                        return Err(Error::Unsupported);
                    }
                    CleanupNext::Conflict => {
                        conflict = true;
                        break;
                    }
                    _ => break,
                }
            }
        }
        for index in 0..evidence.preview().clear().len() {
            running(cancel)?;
            match evidence.clear_next(index).map_err(|_| Error::Unstable)? {
                ClearNext::Start if mode == Mode::Execute => {
                    self.clear_owned(permit, evidence, index, cancel, checkpoints)?;
                    running(cancel)?;
                    self.observe_clear(permit, evidence, index, checkpoints)?;
                    conflict |= evidence.clear_next(index).map_err(|_| Error::Unstable)?
                        == ClearNext::Conflict;
                }
                ClearNext::ObserveStore if mode == Mode::Reconcile => {
                    self.observe_clear(permit, evidence, index, checkpoints)?;
                }
                ClearNext::ObserveStore => return Err(Error::Unsupported),
                ClearNext::Conflict => conflict = true,
                _ => (),
            }
        }
        if conflict && mode == Mode::Execute {
            Err(Error::Unstable)
        } else {
            Ok(())
        }
    }
}

impl Connection {
    pub(in crate::acquisition) fn observe_for_compensation(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .observe_for_compensation(permit, forward)
    }

    pub(in crate::acquisition) fn execute_compensation(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .compensate(permit, evidence, cancel, checkpoints, Mode::Execute)
    }

    pub(in crate::acquisition) fn reconcile_compensation(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .compensate(permit, evidence, cancel, checkpoints, Mode::Reconcile)
    }
}
