use super::{AccountId, CensusError, ScanState};
use crate::acquisition::Provider;
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ADAPTER: &str = "mailroom/read-only-imap/v1";
pub const MAX_FOLDERS: usize = 10_000;
pub const MAX_OPTIONAL_COUNTS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownCount {
    NotRequested,
    NotSelectable,
    Budget,
    Denied,
    Unavailable,
    Malformed,
    Missing,
    Ambiguous,
    NoJunkRole,
    OlderSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CountObservation {
    Observed {
        messages: u32,
        observed_at: i64,
        source: CountSource,
    },
    Unknown {
        reason: UnknownCount,
    },
}
impl CountObservation {
    pub fn unknown(reason: UnknownCount) -> Self {
        Self::Unknown { reason }
    }
    pub fn messages(&self) -> Option<usize> {
        match self {
            Self::Observed { messages, .. } => Some(*messages as usize),
            Self::Unknown { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountSource {
    StatusMessages,
    ExamineSearch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Selected,
    NotSelectable,
    SystemRole,
    GmailLabel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderObservation {
    pub wire: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub attributes: BTreeSet<String>,
    pub roles: BTreeSet<String>,
    pub selectable: bool,
    pub selection: Selection,
    pub count: CountObservation,
    pub uidvalidity: Option<u32>,
    pub selected_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub adapter: String,
    pub account: AccountId,
    pub provider: Provider,
    pub attempt_id: String,
    pub topology_observed_at: i64,
    pub completed_at: Option<i64>,
    pub imap_capabilities: BTreeSet<String>,
    pub folders: Vec<FolderObservation>,
}
impl Snapshot {
    pub fn spam(&self) -> CountObservation {
        let junk: Vec<_> = self
            .folders
            .iter()
            .filter(|f| f.roles.contains("junk"))
            .collect();
        match junk.as_slice() {
            [] => CountObservation::unknown(UnknownCount::NoJunkRole),
            [folder] => folder.count.clone(),
            _ => CountObservation::unknown(UnknownCount::Ambiguous),
        }
    }
    fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        let text = |s: &str| s.len() <= 4096 && !s.chars().any(char::is_control);
        if self.version != 1
            || !matches!(
                self.adapter.as_str(),
                ADAPTER | "mailroom/synthetic/v1" | "mailroom/takeout-mbox/v1"
            )
            || &self.account != account
            || self.attempt_id.len() != 32
            || self.folders.len() > MAX_FOLDERS
            || self.imap_capabilities.len() > 256
            || self
                .imap_capabilities
                .iter()
                .any(|c| c.len() > 256 || !text(c))
            || self.folders.iter().any(|f| {
                !text(&f.wire)
                    || !text(&f.name)
                    || f.delimiter
                        .as_ref()
                        .is_some_and(|d| d.chars().count() != 1 || !text(d))
                    || f.attributes.len() > 128
                    || f.roles.len() > 16
                    || f.attributes
                        .iter()
                        .chain(&f.roles)
                        .any(|v| v.len() > 256 || !text(v))
            })
            || self
                .folders
                .iter()
                .map(|f| &f.name)
                .collect::<BTreeSet<_>>()
                .len()
                != self.folders.len()
            || self.completed_at.is_some()
                && self
                    .folders
                    .iter()
                    .any(|f| f.selection == Selection::Selected && !f.selected_complete)
        {
            return Err(CensusError::InvalidCheckpoint);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    pub adapter: String,
    pub provider: Provider,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: ScanState,
    pub session: Option<SessionObservation>,
    pub checkpoint: Option<Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionObservation {
    pub observed_at: i64,
    pub imap_capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionFacts {
    pub latest: Option<Attempt>,
    pub last_successful: Option<Snapshot>,
}
#[derive(Clone, Debug, Serialize)]
pub struct EvidenceView {
    pub latest: Option<Attempt>,
    pub last_successful: Option<Snapshot>,
    pub age_seconds: Option<i64>,
    pub current: bool,
    pub spam: CountObservation,
}
impl EvidenceView {
    pub(crate) fn historical(&mut self) {
        self.current = false;
        self.spam = CountObservation::unknown(UnknownCount::OlderSnapshot);
    }
}
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("UTC clock")
        .as_secs() as i64
}
impl AcquisitionFacts {
    pub(crate) fn begin(&mut self, provider: Provider, adapter: &str) {
        let mut bytes = [0; 16];
        OsRng.fill_bytes(&mut bytes);
        self.latest = Some(Attempt {
            id: data_encoding::HEXLOWER.encode(&bytes),
            adapter: adapter.into(),
            provider,
            started_at: now(),
            finished_at: None,
            state: ScanState::Running,
            session: None,
            checkpoint: None,
        });
    }
    pub(crate) fn finish(&mut self, state: ScanState) {
        if let Some(a) = &mut self.latest {
            a.state = state;
            a.finished_at = Some(now());
        }
    }
    pub(crate) fn running(&self) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|a| a.state == ScanState::Running)
    }
    pub(crate) fn interrupted(&mut self) {
        if let Some(a) = &mut self.latest {
            if a.state == ScanState::Running {
                a.state = ScanState::Cancelled;
            }
        }
    }
    pub(crate) fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        if let Some(a) = &self.latest {
            if !matches!(
                a.adapter.as_str(),
                ADAPTER | "mailroom/synthetic/v1" | "mailroom/takeout-mbox/v1"
            ) || a.id.len() != 32
                || !a.id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(CensusError::InvalidCheckpoint);
            }
            if a.session.as_ref().is_some_and(|s| {
                s.imap_capabilities.len() > 256
                    || s.imap_capabilities
                        .iter()
                        .any(|c| c.len() > 256 || c.chars().any(char::is_control))
            }) {
                return Err(CensusError::InvalidCheckpoint);
            }
            if let Some(s) = &a.checkpoint {
                s.validate(account)?;
                if s.attempt_id != a.id || s.provider != a.provider || s.adapter != a.adapter {
                    return Err(CensusError::InvalidCheckpoint);
                }
            }
        }
        if let Some(s) = &self.last_successful {
            s.validate(account)?;
            if s.completed_at.is_none() {
                return Err(CensusError::InvalidCheckpoint);
            }
        }
        Ok(())
    }
    pub fn is_protocol(&self) -> bool {
        self.latest.as_ref().is_some_and(|a| a.adapter == ADAPTER)
    }
    pub fn view(&self, as_of: i64) -> EvidenceView {
        let current = self
            .latest
            .as_ref()
            .zip(self.last_successful.as_ref())
            .is_some_and(|(a, s)| {
                a.state == ScanState::Complete
                    && a.id == s.attempt_id
                    && s.completed_at.is_some_and(|t| t <= as_of)
            });
        EvidenceView {
            latest: self.latest.clone(),
            last_successful: self.last_successful.clone(),
            current,
            age_seconds: self
                .last_successful
                .as_ref()
                .and_then(|s| s.completed_at)
                .and_then(|t| as_of.checked_sub(t))
                .filter(|a| *a >= 0),
            spam: if current {
                self.last_successful
                    .as_ref()
                    .expect("current snapshot")
                    .spam()
            } else {
                CountObservation::unknown(UnknownCount::OlderSnapshot)
            },
        }
    }
}
