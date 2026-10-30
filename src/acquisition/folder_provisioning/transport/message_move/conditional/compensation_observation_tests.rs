use super::*;
use crate::acquisition::backlog::compensation_observer::CompensationReadTransport;
use std::cell::Cell;

#[path = "dependent_observation_tests.rs"]
mod dependent;

struct ReviewPeer {
    peer: Peer,
    inventories: usize,
    change_after_read: bool,
}

impl CompensationReadTransport for ReviewPeer {
    fn inventory(&mut self) -> Result<Inventory, Error> {
        self.inventories += 1;
        let mut topology = self.peer.topology.clone();
        if self.change_after_read && self.inventories > 1 {
            topology.capabilities.insert("MOVE".into());
        }
        Ok(topology)
    }

    fn observe(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Error> {
        Conditional {
            io: &mut self.peer.io,
            capabilities: &capabilities(),
        }
        .observe_for_compensation(permit, forward)
    }
}

fn peer(
    repo: &Accounts,
    account: &Account,
    saved: &AccountState,
    request: &CopyCompensationRequest,
) -> ReviewPeer {
    let observation = &request.observations[0];
    let forward = saved.protocol_backlog.runs[0].batches[0].as_copy().unwrap();
    let flight = reads(
        "compensation-review",
        &[
            (
                &observation.source,
                forward.intent().messages().keys().copied().collect(),
            ),
            (
                &observation.destination,
                forward
                    .copy()
                    .mapping()
                    .unwrap()
                    .pairs()
                    .values()
                    .copied()
                    .collect(),
            ),
        ],
    );
    ReviewPeer {
        peer: Peer::new(repo, account, saved, [flight].into()),
        inventories: 0,
        change_after_read: false,
    }
}

#[test]
fn protocol_observations_produce_the_exact_local_review_and_confirm_only_intent() {
    for stage in [Forward::Copied, Forward::Marked, Forward::Filed] {
        let (_root, mut repo, account, store, mut saved, request) = review_fixture(stage);
        let expected = repo
            .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
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
        let observed = repo
            .observe_copy_compensation(
                &account,
                &store,
                &saved,
                &request.run,
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| Ok(()),
            )
            .unwrap();
        assert_eq!(observed.review().receipt(), expected.receipt());
        assert_eq!(observed.review().actions(), expected.actions());
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert!(peer.peer.io.flights.is_empty());
        assert_eq!(peer.inventories, 2);
        assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
        observed
            .confirm(
                &repo,
                &account,
                &store,
                &mut saved,
                expected.receipt(),
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            saved.protocol_backlog.runs[0].undo_receipt.as_deref(),
            Some(expected.receipt())
        );
        assert_eq!(saved.protocol_backlog.runs[0].status, Status::Stopped);
        assert!(store.load_state().unwrap().protocol_backlog == saved.protocol_backlog);
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            repo.confirm_copy_compensation(
                &account,
                &store,
                &mut saved,
                &request,
                expected.receipt(),
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!repo.backlog_installed(Some(&account.id)));
    }
}

#[test]
fn observed_review_and_exact_confirmation_feed_checkpointed_cleanup_without_recopying() {
    let (_root, mut repo, account, store, mut saved, request) = review_fixture(Forward::Copied);
    let forward = saved.protocol_backlog.runs[0].batches.clone();
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
    let observed = repo
        .observe_copy_compensation(
            &account,
            &store,
            &saved,
            &request.run,
            &mut peer,
            &permit,
            &Cancellation::default(),
            &|_| Ok(()),
        )
        .unwrap();
    let receipt = observed.review().receipt().to_owned();
    observed
        .confirm(&repo, &account, &store, &mut saved, &receipt, |_| Ok(()))
        .unwrap();
    assert_eq!(saved.protocol_backlog.runs[0].status, Status::Stopped);
    assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
    peer.peer.io.flights = cleanup_flights(
        &saved.protocol_backlog.runs[0].compensation[0].evidence,
        CleanupSlot::Initial(0),
    );
    drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer.peer,
        CopyCompensationWork::Execute,
    )
    .unwrap();
    assert!(peer.peer.io.flights.is_empty());
    let reopened = store.load_state().unwrap();
    let run = &reopened.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Undone);
    assert_eq!(run.compensation_counts().copies_removed, 2);
    assert_eq!(run.compensation_counts().restored, 0);
    assert!(run.batches == forward);
    assert!(!peer.peer.io.events.borrow().contains(&"copy"));
    assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn stale_authority_unsaved_stop_and_cancellation_reject_before_protocol_reads() {
    for mutation in ["freshness", "stop", "source", "cancel", "topology"] {
        let (_root, mut repo, account, store, mut saved, request) = review_fixture(Forward::Copied);
        let mut peer = peer(&repo, &account, &saved, &request);
        let cancel = Cancellation::default();
        match mutation {
            "stop" => saved.protocol_backlog.runs[0].stop_requested = false,
            "source" => std::sync::Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                .epoch
                .push('x'),
            "cancel" => cancel.cancel(),
            "topology" => {
                peer.peer.topology.capabilities.insert("MOVE".into());
            }
            _ => (),
        }
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
        assert!(
            repo.observe_copy_compensation(
                &account,
                &store,
                &saved,
                &request.run,
                &mut peer,
                &permit,
                &cancel,
                &|_| if mutation == "freshness" {
                    Err("stale".into())
                } else {
                    Ok(())
                },
            )
            .is_err(),
            "{mutation}"
        );
        assert!(peer.peer.io.events.borrow().is_empty());
        assert_eq!(peer.inventories, usize::from(mutation == "topology"));
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    }
}

