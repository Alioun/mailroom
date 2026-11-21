use super::{
    AccountId, CensusError,
    sieve::{Body, Capabilities, Context, ScriptName},
};
use crate::rules::composition::{self, Placement};
use serde::{Deserialize, Serialize};

pub const HISTORY: usize = 64;
pub const JOURNAL_BYTES: usize = 4 * 1024 * 1024;
pub const IMAGE_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub capabilities: Capabilities,
    pub names: Vec<ScriptName>,
    pub bodies: Vec<Body>,
}
impl Image {
    pub fn active(&self) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.active)
            .map(|n| n.name.as_str())
    }
    pub(crate) fn body(&self, target: &str) -> Option<&Body> {
        self.bodies.iter().find(|b| b.name == target)
    }
    pub(crate) fn validate(&self) -> bool {
        self.names.len() <= 8
            && self.names.len() == self.bodies.len()
            && self.names.iter().filter(|n| n.active).count() <= 1
            && self.bodies.iter().map(|b| b.bytes.len()).sum::<usize>() <= IMAGE_BYTES
            && self.names.windows(2).all(|ns| ns[0].name < ns[1].name)
            && self.names.iter().zip(&self.bodies).all(|(n, b)| {
                n.name == b.name
                    && super::sieve::valid_script_name(&n.name)
                    && Body::new(b.name.clone(), b.bytes.clone()) == *b
            })
            && self.capabilities.get("VERSION").map(String::as_str) == Some("1.0")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Upload,
    Activate,
    Disable,
    UploadOwned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Deploy,
    Removal,
    Compensation,
}

