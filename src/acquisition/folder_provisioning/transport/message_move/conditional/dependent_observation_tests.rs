use super::*;
use crate::acquisition::backlog::dependent_cleanup::DependentCleanupTarget;

#[path = "copy_undo_workflow_tests.rs"]
mod workflow;

fn target(request: &DependentCleanupRequest) -> DependentCleanupTarget<'_> {
    DependentCleanupTarget {
        run: &request.run,
        forward: request.forward,
        clear: request.clear,
    }
}

fn input(saved: &AccountState) -> DependentCleanupRequest {
    let run = &saved.protocol_backlog.runs[0];
    let evidence = &run.compensation[0].evidence;
    DependentCleanupRequest {
        run: run.id.clone(),
        forward: 0,
        clear: 0,
        topology: run.plan.topology.clone(),
        source: evidence
            .clear(0)
            .and_then(|clear| clear.readback())
            .unwrap_or(evidence.preview().source())
            .clone(),
        destination: evidence.preview().destination().clone(),
    }
}

fn review_peer(
    repo: &Accounts,
    account: &Account,
    saved: &AccountState,
    request: &DependentCleanupRequest,
) -> ReviewPeer {
    peer(
        repo,
        account,
        saved,
        &CopyCompensationRequest {
            run: request.run.clone(),
            topology: request.topology.clone(),
            observations: vec![CopyCompensationObservation {
                forward: request.forward,
                source: request.source.clone(),
                destination: request.destination.clone(),
            }],
        },
    )
}

fn cleared_fixture(
    partial: bool,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
) {
    let (root, repo, account, store, mut saved) = fixture(Forward::Marked);
    let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
    let mut flights = clear_flights(evidence);
    if partial {
        let before = evidence.preview().clear()[0].clear().before();
        flights
            .iter_mut()
            .find(|flight| flight.event == "clear")
            .unwrap()
            .response = Ok(format!(
            "* 1 FETCH (UID 4 MODSEQ ({}))\r\nF1 OK stored\r\n",
            before.highest_modseq().get() + 10
        )
        .into_bytes());
    }
    let mut peer = Peer::new(&repo, &account, &saved, flights);
    let result = drive(
        &repo,
        &account,
        &store,
        &mut saved,
        &mut peer,
        CopyCompensationWork::Execute,
    );
    assert_eq!(result.is_err(), partial);
    assert!(peer.io.flights.is_empty());
    assert_eq!(
        saved.protocol_backlog.runs[0].status,
        if partial {
            Status::Conflict
        } else {
            Status::Stopped
        }
    );
    (root, repo, account, store, saved)
}

#[test]
fn verified_clear_requires_fresh_separate_approval_before_checkpointed_copy_removal() {
    for partial in [false, true] {
        let (_root, mut repo, account, store, mut saved) = cleared_fixture(partial);
        let request = input(&saved);
        let original_undo = saved.protocol_backlog.runs[0].undo_receipt.clone();
        let forward = saved.protocol_backlog.runs[0].batches.clone();
        let expected = repo
            .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        let accesses = repo.track_credential_accesses();
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
        let before = std::fs::read(store.path()).unwrap();
        let observed = repo
            .observe_dependent_cleanup(
                &account,
                &store,
                &saved,
                target(&request),
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| Ok(()),
            )
            .unwrap();
        let eligible = if partial { 1 } else { 2 };
        assert_eq!(observed.review().receipt(), expected.receipt());
        assert_ne!(Some(observed.review().receipt()), original_undo.as_deref());
        assert_eq!(observed.review().actions().remove_copy, eligible);
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
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
        assert_eq!(saved.protocol_backlog.runs[0].undo_receipt, original_undo);
        assert!(saved.protocol_backlog.runs[0].stop_requested);
        assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
        let evidence = &saved.protocol_backlog.runs[0].compensation[0].evidence;
        peer.peer.io.flights =
            cleanup_flights(evidence, CleanupSlot::AfterClear { clear: 0, group: 0 });
        let result = drive(
            &repo,
            &account,
            &store,
            &mut saved,
            &mut peer.peer,
            CopyCompensationWork::Execute,
        );
        assert_eq!(result.is_err(), partial);
        assert!(peer.peer.io.flights.is_empty());
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
        assert!(run.batches == forward);
        assert_eq!(run.undo_receipt, original_undo);
        assert!(!peer.peer.io.events.borrow().contains(&"clear"));
        assert!(!peer.peer.io.events.borrow().contains(&"copy"));
        assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!repo.backlog_installed(Some(&account.id)));
    }
}

