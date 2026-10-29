use super::*;
use crate::{backlog::imap_move::UidValidity, census::folder_provisioning::ExactName};

fn uid(n: u32) -> Uid {
    Uid::new(n).unwrap()
}
fn seq(n: u64) -> ModSeq {
    ModSeq::new(n).unwrap()
}
fn mailbox(name: &str, validity: u32) -> MailboxIdentity {
    MailboxIdentity {
        name: ExactName::from_wire(name).unwrap(),
        uidvalidity: UidValidity::new(validity).unwrap(),
    }
}
fn metadata(n: u32) -> Metadata {
    Metadata {
        message_id_hash: format!("message-{n}"),
        size: n + 100,
        internal_date: "24-Oct-2026 12:00:00 +0000".into(),
        flags: ["\\Seen".into(), "$owner".into()].into_iter().collect(),
    }
}
fn read(name: &str, validity: u32, uids: &[u32], sequences: &[u64], deleted: bool) -> Readback {
    let messages = uids
        .iter()
        .zip(sequences)
        .map(|(u, s)| {
            let original = match *u {
                60 => 4,
                64 => 8,
                70 => 19,
                n => n,
            };
            let mut metadata = metadata(original);
            if deleted {
                metadata.flags.insert("\\Deleted".into());
            }
            (
                uid(*u),
                VersionedMetadata {
                    metadata,
                    modseq: seq(*s),
                },
            )
        })
        .collect();
    let highest = sequences
        .iter()
        .copied()
        .max()
        .unwrap_or(2000)
        .max(if name == "INBOX" { 905 } else { 1003 });
    Readback::new(mailbox(name, validity), seq(highest), messages).unwrap()
}
fn source() -> Readback {
    read("INBOX", 101, &[4, 8, 19], &[900, 900, 905], false)
}
fn destination() -> Readback {
    read(
        "Privat/News",
        202,
        &[60, 64, 70],
        &[1001, 1002, 1003],
        false,
    )
}
fn evidence() -> CopyEvidence {
    let before = source();
    let intent = MoveIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        before.identity.clone(),
        mailbox("Privat/News", 202),
        before.plain().messages,
    )
    .unwrap();
    CopyEvidence::new(intent, before).unwrap()
}
fn copied() -> CopyEvidence {
    let mut e = evidence();
    assert_eq!(
        e.begin_copy(&source()).unwrap(),
        "UID COPY 4,8,19 \"Privat/News\""
    );
    e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4,8,19 60,64,70]", e.intent()).unwrap())
        .unwrap();
    e.record_copy_completion(Completion::Tagged {
        status: TaggedStatus::Ok,
        code: None,
    })
    .unwrap();
    e.observe_copy(source(), destination()).unwrap();
    roundtrip(&e)
}
fn roundtrip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(e: &T) -> T {
    let bytes = serde_json::to_vec(e).unwrap();
    let restored = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(&restored, e);
    restored
}
fn acknowledged(intent: &FlagIntent, updated: &[(u32, u64)]) -> StoreResult {
    StoreResult::tagged(
        TaggedStatus::Ok,
        None,
        updated.iter().map(|(u, m)| (uid(*u), seq(*m))).collect(),
        intent,
    )
    .unwrap()
}
fn marked() -> (CopyEvidence, usize) {
    let mut e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    assert_eq!(
        group.intent.command(),
        "UID STORE 4,8 (UNCHANGEDSINCE 900) +FLAGS.SILENT (\\Deleted)"
    );
    let result = acknowledged(group.intent(), &[(4, 910), (8, 911)]);
    let index = e.begin_delete(group).unwrap();
    e = roundtrip(&e);
    e.record_store_result(index, result).unwrap();
    e = roundtrip(&e);
    e.observe_store(index, read("INBOX", 101, &[4, 8], &[910, 911], true))
        .unwrap();
    (roundtrip(&e), index)
}