/// Present on every transaction that writes into a foreign host script. Absent records keep
/// the legacy whole fixture-owned replacement semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub purpose: Purpose,
    pub semantics: String,
    pub placement: Option<Placement>,
    pub owned: Option<String>,
    pub foreign_sha256: String,
    pub existing_rules: Vec<String>,
    pub precedence: Option<crate::rules::composition::Counts>,
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
    Preimage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub effect: Effect,
    pub before: Image,
    pub after: Image,
    pub outcome: Outcome,
    pub acknowledgment: bool,
    pub failure: Option<super::sieve::Failure>,
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
pub struct Transaction {
    pub id: String,
    pub compensates: Option<String>,
    pub context: Context,
    pub target: String,
    pub ownership_epoch: String,
    pub artifact: String,
    pub proof_binding: String,
    pub receipt: String,
    pub before: Image,
    pub after: Image,
    pub status: Status,
    pub validation: Option<Result<(), super::sieve::Failure>>,
    pub space: Option<Result<(), super::sieve::Failure>>,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<Composition>,
}
impl Transaction {
    /// Script names this transaction may write: the host, plus Mailroom's own included script.
    pub(crate) fn writable(&self, name: &str) -> bool {
        name == self.target
            || self
                .composition
                .as_ref()
                .and_then(|c| c.owned.as_deref())
                .is_some_and(|owned| owned == name)
    }
}
/// The exact foreign bytes of a host body, with every Mailroom segment removed.
pub(crate) fn foreign(image: &Image, host: &str) -> Option<String> {
    composition::strip(&image.body(host)?.bytes, host)
        .ok()
        .map(|(f, _)| f)
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub history: Vec<Transaction>,
}
impl Journal {
    pub(crate) fn interrupted(&mut self) {
        for t in &mut self.history {
            if t.status == Status::Running {
                t.status = if t.steps.is_empty() {
                    Status::Stopped
                } else {
                    Status::Uncertain
                };
                if let Some(step) = t.steps.last_mut() {
                    if matches!(step.outcome, Outcome::Intent | Outcome::Acknowledged) {
                        step.outcome = Outcome::Uncertain;
                    }
                }
            }
        }
    }
    pub(crate) fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        fn digest(s: &str) -> bool {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        if self.history.len() > HISTORY {
            return Err(CensusError::Storage);
        }
        for (index, t) in self.history.iter().enumerate() {
            super::sieve::validate_context(&t.context, account)?;
            let same_unrelated = |a: &Image, b: &Image| {
                a.capabilities == b.capabilities
                    && a.names
                        .iter()
                        .all(|n| b.names.iter().any(|m| m.name == n.name) || t.writable(&n.name))
                    && b.names
                        .iter()
                        .all(|n| a.names.iter().any(|m| m.name == n.name) || t.writable(&n.name))
                    && a.bodies
                        .iter()
                        .filter(|b| !t.writable(&b.name))
                        .eq(b.bodies.iter().filter(|b| !t.writable(&b.name)))
            };
            let composed = t.composition.as_ref().is_none_or(|c| {
                let foreign_before = foreign(&t.before, &t.target);
                [composition::SEMANTICS, composition::LEGACY_SEMANTICS]
                    .contains(&c.semantics.as_str())
                    && c.precedence
                        .as_ref()
                        .is_none_or(|p| p.matches(&c.semantics))
                    && digest(&c.foreign_sha256)
                    && c.existing_rules.len() <= 256
                    && foreign_before.is_some()
                    && foreign_before == foreign(&t.after, &t.target)
                    && t.before.active() == t.after.active()
                    && t.before.names.iter().all(|n| t.after.names.contains(n))
                    && (c.purpose == Purpose::Compensation) == t.compensates.is_some()
                    && match (&c.purpose, &c.placement, &c.owned) {
                        (Purpose::Deploy, Some(p), owned) => {
                            p.valid_for(&t.target)
                                && owned.as_deref() == p.owned()
                                && owned.as_ref().is_none_or(|o| {
                                    t.after.body(o).is_some_and(|b| {
                                        b.bytes.starts_with(composition::OWNED.as_bytes())
                                    })
                                })
                        }
                        (Purpose::Removal, None, None) => {
                            foreign_before.as_ref().map(String::as_bytes)
                                == t.after.body(&t.target).map(|b| b.bytes.as_slice())
                        }
                        (Purpose::Compensation, _, _) => true,
                        _ => false,
                    }
                    && t.steps.iter().all(|s| {
                        matches!(s.effect, Effect::Upload | Effect::UploadOwned)
                            && foreign(&s.after, &t.target) == foreign_before
                    })
            });
            if [&t.id, &t.receipt, &t.artifact, &t.proof_binding]
                .iter()
                .any(|s| !digest(s))
                || t.ownership_epoch.is_empty()
                || t.ownership_epoch.len() > 256
                || !super::sieve::valid_script_name(&t.target)
                || !t.before.validate()
                || !t.after.validate()
                || !composed
                || t.steps.len() > 2
                || t.before.body(&t.target).is_none()
                || t.after.body(&t.target).is_none()
                || !same_unrelated(&t.before, &t.after)
                || [t.before.active(), t.after.active()]
                    .iter()
                    .any(|a| a.is_some_and(|n| n != t.target))
                || t.steps.first().is_some_and(|s| s.before != t.before)
                || (t.status == Status::Complete
                    && (t.steps.is_empty()
                        || t.steps.last().is_none_or(|s| s.after != t.after)
                        || t.steps.iter().any(|s| s.outcome != Outcome::Observed)))
                || self.history[..index].iter().any(|p| p.id == t.id)
                || t.compensates.as_ref().is_some_and(|id| {
                    !self.history[..index]
                        .iter()
                        .any(|p| &p.id == id && p.compensates.is_none())
                })
                || t.steps.iter().any(|s| {
                    !s.before.validate()
                        || !s.after.validate()
                        || !same_unrelated(&s.before, &s.after)
                        || match s.effect {
                            Effect::Upload if t.composition.is_some() => {
                                s.before.names != s.after.names
                                    || s.after.body(&t.target) != t.after.body(&t.target)
                                    || s.before.bodies.iter().filter(|b| b.name != t.target).ne(s
                                        .after
                                        .bodies
                                        .iter()
                                        .filter(|b| b.name != t.target))
                            }
                            Effect::Upload => {
                                s.before.names != s.after.names || s.after.bodies != t.after.bodies
                            }
                            Effect::UploadOwned => {
                                let owned = t.composition.as_ref().and_then(|c| c.owned.as_deref());
                                owned.is_none_or(|o| {
                                    s.after.body(o) != t.after.body(o)
                                        || s.before.bodies.iter().filter(|b| b.name != o).ne(s
                                            .after
                                            .bodies
                                            .iter()
                                            .filter(|b| b.name != o))
                                })
                            }
                            Effect::Activate => {
                                s.before.bodies != s.after.bodies
                                    || s.after.active() != Some(t.target.as_str())
                            }
                            Effect::Disable => {
                                s.before.bodies != s.after.bodies
                                    || s.after.active().is_some()
                                    || t.compensates.is_none()
                            }
                        }
                        || (s.outcome == Outcome::Acknowledged && !s.acknowledgment)
                        || (s.acknowledgment && s.outcome == Outcome::Intent)
                })
                || t.steps
                    .windows(2)
                    .any(|s| s[0].after != s[1].before || s[0].outcome != Outcome::Observed)
            {
                return Err(CensusError::Storage);
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| CensusError::Storage)?
            .len()
            > JOURNAL_BYTES
        {
            return Err(CensusError::Storage);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeProvider {
    pub address: std::net::SocketAddr,
    pub root: Vec<u8>,
    pub key: Vec<u8>,
    pub epoch: String,
    pub image: Image,
    pub commands: Vec<String>,
}
