use super::*;
use crate::backlog::{
    imap_copy::{CleanupSlot, FlagChange, FlagIntent, ModSeq, Readback, StoreResult},
    protocol::tests::{Stage, run},
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase {
    Prepared,
    StoreUncertain,
    Marked,
    ExpungeUncertain,
    Cleaned,
    ClearUncertain,
    Cleared,
    RestorePrepared,
    Restored,
    AfterClearPrepared,
    AfterClearStoreUncertain,
    AfterClearMarked,
    AfterClearExpungeUncertain,
    AfterClearCleaned,
    AfterClearConflict,
}

fn seq(n: u64) -> ModSeq {
    ModSeq::new(n).unwrap()
}

fn flagged(intent: &FlagIntent) -> (StoreResult, Readback) {
    let highest = seq(intent.before().highest_modseq().get() + 10);
    let messages: std::collections::BTreeMap<_, _> = intent
        .before()
        .messages()
        .iter()
        .map(|(uid, value)| {
            let mut value = value.clone();
            match intent.change() {
                FlagChange::AddDeleted => {
                    value.metadata.flags.insert("\\Deleted".into());
                }
                FlagChange::RemoveOwnedDeleted => {
                    value
                        .metadata
                        .flags
                        .retain(|f| !f.eq_ignore_ascii_case("\\Deleted"));
                }
            }
            value.modseq = highest;
            (*uid, value)
        })
        .collect();
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

pub(crate) fn fixture(account: &AccountId, phase: Phase) -> Run {
    if matches!(
        phase,
        Phase::AfterClearPrepared
            | Phase::AfterClearStoreUncertain
            | Phase::AfterClearMarked
            | Phase::AfterClearExpungeUncertain
            | Phase::AfterClearCleaned
            | Phase::AfterClearConflict
    ) {
        return after_clear_fixture(account, phase);
    }
    let stage = match phase {
        Phase::ClearUncertain | Phase::Cleared => Stage::Marked,
        Phase::RestorePrepared | Phase::Restored => Stage::Filed,
        _ => Stage::Copied,
    };
    let mut run = run(account, stage);
    let forward = run.batches[0].as_copy().unwrap().clone();
    let value = serde_json::to_value(&forward).unwrap();
    let source = match stage {
        Stage::Marked => &value["deletions"][0]["evidence"]["flag"]["readback"],
        Stage::Filed => &value["deletions"][0]["evidence"]["expunge"]["readback"],
        _ => &value["readback"]["source"],
    };
    let source = serde_json::from_value(source.clone()).unwrap();
    let destination = serde_json::from_value(value["readback"]["destination"].clone()).unwrap();
    let preview = forward.compensation_preview(source, destination).unwrap();
    let mut evidence = CompensationEvidence::new(forward, preview).unwrap();
    match phase {
        Phase::Prepared | Phase::RestorePrepared => (),
        Phase::Restored => {
            let mut restore = evidence.restore().unwrap().clone();
            let before = restore.before().clone();
            restore.begin_copy(&before).unwrap();
            restore
                .record_mapping(
                    CopyUidMapping::parse("[COPYUID 101 60,64 80,84]", restore.intent()).unwrap(),
                )
                .unwrap();
            restore.record_copy_completion(Completion::Lost).unwrap();
            let uid = |n| Uid::new(n).unwrap();
            let destination = Readback::new(
                restore.intent().destination().clone(),
                seq(before.highest_modseq().get() + 100),
                [
                    (uid(80), before.messages()[&uid(60)].clone()),
                    (uid(84), before.messages()[&uid(64)].clone()),
                ]
                .into(),
            )
            .unwrap();
            restore
                .observe_copy(before.clone(), destination.clone())
                .unwrap();
            let prepared = restore
                .prepare_deletions(&before, &destination)
                .unwrap()
                .remove(0);
            let intent = prepared.intent().clone();
            let index = restore.begin_delete(prepared).unwrap();
            let (ack, after) = flagged(&intent);
            restore.record_store_result(index, ack).unwrap();
            restore.observe_store(index, after.clone()).unwrap();
            restore
                .begin_expunge(index, after.clone(), destination.clone())
                .unwrap();
            restore
                .record_expunge_completion(index, Completion::Lost)
                .unwrap();
            let empty = Readback::new(
                after.identity().clone(),
                seq(after.highest_modseq().get() + 10),
                Default::default(),
            )
            .unwrap();
            restore.observe_final(index, empty, destination).unwrap();
            evidence.record_restore(restore).unwrap();
        }
        Phase::ClearUncertain | Phase::Cleared => {
            let plan = evidence.preview().clear()[0].clone();
            evidence
                .begin_clear(0, plan.clear().before().clone())
                .unwrap();
            if matches!(phase, Phase::Cleared) {
                let (ack, after) = flagged(plan.clear());
                evidence.record_clear(0, ack).unwrap();
                evidence.observe_clear(0, after).unwrap();
            }
        }
        _ => {
            let plan = evidence.preview().cleanup()[0].clone();
            evidence
                .begin_cleanup(
                    CleanupSlot::Initial(0),
                    plan.retained(),
                    plan.deletion().before(),
                )
                .unwrap();
            if !matches!(phase, Phase::StoreUncertain) {
                let (ack, after) = flagged(plan.deletion());
                evidence
                    .record_cleanup_store(CleanupSlot::Initial(0), ack)
                    .unwrap();
                evidence
                    .observe_cleanup_store(CleanupSlot::Initial(0), after.clone())
                    .unwrap();
                if matches!(phase, Phase::ExpungeUncertain | Phase::Cleaned) {
                    evidence
                        .begin_cleanup_expunge(
                            CleanupSlot::Initial(0),
                            plan.retained().clone(),
                            after.clone(),
                        )
                        .unwrap();
                    if matches!(phase, Phase::Cleaned) {
                        evidence
                            .record_cleanup_expunge(CleanupSlot::Initial(0), Completion::Lost)
                            .unwrap();
                        let empty = Readback::new(
                            after.identity().clone(),
                            seq(after.highest_modseq().get() + 10),
                            Default::default(),
                        )
                        .unwrap();
                        evidence
                            .observe_cleanup_final(
                                CleanupSlot::Initial(0),
                                plan.retained().clone(),
                                empty,
                            )
                            .unwrap();
                    }
                }
            }
        }
    }
    run.compensation = vec![Compensation {
        after_clear_approvals: Vec::new(),
        forward: 0,
        evidence,
    }];
    run.undo_receipt = Some("3".repeat(64));
    run.status = super::outcome(&run);
    run.stamp(1793260800);
    Journal {
        runs: vec![run.clone()],
        summaries: vec![],
    }
    .validate(account)
    .unwrap();
    run
}

fn after_clear_fixture(account: &AccountId, phase: Phase) -> Run {
    let mut run = fixture(account, Phase::Cleared);
    let evidence = &mut run.compensation[0].evidence;
    let mut source = evidence.clear(0).unwrap().readback().unwrap().clone();
    if matches!(phase, Phase::AfterClearConflict) {
        let mut messages = source.messages().clone();
        messages.remove(&Uid::new(4).unwrap());
        source =
            Readback::new(source.identity().clone(), source.highest_modseq(), messages).unwrap();
    }
    let destination = evidence.preview().destination().clone();
    let slot = evidence
        .prepare_after_clear(0, source, destination)
        .unwrap()[0];
    let plan = evidence.cleanup_plan(slot).unwrap().clone();
    if !matches!(phase, Phase::AfterClearPrepared) {
        evidence
            .begin_cleanup(slot, plan.retained(), plan.deletion().before())
            .unwrap();
        if !matches!(phase, Phase::AfterClearStoreUncertain) {
            let (ack, after) = flagged(plan.deletion());
            evidence.record_cleanup_store(slot, ack).unwrap();
            evidence.observe_cleanup_store(slot, after.clone()).unwrap();
            if !matches!(phase, Phase::AfterClearMarked) {
                evidence
                    .begin_cleanup_expunge(slot, plan.retained().clone(), after.clone())
                    .unwrap();
                if !matches!(phase, Phase::AfterClearExpungeUncertain) {
                    evidence
                        .record_cleanup_expunge(slot, Completion::Lost)
                        .unwrap();
                    let empty = Readback::new(
                        after.identity().clone(),
                        seq(after.highest_modseq().get() + 10),
                        Default::default(),
                    )
                    .unwrap();
                    evidence
                        .observe_cleanup_final(slot, plan.retained().clone(), empty)
                        .unwrap();
                }
            }
        }
    }
    run.status = super::outcome(&run);
    run.stamp(1793260800);
    Journal {
        runs: vec![run.clone()],
        summaries: vec![],
    }
    .validate(account)
    .unwrap();
    run
}

#[test]
fn compensation_states_distinguish_unknown_effects_pending_clears_and_removed_copies() {
    let account = AccountId::new("compensation-account").unwrap();
    for (phase, expected) in [
        (Phase::Prepared, Status::Stopped),
        (Phase::StoreUncertain, Status::Uncertain),
        (Phase::Marked, Status::Stopped),
        (Phase::ExpungeUncertain, Status::Uncertain),
        (Phase::Cleaned, Status::Undone),
        (Phase::ClearUncertain, Status::Uncertain),
        (Phase::Cleared, Status::Stopped),
        (Phase::RestorePrepared, Status::Stopped),
        (Phase::Restored, Status::Undone),
    ] {
        let run = fixture(&account, phase);
        assert_eq!(run.status, expected, "{phase:?}");
        assert_eq!(
            run.finished(),
            matches!(phase, Phase::Cleaned | Phase::Restored)
        );
        assert_eq!(
            run.observed_without_uncertainty(),
            expected != Status::Uncertain
        );
        assert!(!run.closable());
        if matches!(phase, Phase::Cleared) {
            assert_eq!(run.compensation_counts().deleted_cleared, 2);
            assert_eq!(run.compensation_counts().copies_removed, 0);
            assert_eq!(run.completed_at, None);
        }
    }
}

#[test]
fn compensation_witness_targets_and_undo_alternatives_are_checked() {
    let account = AccountId::new("compensation-binding").unwrap();
    let initial = fixture(&account, Phase::Prepared);
    for change in 0..6 {
        let mut run = initial.clone();
        match change {
            0 => run.compensation[0].forward = 1,
            1 => run.compensation.push(run.compensation[0].clone()),
            2 => run.batches = super::super::tests::run(&account, Stage::Marked).batches,
            3 => run.reverse = super::super::tests::run(&account, Stage::Undone).reverse,
            4 => run.undo_receipt = None,
            5 => run.status = Status::Undone,
            _ => unreachable!(),
        }
        assert!(
            Journal {
                runs: vec![run],
                summaries: vec![]
            }
            .validate(&account)
            .is_err(),
            "{change}"
        );
    }
}

#[test]
fn cleaned_copies_compact_separately_without_claiming_restored_filing() {
    let account = AccountId::new("compensation-retention").unwrap();
    let cleaned = fixture(&account, Phase::Cleaned);
    let mut journal = Journal {
        runs: vec![cleaned.clone()],
        summaries: vec![],
    };
    assert!(journal.compact(retention::deadline(cleaned.completed_at.unwrap())));
    assert!(journal.runs.is_empty());
    let summary = &journal.summaries[0];
    assert_eq!(summary.applied, 0);
    assert_eq!(summary.restored, 0);
    assert_eq!(summary.copies_removed, 2);
    journal.validate(&account).unwrap();
    let mut invalid = journal.clone();
    invalid.summaries[0].restored = 1;
    assert!(invalid.validate(&account).is_err());
    invalid = journal.clone();
    invalid.summaries[0].copies_removed = 3;
    assert!(invalid.validate(&account).is_err());
    for phase in [
        Phase::Prepared,
        Phase::Marked,
        Phase::StoreUncertain,
        Phase::Cleared,
    ] {
        let run = fixture(&account, phase);
        let mut journal = Journal {
            runs: vec![run.clone()],
            summaries: vec![],
        };
        assert!(!journal.compact(u64::MAX));
        assert!(journal.runs == vec![run]);
    }
}

#[test]
fn interruption_observes_compensation_without_closing_or_repeating_it() {
    let account = AccountId::new("compensation-interrupted").unwrap();
    for phase in [
        Phase::Prepared,
        Phase::StoreUncertain,
        Phase::Marked,
        Phase::ExpungeUncertain,
        Phase::Cleared,
    ] {
        let mut run = fixture(&account, phase);
        let expected = run.status;
        let evidence = run.compensation.clone();
        run.status = Status::UndoRunning;
        let mut journal = Journal {
            runs: vec![run],
            summaries: vec![],
        };
        journal.interrupted();
        assert_eq!(journal.runs[0].status, expected);
        assert_eq!(journal.runs[0].compensation, evidence);
        assert!(journal.runs[0].stop_requested);
        journal.validate(&account).unwrap();
    }
}

#[test]
fn restored_filing_keeps_its_own_count_and_current_flags_in_retention() {
    let account = AccountId::new("compensation-restored").unwrap();
    let run = fixture(&account, Phase::Restored);
    let restored = run.compensation[0].evidence.restore().unwrap();
    assert!(
        restored
            .intent()
            .messages()
            .values()
            .all(|m| m.flags.contains("$owner"))
    );
    assert_eq!(run.compensation_counts().restored, 2);
    assert_eq!(run.compensation_counts().copies_removed, 0);
    let completed = run.completed_at.unwrap();
    let mut journal = Journal {
        runs: vec![run],
        summaries: vec![],
    };
    assert!(journal.compact(retention::deadline(completed)));
    assert_eq!(journal.summaries[0].applied, 2);
    assert_eq!(journal.summaries[0].restored, 2);
    assert_eq!(journal.summaries[0].copies_removed, 0);
    journal.validate(&account).unwrap();
}

#[test]
fn dependent_cleanup_status_retention_and_closure_follow_the_actual_effects() {
    let account = AccountId::new("after-clear-account").unwrap();
    for (phase, status) in [
        (Phase::AfterClearPrepared, Status::Stopped),
        (Phase::AfterClearStoreUncertain, Status::Uncertain),
        (Phase::AfterClearMarked, Status::Stopped),
        (Phase::AfterClearExpungeUncertain, Status::Uncertain),
        (Phase::AfterClearCleaned, Status::Undone),
        (Phase::AfterClearConflict, Status::Conflict),
    ] {
        let run = fixture(&account, phase);
        assert_eq!(run.status, status);
        assert_eq!(run.finished(), status == Status::Undone);
        assert_eq!(run.closable(), status == Status::Conflict);
        assert_eq!(run.compensation_counts().restored, 0);
        let mut journal = Journal {
            runs: vec![run.clone()],
            summaries: vec![],
        };
        assert_eq!(journal.compact(u64::MAX), status == Status::Undone);
        if status == Status::Undone {
            assert_eq!(journal.summaries[0].applied, 0);
            assert_eq!(journal.summaries[0].restored, 0);
            assert_eq!(journal.summaries[0].copies_removed, 2);
        } else {
            assert!(journal.runs == vec![run]);
        }
        journal.validate(&account).unwrap();
    }
}