#[test]
fn sparse_copy_store_and_uid_expunge_have_separate_serialization_boundaries() {
    let mut e = evidence();
    assert!(
        e.resolutions()
            .values()
            .all(|r| *r == Resolution::Unattempted)
    );
    e.begin_copy(&source()).unwrap();
    e = roundtrip(&e);
    assert!(e.begin_copy(&source()).is_err());
    assert!(
        e.resolutions()
            .values()
            .all(|r| *r == Resolution::Uncertain)
    );
    e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4,8,19 60,64,70]", e.intent()).unwrap())
        .unwrap();
    e = roundtrip(&e);
    e.record_copy_completion(Completion::Lost).unwrap();
    e = roundtrip(&e);
    e.observe_copy(source(), destination()).unwrap();
    e = roundtrip(&e);
    assert_eq!(
        e.resolutions()[&uid(4)],
        Resolution::CopyOnly {
            destination: uid(60)
        }
    );
    let groups = e.prepare_deletions(&source(), &destination()).unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[1].intent.command(),
        "UID STORE 19 (UNCHANGEDSINCE 905) +FLAGS.SILENT (\\Deleted)"
    );
    for group in groups {
        let members: Vec<_> = group
            .intent
            .before
            .messages
            .keys()
            .map(|u| u.get())
            .collect();
        let updated: Vec<_> = members.iter().map(|u| (*u, 1200 + u64::from(*u))).collect();
        let result = acknowledged(group.intent(), &updated);
        let destination_uids: Vec<_> = group.destination.messages.keys().map(|u| u.get()).collect();
        let destination_seq: Vec<_> = group
            .destination
            .messages
            .values()
            .map(|m| m.modseq.get())
            .collect();
        let dest = read(
            "Privat/News",
            202,
            &destination_uids,
            &destination_seq,
            false,
        );
        let index = e.begin_delete(group).unwrap();
        e = roundtrip(&e);
        assert_eq!(e.resolutions()[&uid(members[0])], Resolution::Uncertain);
        e.record_store_result(index, result).unwrap();
        e = roundtrip(&e);
        let flagged = read(
            "INBOX",
            101,
            &members,
            &updated.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            true,
        );
        e.observe_store(index, flagged.clone()).unwrap();
        e = roundtrip(&e);
        assert_eq!(
            e.resolutions()[&uid(members[0])],
            Resolution::Marked {
                destination: uid(destination_uids[0])
            }
        );
        let command = e.begin_expunge(index, flagged, dest.clone()).unwrap();
        assert_eq!(
            command,
            format!(
                "UID EXPUNGE {}",
                members
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        );
        e = roundtrip(&e);
        e.record_expunge_completion(index, Completion::Lost)
            .unwrap();
        e = roundtrip(&e);
        e.observe_final(index, read("INBOX", 101, &[], &[], false), dest)
            .unwrap();
        e = roundtrip(&e);
    }
    assert!(
        e.resolutions()
            .values()
            .all(|r| matches!(r, Resolution::Filed { .. }))
    );
}

#[test]
fn lost_copy_acknowledgment_without_mapping_never_retries_or_deletes() {
    let mut e = evidence();
    e.begin_copy(&source()).unwrap();
    e.record_copy_completion(Completion::Lost).unwrap();
    let empty = read("Privat/News", 202, &[], &[], false);
    e.observe_copy(source(), empty.clone()).unwrap();
    let mut restored = roundtrip(&e);
    assert!(restored.begin_copy(&source()).is_err());
    assert!(restored.prepare_deletions(&source(), &empty).is_err());
    assert!(
        restored
            .resolutions()
            .values()
            .all(|r| *r == Resolution::Uncertain)
    );
}

#[test]
fn partial_copy_mapping_leaves_unmapped_members_uncertain() {
    let mut e = evidence();
    e.begin_copy(&source()).unwrap();
    e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4,19 60,70]", e.intent()).unwrap())
        .unwrap();
    e.record_copy_completion(Completion::Lost).unwrap();
    let dest = read("Privat/News", 202, &[60, 70], &[1001, 1003], false);
    e.observe_copy(source(), dest.clone()).unwrap();
    assert_eq!(e.resolutions()[&uid(8)], Resolution::Uncertain);
    assert_eq!(e.prepare_deletions(&source(), &dest).unwrap().len(), 2);
    assert_eq!(roundtrip(&e), e);
}

