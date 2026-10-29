use super::*;
use crate::{
    acquisition::{Provider, SecretValue, persistent_test_accounts},
    backlog::protocol::tests::{Stage, run},
    rules::workspace::{Workspace, WorkspaceInput},
};
use std::{cell::Cell, sync::Arc};

pub(in crate::acquisition) fn fixture(
    evidence: CopyEvidence,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
) {
    let root = tempfile::tempdir().unwrap();
    let mut repo = persistent_test_accounts(root.path());
    let account = repo
        .create(
            Provider::MailboxOrg,
            "copy-owner@example.test".into(),
            &SecretValue::new(b"synthetic-copy-password".to_vec()),
        )
        .unwrap();
    repo.install_synthetic_workspace(&account, "copy-checkpoint-workspace-v1");
    repo.backlog_source = Some(Arc::new(InstalledSource {
        account: account.id.clone(),
        login: account.login.clone(),
        address: "127.0.0.1:9".parse().unwrap(),
        tls_name: "fixture.example.test".into(),
        root: vec![1, 2, 3],
        epoch: "copy-checkpoint-source-v1".into(),
        authority: "fixture-message-copy/v1".into(),
        command_timeout: std::time::Duration::from_secs(1),
        operation_timeout: std::time::Duration::from_secs(5),
    }));
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let input: crate::engine::PreviewInput = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/pre-core-sample.json"
    ))
    .unwrap();
    saved.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: 1793260800,
            baseline: proof::Baseline::Current,
        },
    });
    let context = repo.workspace_context(&account, &store).unwrap();
    let proof = workspace::preview(&saved, &context, store.keys()).unwrap();
    assert!(proof.download_available);
    let mut operation = run(&account.id, Stage::Prepared);
    operation.plan.filing.artifact = proof.proof.artifact;
    operation.plan.saved_proof = proof.receipt;
    operation.plan.execution_proof = store.keys().binding_for(b"dummy-execution-proof", b"proof");
    for change in &mut operation.plan.filing.changes {
        let metadata = &evidence.intent().messages()[&Uid::new(change.source.uid).unwrap()];
        change.source.message_id_hash = metadata.message_id_hash.clone();
        change.flags = metadata.flags.clone();
    }
    operation.batches[0] = model::Batch::copy(evidence);
    operation.plan.binding = repo.backlog_binding(&account, &store, &saved).unwrap();
    operation.status = Status::Running;
    saved.protocol_backlog.runs.push(operation);
    store.save_state(&mut saved).unwrap();
    (root, repo, account, store, saved)
}

fn evidence(stage: Stage) -> CopyEvidence {
    run(&AccountId::new("copy-checkpoint-input").unwrap(), stage).batches[0]
        .as_copy()
        .unwrap()
        .clone()
}

#[test]
fn exact_account_intent_and_observation_survive_encrypted_reopen() {
    let (_root, repo, account, store, mut saved) = fixture(evidence(Stage::Prepared));
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let id = saved.protocol_backlog.runs[0].id.clone();
    let mut intent = evidence(Stage::Prepared);
    intent.begin_copy(&intent.before().clone()).unwrap();
    {
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(()),
            )
            .unwrap();
        assert_eq!(checkpoints.save(&intent), Err(Failure::Unsupported));
        checkpoints.validate_intent(&intent).unwrap();
        checkpoints.save(&intent).unwrap();
        let copied = evidence(Stage::Copied);
        checkpoints.save(&copied).unwrap();
        assert_eq!(checkpoints.previous, copied);
    }
    let reopened = store.load_state().unwrap();
    let mut expected = saved.protocol_backlog.clone();
    expected.interrupted();
    assert!(reopened.protocol_backlog == expected);
    assert_eq!(
        reopened.protocol_backlog.runs[0].batches[0]
            .as_copy()
            .unwrap()
            .next_action(),
        NextAction::PrepareDeletion
    );
    let proof = workspace::preview(
        &saved,
        &repo.workspace_context(&account, &store).unwrap(),
        store.keys(),
    )
    .unwrap();
    assert!(proof.download_available);
}

#[test]
fn stale_local_authority_and_topology_never_admit_a_checkpoint_writer() {
    for mutation in [
        "source",
        "workspace",
        "material",
        "metadata",
        "lifecycle",
        "topology",
        "revision",
        "account",
        "production",
    ] {
        let (root, mut repo, mut account, store, mut saved) = fixture(evidence(Stage::Prepared));
        let mut topology = saved.protocol_backlog.runs[0].plan.topology.clone();
        let id = saved.protocol_backlog.runs[0].id.clone();
        match mutation {
            "source" => Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                .epoch
                .push('x'),
            "workspace" => repo.install_synthetic_workspace(&account, "changed-workspace"),
            "material" => saved.workspace.as_mut().unwrap().revision += 1,
            "metadata" => {
                let path = root
                    .path()
                    .join("config")
                    .join(format!("{}.json", account.id.as_str()));
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["login"] = "changed@example.test".into();
                std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "lifecycle" => {
                std::fs::write(
                    root.path()
                        .join("config/lifecycle")
                        .join(format!("{}.json", account.id.as_str())),
                    b"{}",
                )
                .unwrap();
            }
            "topology" => {
                topology.capabilities.insert("MOVE".into());
            }
            "revision" => {
                let mut concurrent = saved.clone();
                store.save_state(&mut concurrent).unwrap();
            }
            "account" => account.id = AccountId::new("different-account").unwrap(),
            "production" => repo.backlog_source = None,
            _ => unreachable!(),
        }
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            repo.copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(())
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before, "{mutation}");
    }
}

