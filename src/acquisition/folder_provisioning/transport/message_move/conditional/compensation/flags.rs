use super::*;

impl<I: Commands> Conditional<'_, I> {
    fn one(&mut self, before: &Readback) -> Result<Readback, Error> {
        self.readbacks(&[(
            before.identity(),
            &before.messages().keys().copied().collect(),
        )])?
        .pop()
        .ok_or(Error::Protocol)?
    }

    fn flag_effect(
        &mut self,
        intent: &FlagIntent,
        command: String,
        mut record: impl FnMut(StoreResult) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut reply = StoreReply::new(intent);
        let mut completed = false;
        let result = self.exchange(command.into_bytes(), |row, tagged| {
            if let Some(result) = reply.observe(row, tagged)? {
                completed = true;
                record(result)?;
            }
            Ok(())
        });
        if !completed {
            record(match &result {
                Err(
                    Error::Protocol
                    | Error::UnknownCode
                    | Error::Unsupported
                    | Error::Unstable
                    | Error::Limit,
                ) => StoreResult::Rejected,
                _ => StoreResult::Lost,
            })?;
        }
        result
    }

    pub(super) fn cleanup_store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        slot: CleanupSlot,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let plan = evidence
            .cleanup_plan(slot)
            .ok_or(Error::Unsupported)?
            .clone();
        let retained = plan.retained();
        let before = plan.deletion().before();
        let (source, destination) = self.pair(
            retained.identity(),
            &retained.messages().keys().copied().collect(),
            before.identity(),
            &before.messages().keys().copied().collect(),
        )?;
        self.select_source(&destination)?;
        let command = evidence
            .begin_cleanup(slot, &source, &destination)
            .map_err(|_| Error::Unstable)?;
        persist(checkpoints, evidence, self.io.source(), cancel)?;
        self.flag_effect(plan.deletion(), command, |result| {
            evidence
                .record_cleanup_store(slot, result)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)
        })
    }

    pub(super) fn observe_cleanup_store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        slot: CleanupSlot,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.cleanup(slot).ok_or(Error::Unsupported)?;
        if deletion.expunge_before().is_some() {
            return Err(Error::Unsupported);
        }
        let before = deletion.intent().before().clone();
        if deletion.flag().result().is_none() {
            evidence
                .record_cleanup_store(slot, StoreResult::Lost)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)?;
        }
        let found = self.one(&before)?;
        evidence
            .observe_cleanup_store(slot, found)
            .map_err(|_| Error::Unstable)?;
        checkpoints.observe(evidence)
    }

    pub(super) fn cleanup_expunge(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        slot: CleanupSlot,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.cleanup(slot).ok_or(Error::Unsupported)?;
        let owned = deletion.flag().verified();
        if deletion.expunge_before().is_some() || owned.is_empty() {
            return Err(Error::Unsupported);
        }
        let plan = evidence
            .cleanup_plan(slot)
            .ok_or(Error::Unsupported)?
            .clone();
        let originals = plan
            .originals()
            .iter()
            .filter(|(_, target)| owned.contains_key(target))
            .map(|(uid, _)| *uid)
            .collect();
        let (source, destination) = self.pair(
            plan.retained().identity(),
            &originals,
            plan.deletion().before().identity(),
            &owned.keys().copied().collect(),
        )?;
        self.select_source(&destination)?;
        let command = evidence
            .begin_cleanup_expunge(slot, source, destination)
            .map_err(|_| Error::Unstable)?;
        persist(checkpoints, evidence, self.io.source(), cancel)?;
        let mut completed = false;
        let result = self.exchange(command.into_bytes(), |row, tagged| {
            if !tagged {
                return if matches!(
                    row.get(1).and_then(Token::atom),
                    Some("OK" | "NO" | "BAD" | "BYE")
                ) {
                    Session::status(row, 1)
                } else {
                    Ok(())
                };
            }
            let status = match row.get(1).and_then(Token::atom) {
                Some("OK") => TaggedStatus::Ok,
                Some("NO") => TaggedStatus::No,
                Some("BAD") => TaggedStatus::Bad,
                _ => return Err(Error::Protocol),
            };
            if row.get(2).is_some() {
                return Err(Error::UnknownCode);
            }
            completed = true;
            evidence
                .record_cleanup_expunge(slot, Completion::Tagged { status, code: None })
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)
        });
        if !completed {
            evidence
                .record_cleanup_expunge(slot, Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)?;
        }
        result
    }

    pub(super) fn observe_cleanup_expunge(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        slot: CleanupSlot,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.cleanup(slot).ok_or(Error::Unsupported)?;
        let before = deletion.expunge_before().ok_or(Error::Unsupported)?.clone();
        if deletion.expunge_completion().is_none() {
            evidence
                .record_cleanup_expunge(slot, Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)?;
        }
        let plan = evidence.cleanup_plan(slot).ok_or(Error::Unsupported)?;
        let originals = plan
            .originals()
            .iter()
            .filter(|(_, target)| before.messages().contains_key(target))
            .map(|(uid, _)| *uid)
            .collect();
        let (source, destination) = self.pair(
            plan.retained().identity(),
            &originals,
            before.identity(),
            &before.messages().keys().copied().collect(),
        )?;
        evidence
            .observe_cleanup_final(slot, source, destination)
            .map_err(|_| Error::Unstable)?;
        checkpoints.observe(evidence)
    }

    pub(super) fn clear_owned(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        index: usize,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let intent = evidence
            .preview()
            .clear()
            .get(index)
            .ok_or(Error::Unsupported)?
            .clear()
            .clone();
        let fresh = self.one(intent.before())?;
        self.select_source(&fresh)?;
        let command = evidence
            .begin_clear(index, fresh)
            .map_err(|_| Error::Unstable)?;
        persist(checkpoints, evidence, self.io.source(), cancel)?;
        self.flag_effect(&intent, command, |result| {
            evidence
                .record_clear(index, result)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)
        })
    }

    pub(super) fn observe_clear(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        index: usize,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let clear = evidence.clear(index).ok_or(Error::Unsupported)?;
        let before = clear.intent().before().clone();
        if clear.result().is_none() {
            evidence
                .record_clear(index, StoreResult::Lost)
                .map_err(|_| Error::Protocol)?;
            checkpoints.observe(evidence)?;
        }
        let source = self.one(&before)?;
        evidence
            .observe_clear(index, source)
            .map_err(|_| Error::Unstable)?;
        checkpoints.observe(evidence)
    }
}
