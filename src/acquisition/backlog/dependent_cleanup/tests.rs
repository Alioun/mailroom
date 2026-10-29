use super::*;
use crate::{
    acquisition::{CopyCompensationSlot, CopyCompensationWork},
    backlog::{
        imap_copy::CleanupSlot,
        protocol::{CompensationPhase as Phase, compensation_fixture},
    },
};
use std::sync::atomic::Ordering;

fn fixture(
    phase: Phase,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
    DependentCleanupRequest,
) {
    let (root, repo, account, store, mut saved, request) =
        super::super::compensation_review::tests::fixture(Phase::Cleared);
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
    saved.protocol_backlog.runs[0].compensation[0].evidence =
        compensation_fixture(&account.id, phase).compensation[0]
            .evidence
            .clone();
    let run = &mut saved.protocol_backlog.runs[0];
    run.status = model::outcome(run, true);
    run.stop_requested = true;
    store.save_state(&mut saved).unwrap();
    let entry = &saved.protocol_backlog.runs[0].compensation[0];
    let request = DependentCleanupRequest {
        run: saved.protocol_backlog.runs[0].id.clone(),
        forward: 0,
        clear: 0,
        topology: request.topology,
        source: entry
            .evidence
            .clear(0)
            .and_then(|clear| clear.readback())
            .unwrap_or(entry.evidence.preview().source())
            .clone(),
        destination: entry.evidence.preview().destination().clone(),
    };
    (root, repo, account, store, saved, request)
}

fn unchanged(store: &EncryptedStore, saved: &AccountState) -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(store.path()).unwrap(),
        serde_json::to_vec(saved).unwrap(),
    )
}

#[test]
fn confirmed_dependency_survives_reopen_and_admits_only_durable_effect_intents() {
    let (_root, mut repo, account, store, mut saved, request) = fixture(Phase::Cleared);
    let accesses = repo.track_credential_accesses();
    let before = unchanged(&store, &saved);
    let original = saved.protocol_backlog.runs[0].clone();
    let review = repo
        .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    assert_eq!(
        review.actions(),
        CopyCompensationActions {
            remove_copy: 2,
            ..Default::default()
        }
    );
    assert_eq!(unchanged(&store, &saved), before);
    repo.confirm_dependent_cleanup(
        &account,
        &store,
        &mut saved,
        &request,
        review.receipt(),
        |_| Ok(()),
    )
    .unwrap();
    let reopened = store.load_state().unwrap();
    let run = &reopened.protocol_backlog.runs[0];
    assert_eq!(run.undo_receipt, original.undo_receipt);
    assert!(run.batches == original.batches);
    assert_eq!(&run.compensation[0].evidence, review.evidence());
    assert_eq!(
        run.compensation[0].after_clear_approvals[0].receipt,
        review.receipt()
    );
    assert_eq!(run.status, Status::Stopped);
    assert!(run.stop_requested);
    approved_dependencies(&store, run).unwrap();
    let accepted = unchanged(&store, &saved);
    assert!(
        repo.confirm_dependent_cleanup(
            &account,
            &store,
            &mut saved,
            &request,
            review.receipt(),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(unchanged(&store, &saved), accepted);
    saved.protocol_backlog.runs[0].status = Status::UndoRunning;
    saved.protocol_backlog.runs[0].stop_requested = false;
    store.save_state(&mut saved).unwrap();
    let mut evidence = saved.protocol_backlog.runs[0].compensation[0]
        .evidence
        .clone();
    let slot = CleanupSlot::AfterClear { clear: 0, group: 0 };
    let plan = evidence.cleanup_plan(slot).unwrap().clone();
    let fresh = |_: &AccountState| Ok(());
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &request.run,
                forward: 0,
                work: CopyCompensationWork::Execute,
            },
            &request.topology,
            &fresh,
        )
        .unwrap();
    evidence
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    let token = writer.persist_intent(&evidence).unwrap();
    assert_eq!(token.evidence(), &evidence);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].compensation[0].evidence,
        evidence
    );
    evidence
        .record_cleanup_store(slot, crate::backlog::imap_copy::StoreResult::Lost)
        .unwrap();
    writer.observe(&evidence).unwrap();
    let observed = store.load_state().unwrap();
    approved_dependencies(&store, &observed.protocol_backlog.runs[0]).unwrap();
    assert_eq!(observed.protocol_backlog.runs[0].status, Status::Uncertain);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
    assert!(!repo.backlog_installed(Some(&account.id)));
}

