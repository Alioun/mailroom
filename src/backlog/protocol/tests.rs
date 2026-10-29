use super::*;
use crate::{
    backlog::{
        Change, Identity, MoveMethod, Operation,
        imap_copy::{CopyEvidence, ModSeq, Readback, StoreResult, VersionedMetadata},
    },
    census::{
        CensusKeys, EncryptedStore, LocalSecret,
        folder_provisioning::{ExactName, Mailbox, Namespace, Namespaces},
    },
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Prepared,
    CopyUncertain,
    Copied,
    StoreUncertain,
    Marked,
    ExpungeUncertain,
    Filed,
    Undone,
}
const BASE: u64 = 9_007_199_254_740_993;
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
        message_id_hash: format!("copy-journal-{n}"),
        size: 100 + n,
        internal_date: "24-Oct-2026 12:00:00 +0000".into(),
        flags: ["\\Seen".into(), "$owner".into()].into(),
    }
}
fn read(identity: MailboxIdentity, members: &[(u32, u32, u64)], deleted: bool) -> Readback {
    Readback::new(
        identity,
        seq(members
            .iter()
            .map(|(_, _, s)| *s)
            .max()
            .unwrap_or(BASE + 100)),
        members
            .iter()
            .map(|(u, original, s)| {
                let mut m = metadata(*original);
                if deleted {
                    m.flags.insert("\\Deleted".into());
                }
                (
                    uid(*u),
                    VersionedMetadata {
                        metadata: m,
                        modseq: seq(*s),
                    },
                )
            })
            .collect(),
    )
    .unwrap()
}
fn copy(stage: Stage, reverse: bool) -> CopyEvidence {
    let source = if reverse {
        mailbox("Privat/News", 202)
    } else {
        mailbox("INBOX", 101)
    };
    let destination = if reverse {
        mailbox("INBOX", 101)
    } else {
        mailbox("Privat/News", 202)
    };
    let ids = if reverse { [60, 64] } else { [4, 8] };
    let targets = if reverse { [80, 84] } else { [60, 64] };
    let before = read(
        source.clone(),
        &[(ids[0], 4, BASE), (ids[1], 8, BASE)],
        false,
    );
    let intent = MoveIntent::new(
        if reverse {
            Purpose::ConditionalUndo
        } else {
            Purpose::Filing {
                protect_flagged: true,
            }
        },
        source.clone(),
        destination.clone(),
        before
            .messages()
            .iter()
            .map(|(u, m)| (*u, m.metadata.clone()))
            .collect(),
    )
    .unwrap();
    let mut e = CopyEvidence::new(intent, before.clone()).unwrap();
    if matches!(stage, Stage::Prepared) {
        return e;
    }
    e.begin_copy(&before).unwrap();
    if matches!(stage, Stage::CopyUncertain) {
        return e;
    }
    e.record_mapping(
        CopyUidMapping::parse(
            &format!(
                "[COPYUID {} {},{} {},{}]",
                destination.uidvalidity.get(),
                ids[0],
                ids[1],
                targets[0],
                targets[1]
            ),
            e.intent(),
        )
        .unwrap(),
    )
    .unwrap();
    e.record_copy_completion(Completion::Lost).unwrap();
    let copied = read(
        destination.clone(),
        &[(targets[0], 4, BASE + 10), (targets[1], 8, BASE + 10)],
        false,
    );
    e.observe_copy(before.clone(), copied.clone()).unwrap();
    if matches!(stage, Stage::Copied) {
        return e;
    }
    let group = e.prepare_deletions(&before, &copied).unwrap().remove(0);
    let index = e.begin_delete(group).unwrap();
    if matches!(stage, Stage::StoreUncertain) {
        return e;
    }
    e.record_store_result(
        index,
        StoreResult::Tagged {
            status: TaggedStatus::Ok,
            modified: None,
            updated: [(uid(ids[0]), seq(BASE + 20)), (uid(ids[1]), seq(BASE + 20))].into(),
        },
    )
    .unwrap();
    let flagged = read(
        source.clone(),
        &[(ids[0], 4, BASE + 20), (ids[1], 8, BASE + 20)],
        true,
    );
    e.observe_store(index, flagged.clone()).unwrap();
    if matches!(stage, Stage::Marked) {
        return e;
    }
    e.begin_expunge(index, flagged, copied.clone()).unwrap();
    if matches!(stage, Stage::ExpungeUncertain) {
        return e;
    }
    e.record_expunge_completion(index, Completion::Lost)
        .unwrap();
    e.observe_final(index, read(source, &[], false), copied)
        .unwrap();
    e
}

