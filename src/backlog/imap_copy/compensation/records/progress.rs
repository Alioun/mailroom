use super::*;

impl CompensationEvidence {
    pub fn cleanup_next(&self, index: usize) -> Result<CleanupNext, InvalidEvidence> {
        let record = self.cleanup.get(index).ok_or(InvalidEvidence::Bounds)?;
        let Some(effect) = &record.deletion else {
            return Ok(CleanupNext::Start);
        };
        if effect.flag.result.is_none() || effect.flag.readback.is_none() {
            return Ok(CleanupNext::ObserveStore);
        }
        if let Some(expunge) = &effect.expunge {
            if expunge.completion.is_none()
                || expunge.readback.is_none()
                || record.final_retained.is_none()
            {
                return Ok(CleanupNext::ObserveExpunge);
            }
            let removed = effect.removed();
            let complete = self.preview.cleanup[index]
                .originals
                .iter()
                .all(|(uid, target)| {
                    removed.contains(target)
                        && record.final_retained.as_ref().unwrap().messages.get(uid)
                            == self.preview.cleanup[index].retained.messages.get(uid)
                });
            return Ok(if complete {
                CleanupNext::Complete
            } else {
                CleanupNext::Conflict
            });
        }
        Ok(if effect.flag.verified().is_empty() {
            CleanupNext::Conflict
        } else {
            CleanupNext::Expunge
        })
    }
    pub fn clear_next(&self, index: usize) -> Result<ClearNext, InvalidEvidence> {
        let Some(effect) = self.clear.get(index).ok_or(InvalidEvidence::Bounds)? else {
            return Ok(ClearNext::Start);
        };
        if effect.result().is_none() || effect.readback().is_none() {
            return Ok(ClearNext::ObserveStore);
        }
        Ok(
            if effect.verified().len() == effect.intent().before.messages.len() {
                ClearNext::Complete
            } else {
                ClearNext::Conflict
            },
        )
    }

    pub fn resolutions(&self) -> BTreeMap<Uid, CompensationResolution> {
        use CompensationResolution as R;
        let mut result: BTreeMap<_, _> = self
            .preview
            .conflicts
            .keys()
            .map(|u| (*u, R::Conflict))
            .collect();
        if let Some(restore) = &self.restore {
            let resolutions = restore.resolutions();
            for (original, target) in self.forward.copy.mapping().unwrap().pairs() {
                if let Some(resolution) = resolutions.get(target) {
                    result.insert(
                        *original,
                        match resolution {
                            Resolution::Filed { destination } => R::Restored {
                                destination: *destination,
                            },
                            Resolution::Unattempted => R::Unattempted,
                            Resolution::CopyOnly { .. } | Resolution::Marked { .. } => R::Pending,
                            Resolution::Uncertain => R::Uncertain,
                            Resolution::Conflict => R::Conflict,
                        },
                    );
                }
            }
        }
        for (plan, record) in self.preview.cleanup.iter().zip(&self.cleanup) {
            let verified = record
                .deletion
                .as_ref()
                .map(|e| e.flag.verified())
                .unwrap_or_default();
            let removed = record
                .deletion
                .as_ref()
                .map(Deletion::removed)
                .unwrap_or_default();
            for (original, target) in &plan.originals {
                let resolution = match &record.deletion {
                    None => R::Unattempted,
                    Some(effect)
                        if effect.flag.result.is_none() || effect.flag.readback.is_none() =>
                    {
                        R::Uncertain
                    }
                    Some(effect) => match &effect.expunge {
                        None => {
                            if verified.contains_key(target) {
                                R::Pending
                            } else {
                                R::Conflict
                            }
                        }
                        Some(expunge)
                            if expunge.completion.is_none()
                                || expunge.readback.is_none()
                                || record.final_retained.is_none() =>
                        {
                            R::Uncertain
                        }
                        Some(_) => {
                            if removed.contains(target)
                                && record
                                    .final_retained
                                    .as_ref()
                                    .unwrap()
                                    .messages
                                    .get(original)
                                    == plan.retained.messages.get(original)
                            {
                                R::ExtraCopyRemoved
                            } else {
                                R::Conflict
                            }
                        }
                    },
                };
                result.insert(*original, resolution);
            }
        }
        for (plan, effect) in self.preview.clear.iter().zip(&self.clear) {
            let verified = effect
                .as_ref()
                .map(PendingClear::verified)
                .unwrap_or_default();
            for uid in plan.clear.before.messages.keys() {
                result.insert(
                    *uid,
                    match effect {
                        None => R::Unattempted,
                        Some(e) if e.result().is_none() || e.readback().is_none() => R::Uncertain,
                        Some(_) if verified.contains_key(uid) => R::DeletedCleared,
                        Some(_) => R::Conflict,
                    },
                );
            }
        }
        result
    }

    pub fn fully_reversed(&self) -> bool {
        self.resolutions().values().all(|r| {
            matches!(
                r,
                CompensationResolution::Restored { .. } | CompensationResolution::ExtraCopyRemoved
            )
        })
    }
}
