use super::*;
use crate::{
    acquisition::backlog::copy_checkpoints::tests::fixture as account_fixture,
    backlog::protocol::{
        CompensationPhase as Phase, compensation_fixture,
        tests::{Stage, run},
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(in crate::acquisition) fn fixture(
    phase: Phase,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
    CopyCompensationRequest,
) {
    let seed = AccountId::new("copy-review-input").unwrap();
    let evidence = compensation_fixture(&seed, phase).batches[0]
        .as_copy()
        .unwrap()
        .clone();
    let (root, mut repo, account, store, mut saved) = account_fixture(evidence);
    repo.clock = Some(Arc::new(AtomicU64::new(20_000_000)));
    saved.protocol_backlog.runs[0].status = model::outcome(&saved.protocol_backlog.runs[0], false);
    store.save_state(&mut saved).unwrap();
    let saved = store.load_state().unwrap();
    let example = compensation_fixture(&account.id, phase);
    let preview = example.compensation[0].evidence.preview();
    let request = CopyCompensationRequest {
        run: saved.protocol_backlog.runs[0].id.clone(),
        topology: saved.protocol_backlog.runs[0].plan.topology.clone(),
        observations: vec![CopyCompensationObservation {
            forward: 0,
            source: preview.source().clone(),
            destination: preview.destination().clone(),
        }],
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
fn exact_approval_distinguishes_restore_cleanup_and_clear_and_reopens_without_effects() {
    for (phase, actions) in [
        (
            Phase::Prepared,
            CopyCompensationActions {
                remove_copy: 2,
                ..Default::default()
            },
        ),
        (
            Phase::Cleared,
            CopyCompensationActions {
                clear_deleted: 2,
                ..Default::default()
            },
        ),
        (
            Phase::RestorePrepared,
            CopyCompensationActions {
                restore: 2,
                ..Default::default()
            },
        ),
    ] {
        let (_root, mut repo, account, store, mut saved, request) = fixture(phase);
        let credentials = repo.track_credential_accesses();
        let before = unchanged(&store, &saved);
        let review = repo
            .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap();
        assert_eq!(review.actions(), actions);
        assert_eq!(review.compensation().len(), 1);
        assert_eq!(unchanged(&store, &saved), before);
        repo.confirm_copy_compensation(
            &account,
            &store,
            &mut saved,
            &request,
            review.receipt(),
            |_| Ok(()),
        )
        .unwrap();
        let reopened = store.load_state().unwrap();
        let operation = &reopened.protocol_backlog.runs[0];
        assert_eq!(operation.undo_receipt.as_deref(), Some(review.receipt()));
        assert_eq!(operation.status, Status::Stopped);
        assert!(operation.compensation == review.compensation);
        assert!(
            operation.batches
                == serde_json::from_slice::<AccountState>(&before.1)
                    .unwrap()
                    .protocol_backlog
                    .runs[0]
                    .batches
        );
        assert!(!operation.finished());
        assert!(
            operation.compensation[0]
                .evidence
                .resolutions()
                .values()
                .all(|r| *r == crate::backlog::imap_copy::CompensationResolution::Unattempted)
        );
        assert_eq!(credentials.load(Ordering::SeqCst), 0);
        let mut old: serde_json::Value = serde_json::from_slice(&before.1).unwrap();
        let mut current = serde_json::to_value(&reopened).unwrap();
        for value in [&mut old, &mut current] {
            value.as_object_mut().unwrap().remove("revision");
            value.as_object_mut().unwrap().remove("protocol_backlog");
        }
        assert_eq!(old, current);
        let checkpoint = unchanged(&store, &saved);
        assert!(
            repo.confirm_copy_compensation(
                &account,
                &store,
                &mut saved,
                &request,
                review.receipt(),
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(unchanged(&store, &saved), checkpoint);
        assert!(!repo.backlog_installed(Some(&account.id)));
    }
}

#[test]
fn changed_observations_require_a_new_exact_receipt() {
    let (_root, repo, account, store, mut saved, mut request) = fixture(Phase::Prepared);
    let review = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    let mut raw = serde_json::to_value(&request.observations[0].source).unwrap();
    let newer = request.observations[0].source.highest_modseq().get() + 1;
    raw["highest_modseq"] = newer.to_string().into();
    request.observations[0].source = serde_json::from_value(raw).unwrap();
    let changed = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    assert_ne!(review.receipt(), changed.receipt());
    let before = unchanged(&store, &saved);
    assert!(
        repo.confirm_copy_compensation(
            &account,
            &store,
            &mut saved,
            &request,
            review.receipt(),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(unchanged(&store, &saved), before);
    repo.confirm_copy_compensation(
        &account,
        &store,
        &mut saved,
        &request,
        changed.receipt(),
        |_| Ok(()),
    )
    .unwrap();
}

#[test]
fn stale_state_unsaved_history_and_concurrent_revisions_never_consume_approval() {
    for mutation in [
        "revision",
        "memory-history",
        "workspace",
        "concurrent",
        "freshness",
    ] {
        let (_root, repo, account, store, mut saved, request) = fixture(Phase::Prepared);
        let receipt = repo
            .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap()
            .receipt()
            .to_owned();
        match mutation {
            "revision" => store.save_state(&mut saved).unwrap(),
            "memory-history" => saved.protocol_backlog.runs[0].receipt = "e".repeat(64),
            "workspace" => saved.workspace.as_mut().unwrap().revision += 1,
            "concurrent" => {
                let mut current = saved.clone();
                store.save_state(&mut current).unwrap();
            }
            "freshness" => (),
            _ => unreachable!(),
        }
        let before = unchanged(&store, &saved);
        assert!(
            repo.confirm_copy_compensation(
                &account,
                &store,
                &mut saved,
                &request,
                &receipt,
                |_| {
                    if mutation == "freshness" {
                        Err("changed local source".into())
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(unchanged(&store, &saved), before, "{mutation}");
    }
}

#[test]
fn foreign_keys_accounts_and_other_purposes_do_not_authorize_compensation() {
    let (_root, repo, account, store, mut saved, request) = fixture(Phase::Prepared);
    let (_other_root, other_repo, other_account, other_store, other_saved, other_request) =
        fixture(Phase::Prepared);
    let review = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    let other = other_repo
        .review_copy_compensation(
            &other_account,
            &other_store,
            &other_saved,
            &other_request,
            |_| Ok(()),
        )
        .unwrap();
    let payload = crate::census::binding::bound(|| {
        serde_json::to_vec(&(&saved, &request, &review.compensation, review.actions))
    })
    .unwrap();
    let other_purpose = store
        .keys()
        .binding_for(b"mailroom/imap-backlog-undo/v2", &payload);
    for receipt in [
        other.receipt(),
        other_purpose.as_str(),
        saved.protocol_backlog.runs[0].receipt.clone().as_str(),
    ] {
        let before = unchanged(&store, &saved);
        assert!(
            repo.confirm_copy_compensation(
                &account,
                &store,
                &mut saved,
                &request,
                receipt,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(unchanged(&store, &saved), before);
    }
    assert!(
        repo.review_copy_compensation(&other_account, &store, &saved, &request, |_| Ok(()))
            .is_err()
    );
    assert!(
        repo.review_copy_compensation(&account, &other_store, &saved, &request, |_| Ok(()))
            .is_err()
    );
}

#[test]
fn missing_duplicate_foreign_and_changed_topology_inputs_never_save() {
    for mutation in [
        "missing",
        "duplicate",
        "index",
        "run",
        "topology",
        "uidvalidity",
    ] {
        let (_root, repo, account, store, saved, mut request) = fixture(Phase::Prepared);
        match mutation {
            "missing" => request.observations.clear(),
            "duplicate" => request.observations.push(request.observations[0].clone()),
            "index" => request.observations[0].forward = 1,
            "run" => request.run = "e".repeat(64),
            "topology" => {
                request.topology.capabilities.insert("MOVE".into());
            }
            "uidvalidity" => {
                let mut raw = serde_json::to_value(&request.observations[0].destination).unwrap();
                raw["identity"]["uidvalidity"] = 999.into();
                request.observations[0].destination = serde_json::from_value(raw).unwrap();
            }
            _ => unreachable!(),
        }
        let before = unchanged(&store, &saved);
        assert!(
            repo.review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
                .is_err(),
            "{mutation}"
        );
        assert_eq!(unchanged(&store, &saved), before);
    }
}

#[test]
fn expiry_and_production_source_changes_deny_before_credential_access() {
    for mutation in [
        "expired",
        "production",
        "source",
        "workspace-authority",
        "lifecycle",
    ] {
        let (root, mut repo, account, store, mut saved, request) = fixture(Phase::RestorePrepared);
        let receipt = repo
            .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
            .unwrap()
            .receipt()
            .to_owned();
        let credentials = repo.track_credential_accesses();
        match mutation {
            "expired" => {
                saved.protocol_backlog.runs[0].completed_at =
                    Some(repo.now() - crate::backlog::retention::WINDOW_SECS);
                store.save_state(&mut saved).unwrap();
            }
            "production" => repo.backlog_source = None,
            "source" => Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                .epoch
                .push('x'),
            "workspace-authority" => repo.install_synthetic_workspace(&account, "changed"),
            "lifecycle" => std::fs::write(
                root.path()
                    .join("config/lifecycle")
                    .join(format!("{}.json", account.id.as_str())),
                b"{}",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let before = unchanged(&store, &saved);
        assert!(
            repo.confirm_copy_compensation(
                &account,
                &store,
                &mut saved,
                &request,
                &receipt,
                |_| Ok(())
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(unchanged(&store, &saved), before);
        assert_eq!(credentials.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_review_with_only_conflicts_cannot_be_confirmed_as_work() {
    let (_root, repo, account, store, mut saved, mut request) = fixture(Phase::Prepared);
    let copy = &request.observations[0].destination;
    request.observations[0].destination = Readback::new(
        copy.identity().clone(),
        copy.highest_modseq(),
        Default::default(),
    )
    .unwrap();
    let review = repo
        .review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
        .unwrap();
    assert_eq!(
        review.actions(),
        CopyCompensationActions {
            conflicts: 2,
            ..Default::default()
        }
    );
    let before = unchanged(&store, &saved);
    assert!(
        repo.confirm_copy_compensation(
            &account,
            &store,
            &mut saved,
            &request,
            review.receipt(),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(unchanged(&store, &saved), before);
}

#[test]
fn unsaved_stopping_does_not_enable_compensation_review() {
    let (_root, repo, account, store, mut saved, request) = fixture(Phase::Prepared);
    saved.protocol_backlog.runs[0].status = Status::Running;
    saved.protocol_backlog.runs[0].stop_requested = false;
    store.save_state(&mut saved).unwrap();
    let interrupted = store.load_state().unwrap();
    assert_eq!(interrupted.protocol_backlog.runs[0].status, Status::Stopped);
    assert!(
        repo.review_copy_compensation(&account, &store, &interrupted, &request, |_| Ok(()))
            .is_err()
    );
}

#[test]
fn uncertain_forward_effects_do_not_produce_an_approval() {
    for stage in [
        Stage::CopyUncertain,
        Stage::StoreUncertain,
        Stage::ExpungeUncertain,
    ] {
        let (_root, repo, account, store, mut saved, request) = fixture(Phase::Prepared);
        let forward = run(&account.id, stage).batches.remove(0);
        saved.protocol_backlog.runs[0].batches[0] = forward;
        saved.protocol_backlog.runs[0].status =
            model::outcome(&saved.protocol_backlog.runs[0], false);
        store.save_state(&mut saved).unwrap();
        let saved = store.load_state().unwrap();
        let before = unchanged(&store, &saved);
        assert!(
            repo.review_copy_compensation(&account, &store, &saved, &request, |_| Ok(()))
                .is_err(),
            "{stage:?}"
        );
        assert_eq!(unchanged(&store, &saved), before);
    }
}
