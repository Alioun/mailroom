use super::*;
use crate::backlog::protocol::tests::{Stage, run};
use std::sync::atomic::Ordering;

fn fixture() -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
    Run,
) {
    let evidence = run(
        &AccountId::new("copy-start-input").unwrap(),
        Stage::Prepared,
    )
    .batches[0]
        .as_copy()
        .unwrap()
        .clone();
    let (root, repo, account, store, mut saved) =
        super::super::copy_checkpoints::tests::fixture(evidence);
    let mut operation = saved.protocol_backlog.runs.remove(0);
    store.save_state(&mut saved).unwrap();
    operation.status = Status::Prepared;
    operation.plan.binding = repo.backlog_binding(&account, &store, &saved).unwrap();
    sign(&store, &saved, &mut operation);
    (root, repo, account, store, saved, operation)
}

fn sign(store: &EncryptedStore, saved: &AccountState, operation: &mut Run) {
    operation.receipt = start_receipt(store, saved, &operation.plan, &operation.batches).unwrap();
    operation.id = store
        .keys()
        .binding_for(b"mailroom/imap-backlog-id/v1", operation.receipt.as_bytes());
}

fn snapshot(store: &EncryptedStore, saved: &AccountState) -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(store.path()).unwrap(),
        serde_json::to_vec(saved).unwrap(),
    )
}

