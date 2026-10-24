use super::*;
use crate::{
    census::{AccountId, LocalSecret},
    engine::{Fixture, PreviewInput},
    rules::workspace::{Workspace, WorkspaceInput},
};

fn fixture(
    n: usize,
    gmail: bool,
) -> (
    tempfile::TempDir,
    EncryptedStore,
    AccountState,
    SyntheticMailbox,
) {
    let root = tempfile::tempdir().unwrap();
    let mut input: PreviewInput =
        serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
    input.context.account = "synthetic-backlog-tests".into();
    input.messages = (0..n.min(200)).map(|i| Fixture { id: format!("fixture-{i}"), current_folder: "INBOX".into(), headers: format!("From: billing@shop.example.org\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: invoice\r\nMessage-ID: <fixture-{i}@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n") }).collect();
    let mut census = crate::engine::synthetic_census(&input).unwrap();
    let base = census.folders["INBOX"].messages[&1].clone();
    for i in 201..=n {
        let mut m = base.clone();
        m.uid = i as u32;
        m.headers.message_id_hash = Some(format!("synthetic-hash-{i}"));
        m.gmail_message_id = Some(format!("synthetic-gmail-{i}"));
        census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .insert(i as u32, m);
        census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .live_uids
            .insert(i as u32);
    }
    for m in census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .values_mut()
    {
        if gmail {
            m.gmail_labels = BTreeSet::from(["\\Inbox".into(), "Unrelated".into()]);
        }
        m.flags = BTreeSet::from(["\\Answered".into()]);
    }
    census
        .folder_roles
        .insert("Archive fixture".into(), BTreeSet::from(["archive".into()]));
    let id = AccountId::new("synthetic-backlog-tests").unwrap();
    let store = EncryptedStore::new(
        root.path().join("journal"),
        CensusKeys::derive(&LocalSecret::from_bytes(vec![21; 32]).unwrap(), &id),
    )
    .unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: input.as_of,
            baseline: Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
    let mailbox = SyntheticMailbox::open(&root.path().join("mailbox"), &store, &census).unwrap();
    (root, store, state, mailbox)
}
fn p(
    state: &AccountState,
    store: &EncryptedStore,
    mailbox: &SyntheticMailbox,
    gmail: bool,
) -> Preview {
    preview(
        state,
        if gmail {
            Provider::Gmail
        } else {
            Provider::MailboxOrg
        },
        store.keys(),
        Options::default(),
        Some(&mailbox),
    )
    .unwrap()
}
fn begin(
    state: &mut AccountState,
    store: &EncryptedStore,
    mailbox: &SyntheticMailbox,
    gmail: bool,
) -> String {
    let plan = p(state, store, mailbox, gmail);
    let receipt = plan.receipt.clone();
    start(state, plan, &receipt).unwrap();
    store.save_state(state).unwrap();
    receipt
}
#[test]
fn planner_scopes_exclusions_selections_and_no_flag_or_copy_effects() {
    let (_root, store, state, mut mailbox) = fixture(5, false);
    let inbox = mailbox.state.census.folders.get_mut("INBOX").unwrap();
    inbox
        .messages
        .get_mut(&1)
        .unwrap()
        .flags
        .insert("\\Flagged".into());
    inbox
        .messages
        .get_mut(&2)
        .unwrap()
        .gmail_labels
        .insert("\\Trash".into());
    let mut archive = inbox.clone();
    archive.messages.retain(|uid, _| *uid == 3);
    archive.live_uids = BTreeSet::from([3]);
    inbox.messages.remove(&3);
    inbox.live_uids.remove(&3);
    mailbox
        .state
        .census
        .folders
        .insert("Archive fixture".into(), archive);
    let preview = p(&state, &store, &mailbox, false);
    assert_eq!(preview.plan.changes.len(), 3);
    assert_eq!(preview.plan.excluded["flagged"], 1);
    assert_eq!(preview.plan.excluded["excluded_role"], 1);
    assert!(
        preview.plan.changes.iter().all(
            |c| c.flags.contains("\\Answered") && matches!(c.operation, Operation::Imap { .. })
        )
    );
    let opts = Options {
        scope: Scope::Inbox,
        ..Options::default()
    };
    assert_eq!(
        super::preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            opts,
            Some(&mailbox)
        )
        .unwrap()
        .plan
        .changes
        .len(),
        2
    );
    let opts = Options {
        selected: Some(BTreeSet::new()),
        ..Options::default()
    };
    let empty = super::preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        opts,
        Some(&mailbox),
    )
    .unwrap();
    assert!(!empty.execution_available);
    assert!(empty.plan.changes.is_empty());
    mailbox.state.census.folder_roles.clear();
    assert!(
        p(&state, &store, &mailbox, false)
            .plan
            .diagnostics
            .iter()
            .any(|d| d.contains("Unfiled scope unsupported"))
    );
    let ordinary = super::preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        Options::default(),
        None,
    )
    .unwrap();
    assert!(!ordinary.execution_available);
    assert!(ordinary.plan.changes.is_empty());
}
#[test]
fn bound_receipts_reject_tamper_replay_revision_selection_and_download_authority() {
    let (_root, store, mut state, mailbox) = fixture(2, false);
    let before = p(&state, &store, &mailbox, false);
    let bad = p(&state, &store, &mailbox, false);
    assert!(start(&mut state, bad, "wrong-receipt").is_err());
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml
        .push('\n');
    assert_ne!(before.receipt, p(&state, &store, &mailbox, false).receipt);
    let id = begin(&mut state, &store, &mailbox, false);
    assert!(start(&mut state, before, &id).is_err());
    let download =
        crate::rules::workspace::preview(&state, Provider::MailboxOrg, store.keys()).unwrap();
    assert_ne!(download.receipt, id);
}
#[test]
fn independent_effect_before_observation_reconciles_after_reopen_without_duplicates() {
    let (root, store, mut state, mut mailbox) = fixture(3, false);
    let id = begin(&mut state, &store, &mailbox, false);
    state.backlog[0].outcomes[0] = Outcome::Intent;
    store.save_state(&mut state).unwrap();
    let c = state.backlog[0].plan.changes[0].clone();
    let applied = mailbox.apply(&format!("{id}/0"), &c, true).unwrap();
    assert!(!c.source.matches(&mailbox.state.census));
    assert!(matches!(
        store.load_state().unwrap().backlog[0].outcomes[0],
        Outcome::Intent
    ));
    drop(mailbox);
    let mut mailbox =
        SyntheticMailbox::open(&root.path().join("mailbox"), &store, &state.census).unwrap();
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(state.backlog[0].status, Status::Completed);
    assert!(
        matches!(&state.backlog[0].outcomes[0], Outcome::Applied { observation } if observation == &applied)
    );
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(
        mailbox
            .state
            .census
            .folders
            .values()
            .map(|f| f.messages.len())
            .sum::<usize>(),
        3
    );
    let bytes = std::fs::read(store.path()).unwrap();
    for marker in [&id, &c.destination, &c.source.message_id_hash] {
        assert!(!bytes.windows(marker.len()).any(|b| b == marker.as_bytes()));
    }
}
#[test]
fn stop_before_batch_and_batch_limit_recover_without_losing_progress() {
    let (_root, store, mut state, mut mailbox) = fixture(501, false);
    let id = begin(&mut state, &store, &mailbox, false);
    stop(&mut state, &id).unwrap();
    store.save_state(&mut state).unwrap();
    assert_eq!(mailbox.state.census.folders["INBOX"].messages.len(), 501);
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert!(
        state.backlog[0]
            .outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Pending))
    );
    resume(&mut state, &id).unwrap();
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(
        state.backlog[0]
            .outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Applied { .. }))
            .count(),
        500
    );
    state = store.load_state().unwrap();
    assert_eq!(state.backlog[0].status, Status::Prepared);
    stop(&mut state, &id).unwrap();
    store.save_state(&mut state).unwrap();
    resume(&mut state, &id).unwrap();
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(state.backlog[0].status, Status::Completed);
    assert_eq!(state.backlog[0].batches, 2);
}
#[test]
fn copyuid_fallback_throttle_and_missing_mapping_fail_closed() {
    let (_root, store, mut state, mut mailbox) = fixture(2, false);
    mailbox.capabilities.remove("MOVE");
    mailbox.throttle_remaining = 2;
    let id = begin(&mut state, &store, &mailbox, false);
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    let Outcome::Applied { observation } = &state.backlog[0].outcomes[0] else {
        panic!()
    };
    assert!(
        observation
            .commands
            .iter()
            .any(|s| s.starts_with("UID COPY"))
    );
    assert!(
        observation
            .commands
            .iter()
            .any(|s| s.starts_with("UID EXPUNGE"))
    );
    assert!(
        !observation
            .commands
            .iter()
            .any(|s| s.starts_with("EXPUNGE"))
    );
    let (_root2, store2, mut state2, mut mailbox2) = fixture(1, false);
    let id2 = begin(&mut state2, &store2, &mailbox2, false);
    mailbox2.capabilities.remove("UIDPLUS");
    advance(&store2, &mut state2, &mut mailbox2, &id2).unwrap();
    assert_eq!(state2.backlog[0].status, Status::Uncertain);
    assert_eq!(mailbox2.state.census.folders["INBOX"].messages.len(), 1);
}
#[test]
fn gmail_apply_and_undo_preserve_unrelated_labels_and_current_flags() {
    let (_root, store, mut state, mut mailbox) = fixture(2, true);
    let id = begin(&mut state, &store, &mailbox, true);
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    let c = &state.backlog[0].plan.changes[0];
    let m = mailbox
        .state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&c.source.uid)
        .unwrap();
    assert!(!m.gmail_labels.contains("\\Inbox"));
    assert!(m.gmail_labels.contains("Unrelated"));
    m.gmail_labels.insert("Added later".into());
    m.flags.insert("\\Seen".into());
    mailbox.persist().unwrap();
    let up = undo_preview(&state, &mailbox, store.keys(), &id).unwrap();
    assert_eq!(up.eligible.len(), 2);
    undo(&store, &mut state, &mut mailbox, &id, Some(&up.receipt)).unwrap();
    undo(&store, &mut state, &mut mailbox, &id, None).unwrap();
    let m = &mailbox.state.census.folders["INBOX"].messages[&1];
    assert_eq!(
        m.gmail_labels,
        BTreeSet::from(["\\Inbox".into(), "Unrelated".into(), "Added later".into()])
    );
    assert_eq!(
        m.flags,
        BTreeSet::from(["\\Seen".into(), "\\Answered".into()])
    );
    assert!(
        mailbox
            .state
            .synthetic_effects
            .values()
            .all(|o| o.commands.iter().all(|s| !s.contains("EXPUNGE")))
    );
}
#[test]
fn undo_conflicts_and_intent_recovery_are_conditional() {
    let (root, store, mut state, mut mailbox) = fixture(2, false);
    let id = begin(&mut state, &store, &mailbox, false);
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    let Outcome::Applied { observation: first } = state.backlog[0].outcomes[0].clone() else {
        panic!()
    };
    mailbox
        .state
        .census
        .folders
        .get_mut(&first.destination.folder)
        .unwrap()
        .messages
        .remove(&first.destination.uid);
    let up = undo_preview(&state, &mailbox, store.keys(), &id).unwrap();
    assert_eq!(up.conflicts.len(), 1);
    undo(&store, &mut state, &mut mailbox, &id, Some(&up.receipt)).unwrap();
    assert!(matches!(state.backlog[0].undo[0], Undo::Conflict { .. }));
    assert!(matches!(state.backlog[0].undo[1], Undo::Restored { .. }));
    let restored = mailbox.state.census.clone();
    drop(mailbox);
    let mut mailbox =
        SyntheticMailbox::open(&root.path().join("mailbox"), &store, &state.census).unwrap();
    undo(&store, &mut state, &mut mailbox, &id, None).unwrap();
    assert_eq!(mailbox.state.census, restored);
}
#[test]
fn uidvalidity_mismatch_and_unknown_effect_are_never_success() {
    let (_root, store, mut state, mut mailbox) = fixture(1, false);
    let id = begin(&mut state, &store, &mailbox, false);
    mailbox
        .state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .uidvalidity += 1;
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(state.backlog[0].status, Status::Uncertain);
    assert!(matches!(
        state.backlog[0].outcomes[0],
        Outcome::Conflict { .. }
    ));
}