pub(crate) fn run(account: &AccountId, stage: Stage) -> Run {
    let mut input: crate::engine::PreviewInput =
        serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json")).unwrap();
    input.context.account = account.as_str().into();
    let artifact = crate::engine::artifact(&input.documents, &input.context).unwrap();
    let e = copy(stage, false);
    let changes = e
        .intent()
        .messages()
        .iter()
        .map(|(u, m)| Change {
            source: Identity {
                folder: "INBOX".into(),
                uidvalidity: 101,
                uid: u.get(),
                message_id_hash: m.message_id_hash.clone(),
                gmail_id: None,
            },
            flags: m.flags.clone(),
            original_labels: BTreeSet::new(),
            destination: "Privat/News".into(),
            sender: "news@synthetic.example.test".into(),
            operation: Operation::Imap {
                method: MoveMethod::CopyUidExpunge,
            },
        })
        .collect();
    let mut r = Run {
        id: "a".repeat(64),
        receipt: "b".repeat(64),
        plan: SavedPlan {
            filing: Plan {
                account: account.as_str().into(),
                provider: crate::acquisition::Provider::MailboxOrg,
                source: "synthetic-copy-journal".into(),
                state_revision: 0,
                ledger_version: String::new(),
                mailbox_revision: None,
                semantics: COPY_SEMANTICS.into(),
                artifact,
                proof_gate: "warnings".into(),
                options: Options::default(),
                changes,
                excluded: BTreeMap::new(),
                unmatched: 0,
                review: 0,
                groups: BTreeMap::new(),
                stopped_folders: BTreeSet::new(),
                diagnostics: vec![],
            },
            binding: Binding {
                account: account.clone(),
                source: "c".repeat(64),
                authority: "d".repeat(64),
                workspace: "e".repeat(64),
                material: "f".repeat(64),
            },
            topology: Inventory {
                capabilities: [
                    "IMAP4REV1".into(),
                    "NAMESPACE".into(),
                    "UIDPLUS".into(),
                    "CONDSTORE".into(),
                ]
                .into(),
                namespaces: Namespaces {
                    personal: vec![Namespace {
                        prefix: String::new(),
                        delimiter: Some("/".into()),
                    }],
                    other: vec![],
                    shared: vec![],
                },
                mailboxes: ["INBOX", "Privat/News"]
                    .into_iter()
                    .map(|name| Mailbox {
                        name: ExactName::from_wire(name).unwrap(),
                        delimiter: Some("/".into()),
                        attributes: BTreeSet::new(),
                    })
                    .collect(),
            },
            saved_proof: "1".repeat(64),
            execution_proof: "2".repeat(64),
        },
        status: Status::Prepared,
        stop_requested: false,
        batches: vec![Batch::copy(e)],
        reverse: vec![],
        compensation: vec![],
        undo_receipt: None,
        undo_conflicts: 0,
        completed_at: None,
        closed: None,
    };
    if !matches!(stage, Stage::Prepared) {
        r.status = outcome(&r, false);
    }
    if matches!(stage, Stage::Undone) {
        r.reverse.push(Reverse {
            forward: 0,
            evidence: Batch::copy(copy(Stage::Filed, true)),
        });
        r.undo_receipt = Some("3".repeat(64));
        r.status = outcome(&r, true);
    }
    r.stamp(1793260800);
    Journal {
        runs: vec![r.clone()],
        summaries: vec![],
    }
    .validate(account)
    .unwrap();
    r
}
fn journal(account: &AccountId, stage: Stage) -> Journal {
    Journal {
        runs: vec![run(account, stage)],
        summaries: vec![],
    }
}

#[test]
fn copy_journals_classify_known_partial_effects_without_claiming_completion() {
    let account = AccountId::new("copy-state").unwrap();
    for (stage, status) in [
        (Stage::Prepared, Status::Prepared),
        (Stage::CopyUncertain, Status::Uncertain),
        (Stage::Copied, Status::Stopped),
        (Stage::StoreUncertain, Status::Uncertain),
        (Stage::Marked, Status::Stopped),
        (Stage::ExpungeUncertain, Status::Uncertain),
        (Stage::Filed, Status::Complete),
        (Stage::Undone, Status::Undone),
    ] {
        let j = journal(&account, stage);
        assert_eq!(j.runs[0].status, status, "{stage:?}");
        let bytes = serde_json::to_vec(&j).unwrap();
        let loaded: Journal = serde_json::from_slice(&bytes).unwrap();
        loaded.validate(&account).unwrap();
        assert_eq!(serde_json::to_vec(&loaded).unwrap(), bytes);
    }
}

