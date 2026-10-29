use super::*;

#[test]
fn copied_messages_plan_only_exact_extra_copy_removal_while_retaining_originals() {
    let e = copied();
    let before = serde_json::to_vec(&e).unwrap();
    let preview = e.compensation_preview(source(), destination()).unwrap();
    assert!(preview.restore().is_none());
    assert!(preview.clear().is_empty());
    assert!(preview.conflicts().is_empty());
    assert_eq!(preview.cleanup().len(), 3);
    let originals: BTreeMap<_, _> = preview
        .cleanup()
        .iter()
        .flat_map(|c| c.originals().clone())
        .collect();
    assert_eq!(
        originals,
        [(uid(4), uid(60)), (uid(8), uid(64)), (uid(19), uid(70))].into()
    );
    for plan in preview.cleanup() {
        assert_eq!(plan.deletion().change(), FlagChange::AddDeleted);
        assert_eq!(
            plan.deletion().before().identity(),
            destination().identity()
        );
        assert_eq!(plan.retained().identity(), source().identity());
        assert_eq!(
            plan.retained()
                .messages()
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            plan.originals().keys().copied().collect()
        );
        assert_eq!(
            plan.deletion()
                .before()
                .messages()
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            plan.originals().values().copied().collect()
        );
    }
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    assert_eq!(preview.source(), &source());
    assert_eq!(preview.destination(), &destination());
}

#[test]
fn marked_originals_plan_owned_flag_clears_separately_from_extra_copies() {
    let (e, index) = marked();
    let mut current = source();
    let owned = e.deletions[index].evidence.flag.verified();
    current.messages.extend(owned.clone());
    current.highest_modseq = seq(911);
    let preview = e
        .compensation_preview(current.clone(), destination())
        .unwrap();
    assert!(preview.restore().is_none());
    assert!(preview.conflicts().is_empty());
    assert_eq!(preview.clear().len(), 2);
    assert_eq!(preview.cleanup().len(), 1);
    assert_eq!(
        preview.cleanup()[0].originals(),
        &[(uid(19), uid(70))].into()
    );
    for plan in preview.clear() {
        assert_eq!(plan.deletion(), index);
        assert_eq!(plan.clear().change(), FlagChange::RemoveOwnedDeleted);
        let pending = e.deletions[index]
            .evidence
            .begin_clear(plan.clear().clone(), plan.clear().before().clone())
            .unwrap();
        assert_eq!(pending.intent(), plan.clear());
        assert!(pending.verified().is_empty());
    }
    let marked: BTreeSet<_> = preview
        .clear()
        .iter()
        .flat_map(|c| c.clear().before().messages().keys().copied())
        .collect();
    assert_eq!(marked, owned.keys().copied().collect());
    assert!(
        preview
            .cleanup()
            .iter()
            .all(|p| p.originals().keys().all(|u| !marked.contains(u)))
    );
    let mut changed = current;
    changed.highest_modseq = seq(920);
    changed.messages.get_mut(&uid(4)).unwrap().modseq = seq(920);
    changed
        .messages
        .get_mut(&uid(4))
        .unwrap()
        .metadata
        .flags
        .insert("$later".into());
    let preview = e.compensation_preview(changed, destination()).unwrap();
    assert_eq!(
        preview.conflicts()[&uid(4)],
        CompensationConflict::UnprovenFlagOwnership
    );
    assert_eq!(preview.clear().len(), 1);
    assert!(
        preview
            .cleanup()
            .iter()
            .all(|c| !c.originals().contains_key(&uid(4)))
    );
}

fn filed() -> (CopyEvidence, Readback, Readback) {
    use crate::{
        backlog::protocol::tests::{Stage, run},
        census::AccountId,
    };
    let e = run(
        &AccountId::new("compensation-preview").unwrap(),
        Stage::Filed,
    )
    .batches[0]
        .as_copy()
        .unwrap()
        .clone();
    let batch = &e.deletions[0];
    let source = batch
        .evidence
        .expunge
        .as_ref()
        .unwrap()
        .readback
        .as_ref()
        .unwrap()
        .clone();
    let destination = batch.final_destination.clone().unwrap();
    (e, source, destination)
}

