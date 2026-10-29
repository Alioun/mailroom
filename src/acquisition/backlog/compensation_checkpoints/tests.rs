use super::*;
use crate::{
    acquisition::backlog::compensation_review::tests::fixture as review_fixture,
    backlog::{
        imap_copy::{CleanupSlot, FlagChange, FlagIntent, ModSeq, Readback, StoreResult},
        protocol::CompensationPhase as Phase,
    },
};
use std::{
    cell::Cell,
    sync::{Arc, atomic::Ordering},
};

fn fixture(
    phase: Phase,
    work: CopyCompensationWork,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
) {
    let (root, repo, account, store, mut saved, request) = review_fixture(phase);
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
    if work == CopyCompensationWork::Execute {
        saved.protocol_backlog.runs[0].status = Status::UndoRunning;
        saved.protocol_backlog.runs[0].stop_requested = false;
        store.save_state(&mut saved).unwrap();
    }
    (root, repo, account, store, saved)
}

fn flagged(intent: &FlagIntent) -> (StoreResult, Readback) {
    let highest = ModSeq::new(intent.before().highest_modseq().get() + 10).unwrap();
    let mut messages = intent.before().messages().clone();
    for message in messages.values_mut() {
        match intent.change() {
            FlagChange::AddDeleted => {
                message.metadata.flags.insert("\\Deleted".into());
            }
            FlagChange::RemoveOwnedDeleted => message
                .metadata
                .flags
                .retain(|f| !f.eq_ignore_ascii_case("\\Deleted")),
        }
        message.modseq = highest;
    }
    let result = StoreResult::tagged(
        TaggedStatus::Ok,
        None,
        messages.keys().map(|u| (*u, highest)).collect(),
        intent,
    )
    .unwrap();
    (
        result,
        Readback::new(intent.before().identity().clone(), highest, messages).unwrap(),
    )
}

fn scope(
    saved: &AccountState,
    work: CopyCompensationWork,
) -> (
    String,
    Inventory,
    CompensationEvidence,
    CopyCompensationWork,
) {
    let run = &saved.protocol_backlog.runs[0];
    (
        run.id.clone(),
        run.plan.topology.clone(),
        run.compensation[0].evidence.clone(),
        work,
    )
}

fn exact_saved(store: &EncryptedStore, expected: &CompensationEvidence) {
    let reopened = store.load_state().unwrap();
    assert_eq!(
        &reopened.protocol_backlog.runs[0].compensation[0].evidence,
        expected
    );
}

#[test]
fn cleanup_tokens_follow_synced_intents_and_every_observation_reopens_exactly() {
    let (_root, mut repo, account, store, mut saved) =
        fixture(Phase::Prepared, CopyCompensationWork::Execute);
    let accesses = repo.track_credential_accesses();
    let (id, topology, mut evidence, work) = scope(&saved, CopyCompensationWork::Execute);
    let fresh = |_: &AccountState| Ok(());
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work,
            },
            &topology,
            &fresh,
        )
        .unwrap();
    let initial = evidence.clone();
    let slot = CleanupSlot::Initial(0);
    let plan = evidence.cleanup_plan(slot).unwrap().clone();
    evidence
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert_eq!(writer.observe(&evidence), Err(Failure::Unsupported));
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    let token = writer.persist_intent(&evidence).unwrap();
    assert_eq!(token.evidence(), &evidence);
    assert_eq!(
        token.location(),
        (&account.id, id.as_str(), 0, writer.saved.revision)
    );
    exact_saved(&store, token.evidence());
    assert!(writer.persist_intent(&evidence).is_err());
    let (ack, after) = flagged(plan.deletion());
    evidence.record_cleanup_store(slot, ack).unwrap();
    evidence.observe_cleanup_store(slot, after.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    let unchanged = std::fs::read(store.path()).unwrap();
    assert!(writer.observe(&initial).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), unchanged);
    evidence
        .begin_cleanup_expunge(slot, plan.retained().clone(), after.clone())
        .unwrap();
    let token = writer.persist_intent(&evidence).unwrap();
    exact_saved(&store, token.evidence());
    evidence
        .record_cleanup_expunge(slot, Completion::Lost)
        .unwrap();
    let empty = Readback::new(
        after.identity().clone(),
        after.highest_modseq(),
        Default::default(),
    )
    .unwrap();
    evidence
        .observe_cleanup_final(slot, plan.retained().clone(), empty)
        .unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].status,
        Status::Undone
    );
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
}