#[test]
fn observations_are_retained_after_freshness_changes_but_a_next_effect_is_denied() {
    let (_root, repo, account, store, mut saved) = fixture(evidence(Stage::Copied));
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let id = saved.protocol_backlog.runs[0].id.clone();
    let stale = Cell::new(false);
    let fresh = |_: &AccountState| {
        if stale.get() {
            Err("ledger changed".into())
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
        let intent = evidence(Stage::StoreUncertain);
        checkpoints.validate_intent(&intent).unwrap();
        checkpoints.save(&intent).unwrap();
        stale.set(true);
        checkpoints.save(&evidence(Stage::Marked)).unwrap();
        assert_eq!(
            checkpoints.validate_intent(&evidence(Stage::ExpungeUncertain)),
            Err(Failure::Unstable)
        );
        assert_eq!(checkpoints.previous.next_action(), NextAction::Expunge(0));
    }
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].batches[0]
            .as_copy()
            .unwrap(),
        &evidence(Stage::Marked)
    );
}

#[test]
fn observation_only_checkpoint_cannot_authorize_an_effect_or_override_a_run_hold() {
    for stage in [Stage::CopyUncertain, Stage::Copied, Stage::Marked] {
        let (_root, repo, account, store, mut saved) = fixture(evidence(stage));
        let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
        let id = saved.protocol_backlog.runs[0].id.clone();
        saved.protocol_backlog.runs[0].status =
            model::outcome(&saved.protocol_backlog.runs[0], false);
        store.save_state(&mut saved).unwrap();
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Reconcile { run: &id, batch: 0 },
                &topology,
                &|_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            checkpoints.validate_intent(&evidence(stage)),
            Err(Failure::Unsupported)
        );
        assert!(
            checkpoints
                .save(&evidence(Stage::ExpungeUncertain))
                .is_err()
        );
    }
    for status in [
        Status::Prepared,
        Status::Stopped,
        Status::Uncertain,
        Status::Conflict,
        Status::Complete,
        Status::Undone,
        Status::UndoRunning,
    ] {
        let (_root, repo, account, store, mut saved) = fixture(evidence(Stage::Prepared));
        let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
        let id = saved.protocol_backlog.runs[0].id.clone();
        saved.protocol_backlog.runs[0].status = status;
        assert!(
            repo.copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(())
            )
            .is_err()
        );
    }
}

#[test]
fn failed_revision_checked_save_preserves_the_callers_last_acknowledged_checkpoint() {
    let (_root, repo, account, store, mut saved) = fixture(evidence(Stage::Prepared));
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let id = saved.protocol_backlog.runs[0].id.clone();
    let mut concurrent = saved.clone();
    let before = serde_json::to_vec(&saved).unwrap();
    {
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(()),
            )
            .unwrap();
        let mut intent = evidence(Stage::Prepared);
        intent.begin_copy(&intent.before().clone()).unwrap();
        checkpoints.validate_intent(&intent).unwrap();
        store.save_state(&mut concurrent).unwrap();
        assert_eq!(checkpoints.save(&intent), Err(Failure::Limit));
        assert_eq!(checkpoints.previous.next_action(), NextAction::Copy);
    }
    assert_eq!(serde_json::to_vec(&saved).unwrap(), before);
    concurrent.protocol_backlog.interrupted();
    assert!(store.load_state().unwrap().protocol_backlog == concurrent.protocol_backlog);
}

#[test]
fn a_past_checkpoint_or_altered_initial_modseq_cannot_replace_durable_history() {
    use crate::backlog::imap_copy::{ModSeq, Readback};
    let (_root, repo, account, store, mut saved) = fixture(evidence(Stage::Copied));
    let topology = saved.protocol_backlog.runs[0].plan.topology.clone();
    let id = saved.protocol_backlog.runs[0].id.clone();
    let before = std::fs::read(store.path()).unwrap();
    let original = evidence(Stage::Prepared);
    let observation = original.before();
    let changed = Readback::new(
        observation.identity().clone(),
        ModSeq::new(observation.highest_modseq().get() + 1).unwrap(),
        observation.messages().clone(),
    )
    .unwrap();
    let altered = CopyEvidence::new(original.intent().clone(), changed).unwrap();
    {
        let mut checkpoints = repo
            .copy_checkpoints(
                &account,
                &store,
                &mut saved,
                CopySlot::Filing { run: &id, batch: 0 },
                &topology,
                &|_| Ok(()),
            )
            .unwrap();
        assert_eq!(checkpoints.save(&original), Err(Failure::Unstable));
        assert_eq!(checkpoints.save(&altered), Err(Failure::Unstable));
    }
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}
