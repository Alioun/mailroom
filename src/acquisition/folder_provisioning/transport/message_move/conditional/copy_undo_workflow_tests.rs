use super::*;
use crate::acquisition::backlog::copy_undo::{CopyUndoReview, CopyUndoTarget};

impl CompensationTransport for ReviewPeer {
    fn inventory(&mut self) -> Result<Inventory, Error> {
        CompensationReadTransport::inventory(self)
    }

    fn execute(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.peer
            .drive(permit, evidence, cancel, checkpoints, Mode::Execute)
    }

    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.peer
            .drive(permit, evidence, cancel, checkpoints, Mode::Reconcile)
    }
}

#[test]
fn initial_preview_and_fresh_confirmation_schedule_the_exact_compensation_on_one_peer() {
    for stage in [Forward::Copied, Forward::Marked, Forward::Filed] {
        let (_root, mut repo, account, store, mut saved, request) = review_fixture(stage);
        let accesses = repo.track_credential_accesses();
        let mut peer = peer(&repo, &account, &saved, &request);
        let source = repo.backlog_source.as_ref().unwrap();
        let permit = source
            .permit(
                &account.id,
                Provider::MailboxOrg,
                &account.login,
                &source.authority,
            )
            .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let target = CopyUndoTarget::Initial { run: &request.run };
        let preview = repo
            .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .preview_with(&mut peer, &permit, &Cancellation::default())
            .unwrap();
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
        let CopyUndoReview::Initial(review) = &preview else {
            panic!("initial review required")
        };
        let evidence = &review.compensation()[0].evidence;
        let receipt = preview.receipt().to_owned();
        let mut flights = super::super::peer(&repo, &account, &saved, &request)
            .peer
            .io
            .flights;
        flights.extend(match stage {
            Forward::Copied => cleanup_flights(evidence, CleanupSlot::Initial(0)),
            Forward::Marked => clear_flights(evidence),
            Forward::Filed => restore_flights(evidence),
        });
        peer.peer.io.flights = flights;
        repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .confirm_with(&mut peer, &permit, &receipt, &Cancellation::default())
            .unwrap();
        assert!(peer.peer.io.flights.is_empty());
        let reopened = store.load_state().unwrap();
        let run = &reopened.protocol_backlog.runs[0];
        assert_eq!(run.undo_receipt.as_deref(), Some(receipt.as_str()));
        assert_eq!(
            run.status,
            if matches!(stage, Forward::Marked) {
                Status::Stopped
            } else {
                Status::Undone
            }
        );
        assert_eq!(
            peer.peer
                .io
                .events
                .borrow()
                .iter()
                .filter(|event| **event == "compensation-review")
                .count(),
            2
        );
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
                .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!repo.backlog_installed(Some(&account.id)));
    }
}

#[test]
fn dependent_preview_and_confirmation_keep_the_initial_undo_receipt_and_clear_history() {
    for partial in [false, true] {
        let (_root, repo, account, store, mut saved) = cleared_fixture(partial);
        let request = input(&saved);
        let original = saved.protocol_backlog.runs[0].clone();
        let mut peer = review_peer(&repo, &account, &saved, &request);
        let source = repo.backlog_source.as_ref().unwrap();
        let permit = source
            .permit(
                &account.id,
                Provider::MailboxOrg,
                &account.login,
                &source.authority,
            )
            .unwrap();
        let target = CopyUndoTarget::AfterClear {
            run: &request.run,
            forward: 0,
            clear: 0,
        };
        let before = std::fs::read(store.path()).unwrap();
        let preview = repo
            .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .preview_with(&mut peer, &permit, &Cancellation::default())
            .unwrap();
        let eligible = if partial { 1 } else { 2 };
        assert_eq!(preview.actions().remove_copy, eligible);
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        let CopyUndoReview::AfterClear(review) = &preview else {
            panic!("dependent review required")
        };
        let mut flights = review_peer(&repo, &account, &saved, &request)
            .peer
            .io
            .flights;
        flights.extend(cleanup_flights(
            review.evidence(),
            CleanupSlot::AfterClear { clear: 0, group: 0 },
        ));
        peer.peer.io.flights = flights;
        let result = repo
            .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .confirm_with(
                &mut peer,
                &permit,
                preview.receipt(),
                &Cancellation::default(),
            );
        assert_eq!(result.is_err(), partial);
        let reopened = store.load_state().unwrap();
        let run = &reopened.protocol_backlog.runs[0];
        assert_eq!(
            run.status,
            if partial {
                Status::Conflict
            } else {
                Status::Undone
            }
        );
        assert_eq!(run.compensation_counts().copies_removed, eligible);
        assert_eq!(run.undo_receipt, original.undo_receipt);
        assert_eq!(
            run.compensation[0].after_clear_approvals[0].receipt,
            preview.receipt()
        );
        assert_eq!(
            run.compensation[0].evidence.clear(0),
            original.compensation[0].evidence.clear(0)
        );
        assert!(!peer.peer.io.events.borrow().contains(&"clear"));
        assert!(!peer.peer.io.events.borrow().contains(&"copy"));
        assert!(peer.peer.io.flights.is_empty());
    }
}