#[test]
fn source_or_destination_flag_date_and_content_changes_prevent_deletion() {
    let e = copied();
    for change in 0..7 {
        let mut src = source();
        let mut dest = destination();
        let member = if change < 4 {
            src.messages.get_mut(&uid(4)).unwrap()
        } else {
            dest.messages.get_mut(&uid(60)).unwrap()
        };
        match change {
            0 => {
                member.metadata.flags.insert("\\Flagged".into());
            }
            1 => {
                member.metadata.flags.insert("\\Deleted".into());
            }
            2 | 4 => {
                member.metadata.flags.remove("\\Seen");
            }
            3 | 5 => {
                member.metadata.internal_date = "25-Oct-2026 12:00:00 +0000".into();
            }
            6 => {
                member.metadata.size += 1;
            }
            _ => unreachable!(),
        }
        member.modseq = seq(1500);
        let groups = e.prepare_deletions(&src, &dest).unwrap();
        assert!(
            groups
                .iter()
                .all(|g| !g.intent.before.messages.contains_key(&uid(4)))
        );
        assert!(
            groups
                .iter()
                .any(|g| g.intent.before.messages.contains_key(&uid(8)))
        );
    }
}

#[test]
fn different_actual_source_modseq_values_produce_separate_store_groups() {
    let e = copied();
    let mut fresh = source();
    fresh.messages.get_mut(&uid(8)).unwrap().modseq = seq(901);
    let groups = e.prepare_deletions(&fresh, &destination()).unwrap();
    assert_eq!(groups.len(), 3);
    assert!(groups.iter().all(|g| g.intent.before.messages.len() == 1));
}

#[test]
fn partial_modified_result_authorizes_only_acknowledged_unchanged_members() {
    for status in [TaggedStatus::Ok, TaggedStatus::No] {
        let mut e = copied();
        let group = e
            .prepare_deletions(&source(), &destination())
            .unwrap()
            .remove(0);
        let result = StoreResult::tagged(
            status,
            Some("[MODIFIED 8]"),
            [(uid(4), seq(910))].into(),
            group.intent(),
        )
        .unwrap();
        let index = e.begin_delete(group).unwrap();
        e.record_store_result(index, result).unwrap();
        let mut flagged = read("INBOX", 101, &[4, 8], &[910, 915], true);
        flagged
            .messages
            .get_mut(&uid(8))
            .unwrap()
            .metadata
            .flags
            .remove("\\Deleted");
        e.observe_store(index, flagged.clone()).unwrap();
        assert_eq!(
            e.deletions[index]
                .evidence
                .flag
                .verified()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![uid(4)]
        );
        let only_four = flagged.subset(&[uid(4)].into());
        let dest = destination().subset(&[uid(60), uid(64)].into());
        assert_eq!(
            e.begin_expunge(index, only_four, dest).unwrap(),
            "UID EXPUNGE 4"
        );
        assert_eq!(roundtrip(&e), e);
    }
}

#[test]
fn lost_store_acknowledgment_cannot_claim_a_concurrent_deleted_bit() {
    let mut e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    let index = e.begin_delete(group).unwrap();
    e.record_store_result(index, StoreResult::Lost).unwrap();
    let fresh = read("INBOX", 101, &[4, 8], &[910, 911], true);
    e.observe_store(index, fresh.clone()).unwrap();
    assert!(e.deletions[index].evidence.flag.verified().is_empty());
    assert!(
        e.deletions[index]
            .evidence
            .pending_clear(fresh.clone())
            .is_err()
    );
    assert!(
        e.begin_expunge(
            index,
            fresh,
            destination().subset(&[uid(60), uid(64)].into())
        )
        .is_err()
    );
    assert!(
        e.prepare_deletions(&source(), &destination())
            .unwrap()
            .iter()
            .all(|g| !g.intent.before.messages.contains_key(&uid(4)))
    );
    assert_eq!(roundtrip(&e), e);
}

#[test]
fn later_metadata_or_missing_store_modseq_prevents_ownership() {
    for case in 0..4 {
        let mut e = copied();
        let group = e
            .prepare_deletions(&source(), &destination())
            .unwrap()
            .remove(0);
        let result = if case == 0 {
            acknowledged(group.intent(), &[])
        } else if case == 1 {
            StoreResult::tagged(
                TaggedStatus::No,
                None,
                [(uid(4), seq(910))].into(),
                group.intent(),
            )
            .unwrap()
        } else {
            acknowledged(group.intent(), &[(4, 910)])
        };
        let index = e.begin_delete(group).unwrap();
        e.record_store_result(index, result).unwrap();
        let mut fresh = read(
            "INBOX",
            101,
            &[4],
            &[if case == 2 { 911 } else { 910 }],
            true,
        );
        if case == 3 {
            fresh
                .messages
                .get_mut(&uid(4))
                .unwrap()
                .metadata
                .flags
                .insert("$later".into());
        }
        e.observe_store(index, fresh.clone()).unwrap();
        assert!(e.deletions[index].evidence.flag.verified().is_empty());
        assert!(e.deletions[index].evidence.pending_clear(fresh).is_err());
        assert_eq!(roundtrip(&e), e);
    }
}

