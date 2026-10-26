use super::*;

#[test]
fn actual_saved_workflow_validates_phases_and_retains_journal_on_overflow() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("workspace-authority"),
        "journal-fixture-v1",
    )
    .unwrap();
    let (repo, account, _fake) = fixture_install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let preview = repo
        .backlog_preview(
            &account,
            &store,
            &saved,
            Options::default(),
            Cancellation::default(),
        )
        .unwrap();
    repo.backlog_confirm(
        &account,
        &store,
        &mut saved,
        Options::default(),
        &preview.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.protocol_backlog.runs[0].status, Status::Complete);
    let id = &reopened.protocol_backlog.runs[0].id;
    let reverse = repo
        .backlog_undo_preview(&account, &store, &reopened, id, Cancellation::default())
        .unwrap();
    repo.backlog_undo(
        &account,
        &store,
        &mut saved,
        id,
        &reverse.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    let original = std::fs::read(store.path()).unwrap();
    let canonical = serde_json::to_value(&saved.protocol_backlog).unwrap();
    let decoded: model::Journal = serde_json::from_value(canonical.clone()).unwrap();
    decoded.validate(&account.id).unwrap();
    for mutation in [
        "phase",
        "account",
        "duplicate",
        "reverse",
        "binding",
        "conflicts",
        "digest",
        "method",
        "semantics",
        "reverse_empty",
        "uncertain",
        "stop_phase",
    ] {
        let mut invalid = canonical.clone();
        match mutation {
            "phase" => invalid["runs"][0]["status"] = "prepared".into(),
            "account" => {
                invalid["runs"][0]["plan"]["binding"]["account"] = "different-account".into()
            }
            "duplicate" => {
                let run = invalid["runs"][0].clone();
                invalid["runs"].as_array_mut().unwrap().push(run);
            }
            "reverse" => invalid["runs"][0]["reverse"][0]["forward"] = 999.into(),
            "binding" => invalid["runs"][0]["plan"]["binding"]["authority"] = "forged".into(),
            "digest" => invalid["runs"][0]["id"] = "z".repeat(64).into(),
            "method" => {
                invalid["runs"][0]["plan"]["filing"]["changes"][0]["operation"]["method"] =
                    "copy_uid_expunge".into()
            }
            "semantics" => invalid["runs"][0]["plan"]["filing"]["semantics"] = "legacy".into(),
            "reverse_empty" => invalid["runs"][0]["reverse"] = serde_json::json!([]),
            "uncertain" => invalid["runs"][0]["status"] = "uncertain".into(),
            "stop_phase" => {
                invalid["runs"][0]["status"] = "undo_running".into();
                invalid["runs"][0]["stop_requested"] = true.into();
            }
            _ => invalid["runs"][0]["undo_conflicts"] = 9999.into(),
        }
        let invalid: model::Journal = serde_json::from_value(invalid).unwrap();
        assert!(invalid.validate(&account.id).is_err(), "{mutation}");
    }
    saved.protocol_backlog.runs = vec![saved.protocol_backlog.runs[0].clone(); model::HISTORY + 1];
    assert!(store.save_state(&mut saved).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), original);
}

struct Workflow {
    _fake: crate::acquisition::folder_provisioning::transport::message_move::tests::Fake,
    repo: Accounts,
    account: Account,
    root: tempfile::TempDir,
}

