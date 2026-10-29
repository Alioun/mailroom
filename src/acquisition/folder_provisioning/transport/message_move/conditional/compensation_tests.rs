use super::super::super::compensation::Mode;
use super::*;
use crate::{
    acquisition::{
        Account, Accounts, CopyCompensationObservation, CopyCompensationRequest,
        CopyCompensationWork, DependentCleanupRequest,
        backlog::compensation_worker::CompensationTransport,
    },
    backlog::{
        imap_copy::{CleanupSlot, CompensationEvidence},
        protocol::Status,
    },
    census::{AccountState, EncryptedStore, folder_provisioning::Inventory},
};

#[derive(Clone, Copy)]
enum Forward {
    Copied,
    Marked,
    Filed,
}

fn changed(before: &Readback, deleted: bool) -> Readback {
    let highest = ModSeq::new(before.highest_modseq().get() + 10).unwrap();
    let messages = before
        .messages()
        .iter()
        .map(|(uid, message)| {
            let mut message = message.clone();
            message.modseq = highest;
            if deleted {
                message.metadata.flags.insert("\\Deleted".into());
            } else {
                message.metadata.flags.remove("\\Deleted");
            }
            (*uid, message)
        })
        .collect();
    Readback::new(before.identity().clone(), highest, messages).unwrap()
}
fn empty(before: &Readback) -> Readback {
    Readback::new(
        before.identity().clone(),
        ModSeq::new(before.highest_modseq().get() + 10).unwrap(),
        BTreeMap::new(),
    )
    .unwrap()
}
fn store_result(before: &Readback) -> StoreResult {
    StoreResult::Tagged {
        status: TaggedStatus::Ok,
        modified: None,
        updated: before
            .messages()
            .keys()
            .map(|uid| {
                (
                    *uid,
                    ModSeq::new(before.highest_modseq().get() + 10).unwrap(),
                )
            })
            .collect(),
    }
}
fn fixture(
    stage: Forward,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
) {
    let (mut forward, delete) = prepared();
    let mut source = forward.before().clone();
    let destination = delete.destination().clone();
    if !matches!(stage, Forward::Copied) {
        let index = forward.begin_delete(delete.clone()).unwrap();
        forward
            .record_store_result(index, store_result(&source))
            .unwrap();
        source = changed(&source, true);
        forward.observe_store(index, source.clone()).unwrap();
        if matches!(stage, Forward::Filed) {
            forward
                .begin_expunge(index, source.clone(), destination.clone())
                .unwrap();
            forward
                .record_expunge_completion(
                    index,
                    Completion::Tagged {
                        status: TaggedStatus::Ok,
                        code: None,
                    },
                )
                .unwrap();
            source = empty(&source);
            forward
                .observe_final(index, source.clone(), destination.clone())
                .unwrap();
        }
    }
    let (root, repo, account, store, mut saved) =
        crate::acquisition::backlog::copy_checkpoints::tests::fixture(forward);
    let run = &mut saved.protocol_backlog.runs[0];
    run.status = crate::backlog::protocol::outcome(run, false);
    run.stop_requested = true;
    store.save_state(&mut saved).unwrap();
    let request = CopyCompensationRequest {
        run: saved.protocol_backlog.runs[0].id.clone(),
        topology: saved.protocol_backlog.runs[0].plan.topology.clone(),
        observations: vec![CopyCompensationObservation {
            forward: 0,
            source,
            destination,
        }],
    };
    let review = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    repo.confirm_copy_compensation(
        &account,
        &store,
        &mut saved,
        &request,
        review.receipt(),
        |_| Ok(()),
    )
    .unwrap();
    (root, repo, account, store, saved)
}

