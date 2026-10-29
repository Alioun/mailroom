use super::*;

struct Restoration<'a, C> {
    evidence: &'a mut CompensationEvidence,
    checkpoints: &'a mut C,
    source: String,
    cancel: &'a Cancellation,
}

impl<C: CompensationCheckpoints> CopyCheckpoints for Restoration<'_, C> {
    fn validate_intent(&mut self, restore: &CopyEvidence) -> Result<(), Error> {
        running(self.cancel)?;
        let mut next = self.evidence.clone();
        next.record_restore(restore.clone())
            .map_err(|_| Error::Unstable)?;
        if next
            .checkpoint_change(self.evidence)
            .map_err(|_| Error::Unstable)?
            != CheckpointChange::Intent
        {
            return Err(Error::Unsupported);
        }
        Ok(())
    }

    fn save(&mut self, restore: &CopyEvidence) -> Result<(), Error> {
        let mut next = self.evidence.clone();
        next.record_restore(restore.clone())
            .map_err(|_| Error::Unstable)?;
        match next
            .checkpoint_change(self.evidence)
            .map_err(|_| Error::Unstable)?
        {
            CheckpointChange::Intent => {
                persist(self.checkpoints, &next, &self.source, self.cancel)?;
            }
            CheckpointChange::Observation => self.checkpoints.observe(&next)?,
        }
        *self.evidence = next;
        Ok(())
    }
}

impl<I: Commands> Conditional<'_, I> {
    pub(super) fn restore(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
        mode: Mode,
    ) -> Result<(), Error> {
        let mut restore = evidence.restore().ok_or(Error::Unsupported)?.clone();
        let mut adapter = Restoration {
            evidence,
            checkpoints,
            source: self.io.source().to_owned(),
            cancel,
        };
        match mode {
            Mode::Execute => self.file(permit, &mut restore, cancel, &mut adapter),
            Mode::Reconcile => self.reconcile(permit, &mut restore, cancel, &mut adapter),
        }
    }
}