#[test]
fn move_wrapper_keeps_historical_serialization_exact_and_never_accepts_copy_fields() {
    let account = AccountId::new("move-serialization").unwrap();
    let old = MoveEvidence::intent(run(&account, Stage::Prepared).batches[0].command().clone());
    let bytes = serde_json::to_vec(&old).unwrap();
    let wrapped: Batch = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&wrapped).unwrap(), bytes);
    let mut raw = serde_json::to_value(&old).unwrap();
    raw["kind"] = "copy_uid_expunge".into();
    assert!(serde_json::from_value::<Batch>(raw).is_err());
    let mut copy = Batch::copy(copy(Stage::Prepared, false));
    assert!(copy.begin_attempt().is_err());
    assert!(!copy.attempted());
}

#[test]
fn account_bound_copy_plan_rejects_wrong_method_semantics_membership_and_reverse() {
    let account = AccountId::new("copy-validation").unwrap();
    let j = journal(&account, Stage::Undone);
    for mutation in 0..8 {
        let mut raw = serde_json::to_value(&j).unwrap();
        match mutation {
            0 => {
                raw["runs"][0]["plan"]["filing"]["semantics"] =
                    "imap-move-mapping-readback/v1".into();
            }
            1 => {
                raw["runs"][0]["plan"]["filing"]["changes"][0]["operation"]["method"] =
                    "uid_move".into();
            }
            2 => {
                raw["runs"][0]["plan"]["filing"]["changes"][0]["source"]["uid"] = 99.into();
            }
            3 => {
                raw["runs"][0]["plan"]["filing"]["changes"][0]["flags"] =
                    serde_json::json!(["\\Answered"]);
            }
            4 => {
                raw["runs"][0]["reverse"][0]["forward"] = 100.into();
            }
            5 => {
                raw["runs"][0]["plan"]["topology"]["capabilities"] =
                    serde_json::json!(["IMAP4REV1", "NAMESPACE", "MOVE", "UIDPLUS", "CONDSTORE"]);
            }
            6 => {
                raw["runs"][0]["plan"]["topology"]["capabilities"] =
                    serde_json::json!(["IMAP4REV1", "NAMESPACE", "UIDPLUS"]);
            }
            7 => {
                raw["runs"][0]["plan"]["binding"]["account"] = "other-account".into();
            }
            _ => unreachable!(),
        }
        let changed: Journal = serde_json::from_value(raw).unwrap();
        assert!(changed.validate(&account).is_err(), "mutation {mutation}");
    }
}

#[test]
fn copy_journal_encrypts_each_phase_and_reopening_never_resumes_effects() {
    let account = AccountId::new("encrypted-copy-phases").unwrap();
    let root = tempfile::tempdir().unwrap();
    let secret = LocalSecret::random();
    let store =
        EncryptedStore::new(root.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    store
        .save(&crate::census::Census::new(account.clone()))
        .unwrap();
    let mut state = store.load_state().unwrap();
    for stage in [
        Stage::Prepared,
        Stage::CopyUncertain,
        Stage::Copied,
        Stage::StoreUncertain,
        Stage::Marked,
        Stage::ExpungeUncertain,
        Stage::Filed,
        Stage::Undone,
    ] {
        state.protocol_backlog = journal(&account, stage);
        store.save_state(&mut state).unwrap();
        let expected = serde_json::to_vec(&state.protocol_backlog).unwrap();
        let ciphertext = std::fs::read(store.path()).unwrap();
        assert!(!ciphertext.windows(14).any(|w| w == b"copy-journal-4"));
        assert_eq!(
            serde_json::to_vec(&store.load_state().unwrap().protocol_backlog).unwrap(),
            expected
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
    }
    println!(
        "MAILROOM_COPY_JOURNAL_FIXTURE {}",
        serde_json::json!({"account": account, "journal": state.protocol_backlog, "copied": journal(&account, Stage::Copied), "marked": journal(&account, Stage::Marked), "uncertain": journal(&account, Stage::StoreUncertain)})
    );
}

#[test]
fn interrupted_copy_commands_reopen_uncertain_and_cannot_become_complete() {
    let account = AccountId::new("interrupted-copy").unwrap();
    let mut j = journal(&account, Stage::StoreUncertain);
    j.runs[0].status = Status::Running;
    j.interrupted();
    assert_eq!(j.runs[0].status, Status::Uncertain);
    assert!(!j.runs[0].finished());
    assert!(!j.runs[0].closable());
    j.validate(&account).unwrap();
}

#[test]
fn modseq_text_round_trips_the_entire_unsigned_64_bit_range() {
    for value in [1, BASE, u64::MAX] {
        let seq = seq(value);
        let bytes = serde_json::to_vec(&seq).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            format!("\"{value}\"")
        );
        assert_eq!(serde_json::from_slice::<ModSeq>(&bytes).unwrap(), seq);
    }
    for value in [
        "0",
        "9007199254740993",
        "\"0\"",
        "\"01\"",
        "\"18446744073709551616\"",
    ] {
        assert!(serde_json::from_str::<ModSeq>(value).is_err());
    }
}