#[test]
fn backlog_keeps_flags_for_mark_read_rules_and_holds_unknown_roles() {
    let (_root, store, mut state, mut mailbox) = fixture(3, false);
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml
        .push_str("\n");
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml = state
        .workspace
        .as_ref()
        .unwrap()
        .input
        .documents
        .override_yaml
        .replace(
            "patches:\n",
            "patches:\n  - id: topic.money\n    mark_read: true\n",
        );
    let m = mailbox
        .state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&2)
        .unwrap();
    m.headers.delivered_to.clear();
    let p = p(&state, &store, &mailbox, false);
    assert_eq!(p.plan.review, 1);
    assert_eq!(p.plan.changes.len(), 2);
    assert!(p.plan.artifact.executable().contains("\\\\Seen"));
    let id = p.receipt.clone();
    start(&mut state, p, &id).unwrap();
    store.save_state(&mut state).unwrap();
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert!(
        mailbox
            .state
            .census
            .folders
            .values()
            .flat_map(|f| f.messages.values())
            .all(|m| !m.flags.contains("\\Seen"))
    );
    assert!(
        mailbox.state.census.folders["INBOX"]
            .messages
            .contains_key(&2)
    );
}

#[test]
fn every_folder_requires_role_evidence_and_excludes_system_roles() {
    let (_root, store, state, mut mailbox) = fixture(5, false);
    let original = mailbox.state.census.folders.remove("INBOX").unwrap();
    for (i, role) in ["sent", "drafts", "junk", "trash", "unknown"]
        .iter()
        .enumerate()
    {
        let mut folder = original.clone();
        folder.messages.retain(|uid, _| *uid == i as u32 + 1);
        folder.live_uids = BTreeSet::from([i as u32 + 1]);
        mailbox
            .state
            .census
            .folders
            .insert(format!("opaque-{i}"), folder);
        if *role != "unknown" {
            mailbox
                .state
                .census
                .folder_roles
                .insert(format!("opaque-{i}"), BTreeSet::from([role.to_string()]));
        }
    }
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        Options {
            scope: Scope::EveryFolder,
            ..Options::default()
        },
        Some(&mailbox),
    )
    .unwrap();
    assert!(p.plan.changes.is_empty());
    assert_eq!(p.plan.excluded["excluded_role"], 4);
    assert_eq!(p.plan.excluded["unknown_folder_role"], 1);
}

