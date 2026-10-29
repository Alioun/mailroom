use super::compensation_records::{empty, flagged, prepared};
use super::*;
use crate::backlog::protocol::tests::Stage;

fn cleared() -> CompensationEvidence {
    let mut record = prepared(Stage::Marked);
    let intent = record.preview().clear()[0].clear().clone();
    record.begin_clear(0, intent.before().clone()).unwrap();
    let (ack, after) = flagged(&intent);
    record.record_clear(0, ack).unwrap();
    record.observe_clear(0, after).unwrap();
    roundtrip(&record)
}

fn inputs(record: &CompensationEvidence) -> (Readback, Readback) {
    (
        record.clear(0).unwrap().readback().unwrap().clone(),
        record.preview().destination().clone(),
    )
}

fn derived() -> (CompensationEvidence, CleanupSlot) {
    let mut record = cleared();
    let (source, destination) = inputs(&record);
    let slots = record.prepare_after_clear(0, source, destination).unwrap();
    assert_eq!(slots, vec![CleanupSlot::AfterClear { clear: 0, group: 0 }]);
    (roundtrip(&record), slots[0])
}

fn mark(record: &mut CompensationEvidence, slot: CleanupSlot) -> Readback {
    let plan = record.cleanup_plan(slot).unwrap().clone();
    record
        .begin_cleanup(slot, plan.retained(), plan.deletion().before())
        .unwrap();
    *record = roundtrip(record);
    let (ack, after) = flagged(plan.deletion());
    record.record_cleanup_store(slot, ack).unwrap();
    *record = roundtrip(record);
    record.observe_cleanup_store(slot, after.clone()).unwrap();
    *record = roundtrip(record);
    after
}

#[test]
fn verified_clear_cleanup_reopens_every_checkpoint_and_keeps_immutable_forward_history() {
    let (mut record, slot) = derived();
    let forward = serde_json::to_vec(record.forward()).unwrap();
    let clear = record.clear(0).unwrap().clone();
    let initial = record.clone();
    let retained = record.cleanup_plan(slot).unwrap().retained().clone();
    assert_eq!(record.cleanup_next(slot), Ok(CleanupNext::Start));
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::Unattempted)
    );
    assert!(!record.fully_reversed());
    let after = mark(&mut record, slot);
    assert_eq!(record.cleanup_next(slot), Ok(CleanupNext::Expunge));
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::Pending)
    );
    let prior = record.clone();
    let command = record
        .begin_cleanup_expunge(slot, retained.clone(), after.clone())
        .unwrap();
    assert_eq!(command, "UID EXPUNGE 60,64");
    record = roundtrip(&record);
    assert!(record.continues(&prior).is_ok());
    assert_eq!(record.cleanup_next(slot), Ok(CleanupNext::ObserveExpunge));
    record
        .record_cleanup_expunge(slot, Completion::Lost)
        .unwrap();
    record = roundtrip(&record);
    record
        .observe_cleanup_final(slot, retained, empty(&after))
        .unwrap();
    record = roundtrip(&record);
    assert!(record.fully_reversed());
    assert_eq!(record.cleanup_next(slot), Ok(CleanupNext::Complete));
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::ExtraCopyRemoved)
    );
    assert_eq!(serde_json::to_vec(record.forward()).unwrap(), forward);
    assert_eq!(record.clear(0), Some(&clear));
    assert!(record.continues(&initial).is_ok());
    assert!(initial.continues(&record).is_err());
}

