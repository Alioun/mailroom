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
        Self::sized(large.then_some(501))
    }
    fn sized(eligible: Option<u32>) -> Self {
        Self::prepared(eligible, |_| {})
    }
    fn prepared(eligible: Option<u32>, prepare: impl FnOnce(&std::path::Path)) -> Self {
        let root = tempfile::tempdir().unwrap();
        prepare(root.path());
        std::fs::write(
            root.path().join("workspace-authority"),
            "completion-fixture-v1",
        )
        .unwrap();
        if let Some(eligible) = eligible {
            std::fs::write(
                root.path().join("move-large"),
                format!("{eligible} exact eligible messages"),
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
            let store = w.store();
            assert_eq!(
                model::material(&store.load_state().unwrap(), store.keys()).unwrap(),
                run.plan.binding.material,
                "a run reuses its material only while it changes nothing but its journal"
            );
            drop(store);
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

#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn scale_move_backlog() {
    use crate::census::scale;
    let w = Workflow::sized(Some(scale::run_from_env()));
    let setup = scale::inflate_store(&w.store(), false);
    scale::report("move.setup", setup, Default::default());
    let preview = scale::measure("move.preview", serde_json::json!({}), || w.preview()).unwrap();
    let batches = preview.operation.batches.len();
    let result = scale::measure(
        "move.confirm_run",
        serde_json::json!({"batches": batches, "messages": preview.operation.plan.filing.changes.len()}),
        || w.confirm(&preview),
    );
    let run = w.run();
    let journal = serde_json::to_vec(&w.store().load_state().unwrap().protocol_backlog)
        .unwrap()
        .len();
    scale::report(
        "move.result",
        serde_json::json!({"result": format!("{result:?}"), "status": run.status, "journal_bytes": journal}),
        Default::default(),
    );
    let undo = scale::measure("move.undo_preview", serde_json::json!({}), || {
        w.undo_preview()
    })
    .unwrap();
    let result = scale::measure(
        "move.undo",
        serde_json::json!({"batches": undo.reverse.len()}),
        || w.undo(&undo),
    );
    scale::report(
        "move.undo_result",
        serde_json::json!({"result": format!("{result:?}"), "status": w.run().status}),
        Default::default(),
    );
}

fn clocked(w: &mut Workflow, at: u64) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(at));
    w.repo.clock = Some(clock.clone());
    clock
}

fn retain(w: &Workflow) -> AccountState {
    let store = w.store();
    let mut saved = store.load_state().unwrap();
    w.repo.retain_backlog(&store, &mut saved).unwrap();
    saved
}

const T0: u64 = 1_800_000_000;

/// Q27 at the boundary: Undo is offered until the last second of day 90, and
/// at 90 days the per-message journal becomes a summary of counts, folders
/// and dates. A further retention pass writes nothing.
#[test]
fn completed_run_stays_undoable_for_ninety_days_then_compacts_to_a_summary() {
    use crate::backlog::retention::WINDOW_SECS;
    let mut w = Workflow::start(false);
    let clock = clocked(&mut w, T0);
    let preview = w.preview().unwrap();
    w.confirm(&preview).unwrap();
    let run = w.run();
    assert_eq!(run.status, Status::Complete);
    assert_eq!(run.completed_at, Some(T0));
    clock.store(T0 + WINDOW_SECS - 1, std::sync::atomic::Ordering::SeqCst);
    let kept = retain(&w);
    assert_eq!(kept.protocol_backlog.runs.len(), 1);
    assert!(kept.protocol_backlog.summaries.is_empty());
    assert!(w.undo_preview().is_ok(), "undo is offered on day 89");
    let deadlines = kept.protocol_backlog.deadlines(T0 + WINDOW_SECS - 1);
    assert!(deadlines[0].undo_open);
    assert_eq!(deadlines[0].undo_until, Some(T0 + WINDOW_SECS));
    clock.store(T0 + WINDOW_SECS, std::sync::atomic::Ordering::SeqCst);
    assert!(w.undo_preview().is_err(), "undo closes at 90 days");
    let revision = w.store().load_state().unwrap().revision;
    let compacted = retain(&w);
    assert!(compacted.protocol_backlog.runs.is_empty());
    let summary = &compacted.protocol_backlog.summaries[0];
    assert_eq!(summary.id, run.id);
    assert_eq!(summary.status, Status::Complete);
    assert_eq!(
        (summary.completed_at, summary.compacted_at),
        (T0, T0 + WINDOW_SECS)
    );
    assert_eq!(summary.messages, run.plan.filing.changes.len());
    assert_eq!(summary.applied, summary.messages);
    assert_eq!(summary.restored, 0);
    assert_eq!(
        summary
            .folders
            .values()
            .flat_map(|d| d.values())
            .sum::<usize>(),
        summary.messages
    );
    let reopened = w.store().load_state().unwrap();
    assert_eq!(reopened.revision, revision + 1, "compaction is one save");
    assert!(reopened.protocol_backlog == compacted.protocol_backlog);
    reopened.protocol_backlog.validate(&w.account.id).unwrap();
    retain(&w);
    assert_eq!(w.store().load_state().unwrap().revision, revision + 1);
}

