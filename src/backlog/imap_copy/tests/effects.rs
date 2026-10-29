use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Mailboxes {
    source: Readback,
    destination: Readback,
    commands: Vec<String>,
}
impl Mailboxes {
    fn new() -> Self {
        let mut source = source();
        source.messages.insert(
            uid(999),
            VersionedMetadata {
                metadata: {
                    let mut m = metadata(999);
                    m.flags.insert("\\Deleted".into());
                    m
                },
                modseq: seq(899),
            },
        );
        Self {
            source,
            destination: read("Privat/News", 202, &[], &[], false),
            commands: Vec::new(),
        }
    }
    fn source_for(&self, e: &CopyEvidence) -> Readback {
        self.source
            .subset(&e.intent().messages().keys().copied().collect())
    }
    fn destination_for(&self, e: &CopyEvidence) -> Readback {
        self.destination.subset(
            &e.copy
                .mapping()
                .map(|m| m.pairs().values().copied().collect())
                .unwrap_or_default(),
        )
    }
    fn copy(&mut self, command: String) -> String {
        assert_eq!(command, "UID COPY 4,8,19 \"Privat/News\"");
        assert!(self.destination.messages.is_empty());
        self.commands.push(command);
        for (source, destination) in [(4, 60), (8, 64), (19, 70)] {
            let mut copied = self.source.messages[&uid(source)].clone();
            self.destination.highest_modseq = seq(self.destination.highest_modseq.get() + 1);
            copied.modseq = self.destination.highest_modseq;
            self.destination.messages.insert(uid(destination), copied);
        }
        "[COPYUID 202 4,8,19 60,64,70]".into()
    }
    fn store(&mut self, intent: &FlagIntent) -> StoreResult {
        self.commands.push(intent.command());
        assert_eq!(intent.before.identity, self.source.identity);
        let mut modified = BTreeSet::new();
        let mut updated = BTreeMap::new();
        for uid in intent.before.messages.keys() {
            let current = self.source.messages.get_mut(uid).unwrap();
            if current.modseq > intent.threshold() {
                modified.insert(*uid);
                continue;
            }
            match intent.change {
                FlagChange::AddDeleted => {
                    current.metadata.flags.insert("\\Deleted".into());
                }
                FlagChange::RemoveOwnedDeleted => {
                    current
                        .metadata
                        .flags
                        .retain(|f| !f.eq_ignore_ascii_case("\\Deleted"));
                }
            }
            self.source.highest_modseq = seq(self.source.highest_modseq.get() + 1);
            current.modseq = self.source.highest_modseq;
            updated.insert(*uid, current.modseq);
        }
        StoreResult::Tagged {
            status: TaggedStatus::Ok,
            modified: (!modified.is_empty()).then_some(modified),
            updated,
        }
    }
    fn expunge(&mut self, command: String) {
        let set = command.strip_prefix("UID EXPUNGE ").unwrap();
        let requested = uid_set(set).unwrap();
        assert!(!requested.contains(&uid(999)));
        self.commands.push(command);
        for uid in requested {
            if self.source.messages.get(&uid).is_some_and(|m| {
                m.metadata
                    .flags
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case("\\Deleted"))
            }) {
                self.source.messages.remove(&uid);
            }
        }
    }
}
fn copy_effect() -> (CopyEvidence, Mailboxes) {
    let mut mailbox = Mailboxes::new();
    let mut e = evidence();
    let source = mailbox.source_for(&e);
    let command = e.begin_copy(&source).unwrap();
    e = roundtrip(&e);
    let mapping = mailbox.copy(command);
    mailbox = roundtrip(&mailbox);
    e.record_mapping(CopyUidMapping::parse(&mapping, e.intent()).unwrap())
        .unwrap();
    e = roundtrip(&e);
    e.record_copy_completion(Completion::Lost).unwrap();
    e = roundtrip(&e);
    e.observe_copy(mailbox.source_for(&e), mailbox.destination_for(&e))
        .unwrap();
    (roundtrip(&e), mailbox)
}

