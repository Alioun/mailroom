use super::*;
use crate::backlog::{imap_copy::NextAction, protocol::Batch};

struct Checkpoints {
    saved: Vec<Vec<u8>>,
    intents: Vec<NextAction>,
    saves: usize,
    stop_at: Option<usize>,
    fail_save: Option<usize>,
    stale_intent: Option<usize>,
    cancel: Cancellation,
}
impl Default for Checkpoints {
    fn default() -> Self {
        Self {
            saved: vec![],
            intents: vec![],
            saves: 0,
            stop_at: None,
            fail_save: None,
            stale_intent: None,
            cancel: Cancellation::default(),
        }
    }
}
impl CopyCheckpoints for Checkpoints {
    fn validate_intent(&mut self, e: &CopyEvidence) -> Result<(), Error> {
        self.intents.push(e.next_action());
        if self.stale_intent == Some(self.intents.len()) {
            Err(Error::Unstable)
        } else {
            Ok(())
        }
    }
    fn save(&mut self, e: &CopyEvidence) -> Result<(), Error> {
        self.saves += 1;
        if self.fail_save == Some(self.saves) {
            return Err(Error::Limit);
        }
        let bytes = serde_json::to_vec(e).unwrap();
        assert_eq!(serde_json::from_slice::<CopyEvidence>(&bytes).unwrap(), *e);
        self.saved.push(bytes);
        if self.stop_at == Some(self.saves) {
            self.cancel.cancel();
        }
        Ok(())
    }
}
fn complete_script(source: &InstalledSource, e: &CopyEvidence) -> Script {
    let mut io = copying(
        source,
        e,
        Ok(b"F1 OK [COPYUID 202 4,8 60,64] copied\r\n".to_vec()),
    );
    io.flights.push_back(paired(
        e,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    io.flights.push_back(paired(
        e,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    let (_, prepared) = prepared();
    let reply = format!(
        "* 2 FETCH (UID 4 MODSEQ ({}))\r\n* 3 FETCH (UID 8 MODSEQ ({}))\r\nF1 OK marked\r\n",
        BASE + 20,
        BASE + 20
    );
    io.flights
        .extend(script(source, &prepared, Ok(reply.into_bytes())).flights);
    let observed = String::from_utf8(observation(
        e.intent().source(),
        &[(4, BASE + 20), (8, BASE + 20)],
    ))
    .unwrap()
    .replace(
        "FLAGS (\\Seen \\Recent $owner)",
        "FLAGS (\\Seen \\Deleted $owner)",
    );
    io.flights.push_back(Flight { event: "observe-store", commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into(), "UID FETCH 4,8 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into()], response: Ok(observed.into_bytes()) });
    io.flights.push_back(paired(
        e,
        &[4, 8],
        &[(4, BASE + 20, true), (8, BASE + 20, true)],
        BASE + 20,
    ));
    io.flights.push_back(Flight {
        event: "expunge-select",
        commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into()],
        response: Ok(selection(e.intent().source(), BASE + 20, 1)),
    });
    io.flights.push_back(Flight {
        event: "expunge",
        commands: vec!["UID EXPUNGE 4,8".into()],
        response: Ok(b"* 2 EXPUNGE\r\n* 2 EXPUNGE\r\nF1 OK complete\r\n".to_vec()),
    });
    io.flights.push_back(paired(e, &[4, 8], &[], BASE + 30));
    io
}
fn count(io: &Script, event: &str) -> usize {
    io.events.borrow().iter().filter(|e| **e == event).count()
}

#[test]
fn full_filing_uses_three_separate_effects_and_ten_durable_checkpoints() {
    let source = source();
    let permit = permitted(&source);
    let mut e = initial();
    let mut io = complete_script(&source, &e);
    let mut checkpoints = Checkpoints::default();
    io.conditional()
        .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
        .unwrap();
    assert_eq!(e.next_action(), NextAction::Complete);
    assert_eq!(checkpoints.saved.len(), 10);
    assert_eq!(
        checkpoints.intents,
        [
            NextAction::ObserveCopy,
            NextAction::ObserveStore(0),
            NextAction::ObserveExpunge(0)
        ]
    );
    assert_eq!(
        (
            count(&io, "copy"),
            count(&io, "store"),
            count(&io, "expunge")
        ),
        (1, 1, 1)
    );
    assert!(io.flights.is_empty());
    io.conditional()
        .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
        .unwrap();
    assert_eq!(checkpoints.saved.len(), 10);
}

#[test]
fn cancel_after_verified_partial_effects_keeps_them_and_resume_continues_the_next_phase() {
    for (stop_at, next) in [
        (4, NextAction::PrepareDeletion),
        (7, NextAction::Expunge(0)),
    ] {
        let source = source();
        let permit = permitted(&source);
        let mut e = initial();
        let mut io = complete_script(&source, &e);
        let mut checkpoints = Checkpoints {
            stop_at: Some(stop_at),
            ..Default::default()
        };
        let cancel = checkpoints.cancel.clone();
        assert_eq!(
            io.conditional()
                .file(&permit, &mut e, &cancel, &mut checkpoints),
            Err(Error::Cancelled)
        );
        assert_eq!(e.next_action(), next);
        assert_eq!(count(&io, "copy"), 1);
        assert_eq!(count(&io, "expunge"), 0);
        assert!(Batch::copy(e.clone()).resumable());
        let mut reopened: CopyEvidence =
            serde_json::from_slice(checkpoints.saved.last().unwrap()).unwrap();
        checkpoints.stop_at = None;
        io.conditional()
            .file(
                &permit,
                &mut reopened,
                &Cancellation::default(),
                &mut checkpoints,
            )
            .unwrap();
        assert_eq!(reopened.next_action(), NextAction::Complete);
        assert_eq!(
            (
                count(&io, "copy"),
                count(&io, "store"),
                count(&io, "expunge")
            ),
            (1, 1, 1)
        );
        assert!(io.flights.is_empty());
    }
}

#[test]
fn uncertain_checkpoints_require_read_only_reconciliation_before_explicit_resume() {
    for stop_at in [3, 6, 9] {
        let source = source();
        let permit = permitted(&source);
        let mut e = initial();
        let mut io = complete_script(&source, &e);
        let mut checkpoints = Checkpoints {
            stop_at: Some(stop_at),
            ..Default::default()
        };
        let cancel = checkpoints.cancel.clone();
        assert_eq!(
            io.conditional()
                .file(&permit, &mut e, &cancel, &mut checkpoints),
            Err(Error::Cancelled)
        );
        assert!(e.next_action().needs_observation());
        assert!(!Batch::copy(e.clone()).resumable());
        let commands_before = io.events.borrow().len();
        let saves_before = checkpoints.saved.len();
        assert_eq!(
            io.conditional()
                .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints),
            Err(Error::Unsupported)
        );
        assert_eq!(io.events.borrow().len(), commands_before);
        assert_eq!(checkpoints.saved.len(), saves_before);
        let effects = (
            count(&io, "copy"),
            count(&io, "store"),
            count(&io, "expunge"),
        );
        checkpoints.stop_at = None;
        io.conditional()
            .reconcile(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
            .unwrap();
        assert!(!e.next_action().needs_observation());
        assert_eq!(
            (
                count(&io, "copy"),
                count(&io, "store"),
                count(&io, "expunge")
            ),
            effects
        );
        io.conditional()
            .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
            .unwrap();
        assert_eq!(e.next_action(), NextAction::Complete);
        assert_eq!(
            (
                count(&io, "copy"),
                count(&io, "store"),
                count(&io, "expunge")
            ),
            (1, 1, 1)
        );
        assert!(io.flights.is_empty());
    }
}

#[test]
fn intent_revalidation_failure_prevents_the_next_effect_and_preserves_the_prior_checkpoint() {
    for stale_at in 1..=3 {
        let source = source();
        let permit = permitted(&source);
        let mut e = initial();
        let mut io = complete_script(&source, &e);
        let mut checkpoints = Checkpoints {
            stale_intent: Some(stale_at),
            ..Default::default()
        };
        assert_eq!(
            io.conditional()
                .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints),
            Err(Error::Unstable)
        );
        assert_eq!(count(&io, "copy"), usize::from(stale_at > 1));
        assert_eq!(count(&io, "store"), usize::from(stale_at > 2));
        assert_eq!(count(&io, "expunge"), 0);
        let next = checkpoints.saved.last().map(|bytes| {
            serde_json::from_slice::<CopyEvidence>(bytes)
                .unwrap()
                .next_action()
        });
        assert_eq!(
            next,
            match stale_at {
                1 => None,
                2 => Some(NextAction::PrepareDeletion),
                _ => Some(NextAction::Expunge(0)),
            }
        );
    }
}

#[test]
fn failing_or_cancelled_durable_intent_prevents_transmission() {
    for save_at in [1, 5, 8] {
        for cancel_in_save in [false, true] {
            let source = source();
            let permit = permitted(&source);
            let mut e = initial();
            let mut io = complete_script(&source, &e);
            let mut checkpoints = Checkpoints {
                stop_at: cancel_in_save.then_some(save_at),
                fail_save: (!cancel_in_save).then_some(save_at),
                ..Default::default()
            };
            let cancel = checkpoints.cancel.clone();
            assert_eq!(
                io.conditional()
                    .file(&permit, &mut e, &cancel, &mut checkpoints),
                Err(if cancel_in_save {
                    Error::Cancelled
                } else {
                    Error::Limit
                })
            );
            assert_eq!(count(&io, "copy"), usize::from(save_at > 1));
            assert_eq!(count(&io, "store"), usize::from(save_at > 5));
            assert_eq!(count(&io, "expunge"), 0);
            assert!(e.next_action().needs_observation());
            assert!(!Batch::copy(e).resumable());
        }
    }
}

#[test]
fn reconciliation_never_starts_a_known_next_effect_or_undoes_a_completed_one() {
    for mut e in [initial(), prepared().0, marked()] {
        let source = source();
        let permit = permitted(&source);
        let mut io = Script {
            source: source.identity(),
            flights: VecDeque::new(),
            events: Rc::new(RefCell::new(vec![])),
        };
        let mut checkpoints = Checkpoints::default();
        let before = serde_json::to_vec(&e).unwrap();
        io.conditional()
            .reconcile(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
            .unwrap();
        assert_eq!(serde_json::to_vec(&e).unwrap(), before);
        assert!(io.events.borrow().is_empty());
        assert!(checkpoints.saved.is_empty());
        assert!(checkpoints.intents.is_empty());
    }
}

#[test]
fn partial_conditional_store_finishes_only_the_certified_member_and_reports_conflict() {
    let source = source();
    let permit = permitted(&source);
    let mut e = initial();
    let mut io = complete_script(&source, &e);
    io.flights[7].response = Ok(acknowledged());
    io.flights[9] = paired(&e, &[4], &[(4, BASE + 20, true)], BASE + 20);
    io.flights[11].commands = vec!["UID EXPUNGE 4".into()];
    io.flights[12] = paired(&e, &[4], &[], BASE + 30);
    let mut checkpoints = Checkpoints::default();
    assert_eq!(
        io.conditional()
            .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints),
        Err(Error::Unstable)
    );
    assert_eq!(
        e.resolutions()[&uid(4)],
        Resolution::Filed {
            destination: uid(60)
        }
    );
    assert_eq!(e.resolutions()[&uid(8)], Resolution::Conflict);
    assert_eq!(e.next_action(), NextAction::Conflict);
    assert!(!Batch::copy(e).resumable());
    assert!(io.flights.is_empty());
}

#[test]
fn full_byte_filing_persists_ten_account_checkpoints_and_reopens_exact_history() {
    use crate::acquisition::backlog::copy_checkpoints::{CopySlot, tests::fixture};
    let mut e = initial();
    let (_root, repo, account, store, mut saved) = fixture(e.clone());
    let source = repo.backlog_source.as_deref().unwrap();
    let permit = permitted(source);
    let mut io = complete_script(source, &e);
    let id = saved.protocol_backlog.runs[0].id.clone();
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let revision = saved.revision;
    let validations = std::cell::Cell::new(0);
    let fresh = |_: &crate::census::AccountState| {
        validations.set(validations.get() + 1);
        Ok(())
    };
    {
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &fresh,
            )
            .unwrap();
        io.conditional()
            .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints)
            .unwrap();
    }
    assert_eq!(validations.get(), 4);
    assert_eq!(saved.revision, revision + 10);
    assert_eq!(e.next_action(), NextAction::Complete);
    assert_eq!(
        saved.protocol_backlog.runs[0].batches[0].as_copy(),
        Some(&e)
    );
    assert!(io.flights.is_empty());
    saved.protocol_backlog.interrupted();
    store.save_state(&mut saved).unwrap();
    assert_eq!(
        saved.protocol_backlog.runs[0].status,
        crate::backlog::protocol::Status::Complete
    );
    drop(store);
    let reopened_store = repo.store(&account).unwrap();
    let reopened = reopened_store.load_state().unwrap();
    assert!(reopened.protocol_backlog == saved.protocol_backlog);
    assert_eq!(reopened.revision, saved.revision);
}

struct StopAfterSave<C> {
    inner: C,
    count: usize,
    at: usize,
    cancel: Cancellation,
}
impl<C: CopyCheckpoints> CopyCheckpoints for StopAfterSave<C> {
    fn validate_intent(&mut self, e: &CopyEvidence) -> Result<(), Error> {
        self.inner.validate_intent(e)
    }
    fn save(&mut self, e: &CopyEvidence) -> Result<(), Error> {
        self.inner.save(e)?;
        self.count += 1;
        if self.count == self.at {
            self.cancel.cancel();
        }
        Ok(())
    }
}

#[test]
fn reopened_uncertain_account_requires_read_only_reconcile_before_explicit_resume() {
    use crate::{
        acquisition::backlog::copy_checkpoints::{CopySlot, tests::fixture},
        backlog::protocol::Status,
    };
    for stop_at in [3, 6, 9] {
        let mut e = initial();
        let (_root, repo, account, store, mut saved) = fixture(e.clone());
        let source = repo.backlog_source.as_deref().unwrap();
        let permit = permitted(source);
        let mut io = complete_script(source, &e);
        let id = saved.protocol_backlog.runs[0].id.clone();
        let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
        let cancel = Cancellation::default();
        {
            let checkpoints = repo
                .copy_checkpoints(
                    &account,
                    &store,
                    &mut saved,
                    CopySlot::Filing { run: &id, batch: 0 },
                    &topology,
                    &|_| Ok(()),
                )
                .unwrap();
            let mut checkpoints = StopAfterSave {
                inner: checkpoints,
                count: 0,
                at: stop_at,
                cancel: cancel.clone(),
            };
            assert_eq!(
                io.conditional()
                    .file(&permit, &mut e, &cancel, &mut checkpoints),
                Err(Error::Cancelled)
            );
        }
        drop(store);
        let store = repo.store(&account).unwrap();
        let mut reopened = store.load_state().unwrap();
        assert_eq!(reopened.protocol_backlog.runs[0].status, Status::Uncertain);
        let mut evidence = reopened.protocol_backlog.runs[0].batches[0]
            .as_copy()
            .unwrap()
            .clone();
        assert!(
            repo.copy_checkpoints(
                &account,
                &store,
                &mut reopened,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(())
            )
            .is_err()
        );
        let effects = (
            count(&io, "copy"),
            count(&io, "store"),
            count(&io, "expunge"),
        );
        {
            let mut checkpoints = repo
                .copy_checkpoints(
                    &account,
                    &store,
                    &mut reopened,
                    CopySlot::Reconcile { run: &id, batch: 0 },
                    &topology,
                    &|_| Ok(()),
                )
                .unwrap();
            io.conditional()
                .reconcile(
                    &permit,
                    &mut evidence,
                    &Cancellation::default(),
                    &mut checkpoints,
                )
                .unwrap();
        }
        assert_eq!(
            (
                count(&io, "copy"),
                count(&io, "store"),
                count(&io, "expunge")
            ),
            effects
        );
        assert!(!evidence.next_action().needs_observation());
        if reopened.protocol_backlog.runs[0].status != Status::Complete {
            assert_eq!(reopened.protocol_backlog.runs[0].status, Status::Stopped);
            reopened.protocol_backlog.runs[0].status = Status::Running;
            reopened.protocol_backlog.runs[0].stop_requested = false;
            store.save_state(&mut reopened).unwrap();
            let mut checkpoints = repo
                .copy_checkpoints(
                    &account,
                    &store,
                    &mut reopened,
                    CopySlot::Filing { run: &id, batch: 0 },
                    &topology,
                    &|_| Ok(()),
                )
                .unwrap();
            io.conditional()
                .file(
                    &permit,
                    &mut evidence,
                    &Cancellation::default(),
                    &mut checkpoints,
                )
                .unwrap();
        }
        assert_eq!(evidence.next_action(), NextAction::Complete);
        assert_eq!(
            (
                count(&io, "copy"),
                count(&io, "store"),
                count(&io, "expunge")
            ),
            (1, 1, 1)
        );
        assert!(io.flights.is_empty());
    }
}

#[test]
fn changed_ledger_freshness_stops_the_next_byte_effect_and_preserves_durable_copies() {
    use crate::acquisition::backlog::copy_checkpoints::{CopySlot, tests::fixture};
    let mut e = initial();
    let (_root, repo, account, store, mut saved) = fixture(e.clone());
    let source = repo.backlog_source.as_deref().unwrap();
    let permit = permitted(source);
    let mut io = complete_script(source, &e);
    let id = saved.protocol_backlog.runs[0].id.clone();
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let validations = std::cell::Cell::new(0);
    let fresh = |_: &crate::census::AccountState| {
        validations.set(validations.get() + 1);
        if validations.get() >= 3 {
            Err("authoritative ledger changed".into())
        } else {
            Ok(())
        }
    };
    {
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &fresh,
            )
            .unwrap();
        assert_eq!(
            io.conditional()
                .file(&permit, &mut e, &Cancellation::default(), &mut checkpoints),
            Err(Error::Unstable)
        );
    }
    assert_eq!(count(&io, "copy"), 1);
    assert_eq!(count(&io, "store"), 0);
    assert_eq!(count(&io, "expunge"), 0);
    let reopened = store.load_state().unwrap();
    assert_eq!(
        reopened.protocol_backlog.runs[0].batches[0]
            .as_copy()
            .unwrap()
            .next_action(),
        NextAction::PrepareDeletion
    );
    let mut expected = saved.protocol_backlog.clone();
    expected.interrupted();
    assert!(reopened.protocol_backlog == expected);
}