/// Runs saved before schema 19 carry no completion time. The first
/// retention pass stamps them with the current time, so they keep a full
/// 90-day window from the upgrade instead of losing Undo at once.
#[test]
fn runs_saved_before_retention_get_a_full_window_from_the_upgrade() {
    use crate::backlog::retention::WINDOW_SECS;
    let mut w = Workflow::start(false);
    let clock = clocked(&mut w, T0);
    let preview = w.preview().unwrap();
    w.confirm(&preview).unwrap();
    let store = w.store();
    let mut saved = store.load_state().unwrap();
    saved.protocol_backlog.runs[0].completed_at = None;
    store.save_state(&mut saved).unwrap();
    drop(store);
    clock.store(T0 + 10 * WINDOW_SECS, std::sync::atomic::Ordering::SeqCst);
    let stamped = retain(&w);
    assert_eq!(
        stamped.protocol_backlog.runs[0].completed_at,
        Some(T0 + 10 * WINDOW_SECS)
    );
    assert!(w.undo_preview().is_ok());
}

/// Uncertain and conflicted runs are never compacted, however old.
#[test]
fn uncertain_and_conflicted_runs_are_never_compacted() {
    use crate::backlog::retention::WINDOW_SECS;
    let mut w = Workflow::start(false);
    let clock = clocked(&mut w, T0);
    let preview = w.preview().unwrap();
    w.set("move-mode", "partial");
    assert!(w.confirm(&preview).is_err());
    w.set("move-mode", "");
    assert_eq!(w.run().status, Status::Uncertain);
    clock.store(T0 + 100 * WINDOW_SECS, std::sync::atomic::Ordering::SeqCst);
    let uncertain = w.run();
    let kept = retain(&w);
    assert!(kept.protocol_backlog.summaries.is_empty());
    assert!(kept.protocol_backlog.runs[0] == uncertain);
    let store = w.store();
    let mut saved = store.load_state().unwrap();
    saved.protocol_backlog.runs[0].status = Status::Conflict;
    store.save_state(&mut saved).unwrap();
    drop(store);
    let kept = retain(&w);
    assert!(kept.protocol_backlog.summaries.is_empty());
    assert_eq!(kept.protocol_backlog.runs[0].status, Status::Conflict);
    assert!(kept.protocol_backlog.runs[0].completed_at.is_none());
}