impl Workflow {
    fn start(large: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("workspace-authority"),
            "completion-fixture-v1",
        )
        .unwrap();
        if large {
            std::fs::write(
                root.path().join("move-large"),
                "501 exact eligible messages",
            )
            .unwrap();
        }
        let (repo, account, fake) = fixture_install(root.path());
        Self {
            root,
            repo,
            account,
            _fake: fake,
        }
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.root.path().join(name)
    }
    fn set(&self, name: &str, value: &str) {
        std::fs::write(self.path(name), value).unwrap();
    }
    fn store(&self) -> EncryptedStore {
        self.repo.store(&self.account).unwrap()
    }
    fn provider(&self) -> AccountState {
        self.repo
            .fixture_sieve_store(&self.account.id, self.root.path())
            .unwrap()
            .load_state()
            .unwrap()
    }
    fn wires(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.path("move-wire-exact.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn preview(&self) -> Result<Preview, AcquisitionError> {
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo.backlog_preview(
            &self.account,
            &store,
            &saved,
            Options::default(),
            Cancellation::default(),
        )
    }
    fn confirm(&self, preview: &Preview) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.backlog_confirm(
            &self.account,
            &store,
            &mut saved,
            Options::default(),
            &preview.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
    }
    fn run(&self) -> Run {
        self.store()
            .load_state()
            .unwrap()
            .protocol_backlog
            .runs
            .last()
            .unwrap()
            .clone()
    }
    fn reconcile(&self) -> Result<(), AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.backlog_reconcile(
            &self.account,
            &store,
            &mut saved,
            &id,
            Cancellation::default(),
        )
    }
    fn resume(&self, reverse: bool) -> Result<(), AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.backlog_run(
            &self.account,
            &store,
            &mut saved,
            &id,
            reverse,
            Cancellation::default(),
            |_| Ok(()),
        )
    }
    fn undo_preview(&self) -> Result<UndoPreview, AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo
            .backlog_undo_preview(&self.account, &store, &saved, &id, Cancellation::default())
    }
    fn undo(&self, preview: &UndoPreview) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.backlog_undo(
            &self.account,
            &store,
            &mut saved,
            &preview.run,
            &preview.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
    }
    /// Repeated explicit Reconcile adds no evidence, revision or provider write.
    fn reconcile_is_stable(&self) {
        let _ = self.reconcile();
        let first = self.store().load_state().unwrap();
        let provider = self.provider();
        let _ = self.reconcile();
        let second = self.store().load_state().unwrap();
        assert_eq!(second.revision, first.revision);
        assert!(
            serde_json::to_vec(&second.protocol_backlog).unwrap()
                == serde_json::to_vec(&first.protocol_backlog).unwrap(),
            "repeated Reconcile changed the journal"
        );
        untouched(&provider, &self.provider(), &[], &[]);
    }
}

/// Provider state the fake owns apart from mailbox contents and its command log.
fn configuration(state: &AccountState) -> serde_json::Value {
    let mut value = serde_json::to_value(state).unwrap();
    for field in ["revision", "census", "synthetic_uid_next"] {
        value.as_object_mut().unwrap().remove(field);
    }
    value["synthetic_folders"]
        .as_object_mut()
        .unwrap()
        .remove("commands");
    value
}

/// Exact comparison of everything the fake holds except the intended effects:
/// listed mailboxes may change, each listed destination allocated exactly
/// `moved` UIDs, and the command log only gained LOGINs.
fn untouched(before: &AccountState, after: &AccountState, changed: &[&str], moved: &[(&str, u32)]) {
    assert_eq!(configuration(after), configuration(before));
    let mut expected = before.synthetic_uid_next.clone();
    for (folder, count) in moved {
        *expected.get_mut(*folder).unwrap() += 4 * count;
    }
    assert_eq!(after.synthetic_uid_next, expected);
    for (name, folder) in &before.census.folders {
        if !changed.contains(&name.as_str()) {
            assert_eq!(
                serde_json::to_value(&after.census.folders[name]).unwrap(),
                serde_json::to_value(folder).unwrap(),
                "{name}"
            );
        }
    }
    assert_eq!(
        after.census.folders.keys().collect::<Vec<_>>(),
        before.census.folders.keys().collect::<Vec<_>>()
    );
    let commands = |s: &AccountState| s.synthetic_folders.as_ref().unwrap().commands.clone();
    let (old, new) = (commands(before), commands(after));
    assert_eq!(new[..old.len()], old[..]);
    assert!(new[old.len()..].iter().all(|c| c == "LOGIN"));
}

fn count(run: &Run, batch: usize, wanted: fn(&Resolution) -> bool) -> usize {
    run.batches[batch]
        .resolutions()
        .values()
        .filter(|r| wanted(r))
        .count()
}

