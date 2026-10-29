use super::*;
use crate::{
    backlog::protocol::tests::{Stage, run},
    census::AccountId,
};

fn prepared(stage: Stage) -> CompensationEvidence {
    let e = run(&AccountId::new("compensation-records").unwrap(), stage).batches[0]
        .as_copy()
        .unwrap()
        .clone();
    let source = match stage {
        Stage::Copied => e.before.clone(),
        Stage::Marked => e.deletions[0].evidence.flag.readback.clone().unwrap(),
        Stage::Filed => e.deletions[0]
            .evidence
            .expunge
            .as_ref()
            .unwrap()
            .readback
            .clone()
            .unwrap(),
        _ => unreachable!(),
    };
    let destination = e.readback.as_ref().unwrap().destination.clone();
    let preview = e.compensation_preview(source, destination).unwrap();
    CompensationEvidence::new(e, preview).unwrap()
}

fn flagged(intent: &FlagIntent) -> (StoreResult, Readback) {
    let mut after = intent.before().clone();
    after.highest_modseq = seq(after.highest_modseq.get() + 10);
    for current in after.messages.values_mut() {
        match intent.change() {
            FlagChange::AddDeleted => {
                current.metadata.flags.insert("\\Deleted".into());
            }
            FlagChange::RemoveOwnedDeleted => {
                current
                    .metadata
                    .flags
                    .retain(|f| !f.eq_ignore_ascii_case("\\Deleted"));
            }
        }
        current.modseq = after.highest_modseq;
    }
    let result = acknowledged(
        intent,
        &after
            .messages
            .keys()
            .map(|u| (u.get(), after.highest_modseq.get()))
            .collect::<Vec<_>>(),
    );
    (result, after)
}

fn empty(read: &Readback) -> Readback {
    Readback::new(
        read.identity.clone(),
        seq(read.highest_modseq.get() + 10),
        BTreeMap::new(),
    )
    .unwrap()
}

fn mark_cleanup(record: &mut CompensationEvidence, index: usize) -> Readback {
    let plan = record.preview().cleanup()[index].clone();
    let command = record
        .begin_cleanup(index, plan.retained(), plan.deletion().before())
        .unwrap();
    assert_eq!(command, plan.deletion().command());
    *record = roundtrip(record);
    assert_eq!(record.cleanup_next(index), Ok(CleanupNext::ObserveStore));
    let (ack, after) = flagged(plan.deletion());
    record.record_cleanup_store(index, ack).unwrap();
    *record = roundtrip(record);
    record.observe_cleanup_store(index, after.clone()).unwrap();
    *record = roundtrip(record);
    after
}

#[test]
fn each_cleanup_checkpoint_reopens_without_repeating_store_or_expunge() {
    let mut record = prepared(Stage::Copied);
    assert_eq!(record.preview().cleanup().len(), 1);
    let retained = record.preview().cleanup()[0].retained().clone();
    let initial = record.clone();
    let after = mark_cleanup(&mut record, 0);
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Expunge));
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::Pending)
    );
    let before_retry = serde_json::to_vec(&record).unwrap();
    let target = record.preview().cleanup()[0].deletion().before().clone();
    assert!(record.begin_cleanup(0, &retained, &target).is_err());
    assert_eq!(serde_json::to_vec(&record).unwrap(), before_retry);
    assert_eq!(
        record
            .begin_cleanup_expunge(0, retained.clone(), after.clone())
            .unwrap(),
        "UID EXPUNGE 60,64"
    );
    record = roundtrip(&record);
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::ObserveExpunge));
    assert!(
        record
            .begin_cleanup_expunge(0, retained.clone(), after.clone())
            .is_err()
    );
    record.record_cleanup_expunge(0, Completion::Lost).unwrap();
    record = roundtrip(&record);
    record
        .observe_cleanup_final(0, retained.clone(), empty(&after))
        .unwrap();
    record = roundtrip(&record);
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Complete));
    assert!(record.fully_reversed());
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::ExtraCopyRemoved)
    );
    assert_eq!(record.preview().cleanup()[0].retained(), &retained);
    assert!(record.continues(&initial).is_ok());
    assert!(initial.continues(&record).is_err());
}

