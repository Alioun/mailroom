use super::*;

fn mailbox(name: &str, validity: u32) -> MailboxIdentity {
    MailboxIdentity {
        name: ExactName::new(name).unwrap(),
        uidvalidity: UidValidity::new(validity).unwrap(),
    }
}

fn metadata() -> Metadata {
    Metadata {
        message_id_hash: "corroboration-only".into(),
        size: 1200,
        internal_date: "25-Oct-2026 12:00:00 +0000".into(),
        flags: ["\\Seen".into(), "$unrelated".into()].into(),
    }
}

fn intent() -> MoveIntent {
    MoveIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        mailbox("INBOX", 101),
        mailbox("Filed/Receipts", 202),
        [4, 8, 19]
            .map(|n| (Uid::new(n).unwrap(), metadata()))
            .into(),
    )
    .unwrap()
}

#[test]
fn copyuid_preserves_sparse_order_and_expands_descending_ranges_ascending() {
    let i = intent();
    assert_eq!(i.wire_uids(), "4,8,19");
    let m = CopyUidMapping::parse("[COPYUID 202 4,8,19 60,64,70]", &i).unwrap();
    assert_eq!(m.pairs[&Uid::new(8).unwrap()].get(), 64);
    let i = MoveIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        i.source,
        i.destination,
        [4, 5, 6].map(|n| (Uid::new(n).unwrap(), metadata())).into(),
    )
    .unwrap();
    let m = CopyUidMapping::parse("[COPYUID 202 6:4 70:68]", &i).unwrap();
    assert_eq!(m.pairs[&Uid::new(4).unwrap()].get(), 68);
    assert_eq!(m.pairs[&Uid::new(6).unwrap()].get(), 70);
    assert!(CopyUidMapping::parse("[COPYUID 202 4 60]", &i).is_ok());
}

#[test]
fn mapping_rejects_wildcards_duplicates_extraneous_cardinality_and_unbounded_ranges() {
    for code in [
        "[COPYUID 0 4 60]",
        "[COPYUID 203 4 60]",
        "[COPYUID 202 0 60]",
        "[COPYUID 202 * 60]",
        "[COPYUID 202 4 0]",
        "[COPYUID 202 4 60,*]",
        "[COPYUID 202 4,4 60,61]",
        "[COPYUID 202 4,8 60,60]",
        "[COPYUID 202 4,8 60]",
        "[COPYUID 202 4,5 60,61]",
        "[COPYUID 202 4:4294967295 60:4294967295]",
        "[COPYUID 202 4 4294967296]",
        "[COPYUID 202 4 60:61]",
        "[COPYUID 202 4 60:60,60]",
        "[COPYUID 202 4 60] extra",
        "[COPYUID 202 +4 60]",
        "[COPYUID 202 4 -60]",
        "[COPYUID 202  4 60]",
        "[APPENDUID 202 4 60]",
        "[COPYUID 202 4 60:61:62]",
    ] {
        assert!(CopyUidMapping::parse(code, &intent()).is_err(), "{code}");
    }
    let i = intent();
    let overflow = format!("[COPYUID 202 {} 60]", "4,".repeat(9000));
    assert!(CopyUidMapping::parse(&overflow, &i).is_err());
}

#[test]
fn tagged_ok_source_absence_and_matching_metadata_never_replace_mapping() {
    let mut e = MoveEvidence::intent(intent());
    e.begin_attempt().unwrap();
    e.record_completion(Completion::Tagged {
        status: TaggedStatus::Ok,
        code: None,
    })
    .unwrap();
    e.observe(
        MailboxReadback {
            identity: e.intent.source.clone(),
            messages: BTreeMap::new(),
        },
        MailboxReadback {
            identity: e.intent.destination.clone(),
            messages: BTreeMap::new(),
        },
    )
    .unwrap();
    assert!(
        e.resolutions()
            .values()
            .all(|r| *r == Resolution::Uncertain)
    );
    assert!(
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 60]", &e.intent).unwrap())
            .is_err()
    );
    let mut destination = BTreeMap::new();
    destination.insert(Uid::new(60).unwrap(), metadata());
    assert!(
        e.observe(
            MailboxReadback {
                identity: e.intent.source.clone(),
                messages: BTreeMap::new()
            },
            MailboxReadback {
                identity: e.intent.destination.clone(),
                messages: destination
            }
        )
        .is_err()
    );
}

