use crate::census::folder_provisioning::ExactName;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, collections::BTreeSet, num::NonZeroU32};

pub const LIMIT: usize = 500;
const CODE_LIMIT: usize = 16 * 1024;

fn uid_map<'de, D: serde::Deserializer<'de>, V: Deserialize<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Uid, V>, D::Error> {
    struct Visitor<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
        type Value = BTreeMap<Uid, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 500 distinct UIDs")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut found = BTreeMap::new();
            while let Some(uid) = map.next_key::<Uid>()? {
                if found.len() >= LIMIT || found.contains_key(&uid) {
                    return Err(serde::de::Error::custom("Duplicate or excessive UIDs"));
                }
                found.insert(uid, map.next_value()?);
            }
            Ok(found)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidEvidence {
    Bounds,
    Identity,
    Mapping,
    Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Uid(NonZeroU32);

impl<'de> Deserialize<'de> for Uid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Uid;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a nonzero 32-bit UID")
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Uid, E> {
                Uid::new(u32::try_from(value).map_err(|_| E::custom("UID overflow"))?)
                    .map_err(|_| E::custom("Zero UID"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Uid, E> {
                if value.is_empty()
                    || value.len() > 10
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(E::custom("Invalid UID"));
                }
                Uid::new(value.parse().map_err(|_| E::custom("UID overflow"))?)
                    .map_err(|_| E::custom("Zero UID"))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl Uid {
    pub fn new(value: u32) -> Result<Self, InvalidEvidence> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(InvalidEvidence::Identity)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UidValidity(NonZeroU32);

impl UidValidity {
    pub fn new(value: u32) -> Result<Self, InvalidEvidence> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(InvalidEvidence::Identity)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxIdentity {
    pub name: ExactName,
    pub uidvalidity: UidValidity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub message_id_hash: String,
    pub size: u32,
    pub internal_date: String,
    pub flags: BTreeSet<String>,
}

impl Metadata {
    fn validate(&self) -> Result<(), InvalidEvidence> {
        if self.message_id_hash.is_empty()
            || self.message_id_hash.len() > 128
            || !self.message_id_hash.bytes().all(|b| b.is_ascii_graphic())
            || self.internal_date.is_empty()
            || self.internal_date.len() > 64
            || !self
                .internal_date
                .bytes()
                .all(|b| (0x20..=0x7e).contains(&b))
            || self.flags.len() > 64
            || self.flags.iter().any(|f| {
                f.is_empty()
                    || f.len() > 128
                    || !f
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !b"(){%*\"".contains(&b))
                    || f.eq_ignore_ascii_case("\\Recent")
            })
        {
            return Err(InvalidEvidence::Bounds);
        }
        Ok(())
    }

    pub fn protected(&self, protect_flagged: bool) -> bool {
        self.flags.iter().any(|f| {
            f.eq_ignore_ascii_case("\\Deleted")
                || f.eq_ignore_ascii_case("\\Draft")
                || (protect_flagged && f.eq_ignore_ascii_case("\\Flagged"))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Filing { protect_flagged: bool },
    ConditionalUndo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawIntent")]
pub struct MoveIntent {
    purpose: Purpose,
    source: MailboxIdentity,
    destination: MailboxIdentity,
    messages: BTreeMap<Uid, Metadata>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent {
    purpose: Purpose,
    source: MailboxIdentity,
    destination: MailboxIdentity,
    #[serde(deserialize_with = "uid_map")]
    messages: BTreeMap<Uid, Metadata>,
}

impl TryFrom<RawIntent> for MoveIntent {
    type Error = &'static str;
    fn try_from(raw: RawIntent) -> Result<Self, Self::Error> {
        Self::new(raw.purpose, raw.source, raw.destination, raw.messages)
            .map_err(|_| "Invalid MOVE intent")
    }
}

impl MoveIntent {
    pub fn new(
        purpose: Purpose,
        source: MailboxIdentity,
        destination: MailboxIdentity,
        messages: BTreeMap<Uid, Metadata>,
    ) -> Result<Self, InvalidEvidence> {
        if source
            .name
            .decoded
            .eq_ignore_ascii_case(&destination.name.decoded)
            || messages.is_empty()
            || messages.len() > LIMIT
        {
            return Err(InvalidEvidence::Bounds);
        }
        for mailbox in [&source, &destination] {
            if ExactName::from_wire(&mailbox.name.wire).map_err(|_| InvalidEvidence::Identity)?
                != mailbox.name
                || mailbox.name.decoded.split('/').any(str::is_empty)
            {
                return Err(InvalidEvidence::Identity);
            }
        }
        for m in messages.values() {
            m.validate()?;
            if matches!(purpose, Purpose::Filing { protect_flagged } if m.protected(protect_flagged))
            {
                return Err(InvalidEvidence::Identity);
            }
        }
        Ok(Self {
            purpose,
            source,
            destination,
            messages,
        })
    }

    pub fn source(&self) -> &MailboxIdentity {
        &self.source
    }
    pub fn destination(&self) -> &MailboxIdentity {
        &self.destination
    }
    pub fn messages(&self) -> &BTreeMap<Uid, Metadata> {
        &self.messages
    }
    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub fn wire_uids(&self) -> String {
        self.messages
            .keys()
            .map(|u| u.get().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawMapping")]
pub struct CopyUidMapping {
    uidvalidity: UidValidity,
    pairs: BTreeMap<Uid, Uid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMapping {
    uidvalidity: UidValidity,
    #[serde(deserialize_with = "uid_map")]
    pairs: BTreeMap<Uid, Uid>,
}

impl TryFrom<RawMapping> for CopyUidMapping {
    type Error = &'static str;
    fn try_from(raw: RawMapping) -> Result<Self, Self::Error> {
        if raw.pairs.is_empty()
            || raw.pairs.len() > LIMIT
            || raw.pairs.values().copied().collect::<BTreeSet<_>>().len() != raw.pairs.len()
        {
            return Err("Invalid COPYUID mapping");
        }
        Ok(Self {
            uidvalidity: raw.uidvalidity,
            pairs: raw.pairs,
        })
    }
}

fn uid_set(text: &str) -> Result<Vec<Uid>, InvalidEvidence> {
    fn number(value: &str) -> Result<u32, InvalidEvidence> {
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) || value.len() > 10 {
            return Err(InvalidEvidence::Mapping);
        }
        let n = value.parse::<u32>().map_err(|_| InvalidEvidence::Mapping)?;
        if n == 0 {
            return Err(InvalidEvidence::Mapping);
        }
        Ok(n)
    }
    let mut result = vec![];
    let mut seen = BTreeSet::new();
    for part in text.split(',') {
        let (a, b) = match part.split_once(':') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => {
                let n = number(part)?;
                (n, n)
            }
        };
        let (first, last) = (a.min(b), a.max(b));
        let count = u64::from(last) - u64::from(first) + 1;
        if count > (LIMIT - result.len()) as u64 {
            return Err(InvalidEvidence::Bounds);
        }
        for n in first..=last {
            let uid = Uid::new(n)?;
            if !seen.insert(uid) {
                return Err(InvalidEvidence::Mapping);
            }
            result.push(uid);
        }
    }
    Ok(result)
}

impl CopyUidMapping {
    pub fn parse(code: &str, intent: &MoveIntent) -> Result<Self, InvalidEvidence> {
        if code.len() > CODE_LIMIT || !code.is_ascii() {
            return Err(InvalidEvidence::Bounds);
        }
        let fields = code
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .ok_or(InvalidEvidence::Mapping)?
            .split(' ')
            .collect::<Vec<_>>();
        if fields.len() != 4 || !fields[0].eq_ignore_ascii_case("COPYUID") {
            return Err(InvalidEvidence::Mapping);
        }
        let uidvalidity = if fields[1].is_empty()
            || fields[1].len() > 10
            || !fields[1].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(InvalidEvidence::Mapping);
        } else {
            UidValidity::new(fields[1].parse().map_err(|_| InvalidEvidence::Mapping)?)?
        };
        let source = uid_set(fields[2])?;
        let destination = uid_set(fields[3])?;
        if uidvalidity != intent.destination.uidvalidity
            || source.len() != destination.len()
            || source.iter().any(|uid| !intent.messages.contains_key(uid))
        {
            return Err(InvalidEvidence::Mapping);
        }
        Ok(Self {
            uidvalidity,
            pairs: source.into_iter().zip(destination).collect(),
        })
    }

    pub fn pairs(&self) -> &BTreeMap<Uid, Uid> {
        &self.pairs
    }

    pub fn validate_for(&self, intent: &MoveIntent) -> Result<(), InvalidEvidence> {
        if self.uidvalidity != intent.destination.uidvalidity
            || self.pairs.is_empty()
            || self.pairs.len() > intent.messages.len()
            || self.pairs.keys().any(|u| !intent.messages.contains_key(u))
            || self.pairs.values().copied().collect::<BTreeSet<_>>().len() != self.pairs.len()
        {
            return Err(InvalidEvidence::Mapping);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaggedStatus {
    Ok,
    No,
    Bad,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Completion {
    Tagged {
        status: TaggedStatus,
        code: Option<String>,
    },
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxReadback {
    pub identity: MailboxIdentity,
    #[serde(deserialize_with = "uid_map")]
    pub messages: BTreeMap<Uid, Metadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Verified { destination: Uid },
    Uncertain,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawEvidence")]
pub struct MoveEvidence {
    intent: MoveIntent,
    attempted: bool,
    mapping: MappingEvidence,
    completion: Option<Completion>,
    readback: Option<(MailboxReadback, MailboxReadback)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvidence {
    intent: MoveIntent,
    attempted: bool,
    mapping: MappingEvidence,
    completion: Option<Completion>,
    readback: Option<(MailboxReadback, MailboxReadback)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum MappingEvidence {
    Missing,
    Validated { mapping: CopyUidMapping },
    Rejected { prior: Option<CopyUidMapping> },
}

impl TryFrom<RawEvidence> for MoveEvidence {
    type Error = &'static str;
    fn try_from(raw: RawEvidence) -> Result<Self, Self::Error> {
        if !raw.attempted
            && (!matches!(raw.mapping, MappingEvidence::Missing)
                || raw.completion.is_some()
                || raw.readback.is_some())
        {
            return Err("Evidence precedes attempt");
        }
        let mut evidence = Self::intent(raw.intent);
        if raw.attempted {
            evidence.begin_attempt().map_err(|_| "Invalid attempt")?;
        }
        match raw.mapping {
            MappingEvidence::Missing => (),
            MappingEvidence::Validated { mapping } => evidence
                .record_mapping(mapping)
                .map_err(|_| "Invalid mapping")?,
            MappingEvidence::Rejected { prior } => {
                if let Some(mapping) = prior {
                    evidence
                        .record_mapping(mapping)
                        .map_err(|_| "Invalid prior mapping")?;
                }
                evidence.reject_mapping();
            }
        }
        if let Some(completion) = raw.completion {
            evidence
                .record_completion(completion)
                .map_err(|_| "Invalid completion")?;
        }
        if let Some((source, destination)) = raw.readback {
            evidence
                .observe(source, destination)
                .map_err(|_| "Invalid readback")?;
        }
        Ok(evidence)
    }
}

impl MoveEvidence {
    pub fn intent(intent: MoveIntent) -> Self {
        Self {
            intent,
            attempted: false,
            mapping: MappingEvidence::Missing,
            completion: None,
            readback: None,
        }
    }
    pub fn command(&self) -> &MoveIntent {
        &self.intent
    }
    pub fn attempted(&self) -> bool {
        self.attempted
    }

    pub fn begin_attempt(&mut self) -> Result<(), InvalidEvidence> {
        if self.attempted {
            return Err(InvalidEvidence::Phase);
        }
        self.attempted = true;
        Ok(())
    }
    pub fn mapping(&self) -> Option<&CopyUidMapping> {
        match &self.mapping {
            MappingEvidence::Validated { mapping } => Some(mapping),
            _ => None,
        }
    }
    pub fn completion(&self) -> Option<&Completion> {
        self.completion.as_ref()
    }
    pub fn observed(&self) -> bool {
        self.readback.is_some()
    }

    pub fn record_mapping(&mut self, mapping: CopyUidMapping) -> Result<(), InvalidEvidence> {
        if !self.attempted || self.readback.is_some() || self.completion.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        if mapping.validate_for(&self.intent).is_err()
            || matches!(&self.mapping, MappingEvidence::Rejected { .. })
            || self.mapping().is_some_and(|old| *old != mapping)
        {
            self.reject_mapping();
            return Err(InvalidEvidence::Mapping);
        }
        self.mapping = MappingEvidence::Validated { mapping };
        Ok(())
    }

    pub fn reject_mapping(&mut self) {
        self.attempted = true;
        let prior = match &self.mapping {
            MappingEvidence::Validated { mapping } => Some(mapping.clone()),
            MappingEvidence::Rejected { prior } => prior.clone(),
            MappingEvidence::Missing => None,
        };
        self.mapping = MappingEvidence::Rejected { prior };
    }

    pub fn record_completion(&mut self, completion: Completion) -> Result<(), InvalidEvidence> {
        if !self.attempted || self.readback.is_some() || self.completion.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        if let Completion::Tagged {
            code: Some(code), ..
        } = &completion
        {
            if code.len() > CODE_LIMIT || !code.is_ascii() {
                return Err(InvalidEvidence::Bounds);
            }
        }
        self.completion = Some(completion);
        Ok(())
    }

    pub fn observe(
        &mut self,
        source: MailboxReadback,
        destination: MailboxReadback,
    ) -> Result<(), InvalidEvidence> {
        if self.completion.is_none() {
            return Err(InvalidEvidence::Phase);
        }
        if source.messages.len() > LIMIT || destination.messages.len() > LIMIT {
            return Err(InvalidEvidence::Bounds);
        }
        for m in source
            .messages
            .values()
            .chain(destination.messages.values())
        {
            m.validate()?;
        }
        if source
            .messages
            .keys()
            .any(|u| !self.intent.messages.contains_key(u))
            || destination.messages.keys().any(|u| {
                self.mapping()
                    .is_none_or(|m| !m.pairs.values().any(|v| v == u))
            })
        {
            return Err(InvalidEvidence::Identity);
        }
        self.readback = Some((source, destination));
        Ok(())
    }

    pub fn resolutions(&self) -> BTreeMap<Uid, Resolution> {
        self.intent
            .messages
            .iter()
            .map(|(uid, original)| {
                let result = match (self.mapping(), &self.readback) {
                    (Some(mapping), Some((source, destination))) => {
                        if source.identity != self.intent.source
                            || destination.identity != self.intent.destination
                        {
                            Resolution::Conflict
                        } else if let Some(mapped) = mapping.pairs.get(uid) {
                            if source.messages.contains_key(uid) {
                                Resolution::Conflict
                            } else if destination
                                .messages
                                .get(mapped)
                                .is_some_and(|m| m == original)
                            {
                                Resolution::Verified {
                                    destination: *mapped,
                                }
                            } else {
                                Resolution::Conflict
                            }
                        } else {
                            Resolution::Uncertain
                        }
                    }
                    _ if matches!(self.mapping, MappingEvidence::Rejected { .. }) => {
                        Resolution::Conflict
                    }
                    _ => Resolution::Uncertain,
                };
                (*uid, result)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
