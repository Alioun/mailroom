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
    let bound = f.run().plan.binding.material;
    let store = f.store();
    assert_eq!(
        model::material(&store.load_state().unwrap(), store.keys()).unwrap(),
        bound,
        "a run reuses its material only while it changes nothing but its journal"
    );
    drop(store);
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

#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn scale_gmail_label_backlog() {
    use crate::census::scale;
    let f = setup(|root| {
        std::fs::write(root.join("gmail-large"), scale::run_from_env().to_string()).unwrap()
    });
    let setup = scale::inflate_store(&f.store(), true);
    scale::report("gmail.setup", setup, Default::default());
    let p = scale::measure("gmail.preview", serde_json::json!({}), || {
        f.preview(personal_only())
    })
    .unwrap();
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let result = scale::measure(
        "gmail.confirm_run",
        serde_json::json!({"batches": p.operation.batches.len()}),
        || {
            f.repo.gmail_confirm(
                &f.account,
                &store,
                &mut saved,
                personal_only(),
                &p.receipt,
                Cancellation::default(),
                |_| Ok(()),
            )
        },
    );
    drop(store);
    let journal = serde_json::to_vec(&f.saved().gmail_backlog).unwrap().len();
    scale::report(
        "gmail.result",
        serde_json::json!({"result": format!("{result:?}"), "status": f.run().status, "journal_bytes": journal}),
        Default::default(),
    );
    let undo = scale::measure("gmail.undo_preview", serde_json::json!({}), || {
        f.undo_preview()
    })
    .unwrap();
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let result = scale::measure("gmail.undo", serde_json::json!({}), || {
        f.repo.gmail_undo(
            &f.account,
            &store,
            &mut saved,
            &undo.run,
            &undo.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
    });
    drop(store);
    scale::report(
        "gmail.undo_result",
        serde_json::json!({"result": format!("{result:?}"), "status": f.run().status}),
        Default::default(),
    );
}

/// Q27 for Gmail label runs: Undo until day 90, then a summary; an
/// uncertain run is kept whole however old it is.
#[test]
fn label_runs_follow_the_ninety_day_undo_window_and_uncertain_runs_stay_whole() {
    use crate::backlog::retention::WINDOW_SECS;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    const T0: u64 = 1_800_000_000;
    let mut f = setup(|_| {});
    let clock = Arc::new(AtomicU64::new(T0));
    f.repo.clock = Some(clock.clone());
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    let run = f.run();
    assert_eq!((run.status, run.completed_at), (Status::Complete, Some(T0)));
    let retain = |f: &Fixture| {
        let store = f.store();
        let mut saved = store.load_state().unwrap();
        f.repo.retain_backlog(&store, &mut saved).unwrap();
        saved
    };
    clock.store(T0 + WINDOW_SECS - 1, Ordering::SeqCst);
    assert_eq!(retain(&f).gmail_backlog.runs.len(), 1);
    assert!(f.undo_preview().is_ok());
    clock.store(T0 + WINDOW_SECS, Ordering::SeqCst);
    assert!(f.undo_preview().is_err());
    let compacted = retain(&f).gmail_backlog;
    assert!(compacted.runs.is_empty());
    let summary = &compacted.summaries[0];
    assert_eq!(summary.id, run.id);
    assert_eq!(summary.messages, run.plan.filing.changes.len());
    assert_eq!(summary.applied, summary.messages);
    f.saved().gmail_backlog.validate(&f.account.id).unwrap();

    let mut f = setup(|_| {});
    f.repo.clock = Some(Arc::new(AtomicU64::new(T0 + 50 * WINDOW_SECS)));
    f.mode("lost");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    let uncertain = f.run();
    assert_eq!(uncertain.status, Status::Uncertain);
    f.repo.clock = Some(Arc::new(AtomicU64::new(T0 + 100 * WINDOW_SECS)));
    let kept = retain(&f).gmail_backlog;
    assert!(kept.summaries.is_empty());
    assert!(kept.runs[0] == uncertain);
}