#[test]
fn pending_deleted_undo_removes_only_the_owned_flag_conditionally() {
    let (e, index) = marked();
    let deletion = &e.deletions[index].evidence;
    let fresh = read("INBOX", 101, &[4], &[910], true);
    let intent = deletion.pending_clear(fresh.clone()).unwrap();
    assert_eq!(
        intent.command(),
        "UID STORE 4 (UNCHANGEDSINCE 910) -FLAGS.SILENT (\\Deleted)"
    );
    let mut clear = deletion.begin_clear(intent.clone(), fresh).unwrap();
    clear = roundtrip(&clear);
    clear
        .record_result(acknowledged(&intent, &[(4, 920)]))
        .unwrap();
    clear
        .observe(read("INBOX", 101, &[4], &[920], false))
        .unwrap();
    assert_eq!(clear.verified()[&uid(4)].metadata, metadata(4));
    assert_eq!(roundtrip(&clear), clear);
    let later = read("INBOX", 101, &[4], &[911], true);
    assert!(deletion.begin_clear(intent, later).is_err());
}

#[test]
fn expunge_requires_fresh_exact_owned_metadata_and_unchanged_extra_copy() {
    for case in 0..5 {
        let (mut e, index) = marked();
        let mut src = read("INBOX", 101, &[4, 8], &[910, 911], true);
        let mut dest = destination().subset(&[uid(60), uid(64)].into());
        match case {
            0 => {
                src.messages
                    .get_mut(&uid(4))
                    .unwrap()
                    .metadata
                    .flags
                    .remove("\\Deleted");
            }
            1 => {
                src.messages.get_mut(&uid(4)).unwrap().modseq = seq(912);
            }
            2 => {
                dest.messages
                    .get_mut(&uid(60))
                    .unwrap()
                    .metadata
                    .flags
                    .insert("$later".into());
            }
            3 => {
                dest.messages.get_mut(&uid(60)).unwrap().modseq = seq(1009);
            }
            4 => {
                src.messages.insert(
                    uid(999),
                    VersionedMetadata {
                        metadata: metadata(999),
                        modseq: seq(910),
                    },
                );
            }
            _ => unreachable!(),
        }
        assert!(e.begin_expunge(index, src, dest).is_err());
        assert!(e.deletions[index].evidence.expunge.is_none());
    }
}

#[test]
fn expunge_attempt_cannot_be_repeated_or_followed_by_flag_compensation() {
    let (mut e, index) = marked();
    let src = read("INBOX", 101, &[4, 8], &[910, 911], true);
    let dest = destination().subset(&[uid(60), uid(64)].into());
    e.begin_expunge(index, src.clone(), dest.clone()).unwrap();
    let mut restored = roundtrip(&e);
    assert!(restored.begin_expunge(index, src.clone(), dest).is_err());
    assert!(
        restored.deletions[index]
            .evidence
            .pending_clear(src)
            .is_err()
    );
    restored
        .record_expunge_completion(index, Completion::Lost)
        .unwrap();
    assert!(
        restored
            .resolutions()
            .values()
            .any(|r| *r == Resolution::Uncertain)
    );
}

#[test]
fn negative_expunge_completion_is_not_success_without_final_observation() {
    let (mut e, index) = marked();
    let src = read("INBOX", 101, &[4, 8], &[910, 911], true);
    let dest = destination().subset(&[uid(60), uid(64)].into());
    e.begin_expunge(index, src.clone(), dest.clone()).unwrap();
    e.record_expunge_completion(
        index,
        Completion::Tagged {
            status: TaggedStatus::No,
            code: None,
        },
    )
    .unwrap();
    assert_eq!(e.resolutions()[&uid(4)], Resolution::Uncertain);
    e.observe_final(index, src, dest).unwrap();
    assert_eq!(e.resolutions()[&uid(4)], Resolution::Conflict);
    assert_eq!(roundtrip(&e), e);
}