#[test]
fn lost_copyuid_and_exhausted_throttle_remain_uncertain_and_do_not_continue() {
    let (_root, store, mut state, mut mailbox) = fixture(2, false);
    let id = begin(&mut state, &store, &mailbox, false);
    let c = state.backlog[0].plan.changes[0].clone();
    mailbox.apply(&format!("{id}/0"), &c, true).unwrap();
    mailbox.state.synthetic_effects.clear();
    mailbox.persist().unwrap();
    state.backlog[0].outcomes[0] = Outcome::Intent;
    store.save_state(&mut state).unwrap();
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(state.backlog[0].status, Status::Uncertain);
    assert!(resume(&mut state, &id).is_err());
    assert!(matches!(state.backlog[0].outcomes[1], Outcome::Intent));
    assert!(
        mailbox.state.census.folders["INBOX"]
            .messages
            .contains_key(&2)
    );
    let (_other, store2, mut state2, mut mailbox2) = fixture(1, false);
    let id2 = begin(&mut state2, &store2, &mailbox2, false);
    mailbox2.throttle_remaining = 10;
    advance(&store2, &mut state2, &mut mailbox2, &id2).unwrap();
    assert_eq!(state2.backlog[0].status, Status::Uncertain);
    assert!(
        mailbox2.state.census.folders["INBOX"]
            .messages
            .contains_key(&1)
    );
}