#[test]
fn reverse_filing_binds_current_flags_and_persistent_versions_without_restoring_old_flags() {
    let (e, source, mut destination) = filed();
    let before = serde_json::to_vec(&e).unwrap();
    destination.highest_modseq = seq(destination.highest_modseq.get() + 30);
    let current = destination.messages.get_mut(&uid(60)).unwrap();
    current.modseq = destination.highest_modseq;
    current.metadata.flags.remove("\\Seen");
    current
        .metadata
        .flags
        .extend(["\\Flagged".into(), "$later".into()]);
    let preview = e
        .compensation_preview(source.clone(), destination.clone())
        .unwrap();
    assert!(preview.conflicts().is_empty());
    assert!(preview.cleanup().is_empty());
    assert!(preview.clear().is_empty());
    let reverse = preview.restore().unwrap();
    assert_eq!(reverse.intent().purpose(), Purpose::ConditionalUndo);
    assert_eq!(reverse.intent().source(), e.intent().destination());
    assert_eq!(reverse.intent().destination(), e.intent().source());
    assert_eq!(reverse.before(), &destination);
    assert_eq!(
        reverse.intent().messages()[&uid(60)],
        current_message(&destination, 60)
    );
    assert_eq!(reverse.next_action(), NextAction::Copy);
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    let approved = serde_json::to_vec(&preview).unwrap();
    destination.highest_modseq = seq(destination.highest_modseq.get() + 1);
    destination.messages.get_mut(&uid(60)).unwrap().modseq = destination.highest_modseq;
    assert_ne!(
        approved,
        serde_json::to_vec(&e.compensation_preview(source, destination).unwrap()).unwrap()
    );
}

fn current_message(read: &Readback, n: u32) -> Metadata {
    read.messages[&uid(n)].metadata.clone()
}

#[test]
fn reverse_filing_reports_reappeared_originals_and_changed_or_protected_destinations() {
    for mutation in [
        "source", "missing", "size", "date", "hash", "deleted", "draft",
    ] {
        let (e, mut source, mut destination) = filed();
        let expected = match mutation {
            "source" => {
                source.highest_modseq = seq(source.highest_modseq.get() + 1);
                source.messages.insert(
                    uid(4),
                    VersionedMetadata {
                        metadata: e.intent().messages()[&uid(4)].clone(),
                        modseq: source.highest_modseq,
                    },
                );
                CompensationConflict::SourcePresent
            }
            "missing" => {
                destination.messages.remove(&uid(60));
                CompensationConflict::ChangedOrMissingMessage
            }
            "size" | "date" | "hash" => {
                let m = &mut destination.messages.get_mut(&uid(60)).unwrap().metadata;
                match mutation {
                    "size" => m.size += 1,
                    "date" => m.internal_date = "25-Oct-2026 12:00:00 +0000".into(),
                    _ => m.message_id_hash.push('x'),
                }
                CompensationConflict::ChangedOrMissingMessage
            }
            _ => {
                destination
                    .messages
                    .get_mut(&uid(60))
                    .unwrap()
                    .metadata
                    .flags
                    .insert(
                        if mutation == "deleted" {
                            "\\Deleted"
                        } else {
                            "\\Draft"
                        }
                        .into(),
                    );
                CompensationConflict::ProtectedFlags
            }
        };
        let preview = e.compensation_preview(source, destination).unwrap();
        assert_eq!(
            preview.conflicts(),
            &[(uid(4), expected)].into(),
            "{mutation}"
        );
        assert_eq!(
            preview
                .restore()
                .unwrap()
                .intent()
                .messages()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![uid(64)]
        );
        assert!(preview.cleanup().is_empty());
        assert!(preview.clear().is_empty());
    }
}

#[test]
fn extra_copy_cleanup_refuses_changed_copies_and_missing_or_protected_originals() {
    for mutation in [
        "missing-source",
        "source-deleted",
        "source-draft",
        "copy-flags",
        "copy-version",
        "missing-copy",
    ] {
        let e = copied();
        let mut source = source();
        let mut destination = destination();
        match mutation {
            "missing-source" => {
                source.messages.remove(&uid(4));
            }
            "source-deleted" | "source-draft" => {
                source
                    .messages
                    .get_mut(&uid(4))
                    .unwrap()
                    .metadata
                    .flags
                    .insert(
                        if mutation == "source-deleted" {
                            "\\Deleted"
                        } else {
                            "\\Draft"
                        }
                        .into(),
                    );
            }
            "copy-flags" => {
                destination
                    .messages
                    .get_mut(&uid(60))
                    .unwrap()
                    .metadata
                    .flags
                    .insert("$later".into());
            }
            "copy-version" => {
                destination.messages.get_mut(&uid(60)).unwrap().modseq = seq(1004);
                destination.highest_modseq = seq(1004);
            }
            "missing-copy" => {
                destination.messages.remove(&uid(60));
            }
            _ => unreachable!(),
        }
        let preview = e.compensation_preview(source, destination).unwrap();
        assert_eq!(
            preview.conflicts(),
            &[(uid(4), CompensationConflict::ChangedOrMissingMessage)].into(),
            "{mutation}"
        );
        assert_eq!(
            preview
                .cleanup()
                .iter()
                .flat_map(|c| c.originals().keys().copied())
                .collect::<BTreeSet<_>>(),
            [uid(8), uid(19)].into()
        );
        assert!(preview.restore().is_none());
        assert!(preview.clear().is_empty());
    }
}