#[test]
fn compaction_kill_worker() {
    let Some(root) = std::env::var_os("MAILROOM_COMPACTION_KILL_TEST") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let repo = crate::acquisition::persistent_test_accounts(&root);
    let id = AccountId::new(&std::fs::read_to_string(root.join("account-id")).unwrap()).unwrap();
    let store = repo.store(&repo.load(&id).unwrap()).unwrap();
    let mut saved = store.load_state().unwrap();
    let now = std::fs::read_to_string(root.join("now"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(saved.protocol_backlog.compact(now));
    store.save_state(&mut saved).unwrap();
    std::fs::write(root.join("compacted"), b"saved").unwrap();
}

/// SIGKILL at each point of the compaction save: after the new base is
/// staged, after it replaced the file, and with no kill. Every reopen shows
/// either the full run or its summary, never both and never neither.
#[test]
fn sigkill_during_compaction_leaves_the_full_run_or_its_summary() {
    use crate::backlog::retention::WINDOW_SECS;
    use std::os::unix::process::ExitStatusExt;
    for point in ["base_staged", "base_renamed", ""] {
        let mut w = Workflow::start(false);
        clocked(&mut w, T0);
        let preview = w.preview().unwrap();
        w.confirm(&preview).unwrap();
        let run = w.run();
        std::fs::write(w.path("account-id"), w.account.id.as_str()).unwrap();
        std::fs::write(w.path("now"), (T0 + WINDOW_SECS).to_string()).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "acquisition::backlog::tests::compaction_kill_worker",
            ])
            .env("MAILROOM_COMPACTION_KILL_TEST", w.root.path())
            .env("MAILROOM_KILL_AT", point)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let reopened = w.store().load_state().unwrap();
        reopened.protocol_backlog.validate(&w.account.id).unwrap();
        let journal = &reopened.protocol_backlog;
        let full = journal.runs.len() == 1 && journal.runs[0] == run;
        let summarized = journal.runs.is_empty()
            && journal.summaries.len() == 1
            && journal.summaries[0].id == run.id;
        assert!(full != summarized, "{point}: exactly one form survives");
        match point {
            "" => {
                assert!(status.success() && summarized && w.path("compacted").exists());
            }
            "base_staged" => {
                assert_eq!(status.signal(), Some(libc::SIGKILL));
                assert!(full, "{point}: the old file stays until the rename");
            }
            _ => {
                assert_eq!(status.signal(), Some(libc::SIGKILL));
                assert!(summarized, "{point}: the rename committed the summary");
            }
        }
        eprintln!("compaction.kill point={point:?} full={full} summarized={summarized}");
    }
}