#[test]
fn exact_observed_copy_start_is_saved_once_before_worker_admission() {
    let (_root, mut repo, account, store, mut saved, operation) = fixture();
    let accesses = repo.track_credential_accesses();
    let previous_revision = saved.revision;
    let original = serde_json::to_value(&saved).unwrap();
    let receipt = operation.receipt.clone();
    let replay = operation.clone();
    let id = ObservedCopyStart {
        operation: operation.clone(),
    }
    .confirm(
        &repo,
        &account,
        &store,
        &mut saved,
        &receipt,
        &Cancellation::default(),
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(id, operation.id);
    assert_eq!(saved.revision, previous_revision + 1);
    assert!(saved.protocol_backlog.runs == vec![operation.clone()]);
    assert!(
        store
            .checkpoint_matches(&saved, crate::census::ProtocolStateView::Persisted)
            .unwrap()
    );
    let reopened = store.load_state().unwrap();
    assert!(reopened.protocol_backlog.runs[0].plan == operation.plan);
    assert_eq!(reopened.protocol_backlog.runs[0].receipt, receipt);
    assert!(reopened.protocol_backlog.runs[0].batches == operation.batches);
    let mut retained = serde_json::to_value(&saved).unwrap();
    retained["revision"] = original["revision"].clone();
    retained["protocol_backlog"] = original["protocol_backlog"].clone();
    assert_eq!(retained, original);
    repo.copy_worker(
        &account,
        &store,
        &mut saved,
        &id,
        copy_worker::CopyWork::File,
        &|_| Ok(()),
    )
    .unwrap();
    let before = snapshot(&store, &saved);
    assert!(
        ObservedCopyStart { operation: replay }
            .confirm(
                &repo,
                &account,
                &store,
                &mut saved,
                &receipt,
                &Cancellation::default(),
                &|_| Ok(())
            )
            .is_err()
    );
    assert_eq!(snapshot(&store, &saved), before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
}

#[test]
fn changed_receipt_plan_and_observation_never_save_a_copy_start() {
    for mutation in ["receipt", "id", "run_receipt", "proof", "topology", "batch"] {
        let (_root, mut repo, account, store, mut saved, mut operation) = fixture();
        let accesses = repo.track_credential_accesses();
        let mut receipt = operation.receipt.clone();
        match mutation {
            "receipt" => receipt = "f".repeat(64),
            "id" => operation.id = "f".repeat(64),
            "run_receipt" => operation.receipt = "f".repeat(64),
            "proof" => operation.plan.execution_proof = "f".repeat(64),
            "topology" => operation.plan.topology.mailboxes.clear(),
            "batch" => operation.batches = run(&account.id, Stage::Copied).batches,
            _ => unreachable!(),
        }
        let before = snapshot(&store, &saved);
        assert!(
            ObservedCopyStart { operation }
                .confirm(
                    &repo,
                    &account,
                    &store,
                    &mut saved,
                    &receipt,
                    &Cancellation::default(),
                    &|_| Ok(())
                )
                .is_err(),
            "{mutation}"
        );
        assert_eq!(snapshot(&store, &saved), before, "{mutation}");
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn changed_durable_or_unsaved_state_cannot_consume_an_old_copy_preview() {
    for mutation in ["durable", "unsaved", "spoofed_revision", "workspace"] {
        let (_root, mut repo, account, store, mut saved, operation) = fixture();
        let accesses = repo.track_credential_accesses();
        let receipt = operation.receipt.clone();
        match mutation {
            "durable" => {
                let mut newer = saved.clone();
                newer.workspace.as_mut().unwrap().revision += 1;
                store.save_state(&mut newer).unwrap();
            }
            "spoofed_revision" => {
                let mut newer = saved.clone();
                newer.workspace.as_mut().unwrap().revision += 1;
                store.save_state(&mut newer).unwrap();
                saved.revision = newer.revision;
            }
            "workspace" => saved.workspace.as_mut().unwrap().revision += 1,
            "unsaved" => saved.protocol_backlog.runs.push(operation.clone()),
            _ => unreachable!(),
        }
        let before = snapshot(&store, &saved);
        assert!(
            ObservedCopyStart { operation }
                .confirm(
                    &repo,
                    &account,
                    &store,
                    &mut saved,
                    &receipt,
                    &Cancellation::default(),
                    &|_| Ok(())
                )
                .is_err(),
            "{mutation}"
        );
        assert_eq!(snapshot(&store, &saved), before, "{mutation}");
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn changed_account_profile_or_source_never_access_credentials_or_save_intent() {
    for mutation in ["account", "profile", "source", "production"] {
        let (_root, mut repo, mut account, store, mut saved, operation) = fixture();
        let accesses = repo.track_credential_accesses();
        let receipt = operation.receipt.clone();
        let source = std::sync::Arc::get_mut(repo.backlog_source.as_mut().unwrap()).unwrap();
        match mutation {
            "account" => account.id = AccountId::new("another-copy-owner").unwrap(),
            "profile" => source.authority = "fixture-message-move/v1".into(),
            "source" => source.epoch.push_str("-changed"),
            "production" => source.address = "192.0.2.1:993".parse().unwrap(),
            _ => unreachable!(),
        }
        let before = snapshot(&store, &saved);
        assert!(
            ObservedCopyStart { operation }
                .confirm(
                    &repo,
                    &account,
                    &store,
                    &mut saved,
                    &receipt,
                    &Cancellation::default(),
                    &|_| Ok(())
                )
                .is_err(),
            "{mutation}"
        );
        assert_eq!(snapshot(&store, &saved), before, "{mutation}");
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn cancellation_and_failed_freshness_keep_copy_preview_unconsumed() {
    for mutation in ["cancelled", "cancel_during_freshness", "freshness"] {
        let (_root, mut repo, account, store, mut saved, operation) = fixture();
        let accesses = repo.track_credential_accesses();
        let receipt = operation.receipt.clone();
        let cancel = Cancellation::default();
        if mutation == "cancelled" {
            cancel.cancel();
        }
        let fresh = |_: &AccountState| {
            if mutation == "cancel_during_freshness" {
                cancel.cancel();
            }
            if mutation == "freshness" {
                Err("changed fixture ledger".into())
            } else {
                Ok(())
            }
        };
        let before = snapshot(&store, &saved);
        assert!(
            ObservedCopyStart { operation }
                .confirm(
                    &repo, &account, &store, &mut saved, &receipt, &cancel, &fresh
                )
                .is_err(),
            "{mutation}"
        );
        assert_eq!(snapshot(&store, &saved), before, "{mutation}");
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn start_receipt_keeps_existing_serialization_and_separates_archive_intent() {
    let (_root, _repo, _account, store, saved, operation) = fixture();
    let old = store.keys().binding_for(
        operation.plan.filing.options.intent.purpose(
            b"mailroom/imap-backlog-start/v2",
            b"mailroom/imap-backlog-archive/v1",
        ),
        &crate::census::binding::bound(|| {
            serde_json::to_vec(&(&saved, &operation.plan, &operation.batches))
        })
        .unwrap(),
    );
    assert_eq!(operation.receipt, old);
    let archive_purpose = store.keys().binding_for(
        b"mailroom/imap-backlog-archive/v1",
        &crate::census::binding::bound(|| {
            serde_json::to_vec(&(&saved, &operation.plan, &operation.batches))
        })
        .unwrap(),
    );
    assert_ne!(operation.receipt, archive_purpose);
}