fn reads(event: &'static str, requests: &[(&Readback, BTreeSet<Uid>)]) -> Flight {
    let mut commands = vec![];
    let mut response = vec![];
    for (observed, requested) in requests {
        commands.push(format!(
            "SELECT \"{}\" (CONDSTORE)",
            observed.identity().name.wire
        ));
        response.extend(selection(
            observed.identity(),
            observed.highest_modseq().get(),
            commands.len(),
        ));
        for set in crate::acquisition::imap::uid_sets(requested.iter().map(|u| u.get())) {
            commands.push(format!("UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)"));
            for (uid, message) in observed.messages() {
                if !requested.contains(uid) {
                    continue;
                }
                let header = "Message-ID: <conditional@example.test>\r\n\r\n";
                let flags = message
                    .metadata
                    .flags
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                response.extend(format!("* 1 FETCH (UID {} FLAGS ({flags}) INTERNALDATE \"{}\" RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header} MODSEQ ({}))\r\n", uid.get(), message.metadata.internal_date, message.metadata.size, header.len(), message.modseq.get()).as_bytes());
            }
            response.extend(format!("F{} OK fetched\r\n", commands.len()).as_bytes());
        }
    }
    Flight {
        event,
        commands,
        response: Ok(response),
    }
}
fn members(before: &Readback) -> BTreeSet<Uid> {
    before.messages().keys().copied().collect()
}
fn selected_flight(before: &Readback) -> Flight {
    Flight {
        event: "select",
        commands: vec![format!(
            "SELECT \"{}\" (CONDSTORE)",
            before.identity().name.wire
        )],
        response: Ok(selection(
            before.identity(),
            before.highest_modseq().get(),
            1,
        )),
    }
}
fn ack(before: &Readback) -> Vec<u8> {
    let mut bytes = vec![];
    for uid in before.messages().keys() {
        bytes.extend(
            format!(
                "* 1 FETCH (UID {} MODSEQ ({}))\r\n",
                uid.get(),
                before.highest_modseq().get() + 10
            )
            .as_bytes(),
        );
    }
    bytes.extend_from_slice(b"F1 OK stored\r\n");
    bytes
}
fn cleanup_flights(evidence: &CompensationEvidence, slot: CleanupSlot) -> VecDeque<Flight> {
    let plan = evidence.cleanup_plan(slot).unwrap();
    let retained = plan.retained();
    let before = plan.deletion().before();
    let after = changed(before, true);
    [
        reads(
            "retain-before-store",
            &[(retained, members(retained)), (before, members(before))],
        ),
        selected_flight(before),
        Flight {
            event: "store",
            commands: vec![plan.deletion().command()],
            response: Ok(ack(before)),
        },
        reads("observe-store", &[(&after, members(before))]),
        reads(
            "retain-before-expunge",
            &[(retained, members(retained)), (&after, members(before))],
        ),
        selected_flight(&after),
        Flight {
            event: "expunge",
            commands: vec![format!(
                "UID EXPUNGE {}",
                members(before)
                    .iter()
                    .map(|u| u.get().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )],
            response: Ok(b"F1 OK removed\r\n".to_vec()),
        },
        reads(
            "observe-expunge",
            &[
                (retained, members(retained)),
                (&empty(&after), members(before)),
            ],
        ),
    ]
    .into()
}
fn clear_flights(evidence: &CompensationEvidence) -> VecDeque<Flight> {
    let intent = evidence.preview().clear()[0].clear();
    let before = intent.before();
    [
        reads("clear-before", &[(before, members(before))]),
        selected_flight(before),
        Flight {
            event: "clear",
            commands: vec![intent.command()],
            response: Ok(ack(before)),
        },
        reads(
            "clear-observed",
            &[(&changed(before, false), members(before))],
        ),
    ]
    .into()
}

fn restore_flights(evidence: &CompensationEvidence) -> VecDeque<Flight> {
    let restore = evidence.restore().unwrap();
    let before = restore.before();
    let target = evidence.preview().source().clone();
    let destination = Readback::new(
        target.identity().clone(),
        ModSeq::new(target.highest_modseq().get() + 10).unwrap(),
        before
            .messages()
            .values()
            .cloned()
            .enumerate()
            .map(|(i, mut m)| {
                m.modseq = ModSeq::new(target.highest_modseq().get() + 10).unwrap();
                (uid(if i == 0 { 80 } else { 84 }), m)
            })
            .collect(),
    )
    .unwrap();
    let mut copy = restore.clone();
    copy.begin_copy(before).unwrap();
    copy.record_mapping(CopyUidMapping::parse("[COPYUID 101 60,64 80,84]", copy.intent()).unwrap())
        .unwrap();
    copy.record_copy_completion(Completion::Tagged {
        status: TaggedStatus::Ok,
        code: Some("[COPYUID 101 60,64 80,84]".into()),
    })
    .unwrap();
    copy.observe_copy(before.clone(), destination.clone())
        .unwrap();
    let prepared = copy
        .prepare_deletions(before, &destination)
        .unwrap()
        .remove(0);
    let after = changed(before, true);
    [
        reads(
            "restore-before-copy",
            &[(&target, BTreeSet::new()), (before, members(before))],
        ),
        selected_flight(before),
        Flight {
            event: "copy",
            commands: vec!["UID COPY 60,64 \"INBOX\"".into()],
            response: Ok(b"F1 OK [COPYUID 101 60,64 80,84] restored\r\n".to_vec()),
        },
        reads(
            "restore-observe-copy",
            &[
                (before, members(before)),
                (&destination, members(&destination)),
            ],
        ),
        reads(
            "restore-prepare-store",
            &[
                (before, members(before)),
                (&destination, members(&destination)),
            ],
        ),
        reads(
            "restore-before-store",
            &[
                (before, members(before)),
                (&destination, members(&destination)),
            ],
        ),
        selected_flight(before),
        Flight {
            event: "store",
            commands: vec![prepared.intent().command()],
            response: Ok(ack(before)),
        },
        reads("restore-observe-store", &[(&after, members(before))]),
        reads(
            "restore-before-expunge",
            &[
                (&after, members(before)),
                (&destination, members(&destination)),
            ],
        ),
        selected_flight(&after),
        Flight {
            event: "expunge",
            commands: vec!["UID EXPUNGE 60,64".into()],
            response: Ok(b"F1 OK removed\r\n".to_vec()),
        },
        reads(
            "restore-observe-expunge",
            &[
                (&empty(&after), members(before)),
                (&destination, members(&destination)),
            ],
        ),
    ]
    .into()
}

struct Peer {
    io: Script,
    topology: Inventory,
    stop_after: Option<usize>,
    saves: usize,
}
struct Checkpoints<'a, C> {
    inner: &'a mut C,
    saves: &'a mut usize,
    stop_after: Option<usize>,
    cancel: &'a Cancellation,
    events: Rc<RefCell<Vec<&'static str>>>,
}
impl<C: CompensationCheckpoints> CompensationCheckpoints for Checkpoints<'_, C> {
    type Intent = C::Intent;
    fn persist_intent(&mut self, evidence: &CompensationEvidence) -> Result<Self::Intent, Error> {
        let token = self.inner.persist_intent(evidence)?;
        self.events.borrow_mut().push("intent-saved");
        self.saved();
        Ok(token)
    }
    fn consume_intent(&mut self, token: Self::Intent, source: &str) -> Result<(), Error> {
        self.inner.consume_intent(token, source)?;
        self.events.borrow_mut().push("intent-consumed");
        Ok(())
    }
    fn observe(&mut self, evidence: &CompensationEvidence) -> Result<(), Error> {
        self.inner.observe(evidence)?;
        self.events.borrow_mut().push("observation-saved");
        self.saved();
        Ok(())
    }
}
impl<C> Checkpoints<'_, C> {
    fn saved(&mut self) {
        *self.saves += 1;
        if self.stop_after == Some(*self.saves) {
            self.cancel.cancel();
        }
    }
}
impl Peer {
    fn new(
        repo: &Accounts,
        _account: &Account,
        saved: &AccountState,
        flights: VecDeque<Flight>,
    ) -> Self {
        Self {
            io: Script {
                source: repo.backlog_source.as_ref().unwrap().identity(),
                flights,
                events: Rc::new(RefCell::new(vec![])),
            },
            topology: saved.protocol_backlog.runs[0].plan.topology.clone(),
            stop_after: None,
            saves: 0,
        }
    }
    fn drive(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
        mode: Mode,
    ) -> Result<(), Error> {
        let mut checkpoints = Checkpoints {
            inner: checkpoints,
            saves: &mut self.saves,
            stop_after: self.stop_after,
            cancel,
            events: self.io.events.clone(),
        };
        Conditional {
            io: &mut self.io,
            capabilities: &capabilities(),
        }
        .compensate(permit, evidence, cancel, &mut checkpoints, mode)
    }
}
impl CompensationTransport for Peer {
    fn inventory(&mut self) -> Result<Inventory, Error> {
        Ok(self.topology.clone())
    }
    fn execute(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.drive(permit, evidence, cancel, checkpoints, Mode::Execute)
    }
    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.drive(permit, evidence, cancel, checkpoints, Mode::Reconcile)
    }
}
fn drive(
    repo: &Accounts,
    account: &Account,
    store: &EncryptedStore,
    saved: &mut AccountState,
    peer: &mut Peer,
    work: CopyCompensationWork,
) -> Result<(), crate::acquisition::AcquisitionError> {
    let id = saved.protocol_backlog.runs[0].id.clone();
    let source = repo.backlog_source.as_ref().unwrap();
    let permit = source
        .permit(
            &account.id,
            Provider::MailboxOrg,
            &account.login,
            &source.authority,
        )
        .unwrap();
    repo.compensation_worker(account, store, saved, &id, work, &|_| Ok(()))?
        .drive(peer, &permit, &Cancellation::default())
}