#[test]
fn changed_observations_receipt_purpose_and_state_fail_without_consuming_work() {
    for mutation in [
        "observations",
        "receipt",
        "undo-receipt",
        "revision",
        "memory",
        "freshness",
    ] {
        let (_root, repo, account, store, mut saved, mut request) = fixture(Phase::Cleared);
        let review = repo
            .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        let mut receipt = review.receipt().to_owned();
        match mutation {
            "observations" => {
                request.source = Readback::new(
                    request.source.identity().clone(),
                    crate::backlog::imap_copy::ModSeq::new(
                        request.source.highest_modseq().get() + 1,
                    )
                    .unwrap(),
                    request.source.messages().clone(),
                )
                .unwrap();
            }
            "receipt" => receipt = "a".repeat(64),
            "undo-receipt" => {
                receipt = saved.protocol_backlog.runs[0].undo_receipt.clone().unwrap()
            }
            "revision" => {
                let mut other = saved.clone();
                store.save_state(&mut other).unwrap();
            }
            "memory" => {
                saved
                    .census
                    .confirmed_own_addresses
                    .insert("invented@example.test".into());
            }
            "freshness" => (),
            _ => unreachable!(),
        }
        let before = unchanged(&store, &saved);
        assert!(
            repo.confirm_dependent_cleanup(
                &account,
                &store,
                &mut saved,
                &request,
                &receipt,
                |_| if mutation == "freshness" {
                    Err("changed".into())
                } else {
                    Ok(())
                }
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(unchanged(&store, &saved), before, "{mutation}");
    }
}

#[test]
fn missing_clear_uncertainty_source_and_topology_never_produce_an_approval() {
    for mutation in [
        "missing",
        "uncertain",
        "source",
        "topology",
        "forward",
        "clear",
        "status",
        "expiry",
    ] {
        let phase = if mutation == "uncertain" {
            Phase::ClearUncertain
        } else {
            Phase::Cleared
        };
        let (_root, mut repo, account, store, mut saved, mut request) = fixture(phase);
        match mutation {
            "missing" => {
                let initial = crate::backlog::imap_copy::CompensationEvidence::new(
                    saved.protocol_backlog.runs[0].compensation[0]
                        .evidence
                        .forward()
                        .clone(),
                    saved.protocol_backlog.runs[0].compensation[0]
                        .evidence
                        .preview()
                        .clone(),
                )
                .unwrap();
                saved.protocol_backlog.runs[0].compensation[0].evidence = initial;
                store.save_state(&mut saved).unwrap();
            }
            "source" => repo.backlog_source = None,
            "topology" => {
                request.topology.capabilities.insert("MOVE".into());
            }
            "forward" => request.forward = 99,
            "clear" => request.clear = 99,
            "status" => {
                saved.protocol_backlog.runs[0].status = Status::UndoRunning;
                saved.protocol_backlog.runs[0].stop_requested = false;
                store.save_state(&mut saved).unwrap();
            }
            "expiry" => {
                saved.protocol_backlog.runs[0].completed_at = Some(1);
                store.save_state(&mut saved).unwrap();
            }
            "uncertain" => (),
            _ => unreachable!(),
        }
        let before = unchanged(&store, &saved);
        assert!(
            repo.review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
                .is_err(),
            "{mutation}"
        );
        assert_eq!(unchanged(&store, &saved), before);
    }
}

#[test]
fn missing_or_substituted_saved_plan_approvals_never_admit_effects() {
    for mutation in [
        "missing",
        "binding",
        "receipt",
        "duplicate",
        "unknown-clear",
    ] {
        let (_root, repo, account, store, mut saved, request) = fixture(Phase::Cleared);
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
        let entry = &mut saved.protocol_backlog.runs[0].compensation[0];
        match mutation {
            "missing" => entry.after_clear_approvals.clear(),
            "binding" => entry.after_clear_approvals[0].binding = "f".repeat(64),
            "receipt" => entry.after_clear_approvals[0].receipt = "f".repeat(64),
            "duplicate" => entry
                .after_clear_approvals
                .push(entry.after_clear_approvals[0].clone()),
            "unknown-clear" => entry.after_clear_approvals[0].clear = 99,
            _ => unreachable!(),
        }
        saved.protocol_backlog.runs[0].status = Status::UndoRunning;
        saved.protocol_backlog.runs[0].stop_requested = false;
        let before = std::fs::read(store.path()).unwrap();
        let result = store.save_state(&mut saved);
        if matches!(mutation, "duplicate" | "unknown-clear") {
            assert!(result.is_err());
            assert_eq!(std::fs::read(store.path()).unwrap(), before);
            continue;
        }
        result.unwrap();
        assert!(
            repo.copy_compensation_checkpoints(
                &account,
                &store,
                &mut saved,
                CopyCompensationSlot {
                    run: &request.run,
                    forward: 0,
                    work: CopyCompensationWork::Execute
                },
                &request.topology,
                &|_| Ok(())
            )
            .is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn conflicts_remain_separate_and_a_conflict_only_review_cannot_be_consumed() {
    for missing in [1, 2] {
        let (_root, repo, account, store, mut saved, mut request) = fixture(Phase::Cleared);
        let mut messages = request.source.messages().clone();
        let removed: Vec<_> = messages.keys().copied().take(missing).collect();
        for uid in removed {
            messages.remove(&uid);
        }
        request.source = crate::backlog::imap_copy::Readback::new(
            request.source.identity().clone(),
            request.source.highest_modseq(),
            messages,
        )
        .unwrap();
        let before = unchanged(&store, &saved);
        let review = repo
            .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        assert_eq!(review.actions().remove_copy, 2 - missing);
        assert_eq!(review.actions().conflicts, missing);
        let result = repo.confirm_dependent_cleanup(
            &account,
            &store,
            &mut saved,
            &request,
            review.receipt(),
            |_| Ok(()),
        );
        if missing == 2 {
            assert!(result.is_err());
            assert_eq!(unchanged(&store, &saved), before);
        } else {
            result.unwrap();
            let run = &store.load_state().unwrap().protocol_backlog.runs[0];
            assert_eq!(run.status, Status::Conflict);
            approved_dependencies(&store, run).unwrap();
        }
    }
}

#[test]
fn a_different_accounts_exact_receipt_confers_no_authority() {
    let (_root, repo, account, store, saved, request) = fixture(Phase::Cleared);
    let review = repo
        .review_dependent_cleanup(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    let (_other_root, other_repo, other_account, other_store, mut other, other_request) =
        fixture(Phase::Cleared);
    assert_ne!(account.id, other_account.id);
    let before = unchanged(&other_store, &other);
    assert!(
        other_repo
            .confirm_dependent_cleanup(
                &other_account,
                &other_store,
                &mut other,
                &other_request,
                review.receipt(),
                |_| Ok(())
            )
            .is_err()
    );
    assert_eq!(unchanged(&other_store, &other), before);
}