#[test]
fn saturated_saved_workspace_move_at_batch_and_response_limits_never_retries() {
    for mode in [
        "expunge_rows",
        "flood_after_mapping",
        "flood_before_mapping",
    ] {
        let w = Workflow::start(true);
        let before = w.provider();
        let preview = w.preview().unwrap();
        assert_eq!(preview.operation.plan.filing.changes.len(), 501);
        assert_eq!(preview.operation.batches[0].command().messages().len(), 500);
        assert_eq!(preview.operation.batches[1].command().messages().len(), 1);
        w.set("move-mode", mode);
        let result = w.confirm(&preview);
        let wires = w.wires();
        assert_eq!(
            wires[0]["command"]
                .as_str()
                .unwrap()
                .strip_prefix("UID MOVE ")
                .unwrap()
                .split_once(' ')
                .unwrap()
                .0
                .split(',')
                .count(),
            500,
            "{mode}"
        );
        let run = w.run();
        if mode == "expunge_rows" {
            result.unwrap();
            assert_eq!(wires.len(), 2);
            assert_eq!(run.status, Status::Complete);
            assert_eq!(
                count(&run, 0, |r| matches!(r, Resolution::Verified { .. })),
                500
            );
            untouched(
                &before,
                &w.provider(),
                &["INBOX", "Personal/Studio"],
                &[("Personal/Studio", 501)],
            );
            continue;
        }
        assert!(result.is_err(), "{mode}");
        assert_eq!(wires.len(), 1, "{mode}");
        assert_eq!(run.status, Status::Uncertain, "{mode}");
        assert_eq!(run.batches[0].completion(), Some(&Completion::Lost));
        assert!(!run.batches[0].observed());
        assert!(!run.batches[1].attempted());
        assert_eq!(
            run.batches[0].mapping().map(|m| m.pairs().len()),
            (mode == "flood_after_mapping").then_some(500)
        );
        assert!(w.resume(false).is_err(), "{mode}");
        w.reconcile_is_stable();
        let reconciled = w.run();
        assert!(reconciled.batches[0].observed());
        w.set("move-mode", "");
        if mode == "flood_after_mapping" {
            assert_eq!(reconciled.status, Status::Stopped);
            assert_eq!(
                count(&reconciled, 0, |r| matches!(r, Resolution::Verified { .. })),
                500
            );
            w.resume(false).unwrap();
            assert_eq!(w.run().status, Status::Complete);
            assert_eq!(w.wires().len(), 2);
            untouched(
                &before,
                &w.provider(),
                &["INBOX", "Personal/Studio"],
                &[("Personal/Studio", 501)],
            );
        } else {
            assert_eq!(reconciled.status, Status::Uncertain);
            assert_eq!(count(&reconciled, 0, |r| *r == Resolution::Uncertain), 500);
            assert!(w.resume(false).is_err());
            let undo = w.undo_preview().unwrap();
            assert!(undo.reverse.is_empty());
            assert_eq!(undo.conflicts, 500);
            assert!(w.undo(&undo).is_err());
            assert_eq!(w.wires().len(), 1);
            untouched(
                &before,
                &w.provider(),
                &["INBOX", "Personal/Studio"],
                &[("Personal/Studio", 500)],
            );
        }
    }
}

const OWNER: &str = "From: updates@studio.example.com\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: Release notes\r\nMessage-ID: <identical@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n";
const UNKNOWN: &str = "From: updates@studio.example.com\r\nTo: unknown@work.example.test\r\nDelivered-To: unknown@work.example.test\r\nSubject: Release notes\r\nMessage-ID: <identical@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n";
const MIXED: &str = "From: updates@studio.example.com\r\nTo: team@work.example.test, owner@home.example.test\r\nDelivered-To: team@work.example.test, owner@home.example.test\r\nSubject: Release notes\r\nMessage-ID: <identical@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n";