#[test]
fn extra_copy_cleanup_uses_saved_store_and_expunge_intents_and_keeps_the_original() {
    let (_root, mut repo, account, store, mut saved) = fixture(Forward::Copied);
    let accesses = repo.track_credential_accesses();
    let forward = saved.protocol_backlog.runs[0].batches.clone();
    let flights = cleanup_flights(
        &saved.protocol_backlog.runs[0].compensation[0].evidence,
        CleanupSlot::Initial(0),
    );
    let mut peer = Peer::new(&repo, &account, &saved, flights);
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert!(peer.io.flights.is_empty());
    let reopened = store.load_state().unwrap();
    let run = &reopened.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Undone);
    assert_eq!(run.compensation_counts().copies_removed, 2);
    assert_eq!(run.compensation_counts().restored, 0);
    assert!(run.batches == forward);
    let events = peer.io.events.borrow();
    for (index, event) in events.iter().enumerate() {
        if matches!(*event, "store" | "expunge") {
            assert_eq!(
                &events[index - 2..index],
                &["intent-saved", "intent-consumed"]
            );
        }
    }
    assert_eq!(peer.saves, 6);
    assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!repo.backlog_installed(Some(&account.id)));
}

#[test]
fn owned_flag_clear_stops_for_separate_dependency_approval_before_cleanup() {
    let (_root, repo, account, store, mut saved) = fixture(Forward::Marked);
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    let original = evidence.preview().clear()[0].clear().before().clone();
    let destination = evidence.preview().destination().clone();
    let mut peer = Peer::new(&repo, &account, &saved, clear_flights(evidence));
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert!(peer.io.flights.is_empty());
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Stopped);
    assert_eq!(
        saved.protocol_backlog.runs[0]
            .compensation_counts()
            .deleted_cleared,
        2
    );
    assert!(
        !saved.protocol_backlog.runs[0].compensation[0]
            .evidence
            .has_dependent_cleanup()
    );
    let request = DependentCleanupRequest {
        run: saved.protocol_backlog.runs[0].id.clone(),
        forward: 0,
        clear: 0,
        topology: peer.topology.clone(),
        source: changed(&original, false),
        destination,
    };
    let review = repo
        .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    repo.confirm_dependent_cleanup(
        &account,
        &store,
        &mut saved,
        &request,
        review.receipt(),
        |_| Ok(()),
    )
    .unwrap();
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    peer.io.flights = cleanup_flights(evidence, CleanupSlot::AfterClear { clear: 0, group: 0 });
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].status,
        Status::Undone
    );
    assert!(peer.io.flights.is_empty());
}