#[test]
fn partial_no_or_lost_completion_can_verify_only_mapped_readback() {
    for completion in [
        Completion::Lost,
        Completion::Tagged {
            status: TaggedStatus::No,
            code: Some("[OVERQUOTA]".into()),
        },
    ] {
        let mut e = MoveEvidence::intent(intent());
        e.begin_attempt().unwrap();
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4,19 60,70]", &e.intent).unwrap())
            .unwrap();
        e.record_completion(completion).unwrap();
        let source = MailboxReadback {
            identity: e.intent.source.clone(),
            messages: [(Uid::new(8).unwrap(), metadata())].into(),
        };
        let destination = MailboxReadback {
            identity: e.intent.destination.clone(),
            messages: [60, 70].map(|n| (Uid::new(n).unwrap(), metadata())).into(),
        };
        e.observe(source.clone(), destination.clone()).unwrap();
        assert_eq!(
            e.resolutions()[&Uid::new(4).unwrap()],
            Resolution::Verified {
                destination: Uid::new(60).unwrap()
            }
        );
        assert_eq!(
            e.resolutions()[&Uid::new(8).unwrap()],
            Resolution::Uncertain
        );
        let saved = serde_json::to_vec(&e).unwrap();
        let reopened: MoveEvidence = serde_json::from_slice(&saved).unwrap();
        assert_eq!(e, reopened);
        e.observe(source, destination).unwrap();
        assert_eq!(saved, serde_json::to_vec(&e).unwrap());
    }
}

#[test]
fn readback_conflicts_on_changed_identity_flags_date_size_or_source_presence() {
    for change in 0..7 {
        let mut e = MoveEvidence::intent(intent());
        e.begin_attempt().unwrap();
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 60]", &e.intent).unwrap())
            .unwrap();
        e.record_completion(Completion::Lost).unwrap();
        let mut source = MailboxReadback {
            identity: e.intent.source.clone(),
            messages: BTreeMap::new(),
        };
        let mut destination = MailboxReadback {
            identity: e.intent.destination.clone(),
            messages: [(Uid::new(60).unwrap(), metadata())].into(),
        };
        let m = destination
            .messages
            .get_mut(&Uid::new(60).unwrap())
            .unwrap();
        match change {
            0 => source.identity.uidvalidity = UidValidity::new(102).unwrap(),
            1 => destination.identity.uidvalidity = UidValidity::new(203).unwrap(),
            2 => {
                m.flags.insert("\\Flagged".into());
            }
            3 => m.internal_date.push(' '),
            4 => m.size += 1,
            5 => {
                source.messages.insert(Uid::new(4).unwrap(), metadata());
            }
            6 => {
                destination.messages.clear();
            }
            _ => unreachable!(),
        }
        e.observe(source, destination).unwrap();
        assert_eq!(e.resolutions()[&Uid::new(4).unwrap()], Resolution::Conflict);
    }
}

#[test]
fn limits_and_phase_checks_survive_deserialization() {
    assert!(Uid::new(0).is_err());
    assert!(serde_json::from_str::<Uid>("0").is_err());
    assert!(serde_json::from_str::<UidValidity>("0").is_err());
    let i = intent();
    assert!(
        MoveIntent::new(
            Purpose::Filing {
                protect_flagged: true
            },
            i.source.clone(),
            i.destination.clone(),
            (1..=501)
                .map(|n| (Uid::new(n).unwrap(), metadata()))
                .collect()
        )
        .is_err()
    );
    let mut raw = serde_json::to_value(&i).unwrap();
    raw["source"]["name"]["wire"] = "different".into();
    assert!(serde_json::from_value::<MoveIntent>(raw).is_err());
    let mut e = MoveEvidence::intent(i);
    e.begin_attempt().unwrap();
    let m = CopyUidMapping::parse("[COPYUID 202 4 60]", &e.intent).unwrap();
    e.record_mapping(m.clone()).unwrap();
    e.record_mapping(m).unwrap();
    assert!(
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 61]", &e.intent).unwrap())
            .is_err()
    );
    let readback = (
        MailboxReadback {
            identity: e.intent.source.clone(),
            messages: BTreeMap::new(),
        },
        MailboxReadback {
            identity: e.intent.destination.clone(),
            messages: BTreeMap::new(),
        },
    );
    assert!(e.observe(readback.0.clone(), readback.1.clone()).is_err());
    let mut raw = serde_json::to_value(&e).unwrap();
    raw["readback"] = serde_json::to_value(readback).unwrap();
    assert!(serde_json::from_value::<MoveEvidence>(raw).is_err());
}

#[test]
fn an_attempt_cannot_be_repeated_after_serialization_even_without_response_evidence() {
    let mut e = MoveEvidence::intent(intent());
    assert!(!e.attempted());
    assert!(e.record_completion(Completion::Lost).is_err());
    e.begin_attempt().unwrap();
    assert!(e.begin_attempt().is_err());
    let mut reopened: MoveEvidence =
        serde_json::from_slice(&serde_json::to_vec(&e).unwrap()).unwrap();
    assert!(reopened.attempted());
    assert!(reopened.begin_attempt().is_err());
    assert!(
        reopened
            .resolutions()
            .values()
            .all(|r| *r == Resolution::Uncertain)
    );
    let mut raw = serde_json::to_value(&e).unwrap();
    raw["attempted"] = false.into();
    raw["completion"] = serde_json::to_value(Completion::Lost).unwrap();
    assert!(serde_json::from_value::<MoveEvidence>(raw).is_err());
}