#[test]
fn mutable_tls_mixed_recipient_profile_moves_only_proven_owner_mail() {
    use crate::acquisition::folder_provisioning::transport::message_move::tests::recipient_profile;
    let w = Workflow::start(false);
    recipient_profile(
        &w.repo,
        &w.account.id,
        w.root.path(),
        &[(4, OWNER), (8, UNKNOWN), (19, MIXED)],
    );
    let before = w.provider();
    let preview = w.preview().unwrap();
    let moved: Vec<_> = preview
        .operation
        .plan
        .filing
        .changes
        .iter()
        .map(|c| c.source.uid)
        .collect();
    assert_eq!(moved, [4]);
    for uid in [8, 19] {
        let held = preview
            .execution
            .decisions
            .iter()
            .find(|d| d.id == format!("INBOX/101/{uid}"))
            .unwrap();
        assert!(held.review, "{uid}");
        assert_eq!(held.destination, proof::Destination::Inbox, "{uid}");
        assert!(held.copies.is_empty() && !held.mark_read && held.keyword_additions.is_empty());
    }
    w.confirm(&preview).unwrap();
    assert_eq!(w.run().status, Status::Complete);
    let wires = w.wires();
    assert_eq!(wires.len(), 1);
    assert_eq!(wires[0]["command"], "UID MOVE 4 \"Personal/Studio\"");
    let after = w.provider();
    assert_eq!(after.census.folders["INBOX"].live_uids, [8, 19, 99].into());
    untouched(
        &before,
        &after,
        &["INBOX", "Personal/Studio"],
        &[("Personal/Studio", 1)],
    );
    let (store, mut state) = {
        let store = w
            .repo
            .fixture_sieve_store(&w.account.id, w.root.path())
            .unwrap();
        let state = store.load_state().unwrap();
        (store, state)
    };
    let inbox = state.census.folders.get_mut("INBOX").unwrap();
    for uid in [120, 124] {
        let mut m = inbox.messages[&8].clone();
        m.uid = uid;
        inbox.live_uids.insert(uid);
        inbox.messages.insert(uid, m);
    }
    *state.synthetic_uid_next.get_mut("INBOX").unwrap() = 128;
    store.save_state(&mut state).unwrap();
    drop(store);
    recipient_profile(
        &w.repo,
        &w.account.id,
        w.root.path(),
        &[(8, UNKNOWN), (19, MIXED), (120, OWNER), (124, MIXED)],
    );
    let delivered = w.provider();
    let next = w.preview().unwrap();
    let moved: Vec<_> = next
        .operation
        .plan
        .filing
        .changes
        .iter()
        .map(|c| c.source.uid)
        .collect();
    assert_eq!(moved, [120]);
    w.confirm(&next).unwrap();
    assert_eq!(w.wires()[1]["command"], "UID MOVE 120 \"Personal/Studio\"");
    let after = w.provider();
    assert_eq!(
        after.census.folders["INBOX"].live_uids,
        [8, 19, 99, 124].into()
    );
    untouched(
        &delivered,
        &after,
        &["INBOX", "Personal/Studio"],
        &[("Personal/Studio", 1)],
    );
}

/// Expected per-entry outcome when another client acts on the first requested
/// message between the final readback and the MOVE, combined with a response fault.
fn raced(race: &str, fault: &str, requested: &[u32]) -> Vec<Resolution> {
    let present: Vec<u32> = requested
        .iter()
        .copied()
        .filter(|u| !(race == "source_expunged" && *u == requested[0]))
        .collect();
    let effected: Vec<u32> = if fault == "partial" {
        present[..1].to_vec()
    } else {
        present
    };
    requested
        .iter()
        .map(|uid| {
            if fault == "lost" || !effected.contains(uid) {
                Resolution::Uncertain
            } else if *uid == requested[0] && race != "source_expunged" {
                Resolution::Conflict
            } else {
                Resolution::Verified {
                    destination: Uid::new(1).unwrap(),
                }
            }
        })
        .collect()
}

fn kind(r: &Resolution) -> Resolution {
    match r {
        Resolution::Verified { .. } => Resolution::Verified {
            destination: Uid::new(1).unwrap(),
        },
        other => other.clone(),
    }
}