#[test]
fn malformed_modified_and_extraneous_or_contradictory_updates_are_rejected() {
    let e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    for code in [
        "[MODIFIED ]",
        "[MODIFIED *]",
        "[MODIFIED 4,4]",
        "[MODIFIED 19]",
        "[MODIFIED 4:4294967295]",
        "[MODIFIED 4] suffix",
        "[MODIFIED 4:8:9]",
    ] {
        assert!(
            StoreResult::tagged(
                TaggedStatus::Ok,
                Some(code),
                BTreeMap::new(),
                group.intent()
            )
            .is_err(),
            "{code}"
        );
    }
    for (code, updated) in [
        (Some("[MODIFIED 4]"), [(uid(4), seq(910))].into()),
        (None, [(uid(19), seq(910))].into()),
        (None, [(uid(4), seq(900))].into()),
    ] {
        assert!(StoreResult::tagged(TaggedStatus::Ok, code, updated, group.intent()).is_err());
    }
    assert!(
        StoreResult::tagged(
            TaggedStatus::Bad,
            None,
            [(uid(4), seq(910))].into(),
            group.intent()
        )
        .is_err()
    );
}

#[test]
fn malformed_recorded_store_result_permanently_blocks_ownership() {
    let (mut e, index) = marked();
    assert!(e.record_store_result(index, StoreResult::Lost).is_err());
    let mut e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    let index = e.begin_delete(group).unwrap();
    let malformed = StoreResult::Tagged {
        status: TaggedStatus::Ok,
        modified: None,
        updated: [(uid(999), seq(910))].into(),
    };
    assert!(e.record_store_result(index, malformed).is_err());
    assert_eq!(
        e.deletions[index].evidence.flag.result(),
        Some(&StoreResult::Rejected)
    );
    assert!(e.record_store_result(index, StoreResult::Lost).is_err());
    assert_eq!(roundtrip(&e), e);
}

#[test]
fn zero_and_overflow_modseq_and_nonpersistent_readbacks_fail() {
    for text in ["0", "-1", "18446744073709551616", "1.5", "\"900\""] {
        assert!(serde_json::from_str::<ModSeq>(text).is_err());
    }
    assert_eq!(
        serde_json::from_str::<ModSeq>("18446744073709551615")
            .unwrap()
            .get(),
        u64::MAX
    );
    let mut raw = serde_json::to_value(source()).unwrap();
    raw["highest_modseq"] = 899.into();
    assert!(serde_json::from_value::<Readback>(raw).is_err());
    let mut raw = serde_json::to_value(source()).unwrap();
    raw.as_object_mut().unwrap().remove("highest_modseq");
    assert!(serde_json::from_value::<Readback>(raw).is_err());
}

#[test]
fn stale_readbacks_and_uidvalidity_changes_never_authorize_phases() {
    let mut e = evidence();
    let mut changed = source();
    changed.messages.get_mut(&uid(4)).unwrap().modseq = seq(901);
    assert!(e.begin_copy(&changed).is_err());
    let mut e = copied();
    let mut changed = source();
    changed.identity.uidvalidity = UidValidity::new(102).unwrap();
    assert!(e.prepare_deletions(&changed, &destination()).is_err());
    let mut changed = source();
    changed.messages.get_mut(&uid(4)).unwrap().modseq = seq(899);
    assert!(e.prepare_deletions(&changed, &destination()).is_err());
    let mut missing = source();
    missing.messages.remove(&uid(4));
    e.observe_copy(missing, destination()).unwrap();
    assert!(e.observe_copy(source(), destination()).is_err());
}

