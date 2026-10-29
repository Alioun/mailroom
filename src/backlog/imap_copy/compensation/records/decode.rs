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
        evidence.clear = raw.clear;
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
        for (index, record) in self.cleanup.iter().enumerate() {
            let Some(effect) = &record.deletion else {
                continue;
            };
            if next.cleanup[index].deletion.is_none() {
                let plan = &self.preview.cleanup[index];
                next.begin_cleanup(index, &plan.retained, plan.deletion.before())?;
            }
            let prior = next.cleanup[index].deletion.as_ref().unwrap();
            if effect.flag.result != prior.flag.result {
                next.record_cleanup_store(
                    index,
                    effect.flag.result.clone().ok_or(InvalidEvidence::Phase)?,
                )?;
            }
            if effect.flag.readback != next.cleanup[index].deletion.as_ref().unwrap().flag.readback
            {
                next.observe_cleanup_store(
                    index,
                    effect.flag.readback.clone().ok_or(InvalidEvidence::Phase)?,
                )?;
            }
            if let Some(expunge) = &effect.expunge {
                if next.cleanup[index]
                    .deletion
                    .as_ref()
                    .unwrap()
                    .expunge
                    .is_none()
                {
                    next.begin_cleanup_expunge(
                        index,
                        record.retained.clone().ok_or(InvalidEvidence::Phase)?,
                        expunge.before.clone(),
                    )?;
                }
                let prior = next.cleanup[index]
                    .deletion
                    .as_ref()
                    .unwrap()
                    .expunge
                    .as_ref()
                    .unwrap();
                if expunge.completion != prior.completion {
                    next.record_cleanup_expunge(
                        index,
                        expunge.completion.clone().ok_or(InvalidEvidence::Phase)?,
                    )?;
                }
                if expunge.readback
                    != next.cleanup[index]
                        .deletion
                        .as_ref()
                        .unwrap()
                        .expunge
                        .as_ref()
                        .unwrap()
                        .readback
                    || record.final_retained != next.cleanup[index].final_retained
                {
                    next.observe_cleanup_final(
                        index,
                        record
                            .final_retained
                            .clone()
                            .ok_or(InvalidEvidence::Phase)?,
                        expunge.readback.clone().ok_or(InvalidEvidence::Phase)?,
                    )?;
                }
            }
        }
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
        if next == *self {
            Ok(())
        } else {
            Err(InvalidEvidence::Phase)
        }
    }
}