#[test]
fn a_verified_clear_is_distinct_from_completed_full_undo() {
    let mut record = prepared(Stage::Marked);
    assert_eq!(record.preview().clear().len(), 1);
    let plan = record.preview().clear()[0].clone();
    let initial = record.clone();
    assert_eq!(
        record
            .begin_clear(0, plan.clear().before().clone())
            .unwrap(),
        plan.clear().command()
    );
    record = roundtrip(&record);
    assert_eq!(record.clear_next(0), Ok(ClearNext::ObserveStore));
    assert!(
        record
            .begin_clear(0, plan.clear().before().clone())
            .is_err()
    );
    let (ack, after) = flagged(plan.clear());
    record.record_clear(0, ack).unwrap();
    record = roundtrip(&record);
    record.observe_clear(0, after).unwrap();
    record = roundtrip(&record);
    assert_eq!(record.clear_next(0), Ok(ClearNext::Complete));
    assert!(
        record
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::DeletedCleared)
    );
    assert!(!record.fully_reversed());
    assert!(record.continues(&initial).is_ok());
    assert!(initial.continues(&record).is_err());
}

#[test]
fn partial_cleanup_store_allows_only_the_certified_subset_to_finish() {
    let mut record = prepared(Stage::Copied);
    let plan = record.preview().cleanup()[0].clone();
    record
        .begin_cleanup(0, plan.retained(), plan.deletion().before())
        .unwrap();
    let (mut ack, mut after) = flagged(plan.deletion());
    if let StoreResult::Tagged {
        modified, updated, ..
    } = &mut ack
    {
        *modified = Some([uid(64)].into());
        updated.remove(&uid(64));
    } else {
        unreachable!();
    }
    after
        .messages
        .insert(uid(64), plan.deletion().before().messages[&uid(64)].clone());
    record.record_cleanup_store(0, ack).unwrap();
    record.observe_cleanup_store(0, after.clone()).unwrap();
    record = roundtrip(&record);
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Expunge));
    assert_eq!(
        record.resolutions()[&uid(8)],
        CompensationResolution::Conflict
    );
    let retained = plan.retained().subset(&[uid(4)].into());
    let target = after.subset(&[uid(60)].into());
    assert_eq!(
        record
            .begin_cleanup_expunge(0, retained.clone(), target.clone())
            .unwrap(),
        "UID EXPUNGE 60"
    );
    record = roundtrip(&record);
    record.record_cleanup_expunge(0, Completion::Lost).unwrap();
    record
        .observe_cleanup_final(0, retained, empty(&target))
        .unwrap();
    record = roundtrip(&record);
    assert_eq!(
        record.resolutions()[&uid(4)],
        CompensationResolution::ExtraCopyRemoved
    );
    assert_eq!(
        record.resolutions()[&uid(8)],
        CompensationResolution::Conflict
    );
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Conflict));
    assert!(!record.fully_reversed());
}

#[test]
fn lost_store_acknowledgments_never_establish_cleanup_or_clear_ownership() {
    let mut cleanup = prepared(Stage::Copied);
    let plan = cleanup.preview().cleanup()[0].clone();
    cleanup
        .begin_cleanup(0, plan.retained(), plan.deletion().before())
        .unwrap();
    cleanup = roundtrip(&cleanup);
    let (_, after) = flagged(plan.deletion());
    cleanup.record_cleanup_store(0, StoreResult::Lost).unwrap();
    cleanup.observe_cleanup_store(0, after.clone()).unwrap();
    cleanup = roundtrip(&cleanup);
    assert_eq!(cleanup.cleanup_next(0), Ok(CleanupNext::Conflict));
    assert!(
        cleanup
            .begin_cleanup_expunge(0, plan.retained().clone(), after)
            .is_err()
    );
    assert!(!cleanup.fully_reversed());
    let mut clear = prepared(Stage::Marked);
    let plan = clear.preview().clear()[0].clone();
    clear.begin_clear(0, plan.clear().before().clone()).unwrap();
    clear = roundtrip(&clear);
    clear.record_clear(0, StoreResult::Lost).unwrap();
    let (_, after) = flagged(plan.clear());
    clear.observe_clear(0, after).unwrap();
    clear = roundtrip(&clear);
    assert_eq!(clear.clear_next(0), Ok(ClearNext::Conflict));
    assert!(
        clear
            .resolutions()
            .values()
            .all(|r| *r == CompensationResolution::Conflict)
    );
    assert!(!clear.fully_reversed());
}

