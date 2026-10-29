//! Undo retention for backlog sort runs (owner decision Q27).
//!
//! A finished run stays fully undoable for 90 days from its completion. After
//! that its per-message journal is replaced by a [`Summary`] of counts, folders
//! and dates, and Undo is no longer offered. A run that is uncertain, in
//! conflict, stopped or mid-undo is not finished and is never compacted.
//! Compaction replaces the run with its summary in one journaled save, so a
//! crash leaves either the full run or the summary, never a mix.

use super::{Change, protocol::Status};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const WINDOW_SECS: u64 = 90 * 24 * 60 * 60;
/// Generous bound on retained summaries: one daily run for over 270 years.
pub const SUMMARIES: usize = 100_000;
const FOLDER_PAIRS: usize = 4096;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_secs()
}

pub fn deadline(completed_at: u64) -> u64 {
    completed_at.saturating_add(WINDOW_SECS)
}

/// Undo may be offered unless the run finished at least 90 days ago.
pub fn undo_open(completed_at: Option<u64>, now: u64) -> bool {
    completed_at.is_none_or(|c| now < deadline(c))
}

/// What remains of a compacted run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub receipt: String,
    pub status: Status,
    pub completed_at: u64,
    pub compacted_at: u64,
    pub messages: usize,
    pub batches: usize,
    /// Messages whose forward change was verified.
    pub applied: usize,
    /// Messages whose undo was verified.
    pub restored: usize,
    /// Extra copies removed while their original messages remained.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub copies_removed: usize,
    /// Planned messages per source folder and destination.
    pub folders: BTreeMap<String, BTreeMap<String, usize>>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Summary {
    pub(crate) fn valid(&self) -> bool {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let pairs: usize = self.folders.values().map(BTreeMap::len).sum();
        let total: usize = self.folders.values().flat_map(BTreeMap::values).sum();
        digest(&self.id)
            && digest(&self.receipt)
            && matches!(self.status, Status::Complete | Status::Undone)
            && self.compacted_at >= deadline(self.completed_at)
            && self.applied <= self.messages
            && self.restored <= self.applied
            && self.copies_removed <= self.messages.saturating_sub(self.applied)
            && (self.status == Status::Undone || self.restored == 0 && self.copies_removed == 0)
            && pairs <= FOLDER_PAIRS
            && total == self.messages
            && self.folders.iter().all(|(source, targets)| {
                !source.is_empty()
                    && source.len() <= 1024
                    && !targets.is_empty()
                    && targets
                        .iter()
                        .all(|(d, n)| !d.is_empty() && d.len() <= 1024 && *n > 0)
            })
    }
}

/// Planned messages per source folder and destination, capped so a summary
/// stays small. Pairs past the cap are folded into the last listed pair's
/// source under the destination `…`.
pub(crate) fn folders(changes: &[Change]) -> BTreeMap<String, BTreeMap<String, usize>> {
    let mut all: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for c in changes {
        *all.entry((&c.source.folder, &c.destination)).or_default() += 1;
    }
    let mut folders: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut overflow = 0;
    for (index, ((source, destination), count)) in all.into_iter().enumerate() {
        if index + 1 < FOLDER_PAIRS {
            *folders
                .entry(source.into())
                .or_default()
                .entry(destination.into())
                .or_default() += count;
        } else {
            overflow += count;
        }
    }
    if overflow > 0 {
        *folders
            .entry("…".into())
            .or_default()
            .entry("…".into())
            .or_default() += overflow;
    }
    folders
}

/// The undo deadline of one retained run, for display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Deadline {
    pub run: String,
    pub completed_at: Option<u64>,
    pub undo_until: Option<u64>,
    pub undo_open: bool,
}

impl Deadline {
    pub(crate) fn of(run: &str, completed_at: Option<u64>, now: u64) -> Self {
        Self {
            run: run.into(),
            completed_at,
            undo_until: completed_at.map(deadline),
            undo_open: undo_open(completed_at, now),
        }
    }
}
