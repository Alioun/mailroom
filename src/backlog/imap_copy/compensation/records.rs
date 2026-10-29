use super::*;

mod decode;
mod progress;

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
    pub fn cleanup(&self, index: usize) -> Option<&Deletion> {
        self.cleanup.get(index).and_then(|r| r.deletion.as_ref())
    }
    pub fn clear(&self, index: usize) -> Option<&PendingClear> {
        self.clear.get(index).and_then(Option::as_ref)
    }

    pub fn begin_cleanup(
        &mut self,
        index: usize,
        source: &Readback,
        destination: &Readback,
    ) -> Result<String, InvalidEvidence> {
        let plan = self
            .preview
            .cleanup
            .get(index)
            .ok_or(InvalidEvidence::Bounds)?;
        let record = self.cleanup.get_mut(index).ok_or(InvalidEvidence::Bounds)?;
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

    fn cleanup_mut(&mut self, index: usize) -> Result<&mut Deletion, InvalidEvidence> {
        self.cleanup
            .get_mut(index)
            .ok_or(InvalidEvidence::Bounds)?
            .deletion
            .as_mut()
            .ok_or(InvalidEvidence::Phase)
    }
    pub fn record_cleanup_store(
        &mut self,
        index: usize,
        result: StoreResult,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(index)?.record_store_result(result)
    }
    pub fn observe_cleanup_store(
        &mut self,
        index: usize,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(index)?.observe_store(destination)
    }
    pub fn begin_cleanup_expunge(
        &mut self,
        index: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<String, InvalidEvidence> {
        let plan = self
            .preview
            .cleanup
            .get(index)
            .ok_or(InvalidEvidence::Bounds)?;
        let expected = retained_for(plan, &destination.messages.keys().copied().collect())?;
        require_retained(&expected, &source)?;
        let record = self.cleanup.get_mut(index).ok_or(InvalidEvidence::Bounds)?;
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
        index: usize,
        completion: Completion,
    ) -> Result<(), InvalidEvidence> {
        self.cleanup_mut(index)?
            .record_expunge_completion(completion)
    }
    pub fn observe_cleanup_final(
        &mut self,
        index: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        let record = self.cleanup.get_mut(index).ok_or(InvalidEvidence::Bounds)?;
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