#[test]
fn pending_effects_require_reconciliation_before_compensation() {
    use crate::{
        backlog::protocol::tests::{Stage, run},
        census::AccountId,
    };
    for stage in [
        Stage::Prepared,
        Stage::CopyUncertain,
        Stage::StoreUncertain,
        Stage::ExpungeUncertain,
    ] {
        let e = run(&AccountId::new("compensation-hold").unwrap(), stage).batches[0]
            .as_copy()
            .unwrap()
            .clone();
        assert_eq!(
            e.compensation_preview(
                e.before().clone(),
                Readback::new(
                    e.intent().destination().clone(),
                    seq(u64::MAX),
                    BTreeMap::new()
                )
                .unwrap()
            ),
            Err(InvalidEvidence::Phase)
        );
    }
}

#[test]
fn lost_store_acknowledgment_never_authorizes_owned_flag_compensation() {
    let mut e = copied();
    let prepared = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    let index = e.begin_delete(prepared).unwrap();
    e.record_store_result(index, StoreResult::Lost).unwrap();
    let flagged = read("INBOX", 101, &[4, 8], &[910, 911], true);
    e.observe_store(index, flagged.clone()).unwrap();
    let e = roundtrip(&e);
    let mut current = source();
    current.messages.extend(flagged.messages);
    current.highest_modseq = seq(911);
    let preview = e.compensation_preview(current, destination()).unwrap();
    assert!(preview.clear().is_empty());
    assert_eq!(
        preview.conflicts(),
        &[
            (uid(4), CompensationConflict::ForwardConflict),
            (uid(8), CompensationConflict::ForwardConflict)
        ]
        .into()
    );
    assert_eq!(preview.cleanup().len(), 1);
    assert_eq!(
        preview.cleanup()[0].originals(),
        &[(uid(19), uid(70))].into()
    );
}

#[test]
fn compensation_refuses_an_observation_older_than_the_last_filing_readback() {
    let (mut e, source, destination) = filed();
    let mut later = destination.clone();
    later.highest_modseq = seq(later.highest_modseq.get() + 40);
    later.messages.get_mut(&uid(60)).unwrap().modseq = later.highest_modseq;
    e.observe_final(0, source.clone(), later.clone()).unwrap();
    assert_eq!(
        e.compensation_preview(source.clone(), destination),
        Err(InvalidEvidence::Identity)
    );
    assert!(e.compensation_preview(source, later).is_ok());
}

#[test]
fn compensation_cannot_be_generated_for_a_reverse_filing_operation() {
    let (e, source, destination) = filed();
    let preview = e.compensation_preview(source.clone(), destination).unwrap();
    let reverse = preview.restore().unwrap();
    let mut begun = reverse.clone();
    begun.begin_copy(reverse.before()).unwrap();
    begun
        .record_mapping(CopyUidMapping::parse("[COPYUID 101 60,64 80,84]", begun.intent()).unwrap())
        .unwrap();
    begun.record_copy_completion(Completion::Lost).unwrap();
    let mut target = source;
    target.highest_modseq = seq(target.highest_modseq.get() + 1);
    for (a, b) in [(60, 80), (64, 84)] {
        let mut copied = reverse.before().messages()[&uid(a)].clone();
        copied.modseq = target.highest_modseq;
        target.messages.insert(uid(b), copied);
    }
    begun
        .observe_copy(reverse.before().clone(), target.clone())
        .unwrap();
    assert_eq!(
        begun.compensation_preview(reverse.before().clone(), target),
        Err(InvalidEvidence::Phase)
    );
}

#[test]
fn compensation_observations_reject_scope_identity_and_version_regression() {
    for mutation in [
        "source-name",
        "source-validity",
        "destination-validity",
        "extra-source",
        "extra-destination",
        "source-highest",
        "destination-version",
    ] {
        let (e, mut source, mut destination) = filed();
        match mutation {
            "source-name" => source.identity = mailbox("Other", 101),
            "source-validity" => source.identity = mailbox("INBOX", 102),
            "destination-validity" => destination.identity = mailbox("Privat/News", 203),
            "extra-source" => {
                source.messages.insert(
                    uid(999),
                    VersionedMetadata {
                        metadata: metadata(999),
                        modseq: source.highest_modseq,
                    },
                );
            }
            "extra-destination" => {
                destination.messages.insert(
                    uid(999),
                    VersionedMetadata {
                        metadata: metadata(999),
                        modseq: destination.highest_modseq,
                    },
                );
            }
            "source-highest" => source.highest_modseq = seq(e.before().highest_modseq.get() + 1),
            "destination-version" => {
                destination.messages.get_mut(&uid(60)).unwrap().modseq =
                    seq(destination.messages[&uid(60)].modseq.get() - 1)
            }
            _ => unreachable!(),
        }
        assert_eq!(
            e.compensation_preview(source, destination),
            Err(InvalidEvidence::Identity),
            "{mutation}"
        );
    }
}
