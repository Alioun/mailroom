use super::imap_move::{Completion, LIMIT, MailboxIdentity, Purpose, TaggedStatus, Uid};
use crate::census::folder_provisioning::ExactName;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const INBOX: &str = "\\Inbox";
const EXCLUDED: [&str; 5] = ["\\Sent", "\\Draft", "\\Drafts", "\\Spam", "\\Trash"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GmailId(String);

impl GmailId {
    pub fn new(value: &str) -> Option<Self> {
        (!value.is_empty()
            && value.len() <= 20
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<u64>().is_ok_and(|n| n != 0)
            && !value.starts_with('0'))
        .then(|| Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GmailId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).ok_or_else(|| serde::de::Error::custom("Invalid X-GM-MSGID"))
    }
}

/// The observed state of one message in the All Mail context. Labels are
/// decoded names; system labels keep their leading backslash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub gmail_id: GmailId,
    pub message_id_hash: String,
    pub size: u32,
    pub internal_date: String,
    pub flags: BTreeSet<String>,
    pub labels: BTreeSet<String>,
}

fn printable(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

impl Snapshot {
    pub fn validate(&self) -> bool {
        printable(&self.message_id_hash, 128)
            && self.message_id_hash.bytes().all(|b| b.is_ascii_graphic())
            && printable(&self.internal_date, 64)
            && self.internal_date.is_ascii()
            && self.flags.len() <= 64
            && self.flags.iter().all(|f| {
                printable(f, 128)
                    && f.bytes()
                        .all(|b| b.is_ascii_graphic() && !b"(){%*\"".contains(&b))
                    && !f.eq_ignore_ascii_case("\\Recent")
            })
            && self.labels.len() <= 128
            && self.labels.iter().all(|l| printable(l, 1024))
    }
    pub fn same_message(&self, other: &Self) -> bool {
        self.gmail_id == other.gmail_id
            && self.message_id_hash == other.message_id_hash
            && self.size == other.size
            && self.internal_date == other.internal_date
    }
    fn protected(&self, protect_flagged: bool) -> bool {
        self.flags.iter().any(|f| {
            f.eq_ignore_ascii_case("\\Deleted")
                || f.eq_ignore_ascii_case("\\Draft")
                || (protect_flagged && f.eq_ignore_ascii_case("\\Flagged"))
        }) || self
            .labels
            .iter()
            .any(|l| EXCLUDED.iter().any(|e| l.eq_ignore_ascii_case(e)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawIntent")]
pub struct LabelIntent {
    purpose: Purpose,
    context: MailboxIdentity,
    destination: ExactName,
    messages: BTreeMap<Uid, Snapshot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent {
    purpose: Purpose,
    context: MailboxIdentity,
    destination: ExactName,
    messages: BTreeMap<Uid, Snapshot>,
}

impl TryFrom<RawIntent> for LabelIntent {
    type Error = &'static str;
    fn try_from(raw: RawIntent) -> Result<Self, Self::Error> {
        Self::new(raw.purpose, raw.context, raw.destination, raw.messages)
            .ok_or("Invalid Gmail label intent")
    }
}

pub fn user_label(name: &ExactName) -> bool {
    ExactName::from_wire(&name.wire).is_ok_and(|n| n == *name)
        && !name.decoded.starts_with('\\')
        && !name.decoded.eq_ignore_ascii_case("INBOX")
        && !name.decoded.starts_with("[Gmail]")
        && !name.decoded.split('/').any(str::is_empty)
}

impl LabelIntent {
    pub fn new(
        purpose: Purpose,
        context: MailboxIdentity,
        destination: ExactName,
        messages: BTreeMap<Uid, Snapshot>,
    ) -> Option<Self> {
        let ids: BTreeSet<_> = messages.values().map(|m| &m.gmail_id).collect();
        let valid = !messages.is_empty()
            && messages.len() <= LIMIT
            && ids.len() == messages.len()
            && user_label(&destination)
            && ExactName::from_wire(&context.name.wire).is_ok_and(|n| n == context.name)
            && messages.values().all(|m| {
                m.validate()
                    && !matches!(purpose, Purpose::Filing { protect_flagged } if m.protected(protect_flagged))
            });
        valid.then_some(Self {
            purpose,
            context,
            destination,
            messages,
        })
    }
    pub fn purpose(&self) -> Purpose {
        self.purpose
    }
    pub fn context(&self) -> &MailboxIdentity {
        &self.context
    }
    pub fn destination(&self) -> &ExactName {
        &self.destination
    }
    pub fn messages(&self) -> &BTreeMap<Uid, Snapshot> {
        &self.messages
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    AddLabel,
    RemoveInbox,
    RestoreInbox,
    RemoveLabel,
}

impl StepKind {
    pub fn adds(self) -> bool {
        matches!(self, Self::AddLabel | Self::RestoreInbox)
    }
    pub fn inbox(self) -> bool {
        matches!(self, Self::RemoveInbox | Self::RestoreInbox)
    }
    pub fn label(self, destination: &ExactName) -> &str {
        if self.inbox() {
            INBOX
        } else {
            &destination.decoded
        }
    }
    fn order(purpose: Purpose) -> [Self; 2] {
        match purpose {
            Purpose::Filing { .. } => [Self::AddLabel, Self::RemoveInbox],
            Purpose::ConditionalUndo => [Self::RestoreInbox, Self::RemoveLabel],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelReadback {
    pub identity: MailboxIdentity,
    pub messages: BTreeMap<Uid, Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub kind: StepKind,
    pub uids: BTreeSet<Uid>,
    attempted: bool,
    completion: Option<Completion>,
    readback: Option<LabelReadback>,
}

impl Step {
    pub fn attempted(&self) -> bool {
        self.attempted
    }
    pub fn completion(&self) -> Option<&Completion> {
        self.completion.as_ref()
    }
    pub fn readback(&self) -> Option<&LabelReadback> {
        self.readback.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pending,
    Verified,
    Changed,
    Unapplied,
    Uncertain,
    Conflict,
}

impl Outcome {
    /// The step's own label delta is attributable to this run.
    pub fn applied(self) -> bool {
        matches!(self, Self::Verified | Self::Changed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Bounds,
    Phase,
    Identity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBatch")]
pub struct LabelBatch {
    intent: LabelIntent,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBatch {
    intent: LabelIntent,
    steps: Vec<Step>,
}

impl TryFrom<RawBatch> for LabelBatch {
    type Error = &'static str;
    fn try_from(raw: RawBatch) -> Result<Self, Self::Error> {
        let plan = raw
            .steps
            .iter()
            .map(|s| (s.kind, s.uids.clone()))
            .collect::<Vec<_>>();
        let mut batch = Self::with_steps(raw.intent, plan).map_err(|_| "Invalid label steps")?;
        for (index, step) in raw.steps.into_iter().enumerate() {
            if !step.attempted {
                if step.completion.is_some() || step.readback.is_some() {
                    return Err("Evidence precedes attempt");
                }
                continue;
            }
            batch.begin_attempt(index).map_err(|_| "Invalid attempt")?;
            if let Some(completion) = step.completion {
                batch
                    .record_completion(index, completion)
                    .map_err(|_| "Invalid completion")?;
            }
            if let Some(readback) = step.readback {
                batch
                    .observe(index, readback)
                    .map_err(|_| "Invalid readback")?;
            }
        }
        Ok(batch)
    }
}

impl LabelBatch {
    /// Approval-time steps: add the destination label where it is missing, then
    /// remove only Inbox where it is present.
    pub fn forward(intent: LabelIntent) -> Result<Self, Invalid> {
        let label = &intent.destination.decoded;
        let add = intent
            .messages
            .iter()
            .filter(|(_, m)| !m.labels.contains(label))
            .map(|(u, _)| *u)
            .collect();
        let remove = intent
            .messages
            .iter()
            .filter(|(_, m)| m.labels.contains(INBOX))
            .map(|(u, _)| *u)
            .collect();
        Self::with_steps(
            intent,
            vec![(StepKind::AddLabel, add), (StepKind::RemoveInbox, remove)],
        )
    }

    /// Reverse steps over the current state: restore Inbox, then remove the
    /// label this run added.
    pub fn reverse(
        intent: LabelIntent,
        restore: BTreeSet<Uid>,
        remove: BTreeSet<Uid>,
    ) -> Result<Self, Invalid> {
        Self::with_steps(
            intent,
            vec![
                (StepKind::RestoreInbox, restore),
                (StepKind::RemoveLabel, remove),
            ],
        )
    }

    fn with_steps(
        intent: LabelIntent,
        plan: Vec<(StepKind, BTreeSet<Uid>)>,
    ) -> Result<Self, Invalid> {
        let order = StepKind::order(intent.purpose);
        let plan: Vec<_> = plan.into_iter().filter(|(_, u)| !u.is_empty()).collect();
        let mut position = 0;
        let mut covered = BTreeSet::new();
        for (kind, uids) in &plan {
            let Some(at) = order[position..].iter().position(|k| k == kind) else {
                return Err(Invalid::Phase);
            };
            position += at + 1;
            for uid in uids {
                let m = intent.messages.get(uid).ok_or(Invalid::Identity)?;
                let has = m.labels.contains(kind.label(&intent.destination));
                if has == kind.adds() {
                    return Err(Invalid::Identity);
                }
                covered.insert(*uid);
            }
        }
        if plan.is_empty() || covered.len() != intent.messages.len() {
            return Err(Invalid::Bounds);
        }
        Ok(Self {
            intent,
            steps: plan
                .into_iter()
                .map(|(kind, uids)| Step {
                    kind,
                    uids,
                    attempted: false,
                    completion: None,
                    readback: None,
                })
                .collect(),
        })
    }

    pub fn intent(&self) -> &LabelIntent {
        &self.intent
    }
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
    pub fn step(&self, kind: StepKind) -> Option<&Step> {
        self.steps.iter().find(|s| s.kind == kind)
    }

    /// The state each member of a step must have immediately before and after
    /// the step: the approved snapshot plus this run's earlier label deltas.
    pub fn expected(&self, index: usize) -> BTreeMap<Uid, (Snapshot, Snapshot)> {
        let step = &self.steps[index];
        step.uids
            .iter()
            .map(|uid| {
                let mut before = self.intent.messages[uid].clone();
                for earlier in &self.steps[..index] {
                    if earlier.uids.contains(uid) {
                        apply(&mut before, earlier.kind, &self.intent.destination);
                    }
                }
                let mut after = before.clone();
                apply(&mut after, step.kind, &self.intent.destination);
                (*uid, (before, after))
            })
            .collect()
    }

    pub fn begin_attempt(&mut self, index: usize) -> Result<(), Invalid> {
        if index >= self.steps.len()
            || self.steps[index].attempted
            || self.steps[..index]
                .iter()
                .any(|s| !s.attempted || s.readback.is_none())
        {
            return Err(Invalid::Phase);
        }
        self.steps[index].attempted = true;
        Ok(())
    }

    pub fn record_completion(
        &mut self,
        index: usize,
        completion: Completion,
    ) -> Result<(), Invalid> {
        let step = self.steps.get_mut(index).ok_or(Invalid::Phase)?;
        if !step.attempted || step.completion.is_some() || step.readback.is_some() {
            return Err(Invalid::Phase);
        }
        if let Completion::Tagged {
            code: Some(code), ..
        } = &completion
        {
            if code.len() > 16 * 1024 || !code.is_ascii() {
                return Err(Invalid::Bounds);
            }
        }
        step.completion = Some(completion);
        Ok(())
    }

    pub fn observe(&mut self, index: usize, readback: LabelReadback) -> Result<(), Invalid> {
        let step = self.steps.get_mut(index).ok_or(Invalid::Phase)?;
        if step.completion.is_none() || step.readback.is_some() {
            return Err(Invalid::Phase);
        }
        if readback.messages.keys().any(|u| !step.uids.contains(u))
            || !readback.messages.values().all(Snapshot::validate)
        {
            return Err(Invalid::Identity);
        }
        step.readback = Some(readback);
        Ok(())
    }

    pub fn outcomes(&self, index: usize) -> BTreeMap<Uid, Outcome> {
        let step = &self.steps[index];
        let label = step.kind.label(&self.intent.destination);
        self.expected(index)
            .into_iter()
            .map(|(uid, (before, after))| {
                let outcome = match (step.attempted, &step.completion, &step.readback) {
                    (false, _, _) => Outcome::Pending,
                    (true, Some(completion), Some(readback)) => match readback.messages.get(&uid) {
                        _ if readback.identity != self.intent.context => Outcome::Conflict,
                        None => Outcome::Conflict,
                        Some(current) if !current.same_message(&before) => Outcome::Conflict,
                        Some(current) => {
                            let delta = current.labels.contains(label) == step.kind.adds();
                            let exact =
                                current.labels == after.labels && current.flags == after.flags;
                            match completion {
                                Completion::Tagged {
                                    status: TaggedStatus::Ok,
                                    ..
                                } if delta && exact => Outcome::Verified,
                                Completion::Tagged {
                                    status: TaggedStatus::Ok,
                                    ..
                                } if delta => Outcome::Changed,
                                Completion::Tagged {
                                    status: TaggedStatus::Ok,
                                    ..
                                } => Outcome::Conflict,
                                Completion::Tagged { .. } if !delta => Outcome::Unapplied,
                                _ => Outcome::Uncertain,
                            }
                        }
                    },
                    _ => Outcome::Uncertain,
                };
                (uid, outcome)
            })
            .collect()
    }

    pub fn attempted(&self) -> bool {
        self.steps.iter().any(|s| s.attempted)
    }
    /// Every attempted step has saved readback.
    pub fn observed(&self) -> bool {
        self.steps
            .iter()
            .all(|s| !s.attempted || s.readback.is_some())
    }
    pub fn verified(&self) -> bool {
        (0..self.steps.len()).all(|i| {
            self.steps[i].attempted && self.outcomes(i).values().all(|o| *o == Outcome::Verified)
        })
    }
    /// Steps whose readback is still missing after an attempt.
    pub fn unobserved(&self) -> Vec<usize> {
        (0..self.steps.len())
            .filter(|i| self.steps[*i].attempted && self.steps[*i].readback.is_none())
            .collect()
    }
    pub fn all_outcomes(&self) -> impl Iterator<Item = Outcome> + '_ {
        (0..self.steps.len()).flat_map(|i| self.outcomes(i).into_values())
    }
}

fn apply(snapshot: &mut Snapshot, kind: StepKind, destination: &ExactName) {
    let label = kind.label(destination).to_owned();
    if kind.adds() {
        snapshot.labels.insert(label);
    } else {
        snapshot.labels.remove(&label);
    }
}

#[cfg(test)]
mod tests;