#[test]
fn changed_or_missing_originals_and_changed_copies_deny_cleanup_before_expunge() {
    for mutation in [
        "original-flag",
        "original-version",
        "original-missing",
        "copy-flag",
        "copy-version",
    ] {
        let mut record = prepared(Stage::Copied);
        let mut destination = mark_cleanup(&mut record, 0);
        let mut source = record.preview().cleanup()[0].retained().clone();
        match mutation {
            "original-flag" => {
                source
                    .messages
                    .get_mut(&uid(4))
                    .unwrap()
                    .metadata
                    .flags
                    .insert("$later".into());
            }
            "original-version" => {
                source.highest_modseq = seq(source.highest_modseq.get() + 1);
                source.messages.get_mut(&uid(4)).unwrap().modseq = source.highest_modseq;
            }
            "original-missing" => {
                source.messages.remove(&uid(4));
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
        let before = serde_json::to_vec(&record).unwrap();
        assert!(
            record
                .begin_cleanup_expunge(0, source, destination)
                .is_err(),
            "{mutation}"
        );
        assert_eq!(serde_json::to_vec(&record).unwrap(), before);
        assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Expunge));
    }
}

#[test]
fn cleanup_final_observation_records_missing_originals_as_conflict_without_retry() {
    let mut record = prepared(Stage::Copied);
    let after = mark_cleanup(&mut record, 0);
    let retained = record.preview().cleanup()[0].retained().clone();
    record
        .begin_cleanup_expunge(0, retained.clone(), after.clone())
        .unwrap();
    record.record_cleanup_expunge(0, Completion::Lost).unwrap();
    record = roundtrip(&record);
    let mut changed = retained.clone();
    changed.messages.remove(&uid(4));
    record
        .observe_cleanup_final(0, changed, empty(&after))
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
    assert_eq!(record.cleanup_next(0), Ok(CleanupNext::Conflict));
    assert!(!record.fully_reversed());
    assert!(record.begin_cleanup_expunge(0, retained, after).is_err());
}

#[test]
fn substituted_compensation_plans_slots_and_owned_effects_fail_decoding() {
    let mut record = prepared(Stage::Copied);
    let after = mark_cleanup(&mut record, 0);
    let retained = record.preview().cleanup()[0].retained().clone();
    record.begin_cleanup_expunge(0, retained, after).unwrap();
    for mutation in [
        "semantics",
        "preview-target",
        "preview-retained",
        "slot",
        "store-target",
        "retained",
        "unknown",
    ] {
        let mut raw = serde_json::to_value(&record).unwrap();
        match mutation {
            "semantics" => raw["semantics"] = "imap-copy-compensation/v2".into(),
            "preview-target" => {
                raw["preview"]["cleanup"][0]["originals"]["4"] = serde_json::json!(99)
            }
            "preview-retained" => {
                raw["preview"]["cleanup"][0]["retained"]["messages"]["4"]["modseq"] = "1".into()
            }
            "slot" => {
                let extra = raw["cleanup"][0].clone();
                raw["cleanup"].as_array_mut().unwrap().push(extra);
            }
            "store-target" => {
                raw["cleanup"][0]["deletion"]["flag"]["intent"]["before"]["identity"]["uidvalidity"] =
                    serde_json::json!(999)
            }
            "retained" => {
                raw["cleanup"][0]["retained"]["messages"]["4"]["metadata"]["flags"] =
                    serde_json::json!(["\\Seen", "$changed"])
            }
            "unknown" => raw["authority"] = true.into(),
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<CompensationEvidence>(raw).is_err(),
            "{mutation}"
        );
    }
    let (forward, index) = marked();
    let mut source = source();
    source
        .messages
        .extend(forward.deletions[index].evidence.flag.verified());
    source.highest_modseq = seq(911);
    let preview = forward.compensation_preview(source, destination()).unwrap();
    let mut record = CompensationEvidence::new(forward, preview).unwrap();
    for i in 0..record.preview().clear().len() {
        record
            .begin_clear(i, record.preview().clear()[i].clear().before().clone())
            .unwrap();
    }
    let mut raw = serde_json::to_value(record).unwrap();
    raw["clear"].as_array_mut().unwrap().swap(0, 1);
    assert!(serde_json::from_value::<CompensationEvidence>(raw).is_err());
}