#[test]
fn clear_intent_and_readback_persist_but_a_new_cleanup_dependency_stays_held() {
    let (_root, repo, account, store, mut saved) =
        fixture(Phase::Cleared, CopyCompensationWork::Execute);
    let (id, topology, mut evidence, work) = scope(&saved, CopyCompensationWork::Execute);
    let fresh = |_: &AccountState| Ok(());
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work,
            },
            &topology,
            &fresh,
        )
        .unwrap();
    let plan = evidence.preview().clear()[0].clear().clone();
    evidence.begin_clear(0, plan.before().clone()).unwrap();
    writer.persist_intent(&evidence).unwrap();
    exact_saved(&store, &evidence);
    let (ack, after) = flagged(&plan);
    evidence.record_clear(0, ack).unwrap();
    evidence.observe_clear(0, after.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].status,
        Status::Stopped
    );
    let before = std::fs::read(store.path()).unwrap();
    evidence
        .prepare_after_clear(0, after, evidence.preview().destination().clone())
        .unwrap();
    assert!(writer.observe(&evidence).is_err());
    assert!(writer.persist_intent(&evidence).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn restore_copy_store_and_expunge_each_have_a_durable_intent_boundary() {
    let (_root, repo, account, store, mut saved) =
        fixture(Phase::RestorePrepared, CopyCompensationWork::Execute);
    let (id, topology, mut evidence, work) = scope(&saved, CopyCompensationWork::Execute);
    let fresh = |_: &AccountState| Ok(());
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work,
            },
            &topology,
            &fresh,
        )
        .unwrap();
    let mut restore = evidence.restore().unwrap().clone();
    let before = restore.before().clone();
    restore.begin_copy(&before).unwrap();
    evidence.record_restore(restore.clone()).unwrap();
    writer.persist_intent(&evidence).unwrap();
    exact_saved(&store, &evidence);
    restore
        .record_mapping(
            CopyUidMapping::parse("[COPYUID 101 60,64 80,84]", restore.intent()).unwrap(),
        )
        .unwrap();
    restore.record_copy_completion(Completion::Lost).unwrap();
    let destination = Readback::new(
        restore.intent().destination().clone(),
        ModSeq::new(before.highest_modseq().get() + 100).unwrap(),
        [
            (
                Uid::new(80).unwrap(),
                before.messages()[&Uid::new(60).unwrap()].clone(),
            ),
            (
                Uid::new(84).unwrap(),
                before.messages()[&Uid::new(64).unwrap()].clone(),
            ),
        ]
        .into(),
    )
    .unwrap();
    restore
        .observe_copy(before.clone(), destination.clone())
        .unwrap();
    evidence.record_restore(restore.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    let group = restore
        .prepare_deletions(&before, &destination)
        .unwrap()
        .remove(0);
    let intent = group.intent().clone();
    let index = restore.begin_delete(group).unwrap();
    evidence.record_restore(restore.clone()).unwrap();
    writer.persist_intent(&evidence).unwrap();
    exact_saved(&store, &evidence);
    let (ack, after) = flagged(&intent);
    restore.record_store_result(index, ack).unwrap();
    restore.observe_store(index, after.clone()).unwrap();
    evidence.record_restore(restore.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    restore
        .begin_expunge(index, after.clone(), destination.clone())
        .unwrap();
    evidence.record_restore(restore.clone()).unwrap();
    writer.persist_intent(&evidence).unwrap();
    exact_saved(&store, &evidence);
    restore
        .record_expunge_completion(index, Completion::Lost)
        .unwrap();
    let empty = Readback::new(
        after.identity().clone(),
        after.highest_modseq(),
        Default::default(),
    )
    .unwrap();
    restore.observe_final(index, empty, destination).unwrap();
    evidence.record_restore(restore).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].status,
        Status::Undone
    );
}

