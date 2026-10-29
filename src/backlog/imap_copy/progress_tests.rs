use super::*;
use crate::{
    backlog::protocol::{
        Batch,
        tests::{Stage, run},
    },
    census::AccountId,
};

fn evidence(stage: Stage) -> CopyEvidence {
    run(&AccountId::new("copy-progress").unwrap(), stage).batches[0]
        .as_copy()
        .unwrap()
        .clone()
}

#[test]
fn durable_phase_selects_one_next_action_without_changing_serialized_history() {
    for (stage, next) in [
        (Stage::Prepared, NextAction::Copy),
        (Stage::CopyUncertain, NextAction::ObserveCopy),
        (Stage::Copied, NextAction::PrepareDeletion),
        (Stage::StoreUncertain, NextAction::ObserveStore(0)),
        (Stage::Marked, NextAction::Expunge(0)),
        (Stage::ExpungeUncertain, NextAction::ObserveExpunge(0)),
        (Stage::Filed, NextAction::Complete),
    ] {
        let e = evidence(stage);
        let before = serde_json::to_vec(&e).unwrap();
        assert_eq!(e.next_action(), next);
        let reopened: CopyEvidence = serde_json::from_slice(&before).unwrap();
        assert_eq!(reopened.next_action(), next);
        assert_eq!(serde_json::to_vec(&e).unwrap(), before);
        assert_eq!(Batch::copy(reopened).resumable(), !next.needs_observation());
    }
}

#[test]
fn missing_store_acknowledgment_cannot_become_an_expunge_action_after_readback() {
    let mut e = evidence(Stage::StoreUncertain);
    e.record_store_result(0, StoreResult::Lost).unwrap();
    assert_eq!(e.next_action(), NextAction::ObserveStore(0));
    let marked = evidence(Stage::Marked);
    let read = marked
        .deletions()
        .next()
        .unwrap()
        .flag
        .readback
        .as_ref()
        .unwrap()
        .clone();
    e.observe_store(0, read).unwrap();
    assert_eq!(e.next_action(), NextAction::Conflict);
    assert!(!Batch::copy(e).resumable());
}

#[test]
fn an_acknowledged_store_still_requires_a_separate_observation() {
    let mut e = evidence(Stage::StoreUncertain);
    let marked = evidence(Stage::Marked);
    e.record_store_result(
        0,
        marked
            .deletions()
            .next()
            .unwrap()
            .flag
            .result()
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(e.next_action(), NextAction::ObserveStore(0));
    assert!(!Batch::copy(e).resumable());
}

#[test]
fn tagged_expunge_completion_never_skips_the_final_observation() {
    let mut e = evidence(Stage::ExpungeUncertain);
    e.record_expunge_completion(
        0,
        Completion::Tagged {
            status: TaggedStatus::Ok,
            code: None,
        },
    )
    .unwrap();
    assert_eq!(e.next_action(), NextAction::ObserveExpunge(0));
    assert!(!Batch::copy(e).resumable());
}

#[test]
fn one_later_observation_invalidates_flag_ownership_and_blocks_continuation() {
    let mut e = evidence(Stage::Marked);
    let mut later = e
        .deletions()
        .next()
        .unwrap()
        .flag
        .readback
        .as_ref()
        .unwrap()
        .clone();
    for m in later.messages.values_mut() {
        m.metadata.flags.insert("$other-client".into());
    }
    e.observe_store(0, later).unwrap();
    assert_eq!(e.next_action(), NextAction::Conflict);
    assert!(!Batch::copy(e).resumable());
}

#[test]
fn partial_store_expunge_action_never_includes_a_modified_sibling() {
    let mut e = evidence(Stage::StoreUncertain);
    let marked = evidence(Stage::Marked);
    let owned = Uid::new(4).unwrap();
    let changed = Uid::new(8).unwrap();
    let mut result = marked
        .deletions()
        .next()
        .unwrap()
        .flag
        .result()
        .unwrap()
        .clone();
    if let StoreResult::Tagged {
        status,
        modified,
        updated,
    } = &mut result
    {
        *status = TaggedStatus::No;
        *modified = Some([changed].into());
        updated.remove(&changed);
    }
    e.record_store_result(0, result).unwrap();
    e.observe_store(
        0,
        marked
            .deletions()
            .next()
            .unwrap()
            .flag
            .readback
            .as_ref()
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(e.next_action(), NextAction::Expunge(0));
    assert_eq!(
        e.deletions()
            .next()
            .unwrap()
            .flag
            .verified()
            .keys()
            .copied()
            .collect::<BTreeSet<_>>(),
        [owned].into()
    );
}