#[test]
fn missing_clear_acknowledgment_never_authorizes_dependent_cleanup() {
    for acknowledge in [false, true] {
        let mut record = prepared(Stage::Marked);
        let before = record.preview().clear()[0].clear().before().clone();
        let (source, destination) = (before.clone(), record.preview().destination().clone());
        assert!(record.prepare_after_clear(0, source, destination).is_err());
        record.begin_clear(0, before).unwrap();
        let (ack, after) = flagged(record.clear(0).unwrap().intent());
        if acknowledge {
            record.record_clear(0, ack).unwrap();
        } else {
            record.record_clear(0, StoreResult::Lost).unwrap();
        }
        let saved = serde_json::to_vec(&record).unwrap();
        assert!(
            record
                .prepare_after_clear(0, after.clone(), record.preview().destination().clone())
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&record).unwrap(), saved);
        record.observe_clear(0, after.clone()).unwrap();
        if acknowledge {
            assert!(
                !record
                    .prepare_after_clear(0, after, record.preview().destination().clone())
                    .unwrap()
                    .is_empty()
            );
        } else {
            let saved = serde_json::to_vec(&record).unwrap();
            assert!(
                record
                    .prepare_after_clear(0, after, record.preview().destination().clone())
                    .is_err()
            );
            assert_eq!(serde_json::to_vec(&record).unwrap(), saved);
        }
    }
}

#[test]
fn changed_or_missing_originals_and_copies_conflict_without_blocking_eligible_siblings() {
    for mutation in [
        "original-missing",
        "original-protected",
        "original-identity",
        "copy-missing",
        "copy-flag",
        "copy-version",
    ] {
        let mut record = cleared();
        let (mut source, mut destination) = inputs(&record);
        match mutation {
            "original-missing" => {
                source.messages.remove(&uid(4));
            }
            "original-protected" => {
                source
                    .messages
                    .get_mut(&uid(4))
                    .unwrap()
                    .metadata
                    .flags
                    .insert("\\Deleted".into());
            }
            "original-identity" => {
                source.messages.get_mut(&uid(4)).unwrap().metadata.size += 1;
            }
            "copy-missing" => {
                destination.messages.remove(&uid(60));
            }
            "copy-flag" => {
                destination
                    .messages
                    .get_mut(&uid(60))
                    .unwrap()
                    .metadata
                    .flags
                    .insert("$later".into());
            }
            "copy-version" => {
                destination.highest_modseq = seq(destination.highest_modseq.get() + 1);
                destination.messages.get_mut(&uid(60)).unwrap().modseq = destination.highest_modseq;
            }
            _ => unreachable!(),
        }
        let slots = record.prepare_after_clear(0, source, destination).unwrap();
        record = roundtrip(&record);
        assert_eq!(
            record.resolutions()[&uid(4)],
            CompensationResolution::Conflict,
            "{mutation}"
        );
        let slot = slots[0];
        assert_eq!(
            record.cleanup_plan(slot).unwrap().originals(),
            &[(uid(8), uid(64))].into()
        );
        let retained = record.cleanup_plan(slot).unwrap().retained().clone();
        let after = mark(&mut record, slot);
        assert_eq!(
            record
                .begin_cleanup_expunge(slot, retained.clone(), after.clone())
                .unwrap(),
            "UID EXPUNGE 64"
        );
        record
            .record_cleanup_expunge(slot, Completion::Lost)
            .unwrap();
        record
            .observe_cleanup_final(slot, retained, empty(&after))
            .unwrap();
        record = roundtrip(&record);
        assert_eq!(
            record.resolutions()[&uid(8)],
            CompensationResolution::ExtraCopyRemoved
        );
        assert!(!record.fully_reversed());
    }
}

#[test]
fn partial_clear_can_remove_only_its_acknowledged_subset() {
    let mut record = prepared(Stage::Marked);
    let intent = record.preview().clear()[0].clear().clone();
    record.begin_clear(0, intent.before().clone()).unwrap();
    let (mut ack, mut after) = flagged(&intent);
    if let StoreResult::Tagged {
        modified, updated, ..
    } = &mut ack
    {
        *modified = Some([uid(8)].into());
        updated.remove(&uid(8));
    } else {
        unreachable!();
    }
    after
        .messages
        .insert(uid(8), intent.before().messages[&uid(8)].clone());
    record.record_clear(0, ack).unwrap();
    record.observe_clear(0, after.clone()).unwrap();
    let slots = record
        .prepare_after_clear(0, after, record.preview().destination().clone())
        .unwrap();
    record = roundtrip(&record);
    assert_eq!(
        record.resolutions()[&uid(8)],
        CompensationResolution::Conflict
    );
    assert_eq!(
        record.cleanup_plan(slots[0]).unwrap().originals(),
        &[(uid(4), uid(60))].into()
    );
    assert_eq!(
        record.cleanup_plan(slots[0]).unwrap().deletion().command(),
        "UID STORE 60 (UNCHANGEDSINCE 9007199254741003) +FLAGS.SILENT (\\Deleted)"
    );
}

