use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct AfterClear {
    pub(super) clear: usize,
    pub(super) source: Readback,
    pub(super) destination: Readback,
    pub(super) plans: Vec<ExtraCopyPlan>,
    pub(super) conflicts: BTreeSet<Uid>,
    pub(super) cleanup: Vec<CleanupRecord>,
}

impl CompensationEvidence {
    pub fn cleanup_slots(&self) -> impl Iterator<Item = CleanupSlot> + '_ {
        (0..self.cleanup.len())
            .map(CleanupSlot::Initial)
            .chain(self.after_clear.iter().flat_map(|entry| {
                (0..entry.plans.len()).map(move |group| CleanupSlot::AfterClear {
                    clear: entry.clear,
                    group,
                })
            }))
    }

    pub fn cleanup_plan(&self, slot: CleanupSlot) -> Option<&ExtraCopyPlan> {
        self.cleanup_entry(slot).ok().map(|(plan, _)| plan)
    }

    pub(super) fn cleanup_entry(
        &self,
        slot: CleanupSlot,
    ) -> Result<(&ExtraCopyPlan, &CleanupRecord), InvalidEvidence> {
        let (plans, records) = match slot {
            CleanupSlot::Initial(_) => (&self.preview.cleanup, &self.cleanup),
            CleanupSlot::AfterClear { clear, .. } => {
                let entry = self
                    .after_clear
                    .iter()
                    .find(|e| e.clear == clear)
                    .ok_or(InvalidEvidence::Bounds)?;
                (&entry.plans, &entry.cleanup)
            }
        };
        let index = match slot {
            CleanupSlot::Initial(index) => index,
            CleanupSlot::AfterClear { group, .. } => group,
        };
        Ok((
            plans.get(index).ok_or(InvalidEvidence::Bounds)?,
            records.get(index).ok_or(InvalidEvidence::Bounds)?,
        ))
    }

    pub(super) fn cleanup_entry_mut(
        &mut self,
        slot: CleanupSlot,
    ) -> Result<(&ExtraCopyPlan, &mut CleanupRecord), InvalidEvidence> {
        let (plans, records) = match slot {
            CleanupSlot::Initial(_) => (&self.preview.cleanup, &mut self.cleanup),
            CleanupSlot::AfterClear { clear, .. } => {
                let entry = self
                    .after_clear
                    .iter_mut()
                    .find(|e| e.clear == clear)
                    .ok_or(InvalidEvidence::Bounds)?;
                (&entry.plans, &mut entry.cleanup)
            }
        };
        let index = match slot {
            CleanupSlot::Initial(index) => index,
            CleanupSlot::AfterClear { group, .. } => group,
        };
        Ok((
            plans.get(index).ok_or(InvalidEvidence::Bounds)?,
            records.get_mut(index).ok_or(InvalidEvidence::Bounds)?,
        ))
    }

    pub fn prepare_after_clear(
        &mut self,
        clear: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<Vec<CleanupSlot>, InvalidEvidence> {
        if self.after_clear.iter().any(|entry| entry.clear == clear) {
            return Err(InvalidEvidence::Phase);
        }
        let entry = self.derive_after_clear(clear, source, destination)?;
        let slots = (0..entry.plans.len())
            .map(|group| CleanupSlot::AfterClear { clear, group })
            .collect();
        self.after_clear.push(entry);
        Ok(slots)
    }

    pub(super) fn derive_after_clear(
        &self,
        index: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<AfterClear, InvalidEvidence> {
        let plan = self
            .preview
            .clear
            .get(index)
            .ok_or(InvalidEvidence::Bounds)?;
        let effect = self.clear(index).ok_or(InvalidEvidence::Phase)?;
        let readback = effect.readback().ok_or(InvalidEvidence::Phase)?;
        let certified = effect.verified();
        if certified.is_empty() {
            return Err(InvalidEvidence::Phase);
        }
        self.forward.validate_copy_readback(&source, &destination)?;
        let originals: BTreeSet<_> = plan.clear.before.messages.keys().copied().collect();
        let mapping = self
            .forward
            .copy
            .mapping()
            .ok_or(InvalidEvidence::Mapping)?;
        let targets = originals.iter().map(|u| mapping.pairs()[u]).collect();
        let source = source.subset(&originals);
        let destination = destination.subset(&targets);
        check_observation(readback, &source)?;
        check_observation(&self.preview.destination.subset(&targets), &destination)?;
        let mut groups: BTreeMap<ModSeq, BTreeMap<Uid, Uid>> = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        for uid in certified.keys() {
            let target = mapping.pairs()[uid];
            let original = &self.forward.intent().messages()[uid];
            match (source.messages.get(uid), destination.messages.get(&target)) {
                (Some(retained), Some(copy))
                    if !retained.metadata.protected(false)
                        && same_message(&retained.metadata, original)
                        && copy.metadata == *original
                        && self.preview.destination.messages.get(&target) == Some(copy) =>
                {
                    groups.entry(copy.modseq).or_default().insert(*uid, target);
                }
                _ => {
                    conflicts.insert(*uid);
                }
            }
        }
        let plans = groups
            .into_values()
            .map(|originals| {
                let deletion = FlagIntent::new(
                    destination.subset(&originals.values().copied().collect()),
                    FlagChange::AddDeleted,
                )?;
                Ok(ExtraCopyPlan {
                    retained: source.subset(&originals.keys().copied().collect()),
                    originals,
                    deletion,
                })
            })
            .collect::<Result<Vec<_>, InvalidEvidence>>()?;
        Ok(AfterClear {
            clear: index,
            source,
            destination,
            conflicts,
            cleanup: vec![CleanupRecord::default(); plans.len()],
            plans,
        })
    }
}