#[test]
fn independent_memory_effects_survive_each_journal_reopen_and_preserve_unrelated_deletions() {
    let (mut e, mut mailbox) = copy_effect();
    let protected = mailbox.source.messages[&uid(999)].clone();
    let source = mailbox.source_for(&e);
    let destination = mailbox.destination_for(&e);
    let groups = e.prepare_deletions(&source, &destination).unwrap();
    for group in groups {
        let members = group.intent.before.messages.keys().copied().collect();
        let destination = group.destination.clone();
        let intent = group.intent.clone();
        let index = e.begin_delete(group).unwrap();
        e = roundtrip(&e);
        let ack = mailbox.store(&intent);
        mailbox = roundtrip(&mailbox);
        e.record_store_result(index, ack).unwrap();
        e = roundtrip(&e);
        let fresh = mailbox.source.subset(&members);
        e.observe_store(index, fresh.clone()).unwrap();
        e = roundtrip(&e);
        let command = e.begin_expunge(index, fresh, destination.clone()).unwrap();
        e = roundtrip(&e);
        mailbox.expunge(command);
        mailbox = roundtrip(&mailbox);
        e.record_expunge_completion(index, Completion::Lost)
            .unwrap();
        e = roundtrip(&e);
        e.observe_final(index, mailbox.source.subset(&members), destination)
            .unwrap();
        e = roundtrip(&e);
    }
    assert!(
        e.resolutions()
            .values()
            .all(|r| matches!(r, Resolution::Filed { .. }))
    );
    assert_eq!(mailbox.source.messages, [(uid(999), protected)].into());
    assert_eq!(mailbox.destination.messages.len(), 3);
    for (uid, m) in &mailbox.destination.messages {
        assert_eq!(m.metadata.flags, metadata(uid.get()).flags);
    }
    assert_eq!(mailbox.commands.len(), 5);
    assert!(mailbox.commands.iter().all(|c| c.starts_with("UID COPY ")
        || c.starts_with("UID STORE ")
        || c.starts_with("UID EXPUNGE ")));
}

#[test]
fn reopening_after_copy_or_conditional_store_never_executes_compensation() {
    let (mut e, mut mailbox) = copy_effect();
    let copy_only = mailbox.clone();
    e = roundtrip(&e);
    assert_eq!(mailbox, copy_only);
    assert_eq!(mailbox.commands.len(), 1);
    let group = e
        .prepare_deletions(&mailbox.source_for(&e), &mailbox.destination_for(&e))
        .unwrap()
        .remove(0);
    let intent = group.intent.clone();
    let index = e.begin_delete(group).unwrap();
    let ack = mailbox.store(&intent);
    e.record_store_result(index, ack).unwrap();
    let members = intent.before.messages.keys().copied().collect();
    e.observe_store(index, mailbox.source.subset(&members))
        .unwrap();
    let after_store = mailbox.clone();
    e = roundtrip(&e);
    assert_eq!(mailbox, after_store);
    assert_eq!(mailbox.commands.len(), 2);
    let deletion = &e.deletions[index].evidence;
    for uid in &members {
        let member = [*uid].into();
        let fresh = mailbox.source.subset(&member);
        let undo = deletion.pending_clear(fresh.clone()).unwrap();
        let mut clear = deletion.begin_clear(undo.clone(), fresh).unwrap();
        let ack = mailbox.store(&undo);
        clear.record_result(ack).unwrap();
        clear.observe(mailbox.source.subset(&member)).unwrap();
        assert_eq!(clear.verified().len(), 1);
        assert_eq!(roundtrip(&clear), clear);
    }
    assert_eq!(mailbox.commands.len(), 4);
    assert_eq!(mailbox.destination, copy_only.destination);
    assert_eq!(
        mailbox.source.messages[&uid(999)],
        copy_only.source.messages[&uid(999)]
    );
}
