mod synthetic;
pub(crate) use synthetic::SyntheticMailbox;

use crate::{
    acquisition::Provider,
    census::{AccountState, Census, CensusKeys, EncryptedStore, ScanState},
    rules::{
        self, CompiledArtifact, Engine, EngineContext,
        proof::{self, Baseline},
    },
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;

const PURPOSE: &[u8] = b"mailroom/backlog-start/v1";
const SEMANTICS: &str = "backlog-flags-preserved-no-copies-v1";
pub const BATCH_LIMIT: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    InboxUnfiled,
    Inbox,
    EveryFolder,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub scope: Scope,
    pub protect_flagged: bool,
    pub selected: Option<BTreeSet<String>>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            scope: Scope::InboxUnfiled,
            protect_flagged: true,
            selected: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoveMethod {
    UidMove,
    CopyUidExpunge,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    Imap {
        method: MoveMethod,
    },
    Gmail {
        add: BTreeSet<String>,
        remove: BTreeSet<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub folder: String,
    pub uidvalidity: u32,
    pub uid: u32,
    pub message_id_hash: String,
    pub gmail_id: Option<String>,
}
impl Identity {
    fn matches(&self, census: &Census) -> bool {
        census.folders.get(&self.folder).is_some_and(|f| {
            f.uidvalidity == self.uidvalidity
                && f.messages.get(&self.uid).is_some_and(|m| {
                    m.headers.message_id_hash.as_ref() == Some(&self.message_id_hash)
                        && m.gmail_message_id == self.gmail_id
                })
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub source: Identity,
    pub flags: BTreeSet<String>,
    pub original_labels: BTreeSet<String>,
    pub destination: String,
    pub sender: String,
    pub operation: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    pub account: String,
    pub provider: Provider,
    pub source: String,
    pub state_revision: u64,
    pub mailbox_revision: Option<u64>,
    pub semantics: String,
    #[serde(with = "crate::rules::program::persisted")]
    pub artifact: CompiledArtifact,
    pub proof_gate: String,
    pub options: Options,
    pub changes: Vec<Change>,
    pub excluded: BTreeMap<String, usize>,
    pub unmatched: usize,
    pub review: usize,
    pub groups: BTreeMap<String, Group>,
    pub stopped_folders: BTreeSet<String>,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Group {
    pub count: usize,
    pub selected: bool,
    pub top_senders: Vec<(String, usize)>,
}
#[derive(Serialize)]
pub struct Preview {
    pub plan: Plan,
    pub receipt: String,
    pub execution_available: bool,
}

pub(crate) fn preview(
    state: &AccountState,
    provider: Provider,
    keys: &CensusKeys,
    options: Options,
    installed: Option<&SyntheticMailbox>,
) -> Result<Preview, String> {
    let mailbox = installed.map(SyntheticMailbox::snapshot);
    let workspace = state
        .workspace
        .as_ref()
        .ok_or("Save the account rule workspace first")?;
    let census = mailbox.map_or(&state.census, |m| &m.census);
    if census.acquisition_state != ScanState::Complete
        || census
            .folders
            .values()
            .any(|f| f.state != ScanState::Complete || f.messages.len() != f.live_uids.len())
    {
        return Err("A complete census is required".into());
    }
    let engine = if provider == Provider::Gmail {
        Engine::GmailBacklog
    } else {
        Engine::MailboxSieve
    };
    let mut context = EngineContext::local(census.account.as_str().into(), engine);
    if mailbox.is_some() {
        context.recipient_evidence = rules::EvidenceSource::SyntheticDeliveredTo;
        context.capabilities = [
            "fileinto",
            "relational",
            "comparator-i;ascii-numeric",
            "imap4flags",
        ]
        .into_iter()
        .map(String::from)
        .collect();
    }
    let artifact = crate::engine::artifact(&workspace.input.documents, &context)?;
    let proof = proof::prove(
        artifact.clone(),
        census,
        workspace.input.as_of,
        &workspace.input.pins,
        Baseline::Current,
        None,
    )?;
    let mut plan = Plan {
        account: census.account.as_str().into(),
        provider,
        source: if mailbox.is_some() {
            "installed-persistent-synthetic-mailbox; fixture recipient evidence only"
        } else {
            "read-only-saved-census; recipient evidence unknown"
        }
        .into(),
        state_revision: state.revision,
        mailbox_revision: mailbox.map(|m| m.revision),
        semantics: SEMANTICS.into(),
        artifact,
        proof_gate: proof.gate.into(),
        options,
        changes: vec![],
        excluded: BTreeMap::new(),
        unmatched: 0,
        review: 0,
        groups: BTreeMap::new(),
        stopped_folders: BTreeSet::new(),
        diagnostics: proof.warnings.clone(),
    };
    if provider != Provider::Gmail
        && plan.options.scope == Scope::InboxUnfiled
        && !census
            .folder_roles
            .values()
            .any(|roles| roles.contains("archive"))
    {
        plan.diagnostics.push("Unfiled scope unsupported: no discovered Archive special-use role. Only evidenced Inbox mail can be considered.".into());
    }
    let decisions: BTreeMap<_, _> = proof.decisions.iter().map(|d| (d.id.as_str(), d)).collect();
    let mut seen = BTreeSet::new();
    let mut senders: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (folder, f) in &census.folders {
        for m in f.messages.values() {
            let id = if provider == Provider::Gmail {
                m.gmail_message_id.as_ref().map(|v| format!("gmail/{v}"))
            } else {
                None
            }
            .unwrap_or_else(|| format!("{folder}/{}/{}", f.uidvalidity, m.uid));
            if !seen.insert(id.clone()) {
                continue;
            }
            let roles = census.folder_roles.get(folder).cloned().unwrap_or_default();
            let labels = &m.gmail_labels;
            let excluded = m.headers.direction == crate::census::Direction::Sent
                || roles.iter().any(|r| {
                    ["sent", "drafts", "junk", "spam", "trash", "noselect"].contains(&r.as_str())
                })
                || labels.iter().any(|l| {
                    [
                        "\\sent", "\\draft", "\\drafts", "\\spam", "\\junk", "\\trash",
                    ]
                    .contains(&l.to_ascii_lowercase().as_str())
                })
                || m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"));
            let inbox = if provider == Provider::Gmail {
                labels.iter().any(|l| l.eq_ignore_ascii_case("\\Inbox"))
            } else {
                folder.eq_ignore_ascii_case("INBOX")
            };
            let unfiled = if provider == Provider::Gmail {
                !labels.iter().any(|l| !l.starts_with('\\'))
            } else {
                roles.contains("archive")
            };
            let in_scope = match plan.options.scope {
                Scope::Inbox => inbox,
                Scope::InboxUnfiled => inbox || unfiled,
                Scope::EveryFolder => true,
            };
            let reason = if excluded {
                Some("excluded_role")
            } else if provider != Provider::Gmail
                && !inbox
                && !census.folder_roles.contains_key(folder)
            {
                Some("unknown_folder_role")
            } else if !in_scope {
                Some("outside_scope")
            } else if plan.options.protect_flagged
                && m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged"))
            {
                Some("flagged")
            } else {
                None
            };
            if let Some(reason) = reason {
                *plan.excluded.entry(reason.into()).or_default() += 1;
                continue;
            }
            let Some(d) = decisions.get(id.as_str()) else {
                *plan.excluded.entry("missing_decision".into()).or_default() += 1;
                continue;
            };
            if d.review {
                plan.review += 1;
            }
            if matches!(d.destination, proof::Destination::Inbox) {
                plan.unmatched += 1;
                continue;
            }
            let dest = d.destination.label();
            if folder == dest && provider != Provider::Gmail {
                *plan.excluded.entry("already_filed".into()).or_default() += 1;
                continue;
            }
            if !plan.artifact.folders().contains(folder) && !inbox && !unfiled {
                plan.stopped_folders.insert(folder.clone());
            }
            let Some(hash) = &m.headers.message_id_hash else {
                *plan.excluded.entry("missing_identity".into()).or_default() += 1;
                continue;
            };
            let operation = if provider == Provider::Gmail {
                if m.gmail_message_id.is_none() {
                    *plan.excluded.entry("missing_gmail_id".into()).or_default() += 1;
                    continue;
                }
                let add = if labels.contains(dest) {
                    BTreeSet::new()
                } else {
                    BTreeSet::from([dest.into()])
                };
                let remove = labels
                    .iter()
                    .filter(|l| l.eq_ignore_ascii_case("\\Inbox"))
                    .cloned()
                    .collect();
                Operation::Gmail { add, remove }
            } else {
                Operation::Imap {
                    method: if installed.is_some_and(|m| !m.capabilities.contains("MOVE")) {
                        MoveMethod::CopyUidExpunge
                    } else {
                        MoveMethod::UidMove
                    },
                }
            };
            if matches!(&operation, Operation::Gmail { add, remove } if add.is_empty() && remove.is_empty())
            {
                *plan.excluded.entry("already_filed".into()).or_default() += 1;
                continue;
            }
            let group = plan.groups.entry(dest.into()).or_default();
            group.count += 1;
            *senders
                .entry(dest.into())
                .or_default()
                .entry(d.from.clone())
                .or_default() += 1;
            plan.changes.push(Change {
                source: Identity {
                    folder: folder.clone(),
                    uidvalidity: f.uidvalidity,
                    uid: m.uid,
                    message_id_hash: hash.clone(),
                    gmail_id: m.gmail_message_id.clone(),
                },
                flags: m.flags.clone(),
                original_labels: labels.clone(),
                destination: dest.into(),
                sender: d.from.clone(),
                operation,
            });
        }
    }
    if let Some(selected) = &plan.options.selected {
        if selected.iter().any(|d| !plan.groups.contains_key(d)) {
            return Err("Unknown selected destination".into());
        }
    }
    for (destination, group) in &mut plan.groups {
        group.selected = plan
            .options
            .selected
            .as_ref()
            .is_none_or(|s| s.contains(destination));
        let mut top: Vec<_> = senders
            .remove(destination)
            .unwrap_or_default()
            .into_iter()
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(5);
        group.top_senders = top;
    }
    plan.changes
        .retain(|c| plan.groups[&c.destination].selected);
    plan.stopped_folders.extend(
        census
            .folders
            .keys()
            .filter(|f| {
                !f.eq_ignore_ascii_case("INBOX")
                    && !plan.artifact.folders().contains(*f)
                    && census.folder_roles.get(*f).is_some_and(BTreeSet::is_empty)
            })
            .cloned(),
    );
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(
            SEMANTICS,
            state,
            mailbox,
            &plan,
            proof::SEMANTICS,
            &proof,
            installed.map(|m| &m.capabilities),
        ))
        .map_err(|_| "Cannot bind backlog plan")?,
    );
    let receipt = keys.binding_for(PURPOSE, &payload);
    let execution_available = mailbox.is_some()
        && plan.proof_gate != "blocked"
        && !plan.changes.is_empty()
        && installed.is_some_and(|m| {
            if provider == Provider::Gmail {
                m.capabilities.contains("X-GM-EXT-1")
            } else {
                m.capabilities.contains("UIDPLUS")
            }
        });
    Ok(Preview {
        plan,
        receipt,
        execution_available,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Prepared,
    Running,
    Stopped,
    Completed,
    Failed,
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub destination: Identity,
    pub commands: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Outcome {
    Pending,
    Intent,
    Applied { observation: Observation },
    Conflict { reason: String },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Undo {
    #[default]
    NotRequested,
    Intent,
    Restored {
        observation: Observation,
    },
    Conflict {
        reason: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub plan: Plan,
    pub status: Status,
    pub outcomes: Vec<Outcome>,
    pub undo: Vec<Undo>,
    pub undo_requested: bool,
    pub batches: usize,
    pub error: Option<String>,
}
pub(crate) fn start(
    state: &mut AccountState,
    preview: Preview,
    receipt: &str,
) -> Result<String, String> {
    if preview.plan.account != state.census.account.as_str()
        || preview.plan.state_revision != state.revision
    {
        return Err("Wrong account or stale plan".into());
    }
    if !preview.execution_available
        || !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes()))
    {
        return Err("Plan changed or synthetic execution is unavailable".into());
    }
    if state.backlog.iter().any(|r| {
        r.id == receipt
            || r.status != Status::Completed
            || (r.undo_requested && r.undo.iter().any(|u| matches!(u, Undo::Intent)))
    }) {
        return Err("Replay or an unfinished run must be recovered first".into());
    }
    if state.backlog.len() >= 32 {
        return Err("Local run history limit reached".into());
    }
    let count = preview.plan.changes.len();
    state.backlog.push(Run {
        id: receipt.into(),
        plan: preview.plan,
        status: Status::Prepared,
        outcomes: vec![Outcome::Pending; count],
        undo: vec![Undo::NotRequested; count],
        undo_requested: false,
        batches: 0,
        error: None,
    });
    Ok(receipt.into())
}
fn run_index(state: &AccountState, id: &str) -> Result<usize, String> {
    state
        .backlog
        .iter()
        .position(|r| r.id == id)
        .ok_or("Unknown account run".into())
}
pub(crate) fn stop(state: &mut AccountState, id: &str) -> Result<(), String> {
    let ri = run_index(state, id)?;
    let r = &mut state.backlog[ri];
    if r.status != Status::Completed {
        r.status = Status::Stopped;
    }
    Ok(())
}
pub(crate) fn resume(state: &mut AccountState, id: &str) -> Result<(), String> {
    let ri = run_index(state, id)?;
    let r = &mut state.backlog[ri];
    if r.undo_requested
        || r.outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Conflict { .. }))
    {
        return Err("Resolve recorded conflicts before continuing".into());
    }
    if r.status != Status::Completed {
        r.status = Status::Prepared;
    }
    Ok(())
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), String> {
    store
        .save_state(state)
        .map_err(|_| "Encrypted journal write failed; retain files and recover".into())
}

pub(crate) fn advance(
    store: &EncryptedStore,
    state: &mut AccountState,
    mailbox: &mut SyntheticMailbox,
    id: &str,
) -> Result<(), String> {
    let ri = run_index(state, id)?;
    if state.backlog[ri].undo_requested {
        return Err("Undo already requested".into());
    }
    if matches!(
        state.backlog[ri].status,
        Status::Completed | Status::Stopped
    ) {
        return Ok(());
    }
    if state.backlog[ri]
        .outcomes
        .iter()
        .any(|o| matches!(o, Outcome::Conflict { .. }))
    {
        state.backlog[ri].status = Status::Uncertain;
        return Ok(());
    }
    let pending: Vec<_> = state.backlog[ri]
        .outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            if matches!(o, Outcome::Pending | Outcome::Intent) {
                Some(i)
            } else {
                None
            }
        })
        .take(BATCH_LIMIT)
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    state.backlog[ri].status = Status::Running;
    for &i in &pending {
        state.backlog[ri].outcomes[i] = Outcome::Intent;
    }
    state.backlog[ri].batches += 1;
    save(store, state)?;
    synthetic::boundary("before");
    for i in pending {
        let c = state.backlog[ri].plan.changes[i].clone();
        let key = format!("{id}/{i}");
        let result = mailbox.apply(&key, &c, state.backlog[ri].plan.options.protect_flagged);
        synthetic::boundary("effect");
        match result {
            Ok(observation) => state.backlog[ri].outcomes[i] = Outcome::Applied { observation },
            Err(reason) => {
                state.backlog[ri].outcomes[i] = Outcome::Conflict { reason };
                state.backlog[ri].status = Status::Uncertain;
            }
        }
        if state.backlog[ri].status == Status::Uncertain {
            save(store, state)?;
            return Ok(());
        }
    }
    state.backlog[ri].status = if state.backlog[ri]
        .outcomes
        .iter()
        .all(|o| matches!(o, Outcome::Applied { .. }))
    {
        Status::Completed
    } else {
        Status::Running
    };
    save(store, state)?;
    synthetic::boundary("complete");
    Ok(())
}
#[derive(Serialize)]
pub struct UndoPreview {
    pub receipt: String,
    pub eligible: Vec<usize>,
    pub conflicts: BTreeMap<usize, String>,
    pub warning: &'static str,
}
pub(crate) fn undo_preview(
    state: &AccountState,
    mailbox: &SyntheticMailbox,
    keys: &CensusKeys,
    id: &str,
) -> Result<UndoPreview, String> {
    let r = &state.backlog[run_index(state, id)?];
    let mut eligible = vec![];
    let mut conflicts = BTreeMap::new();
    for (i, o) in r.outcomes.iter().enumerate() {
        if matches!(r.undo[i], Undo::Restored { .. }) {
            continue;
        }
        match o {
            Outcome::Applied { observation } => {
                match mailbox.eligible(&r.plan.changes[i], observation) {
                    Ok(()) => eligible.push(i),
                    Err(e) => {
                        conflicts.insert(i, e);
                    }
                }
            }
            Outcome::Intent => {
                conflicts.insert(i, "Uncertain effect must be reconciled before undo".into());
            }
            _ => {}
        }
    }
    let receipt = keys.binding_for(
        b"mailroom/backlog-undo/v1",
        &serde_json::to_vec(&(state, mailbox.snapshot(), id, &eligible, &conflicts))
            .map_err(|_| "Cannot bind undo")?,
    );
    Ok(UndoPreview {
        receipt,
        eligible,
        conflicts,
        warning: "Conditional compensation only. Unrelated changes and flags are preserved. Conflicts are retained.",
    })
}
pub(crate) fn undo(
    store: &EncryptedStore,
    state: &mut AccountState,
    mailbox: &mut SyntheticMailbox,
    id: &str,
    receipt: Option<&str>,
) -> Result<(), String> {
    let ri = run_index(state, id)?;
    if !state.backlog[ri].undo_requested {
        let p = undo_preview(state, mailbox, store.keys(), id)?;
        if !receipt.is_some_and(|v| bool::from(p.receipt.as_bytes().ct_eq(v.as_bytes()))) {
            return Err("Undo preview changed; confirm a fresh preview".into());
        }
        if state.backlog[ri]
            .outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Intent))
        {
            return Err("Reconcile uncertain effects before undo".into());
        }
        state.backlog[ri].undo_requested = true;
        state.backlog[ri].status = Status::Stopped;
        for i in p.eligible {
            state.backlog[ri].undo[i] = Undo::Intent;
        }
        for (i, reason) in p.conflicts {
            state.backlog[ri].undo[i] = Undo::Conflict { reason };
        }
        save(store, state)?;
    }
    let pending: Vec<_> = state.backlog[ri]
        .undo
        .iter()
        .enumerate()
        .filter_map(|(i, u)| matches!(u, Undo::Intent).then_some(i))
        .take(BATCH_LIMIT)
        .collect();
    for i in pending {
        let Outcome::Applied { observation } = state.backlog[ri].outcomes[i].clone() else {
            continue;
        };
        let c = state.backlog[ri].plan.changes[i].clone();
        let result = mailbox.undo(&format!("{id}/{i}/undo"), &c, &observation);
        synthetic::boundary("undo-effect");
        state.backlog[ri].undo[i] = match result {
            Ok(observation) => Undo::Restored { observation },
            Err(reason) => Undo::Conflict { reason },
        };
        save(store, state)?;
    }
    if !state.backlog[ri]
        .undo
        .iter()
        .any(|u| matches!(u, Undo::Intent))
    {
        state.backlog[ri].status = Status::Completed;
        save(store, state)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests;