#[test]
fn observations_survive_freshness_loss_but_new_effects_do_not() {
    let (_root, repo, account, store, mut saved) =
        fixture(Phase::Prepared, CopyCompensationWork::Execute);
    let (id, topology, mut evidence, work) = scope(&saved, CopyCompensationWork::Execute);
    let current = Cell::new(true);
    let fresh = |_: &AccountState| {
        if current.get() {
            Ok(())
        } else {
            Err("changed context".into())
        }
    };
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work,
            },
            &topology,
            &fresh,
        )
        .unwrap();
    let slot = CleanupSlot::Initial(0);
    let plan = evidence.cleanup_plan(slot).unwrap().clone();
    evidence
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    writer.persist_intent(&evidence).unwrap();
    current.set(false);
    let (ack, after) = flagged(plan.deletion());
    evidence.record_cleanup_store(slot, ack).unwrap();
    evidence.observe_cleanup_store(slot, after.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    evidence
        .begin_cleanup_expunge(slot, plan.retained().clone(), after)
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert!(writer.persist_intent(&evidence).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn reconcile_can_read_pending_effects_but_never_persist_another_intent() {
    let (_root, repo, account, store, mut saved) =
        fixture(Phase::Prepared, CopyCompensationWork::Execute);
    let (id, topology, mut evidence, _) = scope(&saved, CopyCompensationWork::Execute);
    let fresh = |_: &AccountState| Ok(());
    let slot = CleanupSlot::Initial(0);
    let plan = evidence.cleanup_plan(slot).unwrap().clone();
    evidence
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    {
        let mut writer = repo
            .copy_compensation_checkpoints(
                &account,
                &store,
                &mut saved,
                CopyCompensationSlot {
                    run: &id,
                    forward: 0,
                    work: CopyCompensationWork::Execute,
                },
                &topology,
                &fresh,
            )
            .unwrap();
        writer.persist_intent(&evidence).unwrap();
    }
    let mut reopened = store.load_state().unwrap();
    let mut writer = repo
        .copy_compensation_checkpoints(
            &account,
            &store,
            &mut reopened,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work: CopyCompensationWork::Reconcile,
            },
            &topology,
            &fresh,
        )
        .unwrap();
    let (ack, after) = flagged(plan.deletion());
    evidence.record_cleanup_store(slot, ack).unwrap();
    evidence.observe_cleanup_store(slot, after.clone()).unwrap();
    writer.observe(&evidence).unwrap();
    exact_saved(&store, &evidence);
    assert_eq!(
        writer.saved.protocol_backlog.runs[0].status,
        Status::Stopped
    );
    evidence
        .begin_cleanup_expunge(slot, plan.retained().clone(), after)
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert_eq!(
        writer.persist_intent(&evidence).err(),
        Some(Failure::Unsupported)
    );
    assert!(writer.observe(&evidence).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn stale_or_substituted_account_state_never_admits_a_writer_or_overwrites_history() {
    for mutation in [
        "memory-approval",
        "memory-census",
        "revision",
        "source",
        "production",
        "workspace",
        "topology",
        "stopped",
    ] {
        let (_root, mut repo, account, store, mut saved) =
            fixture(Phase::Prepared, CopyCompensationWork::Execute);
        let (id, mut topology, _, work) = scope(&saved, CopyCompensationWork::Execute);
        match mutation {
            "memory-approval" => saved.protocol_backlog.runs[0].undo_receipt = Some("e".repeat(64)),
            "memory-census" => {
                saved
                    .census
                    .confirmed_own_addresses
                    .insert("invented@example.test".into());
            }
            "revision" => {
                let mut concurrent = store.load_state().unwrap();
                store.save_state(&mut concurrent).unwrap();
            }
            "source" => Arc::make_mut(repo.backlog_source.as_mut().unwrap())
                .epoch
                .push('x'),
            "production" => repo.backlog_source = None,
            "workspace" => repo.install_synthetic_workspace(&account, "changed"),
            "topology" => {
                topology.capabilities.insert("MOVE".into());
            }
            "stopped" => saved.protocol_backlog.runs[0].stop_requested = true,
            _ => unreachable!(),
        };
        let accesses = repo.track_credential_accesses();
        let before = std::fs::read(store.path()).unwrap();
        let state = serde_json::to_vec(&saved).unwrap();
        assert!(
            repo.copy_compensation_checkpoints(
                &account,
                &store,
                &mut saved,
                CopyCompensationSlot {
                    run: &id,
                    forward: 0,
                    work
                },
                &topology,
                &|_| Ok(())
            )
            .is_err(),
            "{mutation}"
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(serde_json::to_vec(&saved).unwrap(), state);
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn an_unsaved_execute_status_cannot_replace_a_durable_stopped_run() {
    let (_root, repo, account, store, mut saved) =
        fixture(Phase::Prepared, CopyCompensationWork::Execute);
    let (id, topology, _, work) = scope(&saved, CopyCompensationWork::Execute);
    let mut stopped = saved.clone();
    stopped.protocol_backlog.runs[0].status = Status::Stopped;
    stopped.protocol_backlog.runs[0].stop_requested = true;
    store.save_state(&mut stopped).unwrap();
    saved.revision = stopped.revision;
    assert!(
        repo.copy_compensation_checkpoints(
            &account,
            &store,
            &mut saved,
            CopyCompensationSlot {
                run: &id,
                forward: 0,
                work
            },
            &topology,
            &|_| Ok(())
        )
        .is_err()
    );
}

#[test]
fn concurrent_revision_and_expiry_prevent_new_intents_without_discarding_local_history() {
    for mutation in ["concurrent", "expiry"] {
        let (_root, repo, account, store, mut saved) =
            fixture(Phase::Prepared, CopyCompensationWork::Execute);
        if mutation == "expiry" {
            saved.protocol_backlog.runs[0].completed_at = Some(repo.now());
            store.save_state(&mut saved).unwrap();
        }
        let clock = repo.clock.as_ref().unwrap().clone();
        let (id, topology, mut evidence, work) = scope(&saved, CopyCompensationWork::Execute);
        let fresh = |_: &AccountState| Ok(());
        let mut writer = repo
            .copy_compensation_checkpoints(
                &account,
                &store,
                &mut saved,
                CopyCompensationSlot {
                    run: &id,
                    forward: 0,
                    work,
                },
                &topology,
                &fresh,
            )
            .unwrap();
        if mutation == "concurrent" {
            let mut concurrent = store.load_state().unwrap();
            store.save_state(&mut concurrent).unwrap();
        } else {
            clock.fetch_add(crate::backlog::retention::WINDOW_SECS, Ordering::SeqCst);
        }
        let slot = CleanupSlot::Initial(0);
        let plan = evidence.cleanup_plan(slot).unwrap().clone();
        evidence
            .begin_cleanup(slot, plan.retained(), plan.deletion().before())
            .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let state = serde_json::to_vec(&*writer.saved).unwrap();
        assert!(writer.persist_intent(&evidence).is_err(), "{mutation}");
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(serde_json::to_vec(&*writer.saved).unwrap(), state);
    }
}