#[test]
fn gmail_unfiled_means_no_user_label_without_broadening_scope() {
    let (_root, store, state, mut mailbox) = fixture(3, true);
    let folder = mailbox.state.census.folders.get_mut("INBOX").unwrap();
    folder.messages.get_mut(&1).unwrap().gmail_labels = BTreeSet::from(["\\Important".into()]);
    folder.messages.get_mut(&2).unwrap().gmail_labels =
        BTreeSet::from(["Existing user label".into()]);
    folder.messages.get_mut(&3).unwrap().gmail_labels = BTreeSet::from(["\\Spam".into()]);
    let p = p(&state, &store, &mailbox, true);
    assert_eq!(p.plan.changes.len(), 1);
    assert_eq!(p.plan.changes[0].source.uid, 1);
    assert_eq!(p.plan.excluded["outside_scope"], 1);
    assert_eq!(p.plan.excluded["excluded_role"], 1);
    assert!(
        matches!(&p.plan.changes[0].operation, Operation::Gmail { add, remove } if add.len() == 1 && remove.is_empty())
    );
}

#[test]
fn protection_is_rechecked_before_a_new_effect_and_undo_preserves_flags() {
    let (_root, store, mut state, mut mailbox) = fixture(1, false);
    let id = begin(&mut state, &store, &mailbox, false);
    mailbox
        .state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .flags
        .insert("\\Flagged".into());
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    assert_eq!(state.backlog[0].status, Status::Uncertain);
    assert!(
        mailbox.state.census.folders["INBOX"]
            .messages
            .contains_key(&1)
    );
    let (_other, store2, mut state2, mut mailbox2) = fixture(1, false);
    let id2 = begin(&mut state2, &store2, &mailbox2, false);
    advance(&store2, &mut state2, &mut mailbox2, &id2).unwrap();
    let Outcome::Applied { observation } = &state2.backlog[0].outcomes[0] else {
        panic!()
    };
    mailbox2
        .state
        .census
        .folders
        .get_mut(&observation.destination.folder)
        .unwrap()
        .messages
        .get_mut(&observation.destination.uid)
        .unwrap()
        .flags
        .insert("\\Flagged".into());
    let up = undo_preview(&state2, &mailbox2, store2.keys(), &id2).unwrap();
    undo(&store2, &mut state2, &mut mailbox2, &id2, Some(&up.receipt)).unwrap();
    assert!(
        mailbox2.state.census.folders["INBOX"]
            .messages
            .values()
            .all(|m| m.flags.contains("\\Flagged"))
    );
}

#[test]
fn synthetic_uidnext_survives_empty_folders_undo_restart_and_a_new_run() {
    let (root, store, mut state, mut mailbox) = fixture(2, false);
    let id = begin(&mut state, &store, &mailbox, false);
    advance(&store, &mut state, &mut mailbox, &id).unwrap();
    let up = undo_preview(&state, &mailbox, store.keys(), &id).unwrap();
    undo(&store, &mut state, &mut mailbox, &id, Some(&up.receipt)).unwrap();
    assert!(
        mailbox.state.census.folders["INBOX"]
            .messages
            .keys()
            .all(|uid| *uid > 2)
    );
    drop(mailbox);
    let mut mailbox =
        SyntheticMailbox::open(&root.path().join("mailbox"), &store, &state.census).unwrap();
    let next_id = begin(&mut state, &store, &mailbox, false);
    advance(&store, &mut state, &mut mailbox, &next_id).unwrap();
    assert!(state.backlog[1].outcomes.iter().all(|outcome| matches!(outcome, Outcome::Applied { observation } if observation.destination.uid > 2)));
}