#[test]
fn wrong_receipt_and_precancelled_confirmation_never_commit_or_schedule_effects() {
    for precancelled in [false, true] {
        let (_root, repo, account, store, mut saved, request) = review_fixture(Forward::Copied);
        let mut peer = peer(&repo, &account, &saved, &request);
        let source = repo.backlog_source.as_ref().unwrap();
        let permit = source
            .permit(
                &account.id,
                Provider::MailboxOrg,
                &account.login,
                &source.authority,
            )
            .unwrap();
        let cancel = Cancellation::default();
        if precancelled {
            cancel.cancel();
        }
        let before = std::fs::read(store.path()).unwrap();
        let result = repo
            .copy_undo_workflow(
                &account,
                &store,
                &mut saved,
                CopyUndoTarget::Initial { run: &request.run },
                &|_| Ok(()),
            )
            .unwrap()
            .confirm_with(&mut peer, &permit, &"a".repeat(64), &cancel);
        assert!(result.is_err());
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert!(saved.protocol_backlog.runs[0].undo_receipt.is_none());
        assert_eq!(
            peer.peer.io.events.borrow().len(),
            usize::from(!precancelled)
        );
    }
}

#[test]
fn confirmation_reobserves_and_rejects_changed_mail_or_account_without_effects() {
    for change_mail in [false, true] {
        let (_root, mut repo, account, store, mut saved, mut request) =
            review_fixture(Forward::Copied);
        let accesses = repo.track_credential_accesses();
        let mut peer = peer(&repo, &account, &saved, &request);
        let source = repo.backlog_source.as_ref().unwrap();
        let permit = source
            .permit(
                &account.id,
                Provider::MailboxOrg,
                &account.login,
                &source.authority,
            )
            .unwrap();
        let target = CopyUndoTarget::Initial { run: &request.run };
        let preview = repo
            .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .preview_with(&mut peer, &permit, &Cancellation::default())
            .unwrap();
        if change_mail {
            request.observations[0].source = changed(&request.observations[0].source, true);
        } else {
            store.save_state(&mut saved).unwrap();
        }
        peer.peer.io.flights = super::super::peer(&repo, &account, &saved, &request)
            .peer
            .io
            .flights;
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
                .unwrap()
                .confirm_with(
                    &mut peer,
                    &permit,
                    preview.receipt(),
                    &Cancellation::default(),
                )
                .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert!(saved.protocol_backlog.runs[0].undo_receipt.is_none());
        assert_eq!(
            &*peer.peer.io.events.borrow(),
            &["compensation-review", "compensation-review"]
        );
        assert!(peer.peer.io.flights.is_empty());
        assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn loss_after_confirmation_retains_intent_and_requires_explicit_read_only_reconciliation() {
    let (_root, repo, account, store, mut saved, request) = review_fixture(Forward::Copied);
    let mut peer = peer(&repo, &account, &saved, &request);
    let source = repo.backlog_source.as_ref().unwrap();
    let permit = source
        .permit(
            &account.id,
            Provider::MailboxOrg,
            &account.login,
            &source.authority,
        )
        .unwrap();
    let target = CopyUndoTarget::Initial { run: &request.run };
    let preview = repo
        .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
        .unwrap()
        .preview_with(&mut peer, &permit, &Cancellation::default())
        .unwrap();
    let CopyUndoReview::Initial(review) = &preview else {
        panic!("initial review required")
    };
    let mut flights = super::super::peer(&repo, &account, &saved, &request)
        .peer
        .io
        .flights;
    let mut effects = cleanup_flights(&review.compensation()[0].evidence, CleanupSlot::Initial(0));
    effects
        .iter_mut()
        .find(|flight| flight.event == "store")
        .unwrap()
        .response = Err(Error::Eof);
    flights.extend(effects);
    peer.peer.io.flights = flights;
    assert!(
        repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .confirm_with(
                &mut peer,
                &permit,
                preview.receipt(),
                &Cancellation::default()
            )
            .is_err()
    );
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Uncertain);
    assert_eq!(
        saved.protocol_backlog.runs[0].undo_receipt.as_deref(),
        Some(preview.receipt())
    );
    let before = std::fs::read(store.path()).unwrap();
    let events = peer.peer.io.events.borrow().clone();
    assert!(
        repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    saved = store.load_state().unwrap();
    repo.compensation_worker(
        &account,
        &store,
        &mut saved,
        &request.run,
        CopyCompensationWork::Reconcile,
        &|_| Ok(()),
    )
    .unwrap()
    .drive(&mut peer, &permit, &Cancellation::default())
    .unwrap();
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Conflict);
    assert_eq!(
        peer.peer
            .io
            .events
            .borrow()
            .iter()
            .filter(|event| **event == "store")
            .count(),
        1
    );
    assert!(!peer.peer.io.events.borrow().contains(&"expunge"));
    assert!(peer.peer.io.events.borrow().starts_with(&events));
}

#[test]
fn cancellation_after_committed_intent_does_not_send_or_erase_the_approved_action() {
    let (_root, repo, account, store, mut saved, request) = review_fixture(Forward::Copied);
    let mut peer = peer(&repo, &account, &saved, &request);
    let source = repo.backlog_source.as_ref().unwrap();
    let permit = source
        .permit(
            &account.id,
            Provider::MailboxOrg,
            &account.login,
            &source.authority,
        )
        .unwrap();
    let target = CopyUndoTarget::Initial { run: &request.run };
    let preview = repo
        .copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
        .unwrap()
        .preview_with(&mut peer, &permit, &Cancellation::default())
        .unwrap();
    let CopyUndoReview::Initial(review) = &preview else {
        panic!("initial review required")
    };
    let mut flights = super::super::peer(&repo, &account, &saved, &request)
        .peer
        .io
        .flights;
    flights.extend(cleanup_flights(
        &review.compensation()[0].evidence,
        CleanupSlot::Initial(0),
    ));
    peer.peer.io.flights = flights;
    peer.peer.stop_after = Some(1);
    let cancel = Cancellation::default();
    assert_eq!(
        repo.copy_undo_workflow(&account, &store, &mut saved, target, &|_| Ok(()))
            .unwrap()
            .confirm_with(&mut peer, &permit, preview.receipt(), &cancel),
        Err(crate::acquisition::AcquisitionError::Cancelled)
    );
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Uncertain);
    assert!(saved.protocol_backlog.runs[0].stop_requested);
    assert_eq!(
        saved.protocol_backlog.runs[0].undo_receipt.as_deref(),
        Some(preview.receipt())
    );
    assert_eq!(peer.peer.io.flights.front().unwrap().event, "store");
    assert!(!peer.peer.io.events.borrow().contains(&"store"));
    assert!(store.load_state().unwrap().protocol_backlog == saved.protocol_backlog);
}