#[test]
fn cancellation_after_acknowledgment_reconciles_read_only_then_resumes_only_expunge() {
    let (_root, repo, account, store, mut saved) = fixture(Forward::Copied);
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    let mut peer = Peer::new(
        &repo,
        &account,
        &saved,
        cleanup_flights(evidence, CleanupSlot::Initial(0)),
    );
    peer.stop_after = Some(2);
    assert!(
        drive(
            &repo,
            &account,
            &store,
            &mut saved,
            &mut peer,
            CopyCompensationWork::Execute
        )
        .is_err()
    );
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Uncertain);
    saved = store.load_state().unwrap();
    peer.stop_after = None;
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Reconcile,
    )
    .unwrap();
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Stopped);
    assert_eq!(
        peer.io.flights.front().unwrap().event,
        "retain-before-expunge"
    );
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert!(peer.io.flights.is_empty());
    let events = peer.io.events.borrow();
    assert_eq!(events.iter().filter(|e| **e == "store").count(), 1);
    assert_eq!(events.iter().filter(|e| **e == "expunge").count(), 1);
}

#[test]
fn cancellation_after_saved_intent_never_sends_or_retries_the_effect() {
    let (_root, repo, account, store, mut saved) = fixture(Forward::Copied);
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    let before = evidence
        .cleanup_plan(CleanupSlot::Initial(0))
        .unwrap()
        .deletion()
        .before()
        .clone();
    let mut peer = Peer::new(
        &repo,
        &account,
        &saved,
        cleanup_flights(evidence, CleanupSlot::Initial(0)),
    );
    peer.stop_after = Some(1);
    assert!(
        drive(
            &repo,
            &account,
            &store,
            &mut saved,
            &mut peer,
            CopyCompensationWork::Execute
        )
        .is_err()
    );
    assert_eq!(peer.io.flights.front().unwrap().event, "store");
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Uncertain);
    assert!(
        drive(
            &repo,
            &account,
            &store,
            &mut saved,
            &mut peer,
            CopyCompensationWork::Execute
        )
        .is_err()
    );
    peer.stop_after = None;
    peer.io.flights = [reads("reconcile", &[(&before, members(&before))])].into();
    saved = store.load_state().unwrap();
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Reconcile,
    )
    .unwrap();
    assert!(peer.io.flights.is_empty());
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Conflict);
    assert!(!peer.io.events.borrow().contains(&"store"));
}