/// Live Gmail label operations against a fake whose All Mail holds a
/// personal-scale mailbox, over the link `$MAILROOM_FIXTURE_LINK` emulates.
/// Same phases and environment as the MOVE measurement, with `undo_lost`
/// dropping the reply of each label STORE.
#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn latency_gmail_phases() {
    use crate::census::scale;
    let root = std::path::PathBuf::from(std::env::var("MAILROOM_SCALE_ROOT").unwrap());
    let phases = std::env::var("MAILROOM_LATENCY_PHASES").unwrap();
    if phases.starts_with("setup") {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace-authority"), "gmail-label-tests-v1").unwrap();
        std::fs::write(root.join("gmail-large"), "501").unwrap();
        std::fs::write(
            root.join("gmail-scale"),
            scale::spec_from_env(true).messages.to_string(),
        )
        .unwrap();
    }
    let (repo, account, _fake) = fixture_install(&root);
    let open = || repo.store(&account).unwrap();
    let report = |store: &EncryptedStore, operation: &str, result: String| {
        let saved = store.load_state().unwrap();
        let r = saved.gmail_backlog.runs.last();
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
            let result = scale::measure(&format!("gmail.{phase}"), serde_json::json!({}), || {
                repo.scan(&account.id, cancel.clone(), |_| {})
            });
            report(
                &open(),
                &format!("gmail.after_{phase}"),
                format!("{:?}", result.map(|c| c.folders.len())),
            );
            continue;
        }
        let store = open();
        match phase {
            "setup" => report(&store, "gmail.setup", "installed".into()),
            "preview" => {
                let saved = store.load_state().unwrap();
                let result = scale::measure("gmail.preview", serde_json::json!({}), || {
                    repo.gmail_preview(&account, &store, &saved, personal_only(), cancel.clone())
                });
                report(
                    &store,
                    "gmail.after_preview",
                    format!("{:?}", result.map(|p| p.operation.batches.len())),
                );
            }
            "confirm" => {
                let saved = store.load_state().unwrap();
                let preview = repo
                    .gmail_preview(&account, &store, &saved, personal_only(), cancel.clone())
                    .unwrap();
                crate::acquisition::backlog::STOP_BETWEEN_BATCHES
                    .with(|s| *s.borrow_mut() = Some((std::time::Instant::now(), cancel.clone())));
                let mut saved = store.load_state().unwrap();
                let result = scale::measure("gmail.confirm", serde_json::json!({}), || {
                    repo.gmail_confirm(
                        &account,
                        &store,
                        &mut saved,
                        personal_only(),
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                crate::acquisition::backlog::STOP_BETWEEN_BATCHES.with(|s| *s.borrow_mut() = None);
                report(&store, "gmail.after_confirm", format!("{result:?}"));
            }
            "resume" => {
                let mut saved = store.load_state().unwrap();
                let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("gmail.resume", serde_json::json!({}), || {
                    repo.gmail_run(
                        &account,
                        &store,
                        &mut saved,
                        &id,
                        false,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                report(&store, "gmail.after_resume", format!("{result:?}"));
            }
            "undo_preview" => {
                let saved = store.load_state().unwrap();
                let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("gmail.undo_preview", serde_json::json!({}), || {
                    repo.gmail_undo_preview(&account, &store, &saved, &id, cancel.clone())
                });
                report(
                    &store,
                    "gmail.after_undo_preview",
                    format!("{:?}", result.map(|p| p.conflicts)),
                );
            }
            "undo_lost" => {
                let saved = store.load_state().unwrap();
                let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
                let preview = repo
                    .gmail_undo_preview(&account, &store, &saved, &id, cancel.clone())
                    .unwrap();
                std::fs::write(root.join("gmail-mode"), "lost").unwrap();
                let mut saved = store.load_state().unwrap();
                let result = scale::measure("gmail.undo", serde_json::json!({}), || {
                    repo.gmail_undo(
                        &account,
                        &store,
                        &mut saved,
                        &id,
                        &preview.receipt,
                        cancel.clone(),
                        |_| Ok(()),
                    )
                });
                std::fs::write(root.join("gmail-mode"), "").unwrap();
                report(&store, "gmail.after_undo", format!("{result:?}"));
            }
            "reconcile" => {
                let mut saved = store.load_state().unwrap();
                let id = saved.gmail_backlog.runs.last().unwrap().id.clone();
                let result = scale::measure("gmail.reconcile", serde_json::json!({}), || {
                    repo.gmail_reconcile(&account, &store, &mut saved, &id, cancel.clone())
                });
                report(&store, "gmail.after_reconcile", format!("{result:?}"));
            }
            _ => unreachable!("{phase}"),
        }
    }
}

use crate::acquisition::folder_provisioning::Preview as LabelPreview;
use crate::census::folder_provisioning::{
    Failure as LabelFailure, Outcome as StepOutcome, Role, Status as LabelStatus,
};

impl Fixture {
    fn other(&self, action: &str) {
        fake::other_client(&self.repo, &self.account.id, self.path(), action);
    }
    fn label_preview(&self) -> Result<LabelPreview, AcquisitionError> {
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo.label_preview(
            &self.account,
            &store,
            &saved,
            Options::default(),
            Cancellation::default(),
        )
    }
    fn label_confirm_with(
        &self,
        receipt: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.label_confirm(
            &self.account,
            &store,
            &mut saved,
            Options::default(),
            receipt,
            cancel,
            |_| Ok(()),
        )
    }
    fn label_confirm(&self) -> Result<(), AcquisitionError> {
        let p = self.label_preview().unwrap();
        self.label_confirm_with(&p.receipt, Cancellation::default())
    }
    fn label_reconcile(&self) -> Result<(), AcquisitionError> {
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        let id = saved.label_provisioning.history.last().unwrap().id.clone();
        self.repo.label_reconcile(
            &self.account,
            &store,
            &mut saved,
            &id,
            Cancellation::default(),
        )
    }
    fn operation(&self) -> crate::census::folder_provisioning::Operation {
        self.saved()
            .label_provisioning
            .history
            .last()
            .unwrap()
            .clone()
    }
    /// Every CREATE the fake received, with whether it took effect.
    fn creates(&self) -> Vec<(String, bool)> {
        std::fs::read_to_string(self.path().join("gmail-create-exact.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                (
                    v["command"].as_str().unwrap().to_owned(),
                    v["effected"].as_bool().unwrap(),
                )
            })
            .collect()
    }
    fn mailboxes(&self) -> Vec<crate::census::folder_provisioning::Mailbox> {
        self.provider()
            .synthetic_folders
            .unwrap()
            .inventory
            .mailboxes
    }
    fn names(&self) -> BTreeSet<String> {
        self.mailboxes()
            .into_iter()
            .map(|m| m.name.decoded)
            .collect()
    }
    /// Captures the primary and provider stores when `at` is reached, as a
    /// crash there would leave them, and puts them back once `action`
    /// returns. Reopening then sees exactly what a killed process left.
    fn crash(&self, at: &str, action: impl FnOnce()) {
        let primary = self.store().path();
        let provider = self
            .repo
            .fixture_sieve_store(&self.account.id, self.path())
            .unwrap()
            .path();
        let log = self.path().join("gmail-create-exact.jsonl");
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let capture = captured.clone();
        let (p, q, l) = (primary.clone(), provider.clone(), log.clone());
        AT_BOUNDARY.set(Some((
            at.into(),
            Box::new(move || {
                if capture.borrow().is_none() {
                    *capture.borrow_mut() = Some((
                        std::fs::read(&p).unwrap(),
                        std::fs::read(&q).unwrap(),
                        std::fs::read(&l).unwrap_or_default(),
                    ));
                }
            }),
        )));
        action();
        AT_BOUNDARY.set(None);
        let (a, b, c) = captured.take().expect("crash boundary reached");
        std::fs::write(primary, a).unwrap();
        std::fs::write(provider, b).unwrap();
        std::fs::write(log, c).unwrap();
    }
    fn close_preview(&self) -> Result<crate::backlog::protocol::ClosePreview, AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let saved = store.load_state().unwrap();
        self.repo
            .gmail_close_preview(&self.account, &store, &saved, &id, Cancellation::default())
    }
    fn close(&self, receipt: &str) -> Result<(), AcquisitionError> {
        let id = self.run().id;
        let store = self.store();
        let mut saved = store.load_state().unwrap();
        self.repo.gmail_close(
            &self.account,
            &store,
            &mut saved,
            &id,
            receipt,
            Cancellation::default(),
        )
    }
    fn refresh(&self) {
        self.repo
            .scan(&self.account.id, Cancellation::default(), |_| {})
            .unwrap();
    }
}

const BUSINESS_ROOT: &str = "Geschäftlich";

/// A fixture whose Gmail lacks the business root and its Studio label.
fn without_business() -> Fixture {
    let f = setup(|_| {});
    f.other(&format!("drop_label:{BUSINESS}"));
    f.other(&format!("drop_label:{BUSINESS_ROOT}"));
    f
}

#[test]
fn missing_labels_are_created_top_down_read_back_and_then_sorted() {
    let f = without_business();
    let before = f.mailboxes();
    let sorting = f.preview(Options::default()).unwrap();
    assert_eq!(
        sorting.missing_labels,
        [MissingLabel {
            name: BUSINESS.into(),
            messages: 1,
            blocked: None
        }]
    );
    assert_eq!(
        sorting.operation.plan.filing.excluded.get("missing_label"),
        Some(&1)
    );
    let p = f.label_preview().unwrap();
    let plan: Vec<_> = p
        .operation
        .plan
        .entries
        .iter()
        .map(|e| {
            (
                e.name.decoded.as_str(),
                e.name.wire.as_str(),
                e.role,
                e.existing.is_some(),
            )
        })
        .collect();
    assert_eq!(
        plan,
        [
            (BUSINESS_ROOT, "Gesch&AOQ-ftlich", Role::Ancestor, false),
            (BUSINESS, "Gesch&AOQ-ftlich/Studio", Role::Leaf, false),
        ]
    );
    f.label_confirm_with(&p.receipt, Cancellation::default())
        .unwrap();
    let op = f.operation();
    assert_eq!(op.status, LabelStatus::Complete);
    assert!(
        op.steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Observed && s.acknowledgment)
    );
    assert_eq!(
        f.creates(),
        [
            ("CREATE \"Gesch&AOQ-ftlich\"".to_owned(), true),
            ("CREATE \"Gesch&AOQ-ftlich/Studio\"".to_owned(), true),
        ]
    );
    let after = f.mailboxes();
    let parent = after
        .iter()
        .find(|m| m.name.decoded == BUSINESS_ROOT)
        .unwrap();
    assert!(
        parent.attributes.contains("\\HasChildren"),
        "Gmail is assumed to flag the parent"
    );
    assert_eq!(
        after
            .iter()
            .filter(|m| m.name.decoded != BUSINESS_ROOT && m.name.decoded != BUSINESS)
            .collect::<Vec<_>>(),
        before.iter().collect::<Vec<_>>(),
        "no other label renamed, removed or changed"
    );
    assert!(
        f.wires().is_empty(),
        "creating labels stores no message labels"
    );
    assert_eq!(
        f.label_preview().unwrap_err(),
        AcquisitionError::InvalidInput,
        "nothing left to create"
    );
    let sorting = f.preview(Options::default()).unwrap();
    assert!(sorting.missing_labels.is_empty());
    assert!(
        !sorting
            .operation
            .plan
            .filing
            .excluded
            .contains_key("missing_label")
    );
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert!(f.labels("1006").1.contains(BUSINESS));
    assert_eq!(f.creates().len(), 2, "sorting never creates labels");
    f.saved()
        .label_provisioning
        .validate_as(
            &f.account.id,
            crate::census::folder_provisioning::GMAIL_PROFILE,
        )
        .unwrap();
}

#[test]
fn an_existing_label_needs_no_create_and_reconcile_writes_nothing() {
    let f = setup(|_| {});
    assert_eq!(
        f.label_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );
    let f = setup(|_| {});
    f.other(&format!("drop_label:{BUSINESS}"));
    let p = f.label_preview().unwrap();
    assert_eq!(
        p.operation
            .plan
            .entries
            .iter()
            .map(|e| (e.name.decoded.as_str(), e.existing.is_some()))
            .collect::<Vec<_>>(),
        [(BUSINESS_ROOT, true), (BUSINESS, false)]
    );
    f.label_confirm_with(&p.receipt, Cancellation::default())
        .unwrap();
    assert_eq!(
        f.creates(),
        [("CREATE \"Gesch&AOQ-ftlich/Studio\"".to_owned(), true)]
    );
    let revision = f.saved().revision;
    f.label_reconcile().unwrap();
    f.label_reconcile().unwrap();
    assert_eq!(f.saved().revision, revision);
    assert_eq!(f.operation().status, LabelStatus::Complete);
    assert_eq!(f.creates().len(), 1);
}

#[test]
fn a_label_another_client_creates_is_never_created_again() {
    // Before the preview: nothing to create.
    let f = without_business();
    f.other(&format!("create_label:{BUSINESS}"));
    assert_eq!(
        f.label_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );
    assert!(f.creates().is_empty());

    // Between preview and confirmation: the approval is stale.
    let f = without_business();
    let p = f.label_preview().unwrap();
    f.other(&format!("create_label:{BUSINESS}"));
    assert!(
        f.label_confirm_with(&p.receipt, Cancellation::default())
            .is_err()
    );
    assert!(f.creates().is_empty());
    assert!(f.saved().label_provisioning.is_empty());

    // After the final read, before the CREATE: refused, a conflict, no retry.
    let f = setup(|_| {});
    f.other(&format!("drop_label:{BUSINESS}"));
    std::fs::write(f.path().join("gmail-create-mode"), "concurrent").unwrap();
    assert!(f.label_confirm().is_err());
    std::fs::write(f.path().join("gmail-create-mode"), "").unwrap();
    let op = f.operation();
    assert_eq!(op.status, LabelStatus::Conflict);
    assert_eq!(op.steps[0].outcome, StepOutcome::Refused);
    assert_eq!(op.steps[0].failure, Some(LabelFailure::No));
    assert_eq!(
        f.creates(),
        [("CREATE \"Gesch&AOQ-ftlich/Studio\"".to_owned(), false)]
    );
    f.label_reconcile().unwrap();
    assert_eq!(f.operation(), op, "a conflict stays recorded");
    assert_eq!(f.creates().len(), 1);
    assert!(
        f.preview(Options::default())
            .unwrap()
            .missing_labels
            .is_empty()
    );
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    assert!(f.labels("1006").1.contains(BUSINESS));
}

#[test]
fn case_variants_and_case_changes_are_conflicts_never_renames() {
    // A label that differs only in case blocks creation; nothing is reused.
    let f = setup(|_| {});
    f.other(&format!("drop_label:{BUSINESS}"));
    f.other("create_label:Geschäftlich/studio");
    let sorting = f.preview(Options::default()).unwrap();
    assert_eq!(
        sorting.missing_labels,
        [MissingLabel {
            name: BUSINESS.into(),
            messages: 1,
            blocked: Some(LabelFailure::UnsupportedName)
        }]
    );
    assert_eq!(
        f.label_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );
    assert!(f.creates().is_empty());
    assert!(f.names().contains("Geschäftlich/studio"));

    // Gmail storing another case than requested reads back as a conflict.
    let f = setup(|_| {});
    f.other(&format!("drop_label:{BUSINESS}"));
    std::fs::write(f.path().join("gmail-create-mode"), "case_fold").unwrap();
    assert!(f.label_confirm().is_err());
    let op = f.operation();
    assert_eq!(op.status, LabelStatus::Conflict);
    assert_eq!(op.steps[0].outcome, StepOutcome::Conflict);
    assert!(op.steps[0].acknowledgment);
    f.label_reconcile().unwrap();
    assert_eq!(f.creates().len(), 1, "never retried");
    assert!(f.names().contains("Geschäftlich/studio"));
    assert!(!f.names().contains(BUSINESS));
    assert_eq!(
        f.label_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );

    // Labels under the system root are never created.
    let topology = f.provider().synthetic_folders.unwrap().inventory;
    assert_eq!(
        creatable("[Gmail]/Neu", &topology, Some("[Gmail]")),
        Err(LabelFailure::Unsupported)
    );
    assert_eq!(
        creatable("[Google Mail]/Neu", &topology, Some("[Google Mail]")),
        Err(LabelFailure::Unsupported)
    );
    assert_eq!(creatable("Privat/Neu", &topology, Some("[Gmail]")), Ok(()));
}

#[test]
fn refused_and_lost_creates_stay_honest_and_need_fresh_review() {
    let f = without_business();
    std::fs::write(f.path().join("gmail-create-mode"), "refuse").unwrap();
    assert!(f.label_confirm().is_err());
    std::fs::write(f.path().join("gmail-create-mode"), "").unwrap();
    let op = f.operation();
    assert_eq!((op.status, op.steps.len()), (LabelStatus::Refused, 1));
    assert_eq!(op.steps[0].outcome, StepOutcome::Refused);
    assert!(
        f.label_preview().is_err(),
        "a refused operation needs reconcile first"
    );
    f.label_reconcile().unwrap();
    assert_eq!(f.operation().steps[0].outcome, StepOutcome::Absent);
    assert_eq!(f.operation().status, LabelStatus::FreshReview);
    f.label_confirm().unwrap();
    assert_eq!(f.operation().status, LabelStatus::Complete);

    let f = without_business();
    std::fs::write(f.path().join("gmail-create-mode"), "lost").unwrap();
    assert!(f.label_confirm().is_err());
    std::fs::write(f.path().join("gmail-create-mode"), "").unwrap();
    let op = f.operation();
    assert_eq!(op.status, LabelStatus::Uncertain);
    assert_eq!(op.steps[0].outcome, StepOutcome::Uncertain);
    assert!(f.names().contains(BUSINESS_ROOT), "the effect happened");
    assert!(f.label_preview().is_err());
    f.label_reconcile().unwrap();
    let op = f.operation();
    assert_eq!(op.steps[0].outcome, StepOutcome::Observed);
    assert_eq!(
        op.status,
        LabelStatus::FreshReview,
        "the leaf still needs a fresh approval"
    );
    assert_eq!(f.creates().len(), 1, "observation never creates");
    let p = f.label_preview().unwrap();
    assert_eq!(
        p.operation
            .plan
            .entries
            .iter()
            .filter(|e| e.existing.is_none())
            .map(|e| e.name.decoded.as_str())
            .collect::<Vec<_>>(),
        [BUSINESS]
    );
}

#[test]
fn label_creation_crash_boundaries_never_repeat_or_continue_on_reopen() {
    // (boundary, CREATEs at the crash, step outcome after reopen, outcome
    // after reconcile, status after reconcile)
    for (at, created, reopened, reconciled, status) in [
        ("label_consumed", 0, None, None, LabelStatus::FreshReview),
        (
            "label_first_intent",
            0,
            Some(StepOutcome::Uncertain),
            Some(StepOutcome::Absent),
            LabelStatus::FreshReview,
        ),
        (
            "label_acknowledged",
            1,
            Some(StepOutcome::Uncertain),
            Some(StepOutcome::Observed),
            LabelStatus::FreshReview,
        ),
        (
            "label_observed",
            1,
            Some(StepOutcome::Observed),
            Some(StepOutcome::Observed),
            LabelStatus::FreshReview,
        ),
        (
            "label_leaf_intent",
            1,
            Some(StepOutcome::Uncertain),
            Some(StepOutcome::Absent),
            LabelStatus::FreshReview,
        ),
    ] {
        let f = without_business();
        f.crash(at, || {
            let _ = f.label_confirm();
        });
        let effected = |f: &Fixture| f.creates().iter().filter(|(_, e)| *e).count();
        let provider = f.names();
        assert_eq!(provider.contains(BUSINESS_ROOT), created == 1, "{at}");
        assert!(!provider.contains(BUSINESS), "{at}");
        let op = f.operation();
        let expected = if op.steps.is_empty() {
            LabelStatus::Stopped
        } else if op.steps.last().unwrap().outcome == StepOutcome::Uncertain {
            LabelStatus::Uncertain
        } else {
            LabelStatus::Stopped
        };
        assert_eq!(
            op.status, expected,
            "{at}: reopening never implies a running worker"
        );
        assert_eq!(op.steps.last().map(|s| s.outcome), reopened, "{at}");
        let sent = f.creates().len();
        assert!(
            f.label_preview().is_err(),
            "{at}: unresolved work blocks a new approval"
        );
        f.label_reconcile().unwrap();
        let op = f.operation();
        assert_eq!(op.steps.last().map(|s| s.outcome), reconciled, "{at}");
        assert_eq!(op.status, status, "{at}");
        f.label_reconcile().unwrap();
        assert_eq!(f.operation(), op, "{at}: repeated reconcile converges");
        assert_eq!(
            f.creates().len(),
            sent,
            "{at}: reopen and reconcile never create"
        );
        f.label_confirm().unwrap();
        assert_eq!(f.operation().status, LabelStatus::Complete, "{at}");
        assert_eq!(effected(&f), 2, "{at}: each label created exactly once");
        assert!(f.names().contains(BUSINESS));
    }
}

#[test]
fn cancel_between_labels_keeps_the_created_parent() {
    let f = without_business();
    let p = f.label_preview().unwrap();
    let cancel = Cancellation::default();
    let stopping = cancel.clone();
    AT_BOUNDARY.set(Some((
        "label_observed".into(),
        Box::new(move || stopping.cancel()),
    )));
    let result = f.label_confirm_with(&p.receipt, cancel);
    AT_BOUNDARY.set(None);
    assert_eq!(result.unwrap_err(), AcquisitionError::Cancelled);
    let op = f.operation();
    assert_eq!(op.status, LabelStatus::Stopped);
    assert_eq!(op.steps.len(), 1);
    assert!(f.names().contains(BUSINESS_ROOT));
    assert!(!f.names().contains(BUSINESS));
    f.label_reconcile().unwrap();
    assert_eq!(f.operation().status, LabelStatus::FreshReview);
    f.label_confirm().unwrap();
    assert_eq!(
        f.creates(),
        [
            ("CREATE \"Gesch&AOQ-ftlich\"".to_owned(), true),
            ("CREATE \"Gesch&AOQ-ftlich/Studio\"".to_owned(), true),
        ]
    );
}

#[test]
fn closing_a_conflicted_run_with_nothing_to_undo_unblocks_new_runs() {
    let f = setup(|_| {});
    f.mode("refuse");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    let run = f.run();
    assert_eq!(run.status, Status::Conflict);
    assert!(
        f.preview(Options::default()).is_err(),
        "the conflicted run blocks new runs"
    );
    let wires = f.wires();
    let p = f.close_preview().unwrap();
    assert_eq!(
        p.reason,
        crate::backlog::protocol::CloseReason::NothingToUndo
    );
    assert!(f.close(&"0".repeat(64)).is_err());
    let labels: Vec<_> = ["1001", "1002", "1003"]
        .iter()
        .map(|id| f.labels(id))
        .collect();
    f.close(&p.receipt).unwrap();
    let closed = f.run();
    assert_eq!(
        closed.status,
        Status::Conflict,
        "the conflict stays recorded"
    );
    assert_eq!(closed.closed.as_ref().unwrap().receipt, p.receipt);
    assert!(closed.batches == run.batches, "the journal stays whole");
    assert_eq!(f.wires(), wires, "closing changes no mail");
    assert_eq!(
        ["1001", "1002", "1003"]
            .iter()
            .map(|id| f.labels(id))
            .collect::<Vec<_>>(),
        labels
    );
    assert!(f.close_preview().is_err(), "a run closes once");
    assert!(f.undo_preview().is_err());
    assert!(f.reconcile().is_err());
    assert!(f.resume(false).is_err());
    let mut retained = f.saved().gmail_backlog;
    assert!(
        !retained.compact(u64::MAX / 2),
        "a closed conflict is never compacted"
    );
    f.saved().gmail_backlog.validate(&f.account.id).unwrap();
    f.confirm(Options::default(), |_| Ok(())).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert_eq!(f.saved().gmail_backlog.runs.len(), 2);
}

#[test]
fn a_run_with_anything_undoable_or_uncertain_cannot_be_closed() {
    // Undoable: a label was added before another client flagged the message.
    let f = setup(|_| {});
    std::fs::write(f.path().join("gmail-race"), "after:flag").unwrap();
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    assert_eq!(f.run().status, Status::Conflict);
    assert!(!f.undo_preview().unwrap().reverse.is_empty());
    assert_eq!(
        f.close_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );
    f.undo().unwrap();
    assert!(f.run().settled());

    // Uncertain members, even beside observed conflicts.
    let f = setup(|_| {});
    f.mode("partial");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    assert_eq!(f.run().status, Status::Conflict);
    assert_eq!(
        f.close_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );

    // An uncertain run is reconciled, not closed.
    let f = setup(|_| {});
    f.mode("lost");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    assert_eq!(f.run().status, Status::Uncertain);
    assert_eq!(
        f.close_preview().unwrap_err(),
        AcquisitionError::InvalidInput
    );

    // A smuggled closure on an undoable or uncertain run fails validation.
    let mut journal = f.saved().gmail_backlog;
    journal.runs[0].status = Status::Conflict;
    journal.runs[0].closed = Some(Closure {
        at: 1,
        receipt: "a".repeat(64),
    });
    assert!(journal.validate(&f.account.id).is_err());
}

#[test]
fn closing_is_offered_when_undo_is_unavailable_or_already_ran() {
    // The approval inputs changed after effects: Undo is not offered.
    let f = setup(|_| {});
    std::fs::write(f.path().join("gmail-race"), "after:flag").unwrap();
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    fake::save_workspace(&f.repo, &f.account);
    assert!(f.undo_preview().is_err());
    let p = f.close_preview().unwrap();
    assert_eq!(
        p.reason,
        crate::backlog::protocol::CloseReason::UndoUnavailable
    );
    let labels = f.labels("1001");
    f.close(&p.receipt).unwrap();
    assert_eq!(f.labels("1001"), labels, "completed changes stay");

    // Undo ran and ended in conflict.
    let f = setup(|_| {});
    f.confirm(personal_only(), |_| Ok(())).unwrap();
    f.mode("refuse");
    assert!(f.undo().is_err());
    f.mode("");
    assert_eq!(f.run().status, Status::Conflict);
    assert!(f.preview(Options::default()).is_err());
    let p = f.close_preview().unwrap();
    assert_eq!(p.reason, crate::backlog::protocol::CloseReason::UndoEnded);
    f.close(&p.receipt).unwrap();
    assert!(f.preview(Options::default()).is_ok());
}

#[test]
fn a_refresh_keeps_resume_and_undo_while_every_effect_rechecks_messages() {
    // Resume after a refresh runs only the remainder.
    let f = setup(|root| std::fs::write(root.join("gmail-large"), "501").unwrap());
    let p = f.preview(personal_only()).unwrap();
    let cancel = Cancellation::default();
    let stopping = cancel.clone();
    let calls = std::cell::Cell::new(0);
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let _ = f.repo.gmail_confirm(
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
    drop(store);
    assert_eq!(f.run().status, Status::Stopped);
    let census = f.saved().census.folders[ALL].clone();
    f.refresh();
    assert!(
        f.saved().census.folders[ALL] != census,
        "the refresh saw the labels change"
    );
    f.resume(false).unwrap();
    assert_eq!(f.run().status, Status::Complete);
    assert_eq!(f.wires().len(), 4);

    // Undo after a refresh, and a stale Undo approval after a second one.
    f.refresh();
    let undo = f.undo_preview().unwrap();
    assert!(!undo.reverse.is_empty());
    fake::other_client(&f.repo, &f.account.id, f.path(), "flag:1002");
    f.refresh();
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    assert!(
        f.repo
            .gmail_undo(
                &f.account,
                &store,
                &mut saved,
                &undo.run,
                &undo.receipt,
                Cancellation::default(),
                |_| Ok(())
            )
            .is_err(),
        "an Undo approval binds the state it was previewed on"
    );
    drop(store);
    f.undo().unwrap();
    assert_eq!(f.run().status, Status::Undone);
    assert!(f.labels("1002").2.contains("\\Flagged"), "later flags stay");
}

#[test]
fn after_a_refresh_a_changed_message_is_still_refused_before_any_store() {
    let f = setup(|root| std::fs::write(root.join("gmail-large"), "501").unwrap());
    let p = f.preview(personal_only()).unwrap();
    let cancel = Cancellation::default();
    let stopping = cancel.clone();
    let calls = std::cell::Cell::new(0);
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let _ = f.repo.gmail_confirm(
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
    drop(store);
    let remaining = f.run().batches[1]
        .intent()
        .messages()
        .values()
        .next()
        .unwrap()
        .gmail_id
        .clone();
    fake::other_client(
        &f.repo,
        &f.account.id,
        f.path(),
        &format!("flag:{}", remaining.as_str()),
    );
    f.refresh();
    let wires = f.wires().len();
    assert!(f.resume(false).is_err());
    assert_eq!(
        f.wires().len(),
        wires,
        "the changed message stops the batch before its store"
    );
}

#[test]
fn runs_approved_before_this_rule_keep_their_census_binding() {
    let f = setup(|_| {});
    f.mode("refuse");
    assert!(f.confirm(personal_only(), |_| Ok(())).is_err());
    f.mode("");
    let store = f.store();
    let mut saved = store.load_state().unwrap();
    let legacy = model::material_legacy(&saved, store.keys());
    saved.gmail_backlog.runs[0].plan.binding.material = legacy[0].clone();
    store.save_state(&mut saved).unwrap();
    drop(store);
    assert!(
        f.undo_preview().is_ok(),
        "an unchanged census still matches the v2 form"
    );
    f.refresh();
    fake::other_client(&f.repo, &f.account.id, f.path(), "label:1009:Später");
    f.refresh();
    assert!(
        f.undo_preview().is_err(),
        "a refresh still invalidates a v2 run"
    );
    let p = f.close_preview().unwrap();
    assert_eq!(
        p.reason,
        crate::backlog::protocol::CloseReason::UndoUnavailable
    );
    f.close(&p.receipt).unwrap();
    assert!(f.preview(Options::default()).is_ok());
}
