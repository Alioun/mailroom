use super::*;
use crate::census::{CensusError, FolderCensus};
use std::path::Path;

pub(crate) struct SyntheticMailbox {
    store: EncryptedStore,
    pub(super) state: AccountState,
    pub(crate) capabilities: BTreeSet<String>,
    pub(crate) throttle_remaining: usize,
}
impl SyntheticMailbox {
    pub(crate) fn open(
        root: &Path,
        journal: &EncryptedStore,
        census: &Census,
    ) -> Result<Self, CensusError> {
        let store =
            EncryptedStore::new(root.to_path_buf(), journal.keys().synthetic_mailbox_keys())?;
        if !store.path().exists() {
            store.save(census)?;
        }
        let mut state = store.load_state()?;
        let mut initialized = false;
        for (name, folder) in &state.census.folders {
            if !state.synthetic_uid_next.contains_key(name) {
                let next = folder
                    .live_uids
                    .last()
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(CensusError::InvalidCheckpoint)?;
                state.synthetic_uid_next.insert(name.clone(), next);
                initialized = true;
            }
        }
        if initialized {
            store.save_state(&mut state)?;
        }
        Ok(Self {
            store,
            state,
            capabilities: ["MOVE", "UIDPLUS", "X-GM-EXT-1"]
                .into_iter()
                .map(String::from)
                .collect(),
            throttle_remaining: 0,
        })
    }
    pub(crate) fn snapshot(&self) -> &AccountState {
        &self.state
    }
    pub(super) fn persist(&mut self) -> Result<(), String> {
        self.store
            .save_state(&mut self.state)
            .map_err(|_| "Synthetic mailbox persistence failed; reconcile before retry".into())
    }
    fn backoff(&mut self) -> Result<(), String> {
        for attempt in 0..3 {
            if self.throttle_remaining == 0 {
                return Ok(());
            }
            self.throttle_remaining -= 1;
            std::thread::sleep(std::time::Duration::from_millis(10 << attempt));
        }
        Err("Synthetic throttling exhausted bounded retries; no new effect".into())
    }
    pub(crate) fn apply(
        &mut self,
        key: &str,
        c: &Change,
        protect_flagged: bool,
    ) -> Result<Observation, String> {
        if let Some(observation) = self.state.synthetic_effects.get(key) {
            if !observation.destination.matches(&self.state.census) {
                return Err("Recorded effect identity or UIDVALIDITY changed".into());
            }
            if let Operation::Gmail { add, remove } = &c.operation {
                let m = &self.state.census.folders[&observation.destination.folder].messages
                    [&observation.destination.uid];
                if !add.is_subset(&m.gmail_labels)
                    || remove.iter().any(|l| m.gmail_labels.contains(l))
                {
                    return Err("Recorded Gmail label effect changed".into());
                }
            }
            return Ok(observation.clone());
        }
        self.backoff()?;
        if !c.source.matches(&self.state.census) {
            return Err("Source identity, UIDVALIDITY or message missing; unknown COPYUID outcome is not success".into());
        }
        let mut m = self.state.census.folders[&c.source.folder].messages[&c.source.uid].clone();
        if protect_flagged && m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged")) {
            return Err("Source became flagged after confirmation; protection retained".into());
        }
        if m.gmail_labels.iter().any(|l| {
            [
                "\\sent", "\\draft", "\\drafts", "\\spam", "\\junk", "\\trash",
            ]
            .contains(&l.to_ascii_lowercase().as_str())
        }) {
            return Err("Source entered an excluded system placement after confirmation".into());
        }
        let mut commands = vec![];
        let destination = match &c.operation {
            Operation::Gmail { add, remove } => {
                if !self.capabilities.contains("X-GM-EXT-1") || c.source.gmail_id.is_none() {
                    return Err("Missing safe Gmail identity/capability".into());
                }
                m.gmail_labels.extend(add.iter().cloned());
                for l in remove {
                    m.gmail_labels.remove(l);
                }
                commands.push(format!(
                    "UID STORE {} +X-GM-LABELS destination; -X-GM-LABELS Inbox",
                    m.uid
                ));
                self.state
                    .census
                    .folders
                    .get_mut(&c.source.folder)
                    .unwrap()
                    .messages
                    .insert(m.uid, m);
                c.source.clone()
            }
            Operation::Imap { method } => {
                if !self.capabilities.contains("UIDPLUS") {
                    return Err("COPYUID mapping required".into());
                }
                if *method == MoveMethod::UidMove && !self.capabilities.contains("MOVE") {
                    return Err("Required MOVE capability changed".into());
                }
                match method {
                    MoveMethod::UidMove => {
                        commands.push(format!("UID MOVE {} destination; COPYUID", m.uid))
                    }
                    MoveMethod::CopyUidExpunge => {
                        commands.push(format!("UID COPY {} destination; COPYUID", m.uid));
                        commands.push(format!("UID STORE {} +FLAGS.SILENT (\\Deleted)", m.uid));
                        commands.push(format!("UID EXPUNGE {}", m.uid));
                    }
                }
                self.state
                    .census
                    .folder_roles
                    .entry(c.destination.clone())
                    .or_default();
                let dest = self
                    .state
                    .census
                    .folders
                    .entry(c.destination.clone())
                    .or_insert_with(empty_folder);
                let next_uid = self
                    .state
                    .synthetic_uid_next
                    .entry(c.destination.clone())
                    .or_insert(1);
                let next = *next_uid;
                *next_uid = next.checked_add(1).ok_or("UID overflow")?;
                m.uid = next;
                let identity = Identity {
                    folder: c.destination.clone(),
                    uidvalidity: dest.uidvalidity,
                    uid: next,
                    message_id_hash: c.source.message_id_hash.clone(),
                    gmail_id: c.source.gmail_id.clone(),
                };
                dest.live_uids.insert(next);
                dest.messages.insert(next, m);
                let source = self.state.census.folders.get_mut(&c.source.folder).unwrap();
                source.live_uids.remove(&c.source.uid);
                source.messages.remove(&c.source.uid);
                identity
            }
        };
        let observation = Observation {
            destination,
            commands,
        };
        self.state
            .synthetic_effects
            .insert(key.into(), observation.clone());
        self.persist()?;
        Ok(observation)
    }
    pub(crate) fn eligible(&self, c: &Change, o: &Observation) -> Result<(), String> {
        if !o.destination.matches(&self.state.census) {
            return Err("Destination message or UIDVALIDITY changed; cannot restore".into());
        }
        if let Operation::Gmail { add, remove } = &c.operation {
            let m = &self.state.census.folders[&o.destination.folder].messages[&o.destination.uid];
            if !add.is_subset(&m.gmail_labels) || remove.iter().any(|l| m.gmail_labels.contains(l))
            {
                return Err("This run's Gmail label delta no longer holds".into());
            }
        }
        if let Operation::Imap { .. } = c.operation {
            if self
                .state
                .census
                .folders
                .get(&c.source.folder)
                .is_none_or(|f| f.uidvalidity != c.source.uidvalidity)
            {
                return Err("Original folder UIDVALIDITY changed".into());
            }
        }
        Ok(())
    }
    pub(crate) fn undo(
        &mut self,
        key: &str,
        c: &Change,
        o: &Observation,
    ) -> Result<Observation, String> {
        if let Some(prior) = self.state.synthetic_effects.get(key) {
            if !prior.destination.matches(&self.state.census) {
                return Err("Restored message changed after undo".into());
            }
            return Ok(prior.clone());
        }
        self.eligible(c, o)?;
        let current =
            &self.state.census.folders[&o.destination.folder].messages[&o.destination.uid];
        let operation = match &c.operation {
            Operation::Imap { method } => Operation::Imap {
                method: method.clone(),
            },
            Operation::Gmail { add, remove } => Operation::Gmail {
                add: remove.clone(),
                remove: add.clone(),
            },
        };
        let reverse = Change {
            source: o.destination.clone(),
            flags: current.flags.clone(),
            original_labels: current.gmail_labels.clone(),
            destination: c.source.folder.clone(),
            sender: c.sender.clone(),
            operation,
        };
        self.apply(key, &reverse, false)
    }
}
fn empty_folder() -> FolderCensus {
    FolderCensus {
        uidvalidity: 11,
        state: ScanState::Complete,
        live_uids: BTreeSet::new(),
        messages: BTreeMap::new(),
    }
}

pub(super) fn boundary(name: &str) {
    #[cfg(test)]
    if std::env::var("MAILROOM_BACKLOG_BOUNDARY").is_ok_and(|v| v == name) {
        if let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") {
            let path = std::path::Path::new(&root).join(format!("reached-{name}"));
            std::fs::write(path, b"synthetic crash boundary reached").expect("fixture marker");
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }
    let _ = name;
}