#[test]
fn reverse_filing_reuses_copy_mapping_and_conditional_deletion_with_exact_reopening() {
    let (_root, repo, account, store, mut saved) = fixture(Forward::Filed);
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    let mut peer = Peer::new(&repo, &account, &saved, restore_flights(evidence));
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert!(peer.io.flights.is_empty());
    let reopened = store.load_state().unwrap();
    let run = &reopened.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Undone);
    assert_eq!(run.compensation_counts().restored, 2);
    assert_eq!(run.compensation_counts().copies_removed, 0);
    assert_eq!(peer.saves, 10);
    assert_eq!(
        run.compensation[0].evidence,
        saved.protocol_backlog.runs[0].compensation[0].evidence
    );
}

#[test]
fn missing_acknowledgments_require_read_only_reconciliation_and_never_repeat_effects() {
    for effect in ["store", "expunge"] {
        let (_root, repo, account, store, mut saved) = fixture(Forward::Copied);
        let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
        let mut flights = cleanup_flights(evidence, CleanupSlot::Initial(0));
        flights
            .iter_mut()
            .find(|f| f.event == effect)
            .unwrap()
            .response = Err(Error::Eof);
        let mut peer = Peer::new(&repo, &account, &saved, flights);
        assert!(
            drive(
                &repo,
                &account,
                &store,
                &mut saved,
                &mut peer,
                CopyCompensationWork::Execute
            )
            .is_err()
        );
        assert_eq!(saved.protocol_backlog.runs[0].status, Status::Uncertain);
        saved = store.load_state().unwrap();
        drive(
            &repo,
            &account,
            &store,
            &mut saved,
            &mut peer,
            CopyCompensationWork::Reconcile,
        )
        .unwrap();
        if effect == "store" {
            assert_eq!(saved.protocol_backlog.runs[0].status, Status::Conflict);
            assert_eq!(
                peer.io.flights.front().unwrap().event,
                "retain-before-expunge"
            );
            assert!(!peer.io.events.borrow().contains(&"expunge"));
        } else {
            assert_eq!(saved.protocol_backlog.runs[0].status, Status::Undone);
            assert!(peer.io.flights.is_empty());
        }
        assert_eq!(
            peer.io
                .events
                .borrow()
                .iter()
                .filter(|e| **e == effect)
                .count(),
            1
        );
    }
}

#[test]
fn changed_retained_message_or_topology_never_reaches_cleanup_effects() {
    for mutation in ["original", "topology", "source", "unsaved-status"] {
        let (_root, mut repo, account, store, mut saved) = fixture(Forward::Copied);
        let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
        let mut flights = cleanup_flights(evidence, CleanupSlot::Initial(0));
        if mutation == "original" {
            let retained = evidence
                .cleanup_plan(CleanupSlot::Initial(0))
                .unwrap()
                .retained();
            let destination = evidence
                .cleanup_plan(CleanupSlot::Initial(0))
                .unwrap()
                .deletion()
                .before();
            flights[0] = reads(
                "changed-original",
                &[
                    (&changed(retained, true), members(retained)),
                    (destination, members(destination)),
                ],
            );
        }
        let mut peer = Peer::new(&repo, &account, &saved, flights);
        match mutation {
            "topology" => {
                peer.topology.capabilities.insert("MOVE".into());
            }
            "source" => {
                std::sync::Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                    .epoch
                    .push('x');
            }
            "unsaved-status" => {
                saved.protocol_backlog.runs[0].status = Status::UndoRunning;
                saved.protocol_backlog.runs[0].stop_requested = false;
            }
            _ => (),
        }
        assert!(
            drive(
                &repo,
                &account,
                &store,
                &mut saved,
                &mut peer,
                CopyCompensationWork::Execute
            )
            .is_err(),
            "{mutation}"
        );
        assert!(!peer.io.events.borrow().contains(&"store"));
        assert!(!peer.io.events.borrow().contains(&"expunge"));
    }
}
