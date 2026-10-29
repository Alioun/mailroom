use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointChange {
    Observation,
    Intent,
}

fn started(count: &mut usize, pending: bool) -> Result<(), InvalidEvidence> {
    if !pending {
        return Err(InvalidEvidence::Phase);
    }
    *count += 1;
    Ok(())
}

fn deletion_starts(
    previous: Option<&Deletion>,
    current: &Deletion,
    count: &mut usize,
) -> Result<(), InvalidEvidence> {
    if previous.is_none() {
        started(
            count,
            current.flag.result.is_none() && current.flag.readback.is_none(),
        )?;
    }
    if let Some(expunge) = &current.expunge {
        if previous.is_none_or(|old| old.expunge.is_none()) {
            started(
                count,
                expunge.completion.is_none() && expunge.readback.is_none(),
            )?;
        }
    }
    Ok(())
}

impl CompensationEvidence {
    pub fn has_dependent_cleanup(&self) -> bool {
        !self.after_clear.is_empty()
    }

    pub fn checkpoint_change(&self, previous: &Self) -> Result<CheckpointChange, InvalidEvidence> {
        self.continues(previous)?;
        if self.after_clear.len() != previous.after_clear.len()
            || self
                .after_clear
                .iter()
                .zip(&previous.after_clear)
                .any(|(a, b)| {
                    a.clear != b.clear
                        || a.source != b.source
                        || a.destination != b.destination
                        || a.plans != b.plans
                        || a.conflicts != b.conflicts
                })
        {
            return Err(InvalidEvidence::Identity);
        }
        let mut intents = 0;
        if let (Some(current), Some(old)) = (&self.restore, &previous.restore) {
            if !old.copy().attempted() && current.copy().attempted() {
                started(
                    &mut intents,
                    current.copy().completion().is_none()
                        && current.copy().mapping().is_none()
                        && current.readback.is_none(),
                )?;
            }
            let prior: Vec<_> = old.deletions().collect();
            for (index, deletion) in current.deletions().enumerate() {
                deletion_starts(prior.get(index).copied(), deletion, &mut intents)?;
            }
        }
        for slot in self.cleanup_slots() {
            if let Some(current) = self.cleanup(slot) {
                deletion_starts(previous.cleanup(slot), current, &mut intents)?;
            }
        }
        for (current, old) in self.clear.iter().zip(&previous.clear) {
            if let (Some(current), None) = (current, old) {
                started(
                    &mut intents,
                    current.result().is_none() && current.readback().is_none(),
                )?;
            }
        }
        match intents {
            0 => Ok(CheckpointChange::Observation),
            1 => Ok(CheckpointChange::Intent),
            _ => Err(InvalidEvidence::Phase),
        }
    }
}
