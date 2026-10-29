use super::imap_move::{
    Completion, CopyUidMapping, InvalidEvidence, LIMIT, MailboxIdentity, MailboxReadback, Metadata,
    MoveEvidence, MoveIntent, Purpose, TaggedStatus, Uid, uid_map, uid_set,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, collections::BTreeSet, num::NonZeroU64};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModSeq(NonZeroU64);

impl ModSeq {
    pub fn new(value: u64) -> Result<Self, InvalidEvidence> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(InvalidEvidence::Identity)
    }
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedMetadata {
    pub metadata: Metadata,
    pub modseq: ModSeq,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawReadback")]
pub struct Readback {
    identity: MailboxIdentity,
    highest_modseq: ModSeq,
    messages: BTreeMap<Uid, VersionedMetadata>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReadback {
    identity: MailboxIdentity,
    highest_modseq: ModSeq,
    #[serde(deserialize_with = "uid_map")]
    messages: BTreeMap<Uid, VersionedMetadata>,
}

impl TryFrom<RawReadback> for Readback {
    type Error = &'static str;
    fn try_from(raw: RawReadback) -> Result<Self, Self::Error> {
        Self::new(raw.identity, raw.highest_modseq, raw.messages)
            .map_err(|_| "Invalid persistent CONDSTORE readback")
    }
}

impl Readback {
    pub fn new(
        identity: MailboxIdentity,
        highest_modseq: ModSeq,
        messages: BTreeMap<Uid, VersionedMetadata>,
    ) -> Result<Self, InvalidEvidence> {
        if messages.len() > LIMIT {
            return Err(InvalidEvidence::Bounds);
        }
        if crate::census::folder_provisioning::ExactName::from_wire(&identity.name.wire)
            .map_err(|_| InvalidEvidence::Identity)?
            != identity.name
        {
            return Err(InvalidEvidence::Identity);
        }
        for message in messages.values() {
            message.metadata.validate()?;
            if message.modseq > highest_modseq {
                return Err(InvalidEvidence::Identity);
            }
        }
        Ok(Self {
            identity,
            highest_modseq,
            messages,
        })
    }
    pub fn identity(&self) -> &MailboxIdentity {
        &self.identity
    }
    pub fn messages(&self) -> &BTreeMap<Uid, VersionedMetadata> {
        &self.messages
    }
    fn plain(&self) -> MailboxReadback {
        MailboxReadback {
            identity: self.identity.clone(),
            messages: self
                .messages
                .iter()
                .map(|(uid, m)| (*uid, m.metadata.clone()))
                .collect(),
        }
    }
    fn subset(&self, uids: &BTreeSet<Uid>) -> Self {
        Self {
            identity: self.identity.clone(),
            highest_modseq: self.highest_modseq,
            messages: self
                .messages
                .iter()
                .filter(|(u, _)| uids.contains(u))
                .map(|(u, m)| (*u, m.clone()))
                .collect(),
        }
    }
}

mod deletion;
pub use deletion::{Deletion, FlagChange, FlagEvidence, FlagIntent, PendingClear, StoreResult};
use deletion::{check_observation, wire_uids};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyReadback {
    source: Readback,
    destination: Readback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedDelete {
    intent: FlagIntent,
    destination: Readback,
}
impl PreparedDelete {
    pub fn intent(&self) -> &FlagIntent {
        &self.intent
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBatch {
    evidence: Deletion,
    destination: Readback,
    final_destination: Option<Readback>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCopyEvidence")]
pub struct CopyEvidence {
    before: Readback,
    copy: MoveEvidence,
    readback: Option<CopyReadback>,
    deletions: Vec<DeleteBatch>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCopyEvidence {
    before: Readback,
    copy: MoveEvidence,
    readback: Option<CopyReadback>,
    #[serde(deserialize_with = "delete_batches")]
    deletions: Vec<DeleteBatch>,
}
fn delete_batches<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<DeleteBatch>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<DeleteBatch>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 500 disjoint deletion groups")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut batches = Vec::new();
            while let Some(batch) = seq.next_element()? {
                if batches.len() == LIMIT {
                    return Err(serde::de::Error::custom("Excessive deletion groups"));
                }
                batches.push(batch);
            }
            Ok(batches)
        }
    }
    d.deserialize_seq(Visitor)
}
impl TryFrom<RawCopyEvidence> for CopyEvidence {
    type Error = &'static str;
    fn try_from(raw: RawCopyEvidence) -> Result<Self, Self::Error> {
        let mut e =
            Self::new(raw.copy.command().clone(), raw.before).map_err(|_| "Invalid COPY intent")?;
        if raw.copy.observed() != raw.readback.is_some() {
            return Err("Unpaired COPY readback");
        }
        if let Some(readback) = &raw.readback {
            let mut checked = raw.copy.clone();
            checked
                .observe(readback.source.plain(), readback.destination.plain())
                .map_err(|_| "Invalid COPY readback")?;
            if checked != raw.copy {
                return Err("Different COPY readbacks");
            }
        }
        e.copy = raw.copy;
        if let Some(readback) = raw.readback {
            e.validate_copy_readback(&readback.source, &readback.destination)
                .map_err(|_| "Invalid versioned COPY readback")?;
            e.readback = Some(readback);
        }
        for batch in raw.deletions {
            let prepared = PreparedDelete {
                intent: batch.evidence.intent().clone(),
                destination: batch.destination.clone(),
            };
            e.check_prepared(&prepared)
                .map_err(|_| "Invalid COPY deletion group")?;
            if batch
                .evidence
                .expunge
                .as_ref()
                .is_some_and(|e| e.readback.is_some())
                != batch.final_destination.is_some()
            {
                return Err("Unpaired final COPY observations");
            }
            if let Some(final_destination) = &batch.final_destination {
                check_observation(&batch.destination, final_destination)
                    .map_err(|_| "Invalid final destination")?;
            }
            e.deletions.push(batch);
        }
        Ok(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Unattempted,
    Uncertain,
    CopyOnly { destination: Uid },
    Marked { destination: Uid },
    Filed { destination: Uid },
    Conflict,
}

impl CopyEvidence {
    pub fn new(intent: MoveIntent, before: Readback) -> Result<Self, InvalidEvidence> {
        if before.identity != *intent.source()
            || before.plain().messages != *intent.messages()
            || intent.messages().values().any(|m| {
                m.protected(matches!(
                    intent.purpose(),
                    Purpose::Filing {
                        protect_flagged: true
                    }
                ))
            })
        {
            return Err(InvalidEvidence::Identity);
        }
        Ok(Self {
            before,
            copy: MoveEvidence::intent(intent),
            readback: None,
            deletions: Vec::new(),
        })
    }
    pub fn intent(&self) -> &MoveIntent {
        self.copy.command()
    }
    pub fn copy(&self) -> &MoveEvidence {
        &self.copy
    }
    pub fn deletions(&self) -> impl Iterator<Item = &Deletion> {
        self.deletions.iter().map(|b| &b.evidence)
    }
    pub fn begin_copy(&mut self, fresh: &Readback) -> Result<String, InvalidEvidence> {
        if self.before != *fresh {
            return Err(InvalidEvidence::Identity);
        }
        let name = self
            .intent()
            .destination()
            .name
            .wire
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let command = format!("UID COPY {} \"{name}\"", self.intent().wire_uids());
        self.copy.begin_attempt()?;
        Ok(command)
    }
    pub fn record_mapping(&mut self, mapping: CopyUidMapping) -> Result<(), InvalidEvidence> {
        self.copy.record_mapping(mapping)
    }
    pub fn reject_mapping(&mut self) {
        self.copy.reject_mapping();
    }
    pub fn record_copy_completion(&mut self, result: Completion) -> Result<(), InvalidEvidence> {
        self.copy.record_completion(result)
    }
    fn validate_copy_readback(
        &self,
        source: &Readback,
        destination: &Readback,
    ) -> Result<(), InvalidEvidence> {
        check_observation(&self.before, source)?;
        if destination.identity != *self.intent().destination()
            || destination.messages.keys().any(|uid| {
                self.copy
                    .mapping()
                    .is_none_or(|m| !m.pairs().values().any(|v| v == uid))
            })
        {
            return Err(InvalidEvidence::Identity);
        }
        Ok(())
    }
    pub fn observe_copy(
        &mut self,
        source: Readback,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        if !self.deletions.is_empty() {
            return Err(InvalidEvidence::Phase);
        }
        self.validate_copy_readback(&source, &destination)?;
        if let Some(old) = &self.readback {
            check_observation(&old.source, &source)?;
            check_observation(&old.destination, &destination)?;
        }
        self.copy.observe(source.plain(), destination.plain())?;
        self.readback = Some(CopyReadback {
            source,
            destination,
        });
        Ok(())
    }
    pub fn prepare_copy_cleanup(
        &self,
        source: &Readback,
        destination: &Readback,
    ) -> Result<Vec<FlagIntent>, InvalidEvidence> {
        let readback = self.readback.as_ref().ok_or(InvalidEvidence::Phase)?;
        self.validate_copy_readback(source, destination)?;
        check_observation(&readback.source, source)?;
        check_observation(&readback.destination, destination)?;
        let mapping = self.copy.mapping().ok_or(InvalidEvidence::Mapping)?;
        let attempted: BTreeSet<_> = self
            .deletions
            .iter()
            .flat_map(|b| b.evidence.intent().before.messages.keys())
            .copied()
            .collect();
        let mut groups: BTreeMap<ModSeq, BTreeSet<Uid>> = BTreeMap::new();
        for (uid, mapped) in mapping.pairs() {
            if attempted.contains(uid) {
                continue;
            }
            let Some(current_source) = source.messages.get(uid) else {
                continue;
            };
            let Some(copy) = destination.messages.get(mapped) else {
                continue;
            };
            let original = &self.intent().messages()[uid];
            if !current_source.metadata.protected(false)
                && current_source.metadata.message_id_hash == original.message_id_hash
                && current_source.metadata.size == original.size
                && current_source.metadata.internal_date == original.internal_date
                && copy.metadata == *original
                && readback.destination.messages.get(mapped) == Some(copy)
            {
                groups.entry(copy.modseq).or_default().insert(*mapped);
            }
        }
        groups
            .into_values()
            .map(|uids| FlagIntent::new(destination.subset(&uids), FlagChange::AddDeleted))
            .collect()
    }
    pub fn prepare_deletions(
        &self,
        source: &Readback,
        destination: &Readback,
    ) -> Result<Vec<PreparedDelete>, InvalidEvidence> {
        let readback = self.readback.as_ref().ok_or(InvalidEvidence::Phase)?;
        self.validate_copy_readback(source, destination)?;
        check_observation(&readback.source, source)?;
        check_observation(&readback.destination, destination)?;
        let mapping = self.copy.mapping().ok_or(InvalidEvidence::Mapping)?;
        let attempted: BTreeSet<_> = self
            .deletions
            .iter()
            .flat_map(|b| b.evidence.intent().before.messages.keys())
            .copied()
            .collect();
        let mut groups: BTreeMap<ModSeq, BTreeSet<Uid>> = BTreeMap::new();
        for (uid, original) in self.intent().messages() {
            if attempted.contains(uid) {
                continue;
            }
            let Some(mapped) = mapping.pairs().get(uid) else {
                continue;
            };
            let Some(current) = source.messages.get(uid) else {
                continue;
            };
            if current.metadata == *original
                && readback
                    .source
                    .messages
                    .get(uid)
                    .is_some_and(|m| m.metadata == *original)
                && readback
                    .destination
                    .messages
                    .get(mapped)
                    .is_some_and(|m| m.metadata == *original)
                && destination.messages.get(mapped) == readback.destination.messages.get(mapped)
            {
                groups.entry(current.modseq).or_default().insert(*uid);
            }
        }
        groups
            .into_values()
            .map(|uids| {
                let mapped = uids.iter().map(|u| mapping.pairs()[u]).collect();
                Ok(PreparedDelete {
                    intent: FlagIntent::new(source.subset(&uids), FlagChange::AddDeleted)?,
                    destination: destination.subset(&mapped),
                })
            })
            .collect()
    }
    fn check_prepared(&self, prepared: &PreparedDelete) -> Result<(), InvalidEvidence> {
        if !self
            .prepare_deletions(&prepared.intent.before, &prepared.destination)?
            .iter()
            .any(|p| p == prepared)
        {
            return Err(InvalidEvidence::Identity);
        }
        Ok(())
    }
    pub fn begin_delete(&mut self, prepared: PreparedDelete) -> Result<usize, InvalidEvidence> {
        self.check_prepared(&prepared)?;
        let index = self.deletions.len();
        self.deletions.push(DeleteBatch {
            evidence: Deletion {
                flag: FlagEvidence::attempt(prepared.intent),
                expunge: None,
            },
            destination: prepared.destination,
            final_destination: None,
        });
        Ok(index)
    }
    fn batch_mut(&mut self, index: usize) -> Result<&mut DeleteBatch, InvalidEvidence> {
        self.deletions
            .get_mut(index)
            .ok_or(InvalidEvidence::Identity)
    }
    pub fn record_store_result(
        &mut self,
        index: usize,
        result: StoreResult,
    ) -> Result<(), InvalidEvidence> {
        self.batch_mut(index)?.evidence.record_store_result(result)
    }
    pub fn observe_store(
        &mut self,
        index: usize,
        readback: Readback,
    ) -> Result<(), InvalidEvidence> {
        self.batch_mut(index)?.evidence.observe_store(readback)
    }
    pub fn begin_expunge(
        &mut self,
        index: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<String, InvalidEvidence> {
        let batch = self.batch_mut(index)?;
        check_observation(&batch.destination, &destination)?;
        if batch.destination.messages != destination.messages {
            return Err(InvalidEvidence::Identity);
        }
        batch.evidence.begin_expunge(source)
    }
    pub fn record_expunge_completion(
        &mut self,
        index: usize,
        result: Completion,
    ) -> Result<(), InvalidEvidence> {
        self.batch_mut(index)?
            .evidence
            .record_expunge_completion(result)
    }
    pub fn observe_final(
        &mut self,
        index: usize,
        source: Readback,
        destination: Readback,
    ) -> Result<(), InvalidEvidence> {
        let batch = self.batch_mut(index)?;
        check_observation(
            batch
                .final_destination
                .as_ref()
                .unwrap_or(&batch.destination),
            &destination,
        )?;
        batch.evidence.observe_expunge(source)?;
        batch.final_destination = Some(destination);
        Ok(())
    }
    pub fn resolutions(&self) -> BTreeMap<Uid, Resolution> {
        self.intent()
            .messages()
            .iter()
            .map(|(uid, original)| (*uid, self.resolution(*uid, original)))
            .collect()
    }
    fn resolution(&self, uid: Uid, original: &Metadata) -> Resolution {
        if !self.copy.attempted() {
            return Resolution::Unattempted;
        }
        let Some(mapped) = self
            .copy
            .mapping()
            .and_then(|m| m.pairs().get(&uid))
            .copied()
        else {
            return if self.copy.mapping_rejected() {
                Resolution::Conflict
            } else {
                Resolution::Uncertain
            };
        };
        let Some(readback) = &self.readback else {
            return Resolution::Uncertain;
        };
        if readback
            .source
            .messages
            .get(&uid)
            .is_none_or(|m| m.metadata != *original)
            || readback
                .destination
                .messages
                .get(&mapped)
                .is_none_or(|m| m.metadata != *original)
        {
            return Resolution::Conflict;
        }
        let Some(batch) = self
            .deletions
            .iter()
            .find(|b| b.evidence.intent().before.messages.contains_key(&uid))
        else {
            return Resolution::CopyOnly {
                destination: mapped,
            };
        };
        if let Some(expunge) = &batch.evidence.expunge {
            if expunge.readback.is_none() {
                return Resolution::Uncertain;
            }
            return if batch.evidence.removed().contains(&uid)
                && batch.final_destination.as_ref().is_some_and(|r| {
                    r.messages
                        .get(&mapped)
                        .is_some_and(|m| m.metadata == *original)
                }) {
                Resolution::Filed {
                    destination: mapped,
                }
            } else {
                Resolution::Conflict
            };
        }
        if batch.evidence.flag.verified().contains_key(&uid) {
            return Resolution::Marked {
                destination: mapped,
            };
        }
        if batch.evidence.flag.readback.is_none() {
            return Resolution::Uncertain;
        }
        Resolution::Conflict
    }
}

#[cfg(test)]
mod tests;