#[test]
fn changed_topology_failed_readback_or_late_freshness_never_produce_a_review() {
    for mutation in ["topology", "readback", "freshness"] {
        let (_root, repo, account, store, saved, request) = review_fixture(Forward::Copied);
        let mut peer = peer(&repo, &account, &saved, &request);
        match mutation {
            "topology" => peer.change_after_read = true,
            "readback" => peer.peer.io.flights[0].response = Err(Error::Eof),
            _ => (),
        }
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
        let checks = Cell::new(0);
        assert!(
            repo.observe_copy_compensation(
                &account,
                &store,
                &saved,
                &request.run,
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| {
                    checks.set(checks.get() + 1);
                    if mutation == "freshness" && checks.get() > 1 {
                        Err("stale".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    }
}

#[test]
fn changed_retained_original_invalidates_the_old_receipt_without_removing_copies() {
    let (_root, repo, account, store, mut saved, mut request) = review_fixture(Forward::Copied);
    let expected = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    request.observations[0].source = changed(&request.observations[0].source, true);
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
    let observed = repo
        .observe_copy_compensation(
            &account,
            &store,
            &saved,
            &request.run,
            &mut peer,
            &permit,
            &Cancellation::default(),
            &|_| Ok(()),
        )
        .unwrap();
    assert_ne!(observed.review().receipt(), expected.receipt());
    assert_eq!(observed.review().actions().remove_copy, 0);
    assert_eq!(observed.review().actions().conflicts, 2);
    assert!(
        observed
            .confirm(
                &repo,
                &account,
                &store,
                &mut saved,
                expected.receipt(),
                |_| Ok(())
            )
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
}

#[test]
fn an_observed_review_cannot_be_rebound_to_a_newer_account_revision() {
    for use_new_receipt in [false, true] {
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
        let observed = repo
            .observe_copy_compensation(
                &account,
                &store,
                &saved,
                &request.run,
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| Ok(()),
            )
            .unwrap();
        let receipt = observed.review().receipt().to_owned();
        store.save_state(&mut saved).unwrap();
        let new_review = repo
            .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        assert_ne!(new_review.receipt(), receipt);
        let before = std::fs::read(store.path()).unwrap();
        let selected = if use_new_receipt {
            new_review.receipt()
        } else {
            &receipt
        };
        assert!(
            observed
                .confirm(&repo, &account, &store, &mut saved, selected, |_| Ok(()))
                .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert!(saved.protocol_backlog.runs[0].undo_receipt.is_none());
    }
}

#[test]
fn pending_forward_observations_reject_before_any_review_read() {
    let (mut forward, _) = prepared();
    forward = CopyEvidence::new(forward.intent().clone(), forward.before().clone()).unwrap();
    forward.begin_copy(&forward.before().clone()).unwrap();
    let (_root, repo, account, store, mut saved) =
        crate::acquisition::backlog::copy_checkpoints::tests::fixture(forward);
    saved.protocol_backlog.runs[0].status = Status::Uncertain;
    saved.protocol_backlog.runs[0].stop_requested = true;
    store.save_state(&mut saved).unwrap();
    let id = saved.protocol_backlog.runs[0].id.clone();
    let mut peer = ReviewPeer {
        peer: Peer::new(&repo, &account, &saved, VecDeque::new()),
        inventories: 0,
        change_after_read: false,
    };
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
    assert!(
        repo.observe_copy_compensation(
            &account,
            &store,
            &saved,
            &id,
            &mut peer,
            &permit,
            &Cancellation::default(),
            &|_| Ok(()),
        )
        .is_err()
    );
    assert_eq!(peer.inventories, 0);
    assert!(peer.peer.io.events.borrow().is_empty());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}