#[test]
fn compensation_replay_rejects_regressed_history_and_omitted_effect_evidence() {
    let mut record = prepared(Stage::Copied);
    let initial = record.clone();
    let after = mark_cleanup(&mut record, 0);
    let marked = record.clone();
    let retained = record.preview().cleanup()[0].retained().clone();
    record
        .begin_cleanup_expunge(0, retained.clone(), after.clone())
        .unwrap();
    let attempted = roundtrip(&record);
    record.record_cleanup_expunge(0, Completion::Lost).unwrap();
    record
        .observe_cleanup_final(0, retained, empty(&after))
        .unwrap();
    let completed = roundtrip(&record);
    for prior in [&initial, &marked, &attempted] {
        assert!(completed.continues(prior).is_ok());
        assert!(prior.continues(&completed).is_err());
    }
    let mut raw = serde_json::to_value(&completed).unwrap();
    raw["cleanup"][0]["retained"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<CompensationEvidence>(raw).is_err());
    let mut raw = serde_json::to_value(&completed).unwrap();
    raw["cleanup"][0]["final_retained"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<CompensationEvidence>(raw).is_err());
    let mut raw = serde_json::to_value(&initial).unwrap();
    raw["cleanup"] = serde_json::Value::Array(vec![raw["cleanup"][0].clone(); LIMIT + 1]);
    assert!(serde_json::from_value::<CompensationEvidence>(raw).is_err());
}

#[test]
fn restore_history_must_continue_the_exact_preview_and_keep_current_flags() {
    let mut record = prepared(Stage::Filed);
    let initial = record.clone();
    let mut restore = record.restore().unwrap().clone();
    let before = restore.before().clone();
    restore.begin_copy(&before).unwrap();
    record.record_restore(restore.clone()).unwrap();
    record = roundtrip(&record);
    assert!(!record.fully_reversed());
    restore
        .record_mapping(
            CopyUidMapping::parse("[COPYUID 101 60,64 80,84]", restore.intent()).unwrap(),
        )
        .unwrap();
    restore.record_copy_completion(Completion::Lost).unwrap();
    let destination = Readback::new(
        restore.intent().destination().clone(),
        seq(before.highest_modseq.get() + 100),
        [
            (uid(80), before.messages[&uid(60)].clone()),
            (uid(84), before.messages[&uid(64)].clone()),
        ]
        .into(),
    )
    .unwrap();
    restore
        .observe_copy(before.clone(), destination.clone())
        .unwrap();
    record.record_restore(restore.clone()).unwrap();
    record = roundtrip(&record);
    let group = restore
        .prepare_deletions(&before, &destination)
        .unwrap()
        .remove(0);
    let intent = group.intent().clone();
    let index = restore.begin_delete(group).unwrap();
    let (ack, after) = flagged(&intent);
    restore.record_store_result(index, ack).unwrap();
    restore.observe_store(index, after.clone()).unwrap();
    record.record_restore(restore.clone()).unwrap();
    record = roundtrip(&record);
    restore
        .begin_expunge(index, after.clone(), destination.clone())
        .unwrap();
    record.record_restore(restore.clone()).unwrap();
    record = roundtrip(&record);
    restore
        .record_expunge_completion(index, Completion::Lost)
        .unwrap();
    restore
        .observe_final(index, empty(&after), destination)
        .unwrap();
    record.record_restore(restore).unwrap();
    record = roundtrip(&record);
    assert!(record.fully_reversed());
    assert_eq!(
        record.resolutions(),
        [
            (
                uid(4),
                CompensationResolution::Restored {
                    destination: uid(80)
                }
            ),
            (
                uid(8),
                CompensationResolution::Restored {
                    destination: uid(84)
                }
            )
        ]
        .into()
    );
    assert!(
        record
            .record_restore(initial.restore().unwrap().clone())
            .is_err()
    );
    assert!(record.record_restore(copied()).is_err());
    assert_eq!(roundtrip(&record), record);
}
