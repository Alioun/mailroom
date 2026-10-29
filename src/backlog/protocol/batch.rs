use super::*;
use crate::backlog::imap_copy::{CopyEvidence, Resolution as CopyResolution};
use std::collections::BTreeMap;

pub const COPY_SEMANTICS: &str = "imap-copy-condstore-readback/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CopyKind {
    CopyUidExpunge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyBatch {
    kind: CopyKind,
    evidence: CopyEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Batch {
    Copy(CopyBatch),
    Move(MoveEvidence),
}

impl From<MoveEvidence> for Batch {
    fn from(evidence: MoveEvidence) -> Self {
        Self::Move(evidence)
    }
}
impl Batch {
    pub fn copy(evidence: CopyEvidence) -> Self {
        Self::Copy(CopyBatch {
            kind: CopyKind::CopyUidExpunge,
            evidence,
        })
    }
    pub fn is_copy(&self) -> bool {
        matches!(self, Self::Copy(_))
    }
    pub fn resumable(&self) -> bool {
        match self {
            Self::Move(e) => !e.attempted() || super::verified(self),
            Self::Copy(b) => {
                let next = b.evidence.next_action();
                !next.needs_observation() && next != crate::backlog::imap_copy::NextAction::Conflict
            }
        }
    }
    pub fn as_copy(&self) -> Option<&CopyEvidence> {
        match self {
            Self::Copy(b) => Some(&b.evidence),
            Self::Move(_) => None,
        }
    }
    pub fn as_copy_mut(&mut self) -> Option<&mut CopyEvidence> {
        match self {
            Self::Copy(b) => Some(&mut b.evidence),
            Self::Move(_) => None,
        }
    }
    pub(crate) fn move_mut(&mut self) -> Result<&mut MoveEvidence, InvalidEvidence> {
        match self {
            Self::Move(e) => Ok(e),
            Self::Copy(_) => Err(InvalidEvidence::Phase),
        }
    }
    pub fn command(&self) -> &MoveIntent {
        match self {
            Self::Move(e) => e.command(),
            Self::Copy(b) => b.evidence.intent(),
        }
    }
    fn original_copy(&self) -> &MoveEvidence {
        match self {
            Self::Move(e) => e,
            Self::Copy(b) => b.evidence.copy(),
        }
    }
    pub fn method(&self) -> super::super::MoveMethod {
        if self.is_copy() {
            super::super::MoveMethod::CopyUidExpunge
        } else {
            super::super::MoveMethod::UidMove
        }
    }
    pub fn attempted(&self) -> bool {
        self.original_copy().attempted()
    }
    pub fn completion(&self) -> Option<&Completion> {
        self.original_copy().completion()
    }
    pub fn mapping(&self) -> Option<&CopyUidMapping> {
        self.original_copy().mapping()
    }
    pub fn observed(&self) -> bool {
        match self {
            Self::Move(e) => e.observed(),
            Self::Copy(b) => b.evidence.observed(),
        }
    }
    pub fn resolutions(&self) -> BTreeMap<Uid, Resolution> {
        match self {
            Self::Move(e) => e.resolutions(),
            Self::Copy(b) => b
                .evidence
                .resolutions()
                .into_iter()
                .map(|(uid, r)| {
                    (
                        uid,
                        match r {
                            CopyResolution::Filed { destination } => {
                                Resolution::Verified { destination }
                            }
                            CopyResolution::Conflict => Resolution::Conflict,
                            CopyResolution::Unattempted
                            | CopyResolution::Uncertain
                            | CopyResolution::CopyOnly { .. }
                            | CopyResolution::Marked { .. } => Resolution::Uncertain,
                        },
                    )
                })
                .collect(),
        }
    }
    pub fn uncertain(&self) -> bool {
        match self {
            Self::Move(e) => e
                .resolutions()
                .values()
                .any(|r| *r == Resolution::Uncertain),
            Self::Copy(b) => b
                .evidence
                .resolutions()
                .values()
                .any(|r| *r == CopyResolution::Uncertain),
        }
    }
    pub fn begin_attempt(&mut self) -> Result<(), InvalidEvidence> {
        self.move_mut()?.begin_attempt()
    }
    pub fn record_mapping(&mut self, mapping: CopyUidMapping) -> Result<(), InvalidEvidence> {
        self.move_mut()?.record_mapping(mapping)
    }
    pub fn record_completion(&mut self, result: Completion) -> Result<(), InvalidEvidence> {
        self.move_mut()?.record_completion(result)
    }
    pub fn observe(
        &mut self,
        source: MailboxReadback,
        destination: MailboxReadback,
    ) -> Result<(), InvalidEvidence> {
        self.move_mut()?.observe(source, destination)
    }
}