fn expected_status(resolutions: &[Resolution], reverse: bool) -> Status {
    if resolutions.contains(&Resolution::Conflict) {
        Status::Conflict
    } else if resolutions.contains(&Resolution::Uncertain) {
        Status::Uncertain
    } else if reverse {
        Status::Undone
    } else {
        Status::Complete
    }
}

fn race_forward(race: &str) {
    for fault in ["tagged", "untagged", "partial", "lost"] {
        let w = Workflow::start(false);
        let before = w.provider();
        let preview = w.preview().unwrap();
        w.set("move-mode", fault);
        w.set("move-race", race);
        let _ = w.confirm(&preview);
        w.set("move-race", "");
        w.set("move-mode", "");
        let expected = raced(race, fault, &[4, 8, 19]);
        w.reconcile_is_stable();
        let run = w.run();
        let found: Vec<_> = run.batches[0].resolutions().values().map(kind).collect();
        assert_eq!(found, expected, "{race} {fault}");
        assert_eq!(
            run.status,
            expected_status(&expected, false),
            "{race} {fault}"
        );
        assert!(w.resume(false).is_err() || run.status == Status::Complete);
        assert_eq!(w.wires().len(), 1, "{race} {fault}");
        assert!(run.undo_receipt.is_none());
        let moved = expected
            .iter()
            .filter(|r| **r != Resolution::Uncertain)
            .count()
            + usize::from(fault == "lost") * (3 - usize::from(race == "source_expunged"));
        let after = w.provider();
        assert!(
            after.census.folders["INBOX"].messages[&99]
                .flags
                .contains("\\Deleted")
        );
        untouched(
            &before,
            &after,
            &["INBOX", "Personal/Studio"],
            &[("Personal/Studio", moved as u32)],
        );
    }
}

#[test]
fn concurrent_source_flag_race_with_each_fault_stays_unresolved_without_retry() {
    race_forward("source_flag");
}

#[test]
fn concurrent_deleted_flag_race_with_each_fault_stays_unresolved_without_retry() {
    race_forward("deleted_flag");
}

#[test]
fn concurrent_source_expunge_race_with_each_fault_stays_unresolved_without_retry() {
    race_forward("source_expunged");
}

#[test]
fn concurrent_destination_flag_race_with_each_fault_stays_unresolved_without_retry() {
    race_forward("destination_flag");
}

#[test]
fn concurrent_metadata_races_during_reverse_move_keep_current_flags_and_never_repeat() {
    for race in ["source_flag", "source_expunged", "destination_flag"] {
        for fault in ["tagged", "partial", "lost"] {
            let w = Workflow::start(false);
            w.confirm(&w.preview().unwrap()).unwrap();
            let before = w.provider();
            let undo = w.undo_preview().unwrap();
            let requested: Vec<u32> = undo.reverse[0]
                .evidence
                .command()
                .messages()
                .keys()
                .map(|u| u.get())
                .collect();
            assert_eq!(requested, [60, 64, 68]);
            w.set("move-mode", fault);
            w.set("move-race", race);
            let _ = w.undo(&undo);
            w.set("move-race", "");
            w.set("move-mode", "");
            w.reconcile_is_stable();
            let run = w.run();
            let expected = raced(race, fault, &requested);
            let found: Vec<_> = run.reverse[0]
                .evidence
                .resolutions()
                .values()
                .map(kind)
                .collect();
            assert_eq!(found, expected, "{race} {fault}");
            assert_eq!(
                run.status,
                expected_status(&expected, true),
                "{race} {fault}"
            );
            assert!(run.batches.iter().all(model::verified));
            assert!(w.resume(true).is_err());
            assert!(w.undo_preview().is_err());
            assert_eq!(w.wires().len(), 2, "{race} {fault}");
            let moved = expected
                .iter()
                .filter(|r| **r != Resolution::Uncertain)
                .count()
                + usize::from(fault == "lost") * (3 - usize::from(race == "source_expunged"));
            let after = w.provider();
            if race == "destination_flag" && fault != "lost" {
                let flagged = after.census.folders["INBOX"]
                    .messages
                    .values()
                    .filter(|m| m.flags.contains("$race-client"))
                    .count();
                assert_eq!(flagged, 1, "{race} {fault}");
            }
            untouched(
                &before,
                &after,
                &["INBOX", "Personal/Studio"],
                &[("INBOX", moved as u32)],
            );
        }
    }
}

