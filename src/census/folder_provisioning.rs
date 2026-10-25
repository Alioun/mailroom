use super::{AccountId, CensusError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROFILE: &str = "loopback-imap4rev1-empty-slash-v1";
pub const HISTORY: usize = 8;
pub const NAMES: usize = 256;
pub const STEPS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactName {
    pub decoded: String,
    pub wire: String,
}
impl ExactName {
    pub(crate) fn new(decoded: &str) -> Result<Self, Failure> {
        if !icu_normalizer::ComposingNormalizerBorrowed::new_nfc().is_normalized(decoded) {
            return Err(Failure::UnsupportedName);
        }
        if decoded.is_empty() || decoded.len() > 1024 || decoded.chars().any(char::is_control) {
            return Err(Failure::UnsupportedName);
        }
        // Combining sequences and Jamo are outside this bounded profile. Do not normalize them.
        use icu_properties::{CodePointMapData, props::GeneralCategory};
        let categories = CodePointMapData::<GeneralCategory>::new();
        if decoded.chars().any(|c| {
            matches!(
                categories.get(c),
                GeneralCategory::NonspacingMark
                    | GeneralCategory::SpacingMark
                    | GeneralCategory::EnclosingMark
            ) || ('\u{1100}'..='\u{11ff}').contains(&c)
        }) {
            return Err(Failure::UnsupportedName);
        }
        let mut wire = String::new();
        let mut shifted = vec![];
        fn flush(bytes: &mut Vec<u8>, wire: &mut String) {
            if !bytes.is_empty() {
                wire.push('&');
                wire.push_str(&data_encoding::BASE64_NOPAD.encode(bytes).replace('/', ","));
                wire.push('-');
                bytes.clear();
            }
        }
        for c in decoded.chars() {
            if (' '..='~').contains(&c) {
                flush(&mut shifted, &mut wire);
                if c == '&' {
                    wire.push_str("&-");
                } else {
                    wire.push(c);
                }
            } else {
                let mut units = [0; 2];
                for unit in c.encode_utf16(&mut units) {
                    shifted.extend_from_slice(&unit.to_be_bytes());
                }
            }
        }
        flush(&mut shifted, &mut wire);
        if wire.len() > 2048 {
            return Err(Failure::Limit);
        }
        Ok(Self {
            decoded: decoded.into(),
            wire,
        })
    }
    pub(crate) fn from_wire(wire: &str) -> Result<Self, Failure> {
        if wire.len() > 2048 || !wire.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(Failure::UnsupportedName);
        }
        let mut decoded = String::new();
        let mut rest = wire;
        while let Some(i) = rest.find('&') {
            decoded.push_str(&rest[..i]);
            rest = &rest[i + 1..];
            let end = rest.find('-').ok_or(Failure::UnsupportedName)?;
            if end == 0 {
                decoded.push('&');
            } else {
                let part = &rest[..end];
                if part.contains('/') || part.contains('=') {
                    return Err(Failure::UnsupportedName);
                }
                let bytes = data_encoding::BASE64_NOPAD
                    .decode(part.replace(',', "/").as_bytes())
                    .map_err(|_| Failure::UnsupportedName)?;
                if bytes.is_empty() || bytes.len() % 2 != 0 {
                    return Err(Failure::UnsupportedName);
                }
                let units: Vec<_> = bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();
                decoded
                    .push_str(&String::from_utf16(&units).map_err(|_| Failure::UnsupportedName)?);
            }
            rest = &rest[end + 1..];
        }
        decoded.push_str(rest);
        let name = Self::new(&decoded)?;
        if name.wire != wire {
            return Err(Failure::UnsupportedName);
        }
        Ok(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Cancelled,
    Deadline,
    Connect,
    Tls,
    Authentication,
    Unsupported,
    UnsupportedName,
    Protocol,
    No,
    Bad,
    Bye,
    Eof,
    Quota,
    UnknownCode,
    Limit,
    Unstable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    pub prefix: String,
    pub delimiter: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespaces {
    pub personal: Vec<Namespace>,
    pub other: Vec<Namespace>,
    pub shared: Vec<Namespace>,
}
impl Namespaces {
    pub(crate) fn supported(&self) -> bool {
        self.personal
            == [Namespace {
                prefix: String::new(),
                delimiter: Some("/".into()),
            }]
            && self.other.len() <= 8
            && self.shared.len() <= 8
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mailbox {
    pub name: ExactName,
    pub delimiter: Option<String>,
    pub attributes: BTreeSet<String>,
}
impl Mailbox {
    pub(crate) fn selectable(&self) -> bool {
        !self.attributes.contains("\\NOSELECT") && !self.attributes.contains("\\NONEXISTENT")
    }
    pub(crate) fn inferiors(&self) -> bool {
        !self.attributes.contains("\\NOINFERIORS")
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub capabilities: BTreeSet<String>,
    pub namespaces: Namespaces,
    pub mailboxes: Vec<Mailbox>,
}
impl Inventory {
    pub(crate) fn validate(&self) -> Result<(), Failure> {
        if !self.namespaces.supported()
            || !self.capabilities.contains("IMAP4REV1")
            || !self.capabilities.contains("NAMESPACE")
            || self.capabilities.contains("UTF8=ONLY")
            || self.capabilities.contains("IMAP4REV2")
        {
            return Err(Failure::Unsupported);
        }
        if self.mailboxes.len() > NAMES
            || self.capabilities.len() > 128
            || self
                .capabilities
                .iter()
                .any(|c| c.len() > 128 || !c.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err(Failure::Limit);
        }
        let mut aliases = BTreeSet::new();
        let mut previous = None;
        for m in &self.mailboxes {
            if ExactName::from_wire(&m.name.wire)? != m.name
                || m.name.decoded.split('/').any(str::is_empty)
                || m.delimiter.as_deref() != Some("/")
                || m.attributes.len() > 32
                || m.attributes
                    .iter()
                    .any(|a| a.len() > 128 || !a.bytes().all(|b| b.is_ascii_graphic()))
            {
                return Err(Failure::Unsupported);
            }
            if previous.is_some_and(|p: &str| p >= m.name.decoded.as_str())
                || !aliases.insert(m.name.decoded.to_uppercase())
            {
                return Err(Failure::UnsupportedName);
            }
            previous = Some(m.name.decoded.as_str());
        }
        Ok(())
    }
    pub(crate) fn get(&self, decoded: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| m.name.decoded == decoded)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Ancestor,
    Leaf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: ExactName,
    pub role: Role,
    pub existing: Option<Mailbox>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderPlan {
    pub logical_leaves: BTreeSet<String>,
    pub entries: Vec<Entry>,
}
impl FolderPlan {
    pub(crate) fn build(leaves: &BTreeSet<String>, before: &Inventory) -> Result<Self, Failure> {
        before.validate()?;
        if leaves.len() > STEPS {
            return Err(Failure::Limit);
        }
        let mut paths = BTreeMap::new();
        for leaf in leaves {
            let segments: Vec<_> = leaf.split('/').collect();
            if segments.len() > 8
                || segments.iter().any(|s| {
                    s.is_empty()
                        || s.len() > 160
                        || s.eq_ignore_ascii_case("INBOX")
                        || s.chars().all(|c| c == '.')
                })
            {
                return Err(Failure::UnsupportedName);
            }
            for i in 1..=segments.len() {
                let path = segments[..i].join("/");
                paths.insert(
                    path.clone(),
                    if leaves.contains(&path) {
                        Role::Leaf
                    } else {
                        Role::Ancestor
                    },
                );
            }
        }
        if paths.len() > STEPS {
            return Err(Failure::Limit);
        }
        let mut aliases: BTreeMap<String, String> = before
            .mailboxes
            .iter()
            .map(|m| (m.name.decoded.to_uppercase(), m.name.decoded.clone()))
            .collect();
        let mut entries = vec![];
        for (path, role) in paths {
            let name = ExactName::new(&path)?;
            if aliases
                .get(&path.to_uppercase())
                .is_some_and(|p| *p != path)
            {
                return Err(Failure::UnsupportedName);
            }
            aliases.insert(path.to_uppercase(), path.clone());
            let existing = before.get(&path).cloned();
            if existing.as_ref().is_some_and(|m| {
                (role == Role::Leaf && !m.selectable()) || m.attributes.contains("\\NONEXISTENT")
            }) {
                return Err(Failure::Unsupported);
            }
            let mut parent = path.as_str();
            while let Some((p, _)) = parent.rsplit_once('/') {
                if before.get(p).is_some_and(|m| !m.inferiors()) {
                    return Err(Failure::Unsupported);
                }
                parent = p;
            }
            entries.push(Entry {
                name,
                role,
                existing,
            });
        }
        entries.sort_by(|a, b| {
            a.name
                .decoded
                .matches('/')
                .count()
                .cmp(&b.name.decoded.matches('/').count())
                .then(a.name.decoded.cmp(&b.name.decoded))
        });
        Ok(Self {
            logical_leaves: leaves.clone(),
            entries,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub account: AccountId,
    pub source: String,
    pub authority: String,
    pub profile: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Intent,
    Acknowledged,
    Observed,
    Refused,
    Uncertain,
    Conflict,
    Absent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Complete,
    Refused,
    Uncertain,
    Stopped,
    Conflict,
    FreshReview,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: ExactName,
    pub outcome: Outcome,
    pub acknowledgment: bool,
    pub failure: Option<Failure>,
    pub before: Inventory,
    pub observed: Option<Inventory>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    pub context: Context,
    pub artifact: String,
    pub proof_binding: String,
    pub receipt: String,
    pub before: Inventory,
    pub plan: FolderPlan,
    pub status: Status,
    pub steps: Vec<Step>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderJournal {
    pub history: Vec<Operation>,
}
impl FolderJournal {
    pub(crate) fn interrupted(&mut self) {
        for op in &mut self.history {
            if op.status == Status::Running {
                op.status =
                    if op.steps.last().is_some_and(|s| {
                        matches!(s.outcome, Outcome::Intent | Outcome::Acknowledged)
                    }) {
                        Status::Uncertain
                    } else {
                        Status::Stopped
                    };
                if let Some(s) = op.steps.last_mut() {
                    if matches!(s.outcome, Outcome::Intent | Outcome::Acknowledged) {
                        s.outcome = Outcome::Uncertain;
                    }
                }
            }
        }
    }
    pub(crate) fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        let fail = || CensusError::Storage;
        if self.history.len() > HISTORY {
            return Err(fail());
        }
        let mut ids = BTreeSet::new();
        for op in &self.history {
            let digest = |s: &str| {
                s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            };
            if op.context.account != *account
                || op.context.profile != PROFILE
                || !digest(&op.context.source)
                || !digest(&op.context.authority)
                || [&op.id, &op.artifact, &op.proof_binding, &op.receipt]
                    .iter()
                    .any(|s| !digest(s))
                || !ids.insert(&op.id)
                || FolderPlan::build(&op.plan.logical_leaves, &op.before).map_err(|_| fail())?
                    != op.plan
            {
                return Err(fail());
            }
            let missing: Vec<_> = op
                .plan
                .entries
                .iter()
                .filter(|e| e.existing.is_none())
                .collect();
            if op.steps.len() > missing.len() {
                return Err(fail());
            }
            let mut expected = op.before.clone();
            for (i, (step, entry)) in op.steps.iter().zip(missing).enumerate() {
                step.before.validate().map_err(|_| fail())?;
                if step.name != entry.name
                    || step.before != expected
                    || (i + 1 < op.steps.len() && step.outcome != Outcome::Observed)
                    || (step.outcome == Outcome::Acknowledged && !step.acknowledgment)
                    || (step.outcome == Outcome::Intent && step.acknowledgment)
                {
                    return Err(fail());
                }
                if (step.outcome == Outcome::Absent && step.observed.as_ref() != Some(&step.before))
                    || (step.outcome == Outcome::Intent
                        && (step.failure.is_some() || step.observed.is_some()))
                    || (step.outcome == Outcome::Acknowledged
                        && (step.failure.is_some() || step.observed.is_some()))
                    || (step.outcome == Outcome::Refused
                        && (step.acknowledgment
                            || !matches!(
                                step.failure,
                                Some(Failure::No | Failure::Bad | Failure::Quota)
                            )))
                {
                    return Err(fail());
                }
                if let Some(observed) = &step.observed {
                    observed.validate().map_err(|_| fail())?;
                }
                if step.outcome == Outcome::Observed {
                    let observed = step.observed.as_ref().ok_or_else(fail)?;
                    if !desired_delta(&step.before, observed, &step.name) {
                        return Err(fail());
                    }
                    expected = observed.clone();
                }
            }
            if op.status == Status::Complete
                && (op.steps.len()
                    != op
                        .plan
                        .entries
                        .iter()
                        .filter(|e| e.existing.is_none())
                        .count()
                    || op.steps.iter().any(|s| s.outcome != Outcome::Observed))
            {
                return Err(fail());
            }
        }
        if serde_json::to_vec(self).map_err(|_| fail())?.len() > 8 * 1024 * 1024 {
            return Err(fail());
        }
        Ok(())
    }
}
pub(crate) fn desired_delta(before: &Inventory, after: &Inventory, name: &ExactName) -> bool {
    before.capabilities == after.capabilities
        && before.namespaces == after.namespaces
        && after
            .get(&name.decoded)
            .is_some_and(|m| m.name == *name && m.selectable())
        && before
            .mailboxes
            .iter()
            .eq(after.mailboxes.iter().filter(|m| m.name != *name))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeProvider {
    pub address: std::net::SocketAddr,
    pub root: Vec<u8>,
    pub key: Vec<u8>,
    pub epoch: String,
    pub inventory: Inventory,
    pub commands: Vec<String>,
}
impl FakeProvider {
    pub(crate) fn validate(&self) -> bool {
        self.address.ip().is_loopback()
            && !self.epoch.is_empty()
            && self.epoch.len() <= 256
            && self.root.len() <= 8192
            && self.key.len() <= 8192
            && self.commands.len() <= 4096
            && self.commands.iter().all(|c| {
                [
                    "CAPABILITY",
                    "LOGIN",
                    "NAMESPACE",
                    "LIST",
                    "CREATE",
                    "LOGOUT",
                ]
                .contains(&c.as_str())
            })
            && self.inventory.validate().is_ok()
    }
}
