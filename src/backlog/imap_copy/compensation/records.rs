use super::*;

mod after_clear;
mod decode;
mod progress;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupSlot {
    Initial(usize),
    AfterClear { clear: usize, group: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Semantics {
    #[serde(rename = "imap-copy-compensation/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupNext {
    Start,
    ObserveStore,
    Expunge,
    ObserveExpunge,
    Complete,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearNext {
    Start,
    ObserveStore,
    Complete,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompensationResolution {
    Unattempted,
    Pending,
    Uncertain,
    Restored { destination: Uid },
    ExtraCopyRemoved,
    DeletedCleared,
    Conflict,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupRecord {
    deletion: Option<Deletion>,
    retained: Option<Readback>,
    final_retained: Option<Readback>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "decode::RawCompensation")]
pub struct CompensationEvidence {
    semantics: Semantics,
    forward: CopyEvidence,
    preview: CompensationPreview,
    restore: Option<CopyEvidence>,
    cleanup: Vec<CleanupRecord>,
    clear: Vec<Option<PendingClear>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    after_clear: Vec<after_clear::AfterClear>,
}

fn retained_for(
    plan: &ExtraCopyPlan,
    targets: &BTreeSet<Uid>,
) -> Result<Readback, InvalidEvidence> {
    let originals: BTreeSet<_> = plan
        .originals
        .iter()
        .filter(|(_, target)| targets.contains(target))
        .map(|(original, _)| *original)
        .collect();
    if targets.is_empty() || originals.len() != targets.len() {
        return Err(InvalidEvidence::Identity);
    }
    Ok(plan.retained.subset(&originals))
}

fn require_retained(expected: &Readback, fresh: &Readback) -> Result<(), InvalidEvidence> {
    check_observation(expected, fresh)?;
    if expected.messages != fresh.messages {
        return Err(InvalidEvidence::Identity);
    }
    Ok(())
}

impl CompensationEvidence {
    pub fn new(
        forward: CopyEvidence,
        preview: CompensationPreview,
    ) -> Result<Self, InvalidEvidence> {
        if forward.compensation_preview(preview.source.clone(), preview.destination.clone())?
            != preview
        {
            return Err(InvalidEvidence::Identity);
        }
        Ok(Self {
            semantics: Semantics::V1,
            forward,
            restore: preview.restore.clone(),
            cleanup: vec![CleanupRecord::default(); preview.cleanup.len()],
            clear: vec![None; preview.clear.len()],
            preview,
            after_clear: Vec::new(),
        })
    }
    pub fn forward(&self) -> &CopyEvidence {
        &self.forward
    }
    pub fn preview(&self) -> &CompensationPreview {
        &self.preview
    }
    pub fn restore(&self) -> Option<&CopyEvidence> {
        self.restore.as_ref()
    }
    pub fn record_restore(&mut self, evidence: CopyEvidence) -> Result<(), InvalidEvidence> {
        let previous = self.restore.as_ref().ok_or(InvalidEvidence::Phase)?;
        evidence.continues(previous)?;
        self.restore = Some(evidence);
        Ok(())
    }
    pub fn cleanup(&self, slot: CleanupSlot) -> Option<&Deletion> {
        self.cleanup_entry(slot)
            .ok()
            .and_then(|(_, r)| r.deletion.as_ref())
    }
    pub fn clear(&self, index: usize) -> Option<&PendingClear> {
        self.clear.get(index).and_then(Option::as_ref)
    }

    pub fn begin_cleanup(
        &mut self,
        slot: CleanupSlot,
        source: &Readback,
        destination: &Readback,
    ) -> Result<String, InvalidEvidence> {
        let (plan, record) = self.cleanup_entry_mut(slot)?;
        if record.deletion.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        if source != &plan.retained || destination != plan.deletion.before() {
            return Err(InvalidEvidence::Identity);
        }
        record.deletion = Some(Deletion {
            flag: FlagEvidence::attempt(plan.deletion.clone()),
            expunge: None,
        });
        Ok(plan.deletion.command())
    }

    fn cleanup_mut(&mut self, slot: CleanupSlot) -> Result<&mut Deletion, InvalidEvidence> {
        self.cleanup_entry_mut(slot)?
            .1
            .deletion
            .as_mut()
            .ok_or(InvalidEvidence::Phase)
    }
    pub fn record_cleanup_store(
        &mut self,
        slot: CleanupSlot,
        result: StoreResult,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(slot)?.record_store_result(result)
    }
    pub fn observe_cleanup_store(
        &mut self,
        slot: CleanupSlot,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(slot)?.observe_store(destination)
    }
    pub fn begin_cleanup_expunge(
        &mut self,
        slot: CleanupSlot,
        source: Readback,
        destination: Readback,
    ) -> Result<String, InvalidEvidence> {
        let (plan, record) = self.cleanup_entry_mut(slot)?;
        let expected = retained_for(plan, &destination.messages.keys().copied().collect())?;
        require_retained(&expected, &source)?;
        let command = record
            .deletion
            .as_mut()
            .ok_or(InvalidEvidence::Phase)?
            .begin_expunge(destination)?;
        record.retained = Some(source);
        Ok(command)
    }
    pub fn record_cleanup_expunge(
        &mut self,
        slot: CleanupSlot,
        completion: Completion,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(slot)?
            .record_expunge_completion(completion)
    }
    pub fn observe_cleanup_final(
        &mut self,
        slot: CleanupSlot,
        source: Readback,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        let (_, record) = self.cleanup_entry_mut(slot)?;
        check_observation(
            record
                .final_retained
                .as_ref()
                .or(record.retained.as_ref())
                .ok_or(InvalidEvidence::Phase)?,
            &source,
        )?;
        record
            .deletion
            .as_mut()
            .ok_or(InvalidEvidence::Phase)?
            .observe_expunge(destination)?;
        record.final_retained = Some(source);
        Ok(())
    }

    pub fn begin_clear(
        &mut self,
        index: usize,
        fresh: Readback,
    ) -> Result<String, InvalidEvidence> {
        let plan = self
            .preview
            .clear
            .get(index)
            .ok_or(InvalidEvidence::Bounds)?;
        let effect = self.clear.get_mut(index).ok_or(InvalidEvidence::Bounds)?;
        if effect.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        let pending = self
            .forward
            .deletions
            .get(plan.deletion)
            .ok_or(InvalidEvidence::Identity)?
            .evidence
            .begin_clear(plan.clear.clone(), fresh)?;
        let command = pending.intent().command();
        *effect = Some(pending);
        Ok(command)
    }
    fn clear_mut(&mut self, index: usize) -> Result<&mut PendingClear, InvalidEvidence> {
        if self.after_clear.iter().any(|entry| entry.clear == index) {
            return Err(InvalidEvidence::Phase);
        }
        self.clear
            .get_mut(index)
            .ok_or(InvalidEvidence::Bounds)?
            .as_mut()
            .ok_or(InvalidEvidence::Phase)
    }
    pub fn record_clear(
        &mut self,
        index: usize,
        result: StoreResult,
    ) -> Result<(), InvalidEvidence> {
        self.clear_mut(index)?.record_result(result)
    }
    pub fn observe_clear(&mut self, index: usize, source: Readback) -> Result<(), InvalidEvidence> {
        self.clear_mut(index)?.observe(source)
    }
}
