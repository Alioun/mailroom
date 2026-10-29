use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagChange {
    AddDeleted,
    RemoveOwnedDeleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawFlagIntent")]
pub struct FlagIntent {
    pub(super) before: Readback,
    pub(super) change: FlagChange,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlagIntent {
    before: Readback,
    change: FlagChange,
}

impl TryFrom<RawFlagIntent> for FlagIntent {
    type Error = &'static str;
    fn try_from(raw: RawFlagIntent) -> Result<Self, Self::Error> {
        Self::new(raw.before, raw.change).map_err(|_| "Invalid conditional flag intent")
    }
}

fn deleted(m: &Metadata) -> bool {
    m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"))
}

impl FlagIntent {
    pub(super) fn new(before: Readback, change: FlagChange) -> Result<Self, InvalidEvidence> {
        let threshold = before
            .messages
            .values()
            .next()
            .ok_or(InvalidEvidence::Bounds)?
            .modseq;
        if before.messages.values().any(|m| {
            m.modseq != threshold
                || deleted(&m.metadata) != (change == FlagChange::RemoveOwnedDeleted)
        }) {
            return Err(InvalidEvidence::Identity);
        }
        Ok(Self { before, change })
    }
    pub fn before(&self) -> &Readback {
        &self.before
    }
    pub fn change(&self) -> FlagChange {
        self.change
    }
    pub fn threshold(&self) -> ModSeq {
        self.before
            .messages
            .values()
            .next()
            .expect("nonempty flag intent")
            .modseq
    }
    pub fn command(&self) -> String {
        let sign = match self.change {
            FlagChange::AddDeleted => '+',
            FlagChange::RemoveOwnedDeleted => '-',
        };
        format!(
            "UID STORE {} (UNCHANGEDSINCE {}) {sign}FLAGS.SILENT (\\Deleted)",
            wire_uids(self.before.messages.keys().copied()),
            self.threshold().get()
        )
    }
    fn after(&self, uid: Uid) -> Metadata {
        let mut m = self.before.messages[&uid].metadata.clone();
        match self.change {
            FlagChange::AddDeleted => {
                m.flags.insert("\\Deleted".into());
            }
            FlagChange::RemoveOwnedDeleted => {
                m.flags.retain(|f| !f.eq_ignore_ascii_case("\\Deleted"));
            }
        }
        m
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreResult {
    Tagged {
        status: TaggedStatus,
        #[serde(deserialize_with = "modified_set")]
        modified: Option<BTreeSet<Uid>>,
        #[serde(deserialize_with = "uid_map")]
        updated: BTreeMap<Uid, ModSeq>,
    },
    Lost,
    Rejected,
}

fn modified_set<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<BTreeSet<Uid>>, D::Error> {
    struct Members(BTreeSet<Uid>);
    impl<'de> Deserialize<'de> for Members {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Members;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("at most 500 distinct MODIFIED UIDs")
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Members, A::Error> {
                    let mut found = BTreeSet::new();
                    while let Some(uid) = seq.next_element::<Uid>()? {
                        if found.len() == LIMIT || !found.insert(uid) {
                            return Err(serde::de::Error::custom(
                                "Duplicate or excessive MODIFIED UIDs",
                            ));
                        }
                    }
                    if found.is_empty() {
                        return Err(serde::de::Error::custom("Empty MODIFIED set"));
                    }
                    Ok(Members(found))
                }
            }
            d.deserialize_seq(Visitor)
        }
    }
    Option::<Members>::deserialize(d).map(|value| value.map(|value| value.0))
}

impl StoreResult {
    pub fn tagged(
        status: TaggedStatus,
        modified: Option<&str>,
        updated: BTreeMap<Uid, ModSeq>,
        intent: &FlagIntent,
    ) -> Result<Self, InvalidEvidence> {
        let modified = modified
            .map(|s| {
                if s.len() > 16 * 1024 || !s.is_ascii() {
                    return Err(InvalidEvidence::Bounds);
                }
                let body = s
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .ok_or(InvalidEvidence::Mapping)?;
                let (code, uids) = body.split_once(' ').ok_or(InvalidEvidence::Mapping)?;
                if !code.eq_ignore_ascii_case("MODIFIED") {
                    return Err(InvalidEvidence::Mapping);
                }
                uid_set(uids).map(|uids| uids.into_iter().collect())
            })
            .transpose()?;
        let result = Self::Tagged {
            status,
            modified,
            updated,
        };
        result.validate(intent)?;
        Ok(result)
    }
    fn validate(&self, intent: &FlagIntent) -> Result<(), InvalidEvidence> {
        if let Self::Tagged {
            status,
            modified,
            updated,
        } = self
        {
            if updated.len() > LIMIT
                || modified.as_ref().is_some_and(|uids| {
                    uids.is_empty()
                        || uids.len() > LIMIT
                        || uids.iter().any(|u| !intent.before.messages.contains_key(u))
                })
                || updated.iter().any(|(u, seq)| {
                    !intent.before.messages.contains_key(u)
                        || *seq <= intent.before.highest_modseq
                        || modified.as_ref().is_some_and(|set| set.contains(u))
                })
                || *status == TaggedStatus::Bad && (!updated.is_empty() || modified.is_some())
            {
                return Err(InvalidEvidence::Identity);
            }
        }
        Ok(())
    }
    fn acknowledged(&self, uid: Uid) -> Option<ModSeq> {
        match self {
            Self::Tagged {
                status,
                modified,
                updated,
            } if (*status == TaggedStatus::Ok
                || *status == TaggedStatus::No && modified.is_some())
                && !modified.as_ref().is_some_and(|uids| uids.contains(&uid)) =>
            {
                updated.get(&uid).copied()
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawFlagEvidence")]
pub struct FlagEvidence {
    pub(super) intent: FlagIntent,
    pub(super) result: Option<StoreResult>,
    pub(super) readback: Option<Readback>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlagEvidence {
    intent: FlagIntent,
    result: Option<StoreResult>,
    readback: Option<Readback>,
}
impl TryFrom<RawFlagEvidence> for FlagEvidence {
    type Error = &'static str;
    fn try_from(raw: RawFlagEvidence) -> Result<Self, Self::Error> {
        let mut e = Self::attempt(raw.intent);
        if let Some(result) = raw.result {
            e.record_result(result)
                .map_err(|_| "Invalid STORE result")?;
        }
        if let Some(readback) = raw.readback {
            e.observe(readback).map_err(|_| "Invalid STORE readback")?;
        }
        Ok(e)
    }
}
impl FlagEvidence {
    // Construction records an attempted effect. It never implies permission to transmit it again.
    pub(super) fn attempt(intent: FlagIntent) -> Self {
        Self {
            intent,
            result: None,
            readback: None,
        }
    }
    pub fn intent(&self) -> &FlagIntent {
        &self.intent
    }
    pub fn result(&self) -> Option<&StoreResult> {
        self.result.as_ref()
    }
    pub fn record_result(&mut self, result: StoreResult) -> Result<(), InvalidEvidence> {
        if self.result.is_some() || self.readback.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        if let Err(error) = result.validate(&self.intent) {
            self.result = Some(StoreResult::Rejected);
            return Err(error);
        }
        self.result = Some(result);
        Ok(())
    }
    pub fn observe(&mut self, readback: Readback) -> Result<(), InvalidEvidence> {
        if self.result.is_none() {
            return Err(InvalidEvidence::Phase);
        }
        check_observation(
            self.readback.as_ref().unwrap_or(&self.intent.before),
            &readback,
        )?;
        self.readback = Some(readback);
        Ok(())
    }
    pub fn verified(&self) -> BTreeMap<Uid, VersionedMetadata> {
        let Some(readback) = &self.readback else {
            return BTreeMap::new();
        };
        self.intent
            .before
            .messages
            .keys()
            .filter_map(|uid| {
                let modseq = self.result.as_ref()?.acknowledged(*uid)?;
                let observed = readback.messages.get(uid)?;
                (observed.modseq == modseq && observed.metadata == self.intent.after(*uid))
                    .then(|| (*uid, observed.clone()))
            })
            .collect()
    }
}

pub(super) fn check_observation(
    before: &Readback,
    observed: &Readback,
) -> Result<(), InvalidEvidence> {
    if before.identity != observed.identity
        || observed.highest_modseq < before.highest_modseq
        || observed
            .messages
            .keys()
            .any(|uid| !before.messages.contains_key(uid))
        || observed
            .messages
            .iter()
            .any(|(uid, m)| m.modseq < before.messages[uid].modseq)
    {
        return Err(InvalidEvidence::Identity);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Expunge {
    pub(super) before: Readback,
    pub(super) completion: Option<Completion>,
    pub(super) readback: Option<Readback>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawDeletion")]
pub struct Deletion {
    pub(super) flag: FlagEvidence,
    pub(super) expunge: Option<Expunge>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeletion {
    flag: FlagEvidence,
    expunge: Option<Expunge>,
}
impl TryFrom<RawDeletion> for Deletion {
    type Error = &'static str;
    fn try_from(raw: RawDeletion) -> Result<Self, Self::Error> {
        if raw.flag.intent.change != FlagChange::AddDeleted {
            return Err("Deletion must add Deleted");
        }
        let mut e = Self {
            flag: raw.flag,
            expunge: None,
        };
        if let Some(expunge) = raw.expunge {
            e.begin_expunge(expunge.before)
                .map_err(|_| "Invalid expunge intent")?;
            if let Some(result) = expunge.completion {
                e.record_expunge_completion(result)
                    .map_err(|_| "Invalid expunge completion")?;
            }
            if let Some(readback) = expunge.readback {
                e.observe_expunge(readback)
                    .map_err(|_| "Invalid expunge readback")?;
            }
        }
        Ok(e)
    }
}
impl Deletion {
    pub(super) fn observed(&self) -> bool {
        self.flag.readback.is_some() && self.expunge.as_ref().is_none_or(|e| e.readback.is_some())
    }
    pub fn intent(&self) -> &FlagIntent {
        self.flag.intent()
    }
    pub fn flag(&self) -> &FlagEvidence {
        &self.flag
    }
    pub fn record_store_result(&mut self, result: StoreResult) -> Result<(), InvalidEvidence> {
        if self.expunge.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        self.flag.record_result(result)
    }
    pub fn observe_store(&mut self, readback: Readback) -> Result<(), InvalidEvidence> {
        if self.expunge.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        self.flag.observe(readback)
    }
    pub fn begin_expunge(&mut self, fresh: Readback) -> Result<String, InvalidEvidence> {
        if self.expunge.is_some() || fresh.messages.is_empty() {
            return Err(InvalidEvidence::Phase);
        }
        check_observation(&self.flag.intent.before, &fresh)?;
        let owned = self.flag.verified();
        if fresh
            .messages
            .iter()
            .any(|(uid, m)| owned.get(uid) != Some(m))
        {
            return Err(InvalidEvidence::Identity);
        }
        let command = format!("UID EXPUNGE {}", wire_uids(fresh.messages.keys().copied()));
        self.expunge = Some(Expunge {
            before: fresh,
            completion: None,
            readback: None,
        });
        Ok(command)
    }
    pub fn record_expunge_completion(
        &mut self,
        completion: Completion,
    ) -> Result<(), InvalidEvidence> {
        let expunge = self.expunge.as_mut().ok_or(InvalidEvidence::Phase)?;
        if expunge.completion.is_some() || expunge.readback.is_some() {
            return Err(InvalidEvidence::Phase);
        }
        validate_completion(&completion)?;
        expunge.completion = Some(completion);
        Ok(())
    }
    pub fn observe_expunge(&mut self, readback: Readback) -> Result<(), InvalidEvidence> {
        let expunge = self.expunge.as_mut().ok_or(InvalidEvidence::Phase)?;
        if expunge.completion.is_none() {
            return Err(InvalidEvidence::Phase);
        }
        check_observation(
            expunge.readback.as_ref().unwrap_or(&expunge.before),
            &readback,
        )?;
        expunge.readback = Some(readback);
        Ok(())
    }
    pub fn removed(&self) -> BTreeSet<Uid> {
        self.expunge
            .as_ref()
            .and_then(|e| e.readback.as_ref().map(|r| (e, r)))
            .map(|(e, r)| {
                e.before
                    .messages
                    .keys()
                    .filter(|u| !r.messages.contains_key(u))
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn pending_clear(&self, fresh: Readback) -> Result<FlagIntent, InvalidEvidence> {
        if self.expunge.is_some() || fresh.messages.is_empty() {
            return Err(InvalidEvidence::Phase);
        }
        check_observation(&self.flag.intent.before, &fresh)?;
        let owned = self.flag.verified();
        if fresh
            .messages
            .iter()
            .any(|(uid, m)| owned.get(uid) != Some(m))
        {
            return Err(InvalidEvidence::Identity);
        }
        FlagIntent::new(fresh, FlagChange::RemoveOwnedDeleted)
    }
    pub fn begin_clear(
        &self,
        approved: FlagIntent,
        fresh: Readback,
    ) -> Result<PendingClear, InvalidEvidence> {
        if self.pending_clear(fresh)? != approved {
            return Err(InvalidEvidence::Identity);
        }
        Ok(PendingClear {
            ownership: self.flag.clone(),
            effect: FlagEvidence::attempt(approved),
        })
    }
}

fn validate_completion(completion: &Completion) -> Result<(), InvalidEvidence> {
    if let Completion::Tagged {
        code: Some(code), ..
    } = completion
    {
        if code.len() > 16 * 1024 || !code.is_ascii() {
            return Err(InvalidEvidence::Bounds);
        }
    }
    Ok(())
}
pub(super) fn wire_uids(uids: impl Iterator<Item = Uid>) -> String {
    uids.map(|u| u.get().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPendingClear")]
pub struct PendingClear {
    ownership: FlagEvidence,
    effect: FlagEvidence,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPendingClear {
    ownership: FlagEvidence,
    effect: FlagEvidence,
}
impl TryFrom<RawPendingClear> for PendingClear {
    type Error = &'static str;
    fn try_from(raw: RawPendingClear) -> Result<Self, Self::Error> {
        if raw.ownership.intent.change != FlagChange::AddDeleted {
            return Err("Missing Deleted ownership");
        }
        let owner = Deletion {
            flag: raw.ownership,
            expunge: None,
        };
        if owner
            .pending_clear(raw.effect.intent.before.clone())
            .map_err(|_| "Invalid clear ownership")?
            != raw.effect.intent
        {
            return Err("Different clear intent");
        }
        Ok(Self {
            ownership: owner.flag,
            effect: raw.effect,
        })
    }
}
impl PendingClear {
    pub fn intent(&self) -> &FlagIntent {
        self.effect.intent()
    }
    pub fn record_result(&mut self, result: StoreResult) -> Result<(), InvalidEvidence> {
        self.effect.record_result(result)
    }
    pub fn observe(&mut self, readback: Readback) -> Result<(), InvalidEvidence> {
        self.effect.observe(readback)
    }
    pub fn verified(&self) -> BTreeMap<Uid, VersionedMetadata> {
        self.effect.verified()
    }
}
