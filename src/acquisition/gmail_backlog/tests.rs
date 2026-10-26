use super::*;
use crate::acquisition::folder_provisioning::transport::gmail_label::tests::{
    self as fake, ALL, BUSINESS, Fake, PERSONAL,
};
use std::path::Path;

struct Fixture {
    root: tempfile::TempDir,
    repo: Accounts,
    account: Account,
    _fake: Fake,
}

fn setup(prepare: impl FnOnce(&Path)) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("workspace-authority"),
        "gmail-label-tests-v1",
    )
    .unwrap();
    prepare(root.path());
    let (repo, account, fake) = fixture_install(root.path());
    Fixture {
        root,
        repo,
        account,
        _fake: fake,
    }
}

impl Fixture {
    fn path(&self) -> &Path {
        self.root.path()
    }
    fn store(&self) -> EncryptedStore {
        self.repo.store(&self.account).unwrap()
    }
    fn saved(&self) -> AccountState {
        self.store().load_state().unwrap()
    }
    fn provider(&self) -> AccountState {
        fake::external(&self.repo, &self.account.id, self.path()).1
    }
    fn labels(&self, id: &str) -> (u32, BTreeSet<String>, BTreeSet<String>) {
        let provider = self.provider();
        let (uid, m) = provider.census.folders[ALL]
            .messages
            .iter()
            .find(|(_, m)| m.gmail_message_id.as_deref() == Some(id))
            .unwrap();
        (*uid, m.gmail_labels.clone(), m.flags.clone())
    }
    fn wires(&self) -> Vec<String> {
        std::fs::read_to_string(self.path().join("gmail-wire-exact.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["command"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    fn mode(&self, mode: &str) {
        std::fs::write(self.path().join("gmail-mode"), mode).unwrap();
    }
    fn preview(&self, options: Options) -> Result<Preview, AcquisitionError> {
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo.gmail_preview(
            &self.account,
            &store,
            &saved,
            options,
            Cancellation::default(),
        )
    }
    fn confirm(
        &self,
        options: Options,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let p = self.preview(options.clone()).unwrap();
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.gmail_confirm(
            &self.account,
            &store,
            &mut saved,
            options,
            &p.receipt,
            Cancellation::default(),
            fresh,
        )
    }
    fn run(&self) -> Run {
        self.saved().gmail_backlog.runs.last().unwrap().clone()
    }
    fn reconcile(&self) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
        self.repo.gmail_reconcile(
            &self.account,
            &store,
            &mut saved,
            &id,
            Cancellation::default(),
        )
    }
    fn resume(&self, reverse: bool) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
        self.repo.gmail_run(
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
            .gmail_undo_preview(&self.account, &store, &saved, &id, Cancellation::default())
    }
    fn undo(&self) -> Result<(), AcquisitionError> {
        let p = self.undo_preview()?;
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.gmail_undo(
            &self.account,
            &store,
            &mut saved,
            &p.run,
            &p.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
    }
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn personal_only() -> Options {
    Options {
        selected: Some(set(&[PERSONAL])),
        ..Options::default()
    }
}

#[test]
fn saved_workspace_adds_labels_removes_only_inbox_and_undo_reverses_only_the_delta() {
    let f = setup(|_| {});
    let p = f.preview(Options::default()).unwrap();
    let plan = &p.operation.plan.filing;
    let changed: BTreeSet<_> = plan
        .changes
        .iter()
        .map(|c| c.source.gmail_id.clone().unwrap())
        .collect();
    assert_eq!(
        changed,
        set(&["1001", "1002", "1003", "1005", "1006"]),
        "{:?} {:?}",
        plan.excluded,
        plan.diagnostics
    );
    assert_eq!(plan.excluded.get("flagged"), Some(&1));
    assert_eq!(plan.excluded.get("excluded_role"), Some(&1));
    assert_eq!(plan.excluded.get("outside_scope"), Some(&1));
    assert_eq!(plan.unmatched, 1);
    assert_eq!(p.operation.batches.len(), 2);
    let ids = [
        "1001", "1002", "1003", "1004", "1005", "1006", "1007", "1008", "1009",
    ];
    let before: BTreeMap<_, _> = ids.iter().map(|id| (*id, f.labels(id))).collect();
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert_eq!(
        f.wires(),
        [
            "UID STORE 31 +X-GM-LABELS (\"Gesch&AOQ-ftlich/Studio\")",
            "UID STORE 31 -X-GM-LABELS (\\Inbox)",
            "UID STORE 4,8,30 +X-GM-LABELS (\"Privat/Studio\")",
            "UID STORE 4,8,19 -X-GM-LABELS (\\Inbox)",
        ]
    );
    for (id, labels) in [
        ("1001", set(&[PERSONAL])),
        ("1002", set(&["Andere", PERSONAL])),
        ("1003", set(&[PERSONAL])),
        ("1004", set(&["\\Inbox"])),
        ("1005", set(&[PERSONAL])),
        ("1006", set(&["\\Important", BUSINESS])),
        ("1007", set(&["\\Sent"])),
        ("1008", set(&["\\Inbox"])),
        ("1009", set(&["Andere"])),
    ] {
        let (uid, now, flags) = f.labels(id);
        assert_eq!(now, labels, "{id}");
        assert_eq!((uid, flags), (before[id].0, before[id].2.clone()), "{id}");
    }
    let inbox: BTreeSet<_> = f.provider().census.folders["INBOX"]
        .messages
        .values()
        .map(|m| m.gmail_message_id.clone().unwrap())
        .collect();
    assert_eq!(inbox, set(&["1004", "1008"]));
    assert!(!f.path().join("gmail-forbidden").exists());

    f.reconcile().unwrap();
    let revision = f.saved().revision;
    f.reconcile().unwrap();
    assert_eq!(f.saved().revision, revision, "Reconcile writes nothing new");

    fake::other_client(&f.repo, &f.account.id, f.path(), "label:1001:Später");
    fake::other_client(&f.repo, &f.account.id, f.path(), "flag:1002");
    let undo = f.undo_preview().unwrap();
    assert_eq!(undo.conflicts, 0);
    f.undo().unwrap();
    assert_eq!(f.run().status, Status::Undone);
    assert_eq!(
        f.wires()[4..],
        [
            "UID STORE 31 +X-GM-LABELS (\\Inbox)",
            "UID STORE 31 -X-GM-LABELS (\"Gesch&AOQ-ftlich/Studio\")",
            "UID STORE 4,8,19 +X-GM-LABELS (\\Inbox)",
            "UID STORE 4,8,30 -X-GM-LABELS (\"Privat/Studio\")",
        ]
    );
    for (id, labels) in [
        ("1001", set(&["\\Inbox", "Später"])),
        ("1002", set(&["\\Inbox", "Andere"])),
        ("1003", set(&["\\Inbox", PERSONAL])),
        ("1005", set(&[])),
        ("1006", set(&["\\Inbox", "\\Important"])),
    ] {
        assert_eq!(f.labels(id).1, labels, "{id}");
    }
    assert!(f.labels("1002").2.contains("\\Flagged"));
    assert!(f.run().finished());
    assert!(f.preview(Options::default()).is_ok());
}

#[test]
fn lost_acknowledgment_stays_uncertain_and_is_never_retried() {
    let f = setup(|_| {});
    f.mode("lost");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    assert_eq!(f.run().status, Status::Uncertain);
    assert_eq!(f.wires().len(), 1);
    assert!(f.resume(false).is_err());
    f.reconcile().unwrap();
    let run = f.run();
    assert_eq!(run.status, Status::Uncertain);
    assert_eq!(
        run.batches[0].steps()[0].completion(),
        Some(&Completion::Lost)
    );
    assert!(
        run.batches[0]
            .outcomes(0)
            .values()
            .all(|o| *o == Outcome::Uncertain)
    );
    assert!(f.resume(false).is_err());
    let undo = f.undo_preview().unwrap();
    assert!(undo.reverse.is_empty());
    assert_eq!(undo.conflicts, 3);
    assert!(f.undo().is_err());
    assert_eq!(f.wires().len(), 1, "No label store was repeated");
    assert!(
        f.preview(Options::default()).is_err(),
        "Uncertain run blocks"
    );
}

#[test]
fn partial_refusal_keeps_attribution_honest() {
    let f = setup(|_| {});
    f.mode("partial");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    let run = f.run();
    assert_eq!(run.status, Status::Conflict);
    let outcomes = run.batches[0].outcomes(0);
    assert_eq!(
        outcomes
            .values()
            .filter(|o| **o == Outcome::Uncertain)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .values()
            .filter(|o| **o == Outcome::Unapplied)
            .count(),
        2
    );
    assert!(!run.batches[0].steps()[1].attempted());
    assert!(f.resume(false).is_err());
    let undo = f.undo_preview().unwrap();
    assert!(undo.reverse.is_empty());
    assert_eq!(undo.conflicts, 1);
    assert_eq!(f.wires().len(), 1);
}

#[test]
fn unsupported_inbox_removal_is_unapplied_and_undo_removes_only_the_added_label() {
    let f = setup(|_| {});
    f.mode("bad_remove");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    let run = f.run();
    assert_eq!(run.status, Status::Conflict);
    assert!(
        run.batches[0]
            .outcomes(0)
            .values()
            .all(|o| *o == Outcome::Verified)
    );
    assert!(
        run.batches[0]
            .outcomes(1)
            .values()
            .all(|o| *o == Outcome::Unapplied)
    );
    assert_eq!(f.labels("1001").1, set(&["\\Inbox", PERSONAL]));
    let undo = f.undo_preview().unwrap();
    assert_eq!(undo.reverse.len(), 1);
    assert!(undo.reverse[0].batch.step(StepKind::RestoreInbox).is_none());
    f.undo().unwrap();
    assert_eq!(f.run().status, Status::Undone);
    assert_eq!(f.labels("1001").1, set(&["\\Inbox"]));
    assert_eq!(f.labels("1003").1, set(&["\\Inbox", PERSONAL]));
    assert_eq!(
        f.wires().last().unwrap(),
        "UID STORE 4,8,30 -X-GM-LABELS (\"Privat/Studio\")"
    );
}

#[test]
fn other_client_flag_between_steps_blocks_inbox_removal_without_effect() {
    let f = setup(|_| {});
    let calls = std::cell::Cell::new(0);
    let repo = &f.repo;
    let (id, path) = (f.account.id.clone(), f.path().to_owned());
    assert!(
        f.confirm(personal_only(), |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                fake::other_client(repo, &id, &path, "flag:1001");
            }
            Ok(())
        })
        .is_err()
    );
    let run = f.run();
    assert_eq!(run.status, Status::Conflict);
    assert!(run.batches[0].steps()[0].readback().is_some());
    assert!(!run.batches[0].steps()[1].attempted(), "Precheck refused");
    assert_eq!(f.wires().len(), 1);
    let (_, labels, flags) = f.labels("1001");
    assert!(labels.contains("\\Inbox") && flags.contains("\\Flagged"));
}

#[test]
fn race_after_final_read_is_reported_and_undo_keeps_the_later_flag() {
    let f = setup(|_| {});
    std::fs::write(f.path().join("gmail-race"), "after:flag").unwrap();
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    let run = f.run();
    assert_eq!(run.status, Status::Conflict);
    let outcomes = run.batches[0].outcomes(0);
    assert_eq!(
        outcomes
            .values()
            .filter(|o| **o == Outcome::Changed)
            .count(),
        1
    );
    f.undo().unwrap();
    let (_, labels, flags) = f.labels("1001");
    assert_eq!(labels, set(&["\\Inbox"]));
    assert!(flags.contains("\\Flagged"));
}

#[test]
fn concurrent_archive_before_store_and_changed_identity_are_conflicts() {
    for (mode, race) in [("", "before:archive"), ("uid_moves", ""), ("case_fold", "")] {
        let f = setup(|_| {});
        f.mode(mode);
        if !race.is_empty() {
            std::fs::write(f.path().join("gmail-race"), race).unwrap();
        }
        let result = f.confirm(personal_only(), |_| Ok(()));
        f.mode("");
        assert!(result.is_err(), "{mode}{race}");
        assert_eq!(f.run().status, Status::Conflict, "{mode}{race}");
        let before = f.wires().len();
        assert!(f.resume(false).is_err());
        let _ = f.reconcile();
        assert_eq!(f.wires().len(), before, "{mode}{race}");
    }
}

#[test]
fn missing_extension_or_label_never_stores_or_creates() {
    let f = setup(|_| {});
    fake::other_client(&f.repo, &f.account.id, f.path(), "no_extension");
    assert!(f.preview(Options::default()).is_err());
    let f = setup(|_| {});
    fake::other_client(
        &f.repo,
        &f.account.id,
        f.path(),
        &format!("drop_label:{BUSINESS}"),
    );
    let inventory = f.provider().synthetic_folders.unwrap().inventory;
    let p = f.preview(Options::default()).unwrap();
    let plan = &p.operation.plan.filing;
    assert_eq!(plan.excluded.get("missing_label"), Some(&1));
    assert!(plan.diagnostics.iter().any(|d| d.contains(BUSINESS)));
    assert!(plan.changes.iter().all(|c| c.destination == PERSONAL));
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert_eq!(f.provider().synthetic_folders.unwrap().inventory, inventory);
    assert_eq!(f.labels("1006").1, set(&["\\Inbox", "\\Important"]));
}

#[test]
fn cancel_between_batches_keeps_effects_and_resume_runs_only_the_remainder() {
    let f = setup(|root| std::fs::write(root.join("gmail-large"), "501").unwrap());
    let p = f.preview(personal_only()).unwrap();
    assert_eq!(p.operation.batches.len(), 2);
    assert_eq!(p.operation.batches[0].intent().messages().len(), 500);
    let cancel = Cancellation::default();
    let stopping = cancel.clone();
    let calls = std::cell::Cell::new(0);
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let result = f.repo.gmail_confirm(
        &f.account,
        &store,
        &mut saved,
        personal_only(),
        &p.receipt,
        cancel,
        |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 5 {
                stopping.cancel();
            }
            Ok(())
        },
    );
    assert_eq!(result.unwrap_err(), AcquisitionError::Cancelled);
    drop(store);
    let run = f.run();
    assert_eq!(run.status, Status::Stopped);
    assert!(run.stop_requested);
    assert!(run.batches[0].verified());
    assert!(!run.batches[1].attempted());
    let wires = f.wires();
    assert_eq!(wires.len(), 2);
    for wire in &wires {
        assert_eq!(wire.split(' ').nth(2).unwrap().split(',').count(), 499);
    }
    f.resume(false).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert_eq!(f.wires()[..2], wires[..]);
    assert_eq!(f.wires().len(), 4);
}
