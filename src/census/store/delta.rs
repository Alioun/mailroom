//! Frame patches for the fields that grow with the mailbox: the census and the
//! two backlog journals. Serializing those whole on every save made each save
//! cost the size of the account, so their changes are found by comparing typed
//! values and only the changed folders, messages, runs and batches are
//! serialized. The patches have the same shape `patch::diff` produces, so any
//! reader that applies patches to JSON values applies these too.

use super::{CensusError, patch, patch::Patch};
use crate::{
    backlog::{gmail_journal, protocol},
    census::{Census, FolderCensus},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

/// What a save changed in one field.
pub(super) enum Delta {
    Same,
    Patch(Patch),
    /// The change has no compact patch (runs removed by compaction, a new
    /// account), so the save writes a new base instead.
    Rewrite,
}

fn value(v: &impl Serialize) -> Result<Value, CensusError> {
    serde_json::to_value(v).map_err(|_| CensusError::Storage)
}

fn replace(v: &impl Serialize) -> Result<Patch, CensusError> {
    Ok(Patch::Replace { value: value(v)? })
}

/// A JSON diff of two small values, checked by applying it.
fn checked(old: &impl Serialize, new: &impl Serialize) -> Result<Option<Patch>, CensusError> {
    let (old, new) = (value(old)?, value(new)?);
    let Some(change) = patch::diff(&old, &new) else {
        return Ok(None);
    };
    let mut check = old;
    patch::apply(&mut check, change.clone())?;
    if check != new {
        return Err(CensusError::Storage);
    }
    Ok(Some(change))
}

#[derive(Default)]
struct Object {
    set: BTreeMap<String, Patch>,
    remove: Vec<String>,
}
impl Object {
    fn field<T: Serialize + PartialEq>(
        &mut self,
        name: &str,
        old: &T,
        new: &T,
    ) -> Result<(), CensusError> {
        if old != new {
            self.set.insert(name.into(), replace(new)?);
        }
        Ok(())
    }
    /// A field serialized only when present.
    fn optional<T: Serialize + PartialEq>(
        &mut self,
        name: &str,
        old: &Option<T>,
        new: &Option<T>,
    ) -> Result<(), CensusError> {
        match (old, new) {
            (Some(_), None) => self.remove.push(name.into()),
            (_, Some(v)) if old != new => {
                self.set.insert(name.into(), replace(v)?);
            }
            _ => (),
        }
        Ok(())
    }
    /// An array patched element by element; only changed elements are
    /// serialized. A shorter array is replaced whole.
    fn elements<T: Serialize + PartialEq>(
        &mut self,
        name: &str,
        old: &[T],
        new: &[T],
    ) -> Result<(), CensusError> {
        if old == new {
            return Ok(());
        }
        if new.len() < old.len() {
            self.set.insert(name.into(), replace(&new)?);
            return Ok(());
        }
        let mut set = vec![];
        for (index, item) in new.iter().enumerate() {
            match old.get(index) {
                Some(before) if before == item => (),
                Some(before) => {
                    if let Some(change) = checked(before, item)? {
                        set.push((index, change));
                    }
                }
                None => set.push((index, replace(item)?)),
            }
        }
        self.set.insert(
            name.into(),
            Patch::Array {
                len: new.len(),
                set,
            },
        );
        Ok(())
    }
    fn nested(&mut self, name: &str, delta: Delta) -> Option<()> {
        match delta {
            Delta::Same => Some(()),
            Delta::Patch(p) => {
                self.set.insert(name.into(), p);
                Some(())
            }
            Delta::Rewrite => None,
        }
    }
    fn finish(self) -> Delta {
        if self.set.is_empty() && self.remove.is_empty() {
            Delta::Same
        } else {
            Delta::Patch(Patch::Object {
                set: self.set,
                remove: self.remove,
            })
        }
    }
}

/// Runs are only appended or changed in place while their journal is live.
/// Compaction removes runs and writes a new base.
macro_rules! journal_delta {
    ($name:ident, $module:ident) => {
        pub(super) fn $name(
            old: &$module::Journal,
            new: &$module::Journal,
        ) -> Result<Delta, CensusError> {
            if old == new {
                return Ok(Delta::Same);
            }
            let $module::Journal { runs, summaries } = new;
            if runs.len() < old.runs.len() {
                return Ok(Delta::Rewrite);
            }
            let mut set = vec![];
            for (index, run) in runs.iter().enumerate() {
                let Some(before) = old.runs.get(index) else {
                    set.push((index, replace(run)?));
                    continue;
                };
                if before == run {
                    continue;
                }
                let $module::Run {
                    id,
                    receipt,
                    plan,
                    status,
                    stop_requested,
                    batches,
                    reverse,
                    undo_receipt,
                    undo_conflicts,
                    completed_at,
                } = run;
                let mut o = Object::default();
                o.field("id", &before.id, id)?;
                o.field("receipt", &before.receipt, receipt)?;
                o.field("plan", &before.plan, plan)?;
                o.field("status", &before.status, status)?;
                o.field("stop_requested", &before.stop_requested, stop_requested)?;
                o.elements("batches", &before.batches, batches)?;
                o.elements("reverse", &before.reverse, reverse)?;
                o.field("undo_receipt", &before.undo_receipt, undo_receipt)?;
                o.field("undo_conflicts", &before.undo_conflicts, undo_conflicts)?;
                o.optional("completed_at", &before.completed_at, completed_at)?;
                if let Delta::Patch(p) = o.finish() {
                    set.push((index, p));
                }
            }
            let mut o = Object::default();
            if !set.is_empty() || runs.len() != old.runs.len() {
                o.set.insert(
                    "runs".into(),
                    Patch::Array {
                        len: runs.len(),
                        set,
                    },
                );
            }
            match (old.summaries.is_empty(), summaries.is_empty()) {
                (_, true) if !old.summaries.is_empty() => o.remove.push("summaries".into()),
                (true, false) => {
                    o.set.insert("summaries".into(), replace(summaries)?);
                }
                _ => o.elements("summaries", &old.summaries, summaries)?,
            }
            Ok(o.finish())
        }
    };
}
journal_delta!(protocol_journal, protocol);
journal_delta!(gmail_journal, gmail_journal);

pub(super) fn census(old: &Census, new: &Census) -> Result<Delta, CensusError> {
    let Census {
        acquisition,
        winddown_evidence,
        folder_roles,
        account,
        folders,
        confirmed_own_addresses,
        account_login,
        acquisition_state,
        last_completed_at,
    } = new;
    if *account != old.account {
        return Ok(Delta::Rewrite);
    }
    let mut o = Object::default();
    o.field("acquisition", &old.acquisition, acquisition)?;
    o.field(
        "winddown_evidence",
        &old.winddown_evidence,
        winddown_evidence,
    )?;
    o.field("folder_roles", &old.folder_roles, folder_roles)?;
    o.field(
        "confirmed_own_addresses",
        &old.confirmed_own_addresses,
        confirmed_own_addresses,
    )?;
    o.field("account_login", &old.account_login, account_login)?;
    o.field(
        "acquisition_state",
        &old.acquisition_state,
        acquisition_state,
    )?;
    o.field(
        "last_completed_at",
        &old.last_completed_at,
        last_completed_at,
    )?;
    let mut f = Object::default();
    for (name, folder) in folders {
        match old.folders.get(name) {
            None => {
                f.set.insert(name.clone(), replace(folder)?);
            }
            Some(before) if before == folder => (),
            Some(before) => {
                if f.nested(name, folder_delta(before, folder)?).is_none() {
                    return Ok(Delta::Rewrite);
                }
            }
        }
    }
    f.remove = old
        .folders
        .keys()
        .filter(|name| !folders.contains_key(*name))
        .cloned()
        .collect();
    o.nested("folders", f.finish());
    Ok(o.finish())
}

fn folder_delta(old: &FolderCensus, new: &FolderCensus) -> Result<Delta, CensusError> {
    let FolderCensus {
        uidvalidity,
        state,
        live_uids,
        messages,
        highestmodseq,
    } = new;
    let mut o = Object::default();
    o.field("uidvalidity", &old.uidvalidity, uidvalidity)?;
    o.field("highestmodseq", &old.highestmodseq, highestmodseq)?;
    o.field("state", &old.state, state)?;
    o.field("live_uids", &old.live_uids, live_uids)?;
    let mut m = Object::default();
    for (uid, record) in messages {
        if old.messages.get(uid) != Some(record) {
            m.set.insert(uid.to_string(), replace(record)?);
        }
    }
    m.remove = old
        .messages
        .keys()
        .filter(|uid| !messages.contains_key(uid))
        .map(u32::to_string)
        .collect();
    o.nested("messages", m.finish());
    Ok(o.finish())
}

fn typed<T: DeserializeOwned>(v: Value) -> Result<T, CensusError> {
    serde_json::from_value(v).map_err(|_| CensusError::Storage)
}

/// Applies a patch to a typed value through its JSON form. Used below the
/// granularity the census patch is applied at.
fn through_value<T: Serialize + DeserializeOwned>(
    target: &mut T,
    change: Patch,
) -> Result<(), CensusError> {
    if let Patch::Replace { value } = change {
        *target = typed(value)?;
        return Ok(());
    }
    let mut v = value(target)?;
    patch::apply(&mut v, change)?;
    *target = typed(v)?;
    Ok(())
}

/// Applies a census patch without converting the whole census to JSON.
pub(super) fn apply_census(census: &mut Census, change: Patch) -> Result<(), CensusError> {
    let (set, remove) = match change {
        Patch::Replace { value } => {
            *census = typed(value)?;
            return Ok(());
        }
        Patch::Object { set, remove } if remove.is_empty() => (set, remove),
        _ => return Err(CensusError::Storage),
    };
    debug_assert!(remove.is_empty());
    for (name, change) in set {
        match name.as_str() {
            "acquisition" => through_value(&mut census.acquisition, change)?,
            "winddown_evidence" => through_value(&mut census.winddown_evidence, change)?,
            "folder_roles" => through_value(&mut census.folder_roles, change)?,
            "confirmed_own_addresses" => {
                through_value(&mut census.confirmed_own_addresses, change)?
            }
            "account_login" => through_value(&mut census.account_login, change)?,
            "acquisition_state" => through_value(&mut census.acquisition_state, change)?,
            "last_completed_at" => through_value(&mut census.last_completed_at, change)?,
            "folders" => apply_folders(&mut census.folders, change)?,
            _ => return Err(CensusError::Storage),
        }
    }
    Ok(())
}

fn apply_folders(
    folders: &mut BTreeMap<String, FolderCensus>,
    change: Patch,
) -> Result<(), CensusError> {
    let (set, remove) = match change {
        Patch::Replace { value } => {
            *folders = typed(value)?;
            return Ok(());
        }
        Patch::Object { set, remove } => (set, remove),
        Patch::Array { .. } => return Err(CensusError::Storage),
    };
    for name in remove {
        folders.remove(&name).ok_or(CensusError::Storage)?;
    }
    for (name, change) in set {
        match change {
            Patch::Replace { value } => {
                folders.insert(name, typed(value)?);
            }
            Patch::Object { set, remove } if remove.is_empty() => {
                let folder = folders.get_mut(&name).ok_or(CensusError::Storage)?;
                for (field, change) in set {
                    match field.as_str() {
                        "uidvalidity" => through_value(&mut folder.uidvalidity, change)?,
                        "state" => through_value(&mut folder.state, change)?,
                        "live_uids" => through_value(&mut folder.live_uids, change)?,
                        "highestmodseq" => through_value(&mut folder.highestmodseq, change)?,
                        "messages" => apply_messages(&mut folder.messages, change)?,
                        _ => return Err(CensusError::Storage),
                    }
                }
            }
            _ => return Err(CensusError::Storage),
        }
    }
    Ok(())
}

fn apply_messages(
    messages: &mut BTreeMap<u32, crate::census::MessageRecord>,
    change: Patch,
) -> Result<(), CensusError> {
    let (set, remove) = match change {
        Patch::Replace { value } => {
            *messages = typed(value)?;
            return Ok(());
        }
        Patch::Object { set, remove } => (set, remove),
        Patch::Array { .. } => return Err(CensusError::Storage),
    };
    let uid = |key: &str| key.parse::<u32>().map_err(|_| CensusError::Storage);
    for key in remove {
        messages.remove(&uid(&key)?).ok_or(CensusError::Storage)?;
    }
    for (key, change) in set {
        let uid = uid(&key)?;
        match change {
            Patch::Replace { value } => {
                messages.insert(uid, typed(value)?);
            }
            change => through_value(messages.get_mut(&uid).ok_or(CensusError::Storage)?, change)?,
        }
    }
    Ok(())
}