#[test]
fn eligible_subset_of_partially_mapped_unresolved_batch_reverses_alone() {
    let w = Workflow::start(false);
    let before = w.provider();
    let preview = w.preview().unwrap();
    w.set("move-mode", "partial");
    assert!(w.confirm(&preview).is_err());
    w.set("move-mode", "");
    let run = w.run();
    assert_eq!(run.status, Status::Uncertain);
    assert!(run.batches[0].observed());
    assert!(matches!(
        run.batches[0].resolutions()[&Uid::new(4).unwrap()],
        Resolution::Verified { .. }
    ));
    let (store, mut provider) = {
        let store = w
            .repo
            .fixture_sieve_store(&w.account.id, w.root.path())
            .unwrap();
        let state = store.load_state().unwrap();
        (store, state)
    };
    provider
        .census
        .folders
        .get_mut("Personal/Studio")
        .unwrap()
        .messages
        .get_mut(&60)
        .unwrap()
        .flags
        .insert("$later-client".into());
    store.save_state(&mut provider).unwrap();
    drop(store);
    w.reconcile_is_stable();
    assert!(
        serde_json::to_vec(&w.run().batches).unwrap() == serde_json::to_vec(&run.batches).unwrap()
    );
    assert_eq!(w.run().status, Status::Uncertain);
    assert!(w.resume(false).is_err());
    let blocked = w.store().load_state().unwrap();
    assert!(
        w.repo
            .backlog_preview(
                &w.account,
                &w.store(),
                &blocked,
                Options::default(),
                Cancellation::default()
            )
            .is_err()
    );
    let undo = w.undo_preview().unwrap();
    assert_eq!(undo.reverse.len(), 1);
    assert_eq!(
        undo.reverse[0]
            .evidence
            .command()
            .messages()
            .keys()
            .map(|u| u.get())
            .collect::<Vec<_>>(),
        [60]
    );
    assert_eq!(undo.conflicts, 2);
    let flagged = w.provider();
    w.undo(&undo).unwrap();
    let run = w.run();
    assert_eq!(run.status, Status::Undone);
    assert_eq!(run.undo_conflicts, 2);
    assert!(!run.finished());
    assert!(
        serde_json::to_vec(&run.batches).unwrap() == serde_json::to_vec(&w.run().batches).unwrap()
    );
    let wires = w.wires();
    assert_eq!(wires.len(), 2);
    assert_eq!(wires[1]["command"], "UID MOVE 60 \"INBOX\"");
    let after = w.provider();
    assert_eq!(
        after.census.folders["INBOX"].live_uids,
        [8, 19, 99, 104].into()
    );
    assert!(
        after.census.folders["INBOX"].messages[&104]
            .flags
            .contains("$later-client")
    );
    assert_eq!(
        after.census.folders["Personal/Studio"].live_uids,
        [40].into()
    );
    untouched(
        &flagged,
        &after,
        &["INBOX", "Personal/Studio"],
        &[("INBOX", 1)],
    );
    untouched(
        &before,
        &flagged,
        &["INBOX", "Personal/Studio"],
        &[("Personal/Studio", 1)],
    );
    w.reconcile_is_stable();
    assert!(w.resume(true).is_err());
    assert!(w.resume(false).is_err());
    assert!(w.undo_preview().is_err());
    let saved = w.store().load_state().unwrap();
    assert!(
        w.repo
            .backlog_preview(
                &w.account,
                &w.store(),
                &saved,
                Options::default(),
                Cancellation::default()
            )
            .is_err()
    );
    assert_eq!(w.wires().len(), 2);
    let reopened: model::Journal =
        serde_json::from_slice(&serde_json::to_vec(&saved.protocol_backlog).unwrap()).unwrap();
    reopened.validate(&w.account.id).unwrap();
}
