use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawCompensation {
    semantics: Semantics,
    forward: CopyEvidence,
    preview: RawPreview,
    restore: Option<CopyEvidence>,
    #[serde(deserialize_with = "bounded")]
    cleanup: Vec<CleanupRecord>,
    #[serde(deserialize_with = "bounded")]
    clear: Vec<Option<PendingClear>>,
    #[serde(default, deserialize_with = "bounded")]
    after_clear: Vec<RawAfterClear>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAfterClear {
    clear: usize,
    source: Readback,
    destination: Readback,
    #[serde(deserialize_with = "bounded")]
    plans: Vec<RawExtraCopy>,
    #[serde(deserialize_with = "bounded")]
    conflicts: Vec<Uid>,
    #[serde(deserialize_with = "bounded")]
    cleanup: Vec<CleanupRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreview {
    source: Readback,
    destination: Readback,
    restore: Option<CopyEvidence>,
    #[serde(deserialize_with = "bounded")]
    cleanup: Vec<RawExtraCopy>,
    #[serde(deserialize_with = "bounded")]
    clear: Vec<RawOwnedDeleted>,
    #[serde(deserialize_with = "uid_map")]
    conflicts: BTreeMap<Uid, CompensationConflict>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtraCopy {
    #[serde(deserialize_with = "uid_map")]
    originals: BTreeMap<Uid, Uid>,
    retained: Readback,
    deletion: FlagIntent,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOwnedDeleted {
    deletion: usize,
    clear: FlagIntent,
}
impl RawPreview {
    fn matches(&self, expected: &CompensationPreview) -> bool {
        self.source == expected.source
            && self.destination == expected.destination
            && self.restore == expected.restore
            && self.conflicts == expected.conflicts
            && self.cleanup.len() == expected.cleanup.len()
            && self.clear.len() == expected.clear.len()
            && self.cleanup.iter().zip(&expected.cleanup).all(|(a, b)| {
                a.originals == b.originals && a.retained == b.retained && a.deletion == b.deletion
            })
            && self
                .clear
                .iter()
                .zip(&expected.clear)
                .all(|(a, b)| a.deletion == b.deletion && a.clear == b.clear)
    }
}

impl TryFrom<RawCompensation> for CompensationEvidence {
    type Error = &'static str;
    fn try_from(raw: RawCompensation) -> Result<Self, Self::Error> {
        let preview = raw
            .forward
            .compensation_preview(raw.preview.source.clone(), raw.preview.destination.clone())
            .map_err(|_| "Invalid compensation witness")?;
        if !raw.preview.matches(&preview) {
            return Err("Changed compensation preview");
        }
        let initial =
            Self::new(raw.forward, preview).map_err(|_| "Invalid compensation preview")?;
        let mut evidence = initial.clone();
        evidence.semantics = raw.semantics;
        evidence.restore = raw.restore;
        evidence.cleanup = raw.cleanup;
        if evidence.clear.len() != raw.clear.len() {
            return Err("Changed clear slot count");
        }
        for (index, effect) in raw.clear.into_iter().enumerate() {
            if let Some(effect) = effect {
                evidence
                    .begin_clear(index, evidence.preview.clear[index].clear.before.clone())
                    .map_err(|_| "Invalid clear prerequisite")?;
                effect
                    .continues(evidence.clear(index).unwrap())
                    .map_err(|_| "Changed clear prerequisite")?;
                evidence.clear[index] = Some(effect);
            }
        }
        for raw in raw.after_clear {
            let mut entry = evidence
                .derive_after_clear(raw.clear, raw.source, raw.destination)
                .map_err(|_| "Invalid cleared cleanup prerequisite")?;
            let conflicts: BTreeSet<_> = raw.conflicts.iter().copied().collect();
            if raw.plans.len() != entry.plans.len()
                || raw.conflicts.len() != conflicts.len()
                || conflicts != entry.conflicts
                || raw.cleanup.len() != entry.cleanup.len()
                || raw.plans.iter().zip(&entry.plans).any(|(a, b)| {
                    a.originals != b.originals
                        || a.retained != b.retained
                        || a.deletion != b.deletion
                })
            {
                return Err("Changed cleared cleanup plan");
            }
            entry.cleanup = raw.cleanup;
            evidence.after_clear.push(entry);
        }
        evidence
            .continues(&initial)
            .map_err(|_| "Invalid compensation history")?;
        Ok(evidence)
    }
}

fn bounded<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most 500 compensation slots")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == LIMIT {
                    return Err(serde::de::Error::custom("Excessive compensation slots"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Visitor(std::marker::PhantomData))
}

impl CompensationEvidence {
    pub(crate) fn continues(&self, previous: &Self) -> Result<(), InvalidEvidence> {
        if self.semantics != previous.semantics
            || self.forward != previous.forward
            || self.preview != previous.preview
            || self.cleanup.len() != previous.cleanup.len()
            || self.clear.len() != previous.clear.len()
            || previous
                .after_clear
                .iter()
                .any(|entry| self.clear.get(entry.clear) != previous.clear.get(entry.clear))
        {
            return Err(InvalidEvidence::Identity);
        }
        let mut next = previous.clone();
        match (&self.restore, &next.restore) {
            (Some(current), Some(prior)) => current.continues(prior)?,
            (None, None) => (),
            _ => return Err(InvalidEvidence::Phase),
        }
        next.restore = self.restore.clone();
        for (index, effect) in self.clear.iter().enumerate() {
            let Some(effect) = effect else {
                continue;
            };
            if next.clear[index].is_none() {
                next.begin_clear(index, self.preview.clear[index].clear.before.clone())?;
            }
            effect.continues(next.clear[index].as_ref().unwrap())?;
            next.clear[index] = Some(effect.clone());
        }
        for entry in &self.after_clear {
            if !next
                .after_clear
                .iter()
                .any(|prior| prior.clear == entry.clear)
            {
                next.prepare_after_clear(
                    entry.clear,
                    entry.source.clone(),
                    entry.destination.clone(),
                )?;
            }
        }
        for slot in self.cleanup_slots() {
            let (_, record) = self.cleanup_entry(slot)?;
            replay_cleanup(&mut next, slot, record)?;
        }
        if next == *self {
            Ok(())
        } else {
            Err(InvalidEvidence::Phase)
        }
    }
}

fn replay_cleanup(
    next: &mut CompensationEvidence,
    slot: CleanupSlot,
    record: &CleanupRecord,
) -> Result<(), InvalidEvidence> {
    let Some(effect) = &record.deletion else {
        return Ok(());
    };
    if next.cleanup(slot).is_none() {
        let plan = next
            .cleanup_plan(slot)
            .ok_or(InvalidEvidence::Bounds)?
            .clone();
        next.begin_cleanup(slot, &plan.retained, plan.deletion.before())?;
    }
    if effect.flag.result != next.cleanup(slot).unwrap().flag.result {
        next.record_cleanup_store(
            slot,
            effect.flag.result.clone().ok_or(InvalidEvidence::Phase)?,
        )?;
    }
    if effect.flag.readback != next.cleanup(slot).unwrap().flag.readback {
        next.observe_cleanup_store(
            slot,
            effect.flag.readback.clone().ok_or(InvalidEvidence::Phase)?,
        )?;
    }
    if let Some(expunge) = &effect.expunge {
        if next.cleanup(slot).unwrap().expunge.is_none() {
            next.begin_cleanup_expunge(
                slot,
                record.retained.clone().ok_or(InvalidEvidence::Phase)?,
                expunge.before.clone(),
            )?;
        }
        if expunge.completion
            != next
                .cleanup(slot)
                .unwrap()
                .expunge
                .as_ref()
                .unwrap()
                .completion
        {
            next.record_cleanup_expunge(
                slot,
                expunge.completion.clone().ok_or(InvalidEvidence::Phase)?,
            )?;
        }
        if expunge.readback
            != next
                .cleanup(slot)
                .unwrap()
                .expunge
                .as_ref()
                .unwrap()
                .readback
            || record.final_retained != next.cleanup_entry(slot)?.1.final_retained
        {
            next.observe_cleanup_final(
                slot,
                record
                    .final_retained
                    .clone()
                    .ok_or(InvalidEvidence::Phase)?,
                expunge.readback.clone().ok_or(InvalidEvidence::Phase)?,
            )?;
        }
    }
    Ok(())
}