#[test]
fn rejected_or_conflicting_mapping_cannot_later_become_verified() {
    let mut e = MoveEvidence::intent(intent());
    e.begin_attempt().unwrap();
    e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 60]", e.command()).unwrap())
        .unwrap();
    assert!(
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 61]", e.command()).unwrap())
            .is_err()
    );
    assert!(e.mapping().is_none());
    assert!(
        e.record_mapping(CopyUidMapping::parse("[COPYUID 202 4 60]", e.command()).unwrap())
            .is_err()
    );
    e.record_completion(Completion::Lost).unwrap();
    let reopened: MoveEvidence = serde_json::from_slice(&serde_json::to_vec(&e).unwrap()).unwrap();
    assert!(
        reopened
            .resolutions()
            .values()
            .all(|r| *r == Resolution::Conflict)
    );
}

#[test]
fn filing_protects_deleted_draft_and_flagged_without_restricting_current_undo_flags() {
    for flag in ["\\Deleted", "\\Draft", "\\Flagged"] {
        let mut messages = intent().messages;
        messages
            .get_mut(&Uid::new(4).unwrap())
            .unwrap()
            .flags
            .insert(flag.into());
        assert!(
            MoveIntent::new(
                Purpose::Filing {
                    protect_flagged: true
                },
                mailbox("INBOX", 101),
                mailbox("Filed/Receipts", 202),
                messages.clone()
            )
            .is_err()
        );
        assert!(
            MoveIntent::new(
                Purpose::Filing {
                    protect_flagged: false
                },
                mailbox("INBOX", 101),
                mailbox("Filed/Receipts", 202),
                messages.clone()
            )
            .is_ok()
                == (flag == "\\Flagged")
        );
        assert!(
            MoveIntent::new(
                Purpose::ConditionalUndo,
                mailbox("Filed/Receipts", 202),
                mailbox("INBOX", 101),
                messages
            )
            .is_ok()
        );
    }
}

#[test]
fn duplicate_uid_json_keys_and_oversized_collections_reject_without_losing_entries() {
    let raw = serde_json::to_string(&intent()).unwrap();
    let value = serde_json::to_string(&metadata()).unwrap();
    let duplicate = raw.replace(
        &format!("\"4\":{value}"),
        &format!("\"4\":{value},\"04\":{value}"),
    );
    assert!(serde_json::from_str::<MoveIntent>(&duplicate).is_err());
    assert!(
        serde_json::from_str::<CopyUidMapping>(
            "{\"uidvalidity\":202,\"pairs\":{\"4\":60,\"04\":61}}"
        )
        .is_err()
    );
    let mut raw = serde_json::to_value(intent()).unwrap();
    raw["messages"] = serde_json::to_value(
        (1..=501)
            .map(|n| (n.to_string(), metadata()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap();
    assert!(serde_json::from_value::<MoveIntent>(raw.clone()).is_err());
    raw["messages"].as_object_mut().unwrap().remove("501");
    let full: MoveIntent = serde_json::from_value(raw).unwrap();
    assert_eq!(full.messages().len(), 500);
    let source = (1..=500)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let destination = (1001..=1500)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mapping =
        CopyUidMapping::parse(&format!("[COPYUID 202 {source} {destination}]"), &full).unwrap();
    assert_eq!(mapping.pairs().len(), 500);
}

#[test]
fn exact_names_support_unicode_ampersand_quotes_backslashes_and_reject_unsupported_profiles() {
    for name in [
        "Filed/Belege & Bücher",
        "Filed/\"quoted\"",
        "Filed/back\\slash",
        "Filed/台北",
    ] {
        let i = MoveIntent::new(
            Purpose::ConditionalUndo,
            mailbox("INBOX", 101),
            mailbox(name, 202),
            [(Uid::new(4).unwrap(), metadata())].into(),
        )
        .unwrap();
        let reopened: MoveIntent =
            serde_json::from_slice(&serde_json::to_vec(&i).unwrap()).unwrap();
        assert_eq!(i, reopened);
    }
    for name in ["Filed/e\u{301}", "Filed/\u{1100}", "Filed/\n"] {
        assert!(ExactName::new(name).is_err());
    }
    let mut m = metadata();
    assert!(!m.protected(true));
    m.flags.insert("\\Deleted".into());
    assert!(m.protected(false));
}
