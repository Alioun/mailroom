use super::compensation_records::{empty, flagged, prepared};
use super::*;
use crate::backlog::protocol::tests::Stage;

#[test]
fn cleanup_intent_and_observation_checkpoints_cannot_be_skipped_or_reversed() {
    let mut record = prepared(Stage::Copied);
    let initial = record.clone();
    let slot = CleanupSlot::Initial(0);
    let plan = record.cleanup_plan(slot).unwrap().clone();
    record
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    assert_eq!(
        record.checkpoint_change(&initial),
        Ok(CheckpointChange::Intent)
    );
    let intent = roundtrip(&record);
    let (ack, after) = flagged(plan.deletion());
    record.record_cleanup_store(slot, ack).unwrap();
    record.observe_cleanup_store(slot, after.clone()).unwrap();
    assert_eq!(
        record.checkpoint_change(&intent),
        Ok(CheckpointChange::Observation)
    );
    assert!(record.checkpoint_change(&initial).is_err());
    assert!(initial.checkpoint_change(&record).is_err());
    let marked = record.clone();
    record
        .begin_cleanup_expunge(slot, plan.retained().clone(), after.clone())
        .unwrap();
    assert_eq!(
        record.checkpoint_change(&marked),
        Ok(CheckpointChange::Intent)
    );
    let expunge = record.clone();
    record
        .record_cleanup_expunge(slot, Completion::Lost)
        .unwrap();
    record
        .observe_cleanup_final(slot, plan.retained().clone(), empty(&after))
        .unwrap();
    assert_eq!(
        record.checkpoint_change(&expunge),
        Ok(CheckpointChange::Observation)
    );
    assert!(record.checkpoint_change(&marked).is_err());
    assert!(record.checkpoint_change(&initial).is_err());
    assert_eq!(
        record.checkpoint_change(&record),
        Ok(CheckpointChange::Observation)
    );
}

#[test]
fn clear_dependencies_require_a_new_plan_approval_instead_of_an_observation_checkpoint() {
    let mut record = prepared(Stage::Marked);
    let initial = record.clone();
    let plan = record.preview().clear()[0].clear().clone();
    record.begin_clear(0, plan.before().clone()).unwrap();
    assert_eq!(
        record.checkpoint_change(&initial),
        Ok(CheckpointChange::Intent)
    );
    let intent = record.clone();
    let (ack, after) = flagged(&plan);
    record.record_clear(0, ack).unwrap();
    record.observe_clear(0, after.clone()).unwrap();
    assert_eq!(
        record.checkpoint_change(&intent),
        Ok(CheckpointChange::Observation)
    );
    assert!(record.checkpoint_change(&initial).is_err());
    let cleared = record.clone();
    record
        .prepare_after_clear(0, after, record.preview().destination().clone())
        .unwrap();
    assert!(record.continues(&cleared).is_ok());
    assert!(record.has_dependent_cleanup());
    assert!(record.checkpoint_change(&cleared).is_err());
    let approved = record.clone();
    let slot = record.cleanup_slots().next().unwrap();
    let plan = record.cleanup_plan(slot).unwrap().clone();
    record
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    assert_eq!(
        record.checkpoint_change(&approved),
        Ok(CheckpointChange::Intent)
    );
}

#[test]
fn one_checkpoint_cannot_authorize_multiple_independent_store_attempts() {
    let seed = prepared(Stage::Copied);
    let mut destination = seed.preview().destination().clone();
    destination.highest_modseq = seq(destination.highest_modseq.get() + 1);
    destination.messages.get_mut(&uid(64)).unwrap().modseq = destination.highest_modseq;
    let mut raw = serde_json::to_value(seed.forward()).unwrap();
    raw["readback"]["destination"] = serde_json::to_value(&destination).unwrap();
    let forward: CopyEvidence = serde_json::from_value(raw).unwrap();
    let preview = forward
        .compensation_preview(seed.preview().source().clone(), destination)
        .unwrap();
    let mut record = CompensationEvidence::new(forward, preview).unwrap();
    let initial = record.clone();
    let slots: Vec<_> = record.cleanup_slots().collect();
    assert_eq!(slots.len(), 2);
    for slot in slots {
        let plan = record.cleanup_plan(slot).unwrap().clone();
        record
            .begin_cleanup(slot, plan.retained(), plan.deletion().before())
            .unwrap();
    }
    assert!(record.continues(&initial).is_ok());
    assert!(record.checkpoint_change(&initial).is_err());
}

#[test]
fn restore_copy_requires_a_saved_unobserved_intent_before_its_completion() {
    let mut record = prepared(Stage::Filed);
    let initial = record.clone();
    let mut restore = record.restore().unwrap().clone();
    restore.begin_copy(&restore.before().clone()).unwrap();
    record.record_restore(restore.clone()).unwrap();
    assert_eq!(
        record.checkpoint_change(&initial),
        Ok(CheckpointChange::Intent)
    );
    let intent = record.clone();
    restore.record_copy_completion(Completion::Lost).unwrap();
    record.record_restore(restore).unwrap();
    assert_eq!(
        record.checkpoint_change(&intent),
        Ok(CheckpointChange::Observation)
    );
    assert!(record.checkpoint_change(&initial).is_err());
}