#[test]
fn deserialization_rejects_phase_smuggling_duplicate_members_and_changed_mappings() {
    let (e, _) = marked();
    for case in 0..9 {
        let mut raw = serde_json::to_value(&e).unwrap();
        match case {
            0 => {
                raw["copy"]["attempted"] = false.into();
            }
            1 => {
                raw["copy"]["completion"] = serde_json::Value::Null;
            }
            2 => {
                raw["readback"] = serde_json::Value::Null;
            }
            3 => {
                raw["readback"]["destination"]["messages"]["60"]["modseq"] = 0.into();
            }
            4 => {
                let duplicate = raw["deletions"][0].clone();
                raw["deletions"].as_array_mut().unwrap().push(duplicate);
            }
            5 => {
                raw["deletions"][0]["evidence"]["flag"]["intent"]["before"]["messages"]["4"]["modseq"] =
                    1.into();
            }
            6 => {
                raw["deletions"][0]["evidence"]["flag"]["result"]["modified"] =
                    serde_json::json!([4, 4]);
            }
            7 => {
                raw["deletions"][0]["evidence"]["flag"]["result"] = serde_json::Value::Null;
            }
            8 => {
                raw["copy"]["mapping"]["mapping"]["pairs"]["4"] = 61.into();
            }
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<CopyEvidence>(raw).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn serialization_does_not_imply_a_source_or_execution_permission() {
    let e = copied();
    let raw = serde_json::to_value(&e).unwrap();
    let text = serde_json::to_string(&raw).unwrap();
    assert!(!text.contains("password"));
    assert!(!text.contains("authority"));
    assert!(!text.contains("credential"));
    let mut raw = raw;
    raw["source"] = "fixture-message-move/v1".into();
    assert!(serde_json::from_value::<CopyEvidence>(raw).is_err());
}

#[test]
fn copy_only_undo_prepares_deletion_of_the_extra_copy_without_copying_back() {
    let e = copied();
    let mut src = source();
    src.messages
        .get_mut(&uid(4))
        .unwrap()
        .metadata
        .flags
        .insert("$later".into());
    src.messages.get_mut(&uid(4)).unwrap().modseq = seq(950);
    src.highest_modseq = seq(950);
    let groups = e.prepare_copy_cleanup(&src, &destination()).unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].before().identity().name.decoded, "Privat/News");
    assert_eq!(
        groups[0].command(),
        "UID STORE 60 (UNCHANGEDSINCE 1001) +FLAGS.SILENT (\\Deleted)"
    );
    assert!(groups.iter().all(|g| !g.command().contains("COPY")));
    assert!(e.deletions.is_empty());
    let mut changed = destination();
    changed
        .messages
        .get_mut(&uid(60))
        .unwrap()
        .metadata
        .flags
        .insert("$later".into());
    changed.messages.get_mut(&uid(60)).unwrap().modseq = seq(1100);
    changed.highest_modseq = seq(1100);
    assert!(
        e.prepare_copy_cleanup(&src, &changed)
            .unwrap()
            .iter()
            .all(|g| !g.before().messages().contains_key(&uid(60)))
    );
    let (e, _) = marked();
    assert!(
        e.prepare_copy_cleanup(&source(), &destination())
            .unwrap()
            .iter()
            .all(|g| !g.before().messages().contains_key(&uid(60))
                && !g.before().messages().contains_key(&uid(64)))
    );
}

#[test]
fn modified_response_codes_are_case_insensitive_but_uid_sets_remain_exact() {
    let e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    let upper = StoreResult::tagged(
        TaggedStatus::No,
        Some("[MODIFIED 8]"),
        [(uid(4), seq(910))].into(),
        group.intent(),
    )
    .unwrap();
    let lower = StoreResult::tagged(
        TaggedStatus::No,
        Some("[modified 8]"),
        [(uid(4), seq(910))].into(),
        group.intent(),
    )
    .unwrap();
    assert_eq!(upper, lower);
}

#[test]
fn pending_clear_never_takes_over_an_unrelated_deleted_message() {
    let (e, index) = marked();
    let deletion = &e.deletions[index].evidence;
    let unrelated = read("INBOX", 101, &[999], &[999], true);
    assert!(deletion.pending_clear(unrelated).is_err());
    let mut changed = read("INBOX", 101, &[4], &[910], true);
    changed
        .messages
        .get_mut(&uid(4))
        .unwrap()
        .metadata
        .flags
        .insert("$other".into());
    assert!(deletion.pending_clear(changed).is_err());
}

#[test]
fn conflicting_mapping_is_permanent_and_never_allows_source_deletion() {
    let mut e = evidence();
    e.begin_copy(&source()).unwrap();
    e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 60]", e.intent()).unwrap())
        .unwrap();
    assert!(
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 61]", e.intent()).unwrap())
            .is_err()
    );
    e.record_copy_completion(Completion::Lost).unwrap();
    let empty = read("Privat/News", 202, &[], &[], false);
    e.observe_copy(source(), empty.clone()).unwrap();
    assert!(e.prepare_deletions(&source(), &empty).is_err());
    assert!(e.resolutions().values().all(|r| *r == Resolution::Conflict));
    assert_eq!(roundtrip(&e), e);
}