#[test]
fn stale_or_foreign_observations_and_wrong_cleanup_slots_leave_history_unchanged() {
    for mutation in [
        "source-validity",
        "destination-validity",
        "source-regression",
        "destination-regression",
    ] {
        let mut record = cleared();
        let before = serde_json::to_vec(&record).unwrap();
        let (mut source, mut destination) = inputs(&record);
        match mutation {
            "source-validity" => source.identity.uidvalidity = UidValidity::new(999).unwrap(),
            "destination-validity" => {
                destination.identity.uidvalidity = UidValidity::new(999).unwrap()
            }
            "source-regression" => source.highest_modseq = seq(1),
            "destination-regression" => destination.highest_modseq = seq(1),
            _ => unreachable!(),
        }
        assert!(
            record.prepare_after_clear(0, source, destination).is_err(),
            "{mutation}"
        );
        assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    }
    let (mut record, slot) = derived();
    let (source, destination) = inputs(&record);
    let before = serde_json::to_vec(&record).unwrap();
    assert!(
        record
            .prepare_after_clear(0, source.clone(), destination.clone())
            .is_err()
    );
    for invalid in [
        CleanupSlot::Initial(0),
        CleanupSlot::AfterClear { clear: 1, group: 0 },
        CleanupSlot::AfterClear { clear: 0, group: 1 },
    ] {
        assert!(
            record
                .begin_cleanup(invalid, &source, &destination)
                .is_err()
        );
        assert!(record.cleanup_plan(invalid).is_none());
        assert!(record.cleanup_next(invalid).is_err());
    }
    assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    assert_eq!(record.cleanup_next(slot), Ok(CleanupNext::Start));
}

#[test]
fn dependent_cleanup_cannot_repeat_effects_or_ignore_changed_retention() {
    let (mut record, slot) = derived();
    let after = mark(&mut record, slot);
    let retained = record.cleanup_plan(slot).unwrap().retained().clone();
    let before = serde_json::to_vec(&record).unwrap();
    let copy = record
        .cleanup_plan(slot)
        .unwrap()
        .deletion()
        .before()
        .clone();
    assert!(record.begin_cleanup(slot, &retained, &copy).is_err());
    let mut changed = retained.clone();
    changed
        .messages
        .get_mut(&uid(4))
        .unwrap()
        .metadata
        .flags
        .insert("$later".into());
    assert!(
        record
            .begin_cleanup_expunge(slot, changed, after.clone())
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    record
        .begin_cleanup_expunge(slot, retained.clone(), after.clone())
        .unwrap();
    assert!(
        record
            .begin_cleanup_expunge(slot, retained.clone(), after.clone())
            .is_err()
    );
    record
        .record_cleanup_expunge(slot, Completion::Lost)
        .unwrap();
    let mut missing = retained;
    missing.messages.remove(&uid(4));
    record
        .observe_cleanup_final(slot, missing, empty(&after))
        .unwrap();
    record = roundtrip(&record);
    assert_eq!(
        record.resolutions()[&uid(4)],
        CompensationResolution::Conflict
    );
    assert_eq!(
        record.resolutions()[&uid(8)],
        CompensationResolution::ExtraCopyRemoved
    );
    assert!(!record.fully_reversed());
}

#[test]
fn altered_dependencies_plans_and_effect_records_fail_decoding_without_panics() {
    let (record, slot) = derived();
    for mutation in [
        "clear-index",
        "missing-clear",
        "missing-ack",
        "duplicate-slot",
        "plan-target",
        "retained-flags",
        "destination-version",
        "extra-slot",
        "unknown-field",
        "conflict-substitution",
    ] {
        let mut raw = serde_json::to_value(&record).unwrap();
        match mutation {
            "clear-index" => raw["after_clear"][0]["clear"] = 99.into(),
            "missing-clear" => raw["clear"][0] = serde_json::Value::Null,
            "missing-ack" => raw["clear"][0]["effect"]["result"] = serde_json::Value::Null,
            "duplicate-slot" => {
                let entry = raw["after_clear"][0].clone();
                raw["after_clear"].as_array_mut().unwrap().push(entry);
            }
            "plan-target" => raw["after_clear"][0]["plans"][0]["originals"]["4"] = 99.into(),
            "retained-flags" => {
                raw["after_clear"][0]["plans"][0]["retained"]["messages"]["4"]["metadata"]["flags"] =
                    serde_json::json!(["$different"])
            }
            "destination-version" => {
                raw["after_clear"][0]["destination"]["highest_modseq"] = "1".into()
            }
            "extra-slot" => raw["after_clear"][0]["cleanup"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"deletion":null,"retained":null,"final_retained":null})),
            "unknown-field" => raw["after_clear"][0]["authority"] = true.into(),
            "conflict-substitution" => {
                raw["after_clear"][0]["conflicts"] = serde_json::json!([4, 4])
            }
            _ => unreachable!(),
        }
        let decoded =
            std::panic::catch_unwind(|| serde_json::from_value::<CompensationEvidence>(raw));
        assert!(decoded.is_ok(), "decoder panicked for {mutation}");
        assert!(decoded.unwrap().is_err(), "{mutation}");
    }
    let mut record = record;
    let old = record.clone();
    mark(&mut record, slot);
    assert!(record.continues(&old).is_ok());
    assert!(old.continues(&record).is_err());
}