/// One phase of a personal-scale MOVE run that a single process cannot
/// finish within its time limit. Each phase reopens the fixture installation
/// at `$MAILROOM_SCALE_ROOT` and stops between batches after
/// `$MAILROOM_SCALE_BUDGET` seconds, like Cancel; the next phase resumes.
/// Phases: `setup`, `confirm`, `resume`, `undo`, `resume_undo`, `verify`.
#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn scale_move_phase() {
    use crate::backlog::retention::WINDOW_SECS;
    use crate::census::scale;
    let root = std::path::PathBuf::from(std::env::var("MAILROOM_SCALE_ROOT").unwrap());
    let phase = std::env::var("MAILROOM_SCALE_PHASE").unwrap();
    let budget: u64 = std::env::var("MAILROOM_SCALE_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(420);
    if phase == "setup" {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace-authority"), "completion-fixture-v1").unwrap();
        std::fs::write(
            root.join("move-large"),
            format!("{} exact eligible messages", scale::run_from_env()),
        )
        .unwrap();
    }
    let (mut repo, account, _fake) = fixture_install(&root);
    let undo_day = T0 + WINDOW_SECS - 86_400;
    let at = if phase.contains("undo") || phase == "verify" {
        undo_day
    } else {
        T0
    };
    repo.clock = Some(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(at)));
    let store = repo.store(&account).unwrap();
    let cancel = Cancellation::default();
    STOP_BETWEEN_BATCHES.with(|s| {
        *s.borrow_mut() = Some((
            std::time::Instant::now() + std::time::Duration::from_secs(budget),
            cancel.clone(),
        ))
    });
    let run = |saved: &AccountState| saved.protocol_backlog.runs.last().unwrap().clone();
    let progress = |run: &Run| {
        let entries: Vec<_> = if run.undo_receipt.is_some() {
            run.reverse.iter().map(|r| &r.evidence).collect()
        } else {
            run.batches.iter().collect()
        };
        serde_json::json!({
            "status": run.status,
            "verified_batches": entries.iter().filter(|e| model::verified(e)).count(),
            "batches": entries.len(),
            "messages": run.plan.filing.changes.len(),
            "completed_at": run.completed_at,
        })
    };
    let report = |operation: &str, result: String| {
        let saved = store.load_state().unwrap();
        let mut extra = progress(&run(&saved));
        extra["result"] = result.into();
        extra["journal_bytes"] = serde_json::to_vec(&saved.protocol_backlog)
            .unwrap()
            .len()
            .into();
        extra["file_bytes"] = std::fs::metadata(store.path()).unwrap().len().into();
        scale::report(operation, extra, Default::default());
    };
    match phase.as_str() {
        "setup" => scale::report(
            "move.setup",
            scale::inflate_store(&store, false),
            Default::default(),
        ),
        "confirm" => {
            let saved = store.load_state().unwrap();
            let preview = scale::measure("move.preview", serde_json::json!({}), || {
                repo.backlog_preview(
                    &account,
                    &store,
                    &saved,
                    Options::default(),
                    Cancellation::default(),
                )
            })
            .unwrap();
            let mut saved = store.load_state().unwrap();
            let result = scale::measure(
                "move.confirm_run",
                serde_json::json!({"batches": preview.operation.batches.len(), "messages": preview.operation.plan.filing.changes.len()}),
                || {
                    repo.backlog_confirm(
                        &account,
                        &store,
                        &mut saved,
                        Options::default(),
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                },
            );
            report("move.after_confirm", format!("{result:?}"));
        }
        "resume" | "resume_undo" => {
            let mut saved = store.load_state().unwrap();
            let id = run(&saved).id;
            let result = scale::measure(&format!("move.{phase}"), serde_json::json!({}), || {
                repo.backlog_run(
                    &account,
                    &store,
                    &mut saved,
                    &id,
                    phase == "resume_undo",
                    cancel.clone(),
                    |_| Ok(()),
                )
            });
            report(&format!("move.after_{phase}"), format!("{result:?}"));
        }
        "undo" => {
            let saved = store.load_state().unwrap();
            let id = run(&saved).id;
            let deadline = saved.protocol_backlog.deadlines(undo_day)[0].clone();
            assert!(deadline.undo_open, "day 89 is inside the undo window");
            let preview = scale::measure(
                "move.undo_preview",
                serde_json::json!({"undo_until": deadline.undo_until}),
                || {
                    repo.backlog_undo_preview(
                        &account,
                        &store,
                        &saved,
                        &id,
                        Cancellation::default(),
                    )
                },
            )
            .unwrap();
            let mut saved = store.load_state().unwrap();
            let result = scale::measure(
                "move.undo",
                serde_json::json!({"batches": preview.reverse.len(), "conflicts": preview.conflicts}),
                || {
                    repo.backlog_undo(
                        &account,
                        &store,
                        &mut saved,
                        &id,
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                },
            );
            report("move.after_undo", format!("{result:?}"));
        }
        "verify" => {
            let saved = store.load_state().unwrap();
            saved.protocol_backlog.validate(&account.id).unwrap();
            let r = run(&saved);
            let forward: usize = r
                .batches
                .iter()
                .filter(|b| model::verified(b))
                .map(|b| b.command().messages().len())
                .sum();
            let reversed: usize = r
                .reverse
                .iter()
                .filter(|b| model::verified(&b.evidence))
                .map(|b| b.evidence.command().messages().len())
                .sum();
            scale::report(
                "move.verify",
                serde_json::json!({"status": r.status, "messages": r.plan.filing.changes.len(), "forward_verified": forward, "reverse_verified": reversed, "completed_at": r.completed_at, "undo_until": saved.protocol_backlog.deadlines(undo_day)[0].undo_until}),
                Default::default(),
            );
            assert_eq!(r.status, Status::Undone);
            assert_eq!(forward, r.plan.filing.changes.len());
            assert_eq!(reversed, forward);
        }
        "recover" => {
            let secret = crate::acquisition::SecretValue::new(vec![0x5c; 32]);
            let raw = std::fs::read(store.path()).unwrap();
            let journal =
                serde_json::to_vec(&store.load_state().unwrap().protocol_backlog).unwrap();
            drop(store);
            let (preview, _) =
                scale::measure("recovery.capture_preview", serde_json::json!({}), || {
                    repo.capture_preview(&account.id, 1_800_000_000)
                })
                .unwrap();
            let bundles = root.join("recovery-bundles");
            let id = scale::measure(
                "recovery.capture",
                serde_json::json!({"journal_bytes": journal.len()}),
                || repo.capture(&preview, &secret, &bundles, |_| Ok(())),
            )
            .unwrap();
            let bundle =
                crate::acquisition::accounts::recovery::bundle_file(&bundles, &id).unwrap();
            drop(repo);
            for lost in ["fake-credentials", "data", "config"] {
                std::fs::rename(root.join(lost), root.join(format!("lost-{lost}"))).unwrap();
            }
            let fresh = crate::acquisition::persistent_test_accounts(&root);
            let restore = scale::measure("recovery.restore_preview", serde_json::json!({}), || {
                fresh.restore_preview(&bundle, &secret)
            })
            .unwrap();
            scale::measure("recovery.restore", serde_json::json!({}), || {
                fresh.restore(&bundle, &secret, &restore, |_| Ok(()))
            })
            .unwrap();
            let restored = fresh.store(&fresh.load(&account.id).unwrap()).unwrap();
            let exact = std::fs::read(restored.path()).unwrap() == raw;
            let same_journal = serde_json::to_vec(&restored.load_state().unwrap().protocol_backlog)
                .unwrap()
                == journal;
            scale::report(
                "recovery.result",
                serde_json::json!({"bundle_bytes": std::fs::metadata(&bundle).unwrap().len(), "exact_file": exact, "same_journal": same_journal, "held": restored.recovery_pending()}),
                Default::default(),
            );
            assert!(exact && same_journal && restored.recovery_pending());
        }
        _ => unreachable!("{phase}"),
    }
}

/// Live MOVE operations against a fake holding a personal-scale mailbox,
/// over the link `$MAILROOM_FIXTURE_LINK` emulates. The installation lives
/// at `$MAILROOM_SCALE_ROOT`; `$MAILROOM_LATENCY_PHASES` lists the phases
/// this process runs, in order: `setup` (`$MAILROOM_SCALE_MESSAGES`
/// messages, default 100,000; the fake advertises CONDSTORE when
/// `$MAILROOM_LATENCY_CONDSTORE` is set), `refresh` (the first acquisition), `rescan`
/// (a refresh of an unchanged mailbox), `preview`, `confirm` (stops after
/// its first batch, like Cancel), `resume`, `undo_preview`, `undo_lost`
/// (the fake drops each MOVE's reply), `reconcile`.
#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn latency_move_phases() {
    use crate::census::scale;
    let root = std::path::PathBuf::from(std::env::var("MAILROOM_SCALE_ROOT").unwrap());
    let phases = std::env::var("MAILROOM_LATENCY_PHASES").unwrap();
    if phases.starts_with("setup") {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace-authority"), "completion-fixture-v1").unwrap();
        std::fs::write(root.join("move-large"), "501 exact eligible messages").unwrap();
        std::fs::write(
            root.join("move-scale"),
            scale::spec_from_env(false).messages.to_string(),
        )
        .unwrap();
        if std::env::var_os("MAILROOM_LATENCY_CONDSTORE").is_some() {
            std::fs::write(root.join("move-condstore"), "advertised").unwrap();
        }
    }
    let (repo, account, _fake) = fixture_install(&root);
    let open = || repo.store(&account).unwrap();
    let report = |store: &EncryptedStore, operation: &str, result: String| {
        let saved = store.load_state().unwrap();
        let r = saved.protocol_backlog.runs.last();
        scale::report(
            operation,
            serde_json::json!({
                "result": result,
                "status": r.as_ref().map(|r| r.status),
                "batches": r.as_ref().map(|r| r.batches.len()),
                "census_messages": scale::messages(&saved.census),
            }),
            Default::default(),
        );
    };
    for phase in phases.split(',') {
        let cancel = Cancellation::default();
        if phase == "refresh" || phase == "rescan" {
            let result = scale::measure(&format!("move.{phase}"), serde_json::json!({}), || {
                repo.scan(&account.id, cancel.clone(), |_| {})
            });
            report(
                &open(),
                &format!("move.after_{phase}"),
                format!("{:?}", result.map(|c| c.folders.len())),
            );
            continue;
        }
        let store = open();
        match phase {
            "setup" => report(&store, "move.setup", "installed".into()),
            "preview" => {
                let saved = store.load_state().unwrap();
                let result = scale::measure("move.preview", serde_json::json!({}), || {
                    repo.backlog_preview(
                        &account,
                        &store,
                        &saved,
                        Options::default(),
                        cancel.clone(),
                    )
                });
                report(
                    &store,
                    "move.after_preview",
                    format!("{:?}", result.map(|p| p.operation.batches.len())),
                );
            }
            "confirm" => {
                let saved = store.load_state().unwrap();
                let preview = repo
                    .backlog_preview(&account, &store, &saved, Options::default(), cancel.clone())
                    .unwrap();
                STOP_BETWEEN_BATCHES
                    .with(|s| *s.borrow_mut() = Some((std::time::Instant::now(), cancel.clone())));
                let mut saved = store.load_state().unwrap();
                let result = scale::measure("move.confirm", serde_json::json!({}), || {
                    repo.backlog_confirm(
                        &account,
                        &store,
                        &mut saved,
                        Options::default(),
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                STOP_BETWEEN_BATCHES.with(|s| *s.borrow_mut() = None);
                report(&store, "move.after_confirm", format!("{result:?}"));
            }
            "resume" => {
                let mut saved = store.load_state().unwrap();
                let id = saved.protocol_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("move.resume", serde_json::json!({}), || {
                    repo.backlog_run(
                        &account,
                        &store,
                        &mut saved,
                        &id,
                        false,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                report(&store, "move.after_resume", format!("{result:?}"));
            }
            "undo_preview" => {
                let saved = store.load_state().unwrap();
                let id = saved.protocol_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("move.undo_preview", serde_json::json!({}), || {
                    repo.backlog_undo_preview(&account, &store, &saved, &id, cancel.clone())
                });
                report(
                    &store,
                    "move.after_undo_preview",
                    format!("{:?}", result.map(|p| p.reverse.len())),
                );
            }
            "undo_lost" => {
                let saved = store.load_state().unwrap();
                let id = saved.protocol_backlog.runs.last().unwrap().id.clone();
                let preview = repo
                    .backlog_undo_preview(&account, &store, &saved, &id, cancel.clone())
                    .unwrap();
                std::fs::write(root.join("move-mode"), "lost").unwrap();
                let mut saved = store.load_state().unwrap();
                let result = scale::measure("move.undo", serde_json::json!({}), || {
                    repo.backlog_undo(
                        &account,
                        &store,
                        &mut saved,
                        &id,
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                std::fs::write(root.join("move-mode"), "").unwrap();
                report(&store, "move.after_undo", format!("{result:?}"));
            }
            "reconcile" => {
                let mut saved = store.load_state().unwrap();
                let id = saved.protocol_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("move.reconcile", serde_json::json!({}), || {
                    repo.backlog_reconcile(&account, &store, &mut saved, &id, cancel.clone())
                });
                report(&store, "move.after_reconcile", format!("{result:?}"));
            }
            _ => unreachable!("{phase}"),
        }
    }
}

impl Workflow {
    fn commands(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.path("move-commands.log")).unwrap_or_default();
        std::fs::remove_file(self.path("move-commands.log")).ok();
        log.lines().map(String::from).collect()
    }
    fn scan(&self) {
        self.repo
            .scan(&self.account.id, Cancellation::default(), |_| {})
            .unwrap();
    }
    fn edit_provider(&self, edit: impl FnOnce(&mut crate::census::FolderCensus)) {
        let store = self
            .repo
            .fixture_sieve_store(&self.account.id, self.root.path())
            .unwrap();
        let mut provider = store.load_state().unwrap();
        edit(provider.census.folders.get_mut("INBOX").unwrap());
        store.save_state(&mut provider).unwrap();
    }
}

fn header_fetches(commands: &[String]) -> Vec<&String> {
    commands
        .iter()
        .filter(|c| c.contains("BODY.PEEK[HEADER.FIELDS (FROM"))
        .collect()
}

/// After a census refresh, a preview reads no header the census already
/// holds: a header is immutable for a UID under one UIDVALIDITY. With
/// CONDSTORE it reads only the metadata that changed, and still sees every
/// change: a message flagged by another client leaves the plan, and only a
/// newly delivered message's header is read. Without CONDSTORE the client
/// never asks for it. A confirm runs over the session its preview opened.
#[test]
fn previews_read_only_what_changed_since_the_census_and_confirm_reuses_the_session() {
    for condstore in [false, true] {
        let w = Workflow::prepared(Some(501), |root| {
            if condstore {
                std::fs::write(root.join("move-condstore"), "advertised").unwrap();
            }
            std::fs::write(root.join("move-command-log"), "on").unwrap();
        });
        w.scan();
        w.commands();
        let first = w.preview().unwrap();
        let commands = w.commands();
        assert!(header_fetches(&commands).is_empty(), "{condstore}");
        assert_eq!(
            commands.iter().any(|c| c.contains("CHANGEDSINCE")),
            condstore
        );
        assert_eq!(
            commands.iter().any(|c| c.contains("(CONDSTORE)")),
            condstore
        );
        let flagged = first.operation.plan.filing.changes[0].source.uid;
        w.edit_provider(|inbox| {
            inbox
                .messages
                .get_mut(&flagged)
                .unwrap()
                .flags
                .insert("\\Flagged".into());
        });
        let second = w.preview().unwrap();
        assert!(
            second
                .operation
                .plan
                .filing
                .changes
                .iter()
                .all(|c| c.source.uid != flagged),
            "{condstore}: the flag reached the plan"
        );
        assert_eq!(
            second.operation.plan.filing.changes.len() + 1,
            first.operation.plan.filing.changes.len()
        );
        let arrived = 100_001;
        w.edit_provider(|inbox| {
            let mut message = inbox.messages[&4].clone();
            message.uid = arrived;
            inbox.live_uids.insert(arrived);
            inbox.messages.insert(arrived, message);
        });
        w.commands();
        let third = w.preview().unwrap();
        let commands = w.commands();
        let headers = header_fetches(&commands);
        assert_eq!(headers.len(), 1, "{condstore}");
        assert!(headers[0].starts_with(&format!("UID FETCH {arrived} ")));
        w.confirm(&third).unwrap();
        assert_eq!(
            w.commands()
                .iter()
                .filter(|c| c.starts_with("LOGIN"))
                .count(),
            2,
            "{condstore}: preview and run share the MOVE session"
        );
        assert_eq!(w.run().status, Status::Complete);
    }
}

impl Workflow {
    fn close_preview(&self) -> Result<crate::backlog::protocol::ClosePreview, AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo
            .backlog_close_preview(&self.account, &store, &saved, &id, Cancellation::default())
    }
    fn close(&self, receipt: &str) -> Result<(), AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.backlog_close(
            &self.account,
            &store,
            &mut saved,
            &id,
            receipt,
            Cancellation::default(),
        )
    }
}

/// A MOVE run whose Undo restored every verified message still holds its
/// forward conflict, so before closing it blocked every new run.
#[test]
fn a_move_run_left_with_only_conflicts_is_closed_without_mail_changes() {
    let w = Workflow::start(false);
    let preview = w.preview().unwrap();
    w.set("move-race", "source_flag");
    let _ = w.confirm(&preview);
    w.set("move-race", "");
    assert_eq!(w.run().status, Status::Conflict);
    assert_eq!(
        w.close_preview().unwrap_err(),
        AcquisitionError::InvalidInput,
        "verified moves are still undoable"
    );
    let undo = w.undo_preview().unwrap();
    w.undo(&undo).unwrap();
    let run = w.run();
    assert_eq!(run.status, Status::Undone);
    assert!(!run.finished());
    assert!(w.preview().is_err(), "the forward conflict blocks new runs");
    let wires = w.wires();
    let provider = w.provider();
    let p = w.close_preview().unwrap();
    assert_eq!(p.reason, crate::backlog::protocol::CloseReason::UndoEnded);
    assert!(w.close(&"0".repeat(64)).is_err());
    w.close(&p.receipt).unwrap();
    let closed = w.run();
    assert!(closed.closed.is_some());
    assert!(closed.batches == run.batches && closed.reverse == run.reverse);
    assert_eq!(w.wires(), wires, "closing sends nothing");
    untouched(&provider, &w.provider(), &[], &[]);
    let mut journal = w.store().load_state().unwrap().protocol_backlog;
    assert!(
        !journal.compact(u64::MAX / 2),
        "a closed run keeps its journal"
    );
    assert!(w.reconcile().is_err());
    assert!(w.preview().is_ok());
}

/// A read-only refresh changes only the census, which every effect reads
/// fresh, so Undo of an earlier run stays available and still refuses a
/// message another client changed.
#[test]
fn a_refresh_keeps_undo_of_an_earlier_move_run() {
    let w = Workflow::start(false);
    let preview = w.preview().unwrap();
    w.confirm(&preview).unwrap();
    assert_eq!(w.run().status, Status::Complete);
    let bound = w.run().plan.binding.material;
    let census = w.store().load_state().unwrap().census.folders.clone();
    w.repo
        .scan(&w.account.id, Cancellation::default(), |_| {})
        .unwrap();
    let saved = w.store().load_state().unwrap();
    assert!(saved.census.folders != census, "the refresh saw the moves");
    assert_eq!(model::material(&saved, w.store().keys()).unwrap(), bound);
    let undo = w.undo_preview().unwrap();
    assert!(!undo.reverse.is_empty());
    w.undo(&undo).unwrap();
    assert_eq!(w.run().status, Status::Undone);
}