#[test]
fn actual_modseq_must_exceed_the_observed_mailbox_highest_value() {
    let e = copied();
    let group = e
        .prepare_deletions(&source(), &destination())
        .unwrap()
        .remove(0);
    for stale in [900, 901, 905] {
        assert!(
            StoreResult::tagged(
                TaggedStatus::Ok,
                None,
                [(uid(4), seq(stale))].into(),
                group.intent()
            )
            .is_err()
        );
    }
}

#[test]
fn mapped_missing_or_changed_destination_never_establishes_filing() {
    for missing in [false, true] {
        let (mut e, index) = marked();
        let src = read("INBOX", 101, &[4, 8], &[910, 911], true);
        let dest = destination().subset(&[uid(60), uid(64)].into());
        e.begin_expunge(index, src, dest.clone()).unwrap();
        e.record_expunge_completion(index, Completion::Lost)
            .unwrap();
        let mut after = dest;
        if missing {
            after.messages.remove(&uid(60));
        } else {
            after
                .messages
                .get_mut(&uid(60))
                .unwrap()
                .metadata
                .flags
                .insert("$later".into());
        }
        e.observe_final(index, read("INBOX", 101, &[], &[], false), after)
            .unwrap();
        assert_eq!(e.resolutions()[&uid(4)], Resolution::Conflict);
        assert_eq!(roundtrip(&e), e);
    }
}

#[test]
fn empty_and_excessive_batches_and_duplicate_json_members_are_refused() {
    let before = source();
    let intent = evidence().intent().clone();
    assert!(CopyEvidence::new(intent.clone(), read("INBOX", 101, &[], &[], false)).is_err());
    let mut raw = serde_json::to_string(&before).unwrap();
    let entry = serde_json::to_string(&before.messages[&uid(4)]).unwrap();
    let at = raw.find("\"4\":").unwrap();
    raw.insert_str(at, &format!("\"4\":{entry},"));
    assert!(serde_json::from_str::<Readback>(&raw).is_err());
    let mut excessive = before.clone();
    excessive.messages = (1..=501)
        .map(|n| {
            (
                uid(n),
                VersionedMetadata {
                    metadata: metadata(n),
                    modseq: seq(900),
                },
            )
        })
        .collect();
    assert!(serde_json::from_value::<Readback>(serde_json::to_value(excessive).unwrap()).is_err());
    let (e, _) = marked();
    let mut raw = serde_json::to_value(&e).unwrap();
    raw["deletions"] = vec![raw["deletions"][0].clone(); 501].into();
    assert!(serde_json::from_value::<CopyEvidence>(raw).is_err());
}

#[test]
fn pending_clear_serialization_requires_the_exact_acknowledged_ownership_record() {
    let (e, index) = marked();
    let owner = &e.deletions[index].evidence;
    let fresh = read("INBOX", 101, &[4], &[910], true);
    let clear = owner
        .begin_clear(owner.pending_clear(fresh.clone()).unwrap(), fresh)
        .unwrap();
    assert_eq!(roundtrip(&clear), clear);
    for change in 0..4 {
        let mut raw = serde_json::to_value(&clear).unwrap();
        match change {
            0 => {
                raw["ownership"]["result"] = serde_json::json!({"kind":"lost"});
            }
            1 => {
                raw["effect"]["intent"]["before"]["messages"]["4"]["modseq"] = 911.into();
            }
            2 => {
                raw["ownership"]["readback"]["messages"]["4"]["metadata"]["flags"] =
                    serde_json::json!(["\\Deleted", "$other"]);
            }
            3 => {
                raw["effect"]["intent"]["change"] = "add_deleted".into();
            }
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<PendingClear>(raw).is_err(),
            "case {change}"
        );
    }
}

#[test]
fn copy_command_quotes_the_validated_exact_destination_name() {
    let before = source();
    let name = r#"Privat/Angebot \"A\""#;
    let destination = mailbox(name, 202);
    let intent = MoveIntent::new(
        Purpose::Filing {
            protect_flagged: false,
        },
        before.identity.clone(),
        destination,
        before.plain().messages,
    )
    .unwrap();
    let mut e = CopyEvidence::new(intent, before.clone()).unwrap();
    let command = e.begin_copy(&before).unwrap();
    assert_eq!(
        command,
        format!(
            "UID COPY 4,8,19 \"{}\"",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        )
    );
}

mod effects;