#[test]
fn later_original_flags_are_retained_and_clear_observations_freeze_before_cleanup() {
    let mut record = cleared();
    let (mut source, destination) = inputs(&record);
    source.highest_modseq = seq(source.highest_modseq.get() + 10);
    let original = source.messages.get_mut(&uid(4)).unwrap();
    original.modseq = source.highest_modseq;
    original.metadata.flags.insert("$later".into());
    let clear = record.clear(0).unwrap().clone();
    let slot = record
        .prepare_after_clear(0, source.clone(), destination)
        .unwrap()[0];
    let before = serde_json::to_vec(&record).unwrap();
    assert!(record.observe_clear(0, source.clone()).is_err());
    assert!(
        record
            .record_clear(0, clear.result().unwrap().clone())
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    let retained = record.cleanup_plan(slot).unwrap().retained().clone();
    assert_eq!(retained.messages[&uid(4)], source.messages[&uid(4)]);
    let after = mark(&mut record, slot);
    record
        .begin_cleanup_expunge(slot, retained.clone(), after.clone())
        .unwrap();
    record
        .record_cleanup_expunge(slot, Completion::Lost)
        .unwrap();
    record
        .observe_cleanup_final(slot, retained, empty(&after))
        .unwrap();
    record = roundtrip(&record);
    assert!(record.fully_reversed());
    assert_eq!(record.clear(0), Some(&clear));
}

#[test]
fn continuation_cannot_change_clear_evidence_after_cleanup_depends_on_it() {
    let mut record = cleared();
    let (mut source, destination) = inputs(&record);
    source.highest_modseq = seq(source.highest_modseq.get() + 10);
    record
        .prepare_after_clear(0, source.clone(), destination)
        .unwrap();
    let mut raw = serde_json::to_value(&record).unwrap();
    let next = record
        .clear(0)
        .unwrap()
        .readback()
        .unwrap()
        .highest_modseq
        .get()
        + 1;
    raw["clear"][0]["effect"]["readback"]["highest_modseq"] = next.to_string().into();
    let changed: CompensationEvidence = serde_json::from_value(raw).unwrap();
    assert!(changed.continues(&record).is_err());
    assert_eq!(record.cleanup_slots().count(), 1);
}