#[test]
fn unavailable_clear_foreign_target_stale_state_and_cancel_reject_before_reads() {
    for mutation in [
        "missing",
        "forward",
        "clear",
        "stop",
        "source",
        "freshness",
        "cancel",
        "expiry",
    ] {
        let (_root, mut repo, account, store, mut saved) = if mutation == "missing" {
            fixture(Forward::Marked)
        } else {
            cleared_fixture(false)
        };
        let mut request = input(&saved);
        let mut peer = review_peer(&repo, &account, &saved, &request);
        let cancel = Cancellation::default();
        match mutation {
            "forward" => request.forward = 99,
            "clear" => request.clear = 99,
            "stop" => saved.protocol_backlog.runs[0].stop_requested = false,
            "source" => std::sync::Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                .epoch
                .push('x'),
            "cancel" => cancel.cancel(),
            "expiry" => {
                saved.protocol_backlog.runs[0].completed_at = Some(1);
                store.save_state(&mut saved).unwrap();
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
            repo.observe_dependent_cleanup(
                &account,
                &store,
                &saved,
                target(&request),
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
        assert_eq!(peer.inventories, 0);
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    }
}

#[test]
fn changed_topology_or_current_originals_cannot_consume_the_previous_cleanup_receipt() {
    for mutation in ["topology", "missing-originals"] {
        let (_root, repo, account, store, mut saved) = cleared_fixture(false);
        let mut request = input(&saved);
        let expected = repo
            .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        if mutation == "missing-originals" {
            request.source = empty(&request.source);
        }
        let mut peer = review_peer(&repo, &account, &saved, &request);
        peer.change_after_read = mutation == "topology";
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
        let result = repo.observe_dependent_cleanup(
            &account,
            &store,
            &saved,
            target(&request),
            &mut peer,
            &permit,
            &Cancellation::default(),
            &|_| Ok(()),
        );
        if mutation == "topology" {
            assert!(result.is_err());
        } else {
            let observed = result.unwrap();
            assert_eq!(observed.review().actions().remove_copy, 0);
            assert_eq!(observed.review().actions().conflicts, 2);
            assert_ne!(observed.review().receipt(), expected.receipt());
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
        }
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(&*peer.peer.io.events.borrow(), &["compensation-review"]);
    }
}

#[test]
fn dependent_observed_holders_reject_rebound_receipts_and_already_approved_clear_slots() {
    for use_new_receipt in [false, true] {
        let (_root, repo, account, store, mut saved) = cleared_fixture(false);
        let request = input(&saved);
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
        let observed = repo
            .observe_dependent_cleanup(
                &account,
                &store,
                &saved,
                target(&request),
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| Ok(()),
            )
            .unwrap();
        let receipt = observed.review().receipt().to_owned();
        store.save_state(&mut saved).unwrap();
        let updated = repo
            .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        assert_ne!(updated.receipt(), receipt);
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            observed
                .confirm(
                    &repo,
                    &account,
                    &store,
                    &mut saved,
                    if use_new_receipt {
                        updated.receipt()
                    } else {
                        &receipt
                    },
                    |_| Ok(()),
                )
                .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        repo.confirm_dependent_cleanup(
            &account,
            &store,
            &mut saved,
            &request,
            updated.receipt(),
            |_| Ok(()),
        )
        .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let count = peer.inventories;
        assert!(
            repo.observe_dependent_cleanup(
                &account,
                &store,
                &saved,
                target(&request),
                &mut peer,
                &permit,
                &Cancellation::default(),
                &|_| Ok(()),
            )
            .is_err()
        );
        assert_eq!(peer.inventories, count);
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    }
}
