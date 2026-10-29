use super::*;

mod records;
pub use records::{
    CleanupNext, CleanupSlot, ClearNext, CompensationEvidence, CompensationResolution,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompensationConflict {
    ForwardConflict,
    UncertainEffect,
    SourcePresent,
    ChangedOrMissingMessage,
    ProtectedFlags,
    UnprovenFlagOwnership,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtraCopyPlan {
    originals: BTreeMap<Uid, Uid>,
    retained: Readback,
    deletion: FlagIntent,
}
impl ExtraCopyPlan {
    pub fn originals(&self) -> &BTreeMap<Uid, Uid> {
        &self.originals
    }
    pub fn retained(&self) -> &Readback {
        &self.retained
    }
    pub fn deletion(&self) -> &FlagIntent {
        &self.deletion
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OwnedDeletedPlan {
    deletion: usize,
    clear: FlagIntent,
}
impl OwnedDeletedPlan {
    pub fn deletion(&self) -> usize {
        self.deletion
    }
    pub fn clear(&self) -> &FlagIntent {
        &self.clear
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompensationPreview {
    source: Readback,
    destination: Readback,
    restore: Option<CopyEvidence>,
    cleanup: Vec<ExtraCopyPlan>,
    clear: Vec<OwnedDeletedPlan>,
    conflicts: BTreeMap<Uid, CompensationConflict>,
}
impl CompensationPreview {
    pub fn source(&self) -> &Readback {
        &self.source
    }
    pub fn destination(&self) -> &Readback {
        &self.destination
    }
    pub fn restore(&self) -> Option<&CopyEvidence> {
        self.restore.as_ref()
    }
    pub fn cleanup(&self) -> &[ExtraCopyPlan] {
        &self.cleanup
    }
    pub fn clear(&self) -> &[OwnedDeletedPlan] {
        &self.clear
    }
    pub fn conflicts(&self) -> &BTreeMap<Uid, CompensationConflict> {
        &self.conflicts
    }
}

impl CopyEvidence {
    pub fn compensation_preview(
        &self,
        source: Readback,
        destination: Readback,
    ) -> Result<CompensationPreview, InvalidEvidence> {
        if !matches!(self.intent().purpose(), Purpose::Filing { .. })
            || self.next_action().needs_observation()
            || !self.observed()
        {
            return Err(InvalidEvidence::Phase);
        }
        self.validate_copy_readback(&source, &destination)?;
        let readback = self.readback.as_ref().ok_or(InvalidEvidence::Phase)?;
        check_observation(&readback.source, &source)?;
        check_observation(&readback.destination, &destination)?;
        let mapping = self.copy.mapping().ok_or(InvalidEvidence::Mapping)?;
        for batch in &self.deletions {
            if let Some(read) = &batch.evidence.flag.readback {
                check_observation(
                    read,
                    &source.subset(&read.messages.keys().copied().collect()),
                )?;
            }
            if let Some(read) = batch
                .evidence
                .expunge
                .as_ref()
                .and_then(|e| e.readback.as_ref())
            {
                check_observation(
                    read,
                    &source.subset(&read.messages.keys().copied().collect()),
                )?;
            }
            if let Some(read) = &batch.final_destination {
                check_observation(
                    read,
                    &destination.subset(&read.messages.keys().copied().collect()),
                )?;
            }
        }
        let resolutions = self.resolutions();
        let mut restored = BTreeSet::new();
        let mut conflicts = BTreeMap::new();
        for (uid, resolution) in &resolutions {
            let conflict = match resolution {
                Resolution::Filed {
                    destination: mapped,
                } => {
                    let original = &self.intent().messages()[uid];
                    match destination.messages.get(mapped) {
                        _ if source.messages.contains_key(uid) => {
                            CompensationConflict::SourcePresent
                        }
                        Some(current) if same_message(&current.metadata, original) => {
                            if current.metadata.protected(false) {
                                CompensationConflict::ProtectedFlags
                            } else {
                                restored.insert(*mapped);
                                continue;
                            }
                        }
                        _ => CompensationConflict::ChangedOrMissingMessage,
                    }
                }
                Resolution::CopyOnly { .. } => CompensationConflict::ChangedOrMissingMessage,
                Resolution::Marked { .. } => CompensationConflict::UnprovenFlagOwnership,
                Resolution::Uncertain => CompensationConflict::UncertainEffect,
                Resolution::Conflict | Resolution::Unattempted => {
                    CompensationConflict::ForwardConflict
                }
            };
            conflicts.insert(*uid, conflict);
        }
        let restore = if restored.is_empty() {
            None
        } else {
            let before = destination.subset(&restored);
            let intent = MoveIntent::new(
                Purpose::ConditionalUndo,
                before.identity.clone(),
                source.identity.clone(),
                before.plain().messages,
            )?;
            Some(CopyEvidence::new(intent, before)?)
        };
        let mut cleanup = Vec::new();
        for intent in self.prepare_copy_cleanup(&source, &destination)? {
            let originals: BTreeMap<_, _> = mapping
                .pairs()
                .iter()
                .filter(|(uid, mapped)| {
                    matches!(resolutions[uid], Resolution::CopyOnly { .. })
                        && intent.before.messages.contains_key(mapped)
                })
                .map(|(uid, mapped)| (*uid, *mapped))
                .collect();
            if originals.is_empty() {
                continue;
            }
            let deletion = FlagIntent::new(
                destination.subset(&originals.values().copied().collect()),
                FlagChange::AddDeleted,
            )?;
            let retained = source.subset(&originals.keys().copied().collect());
            for uid in originals.keys() {
                conflicts.remove(uid);
            }
            cleanup.push(ExtraCopyPlan {
                originals,
                retained,
                deletion,
            });
        }
        let mut clear = Vec::new();
        for (index, batch) in self.deletions.iter().enumerate() {
            let mut groups: BTreeMap<ModSeq, BTreeSet<Uid>> = BTreeMap::new();
            for (uid, owned) in batch.evidence.flag.verified() {
                if matches!(resolutions[&uid], Resolution::Marked { .. })
                    && source.messages.get(&uid) == Some(&owned)
                {
                    groups.entry(owned.modseq).or_default().insert(uid);
                }
            }
            for uids in groups.into_values() {
                let Ok(intent) = batch.evidence.pending_clear(source.subset(&uids)) else {
                    continue;
                };
                for uid in uids {
                    conflicts.remove(&uid);
                }
                clear.push(OwnedDeletedPlan {
                    deletion: index,
                    clear: intent,
                });
            }
        }
        Ok(CompensationPreview {
            source,
            destination,
            restore,
            cleanup,
            clear,
            conflicts,
        })
    }
}

fn same_message(current: &Metadata, original: &Metadata) -> bool {
    current.message_id_hash == original.message_id_hash
        && current.size == original.size
        && current.internal_date == original.internal_date
}
