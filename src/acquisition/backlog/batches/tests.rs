use super::*;
use crate::{
    backlog::{
        MoveMethod, Operation,
        imap_copy::{ModSeq, Readback},
        protocol::tests::{Stage, run},
    },
    census::{Direction, MessageRecord, parse_headers},
};

fn fixture() -> (Run, Census) {
    let account = AccountId::new("batch-preview").unwrap();
    let operation = run(&account, Stage::Prepared);
    let mut census = Census::new(account);
    census
        .reconcile_folder("INBOX", 101, [4, 8].into())
        .unwrap();
    census
        .reconcile_folder("Privat/News", 202, BTreeSet::new())
        .unwrap();
    let records = operation.batches[0].command().messages().iter().map(|(uid, m)| (*uid, MessageRecord {
        delivery: Default::default(), uid: uid.get(), size: m.size.into(), internal_date: m.internal_date.clone(),
        flags_observed: true, flags: m.flags.clone(), gmail_message_id: None, gmail_thread_id: None, gmail_labels: BTreeSet::new(),
            headers: parse_headers(b"From: news@example.test\r\nTo: owner@example.test\r\nMessage-ID: <batch@example.test>\r\n\r\n", Direction::Received).unwrap(),
    }));
    census.folders.get_mut("INBOX").unwrap().messages =
        records.map(|(uid, m)| (uid.get(), m)).collect();
    (operation, census)
}

#[test]
fn copy_preview_batches_bind_exact_persistent_versions_without_attempting_effects() {
    let (operation, census) = fixture();
    let e = operation.batches[0].as_copy().unwrap();
    let mut reads = 0;
    let batches = prepare(
        &operation.plan.filing,
        &operation.plan.topology,
        &census,
        Profile::Copy,
        |intent| {
            reads += 1;
            assert_eq!(intent, e.intent());
            Ok(e.before().clone())
        },
    )
    .unwrap();
    assert_eq!(reads, 1);
    assert!(batches == operation.batches);
    assert!(!batches[0].attempted());
    let mut next = operation.clone();
    next.batches = batches;
    model::Journal {
        runs: vec![next],
        summaries: vec![],
    }
    .validate(&census.account)
    .unwrap();
}

#[test]
fn move_preview_keeps_historical_batch_bytes_and_never_requests_persistent_versions() {
    let (mut operation, census) = fixture();
    operation.plan.topology.capabilities.insert("MOVE".into());
    for c in &mut operation.plan.filing.changes {
        c.operation = Operation::Imap {
            method: MoveMethod::UidMove,
        };
    }
    let expected = MoveEvidence::intent(operation.batches[0].command().clone());
    let batches = prepare(
        &operation.plan.filing,
        &operation.plan.topology,
        &census,
        Profile::Move,
        |_| panic!("MOVE requested persistent versions"),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&batches[0]).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

#[test]
fn changed_observations_capabilities_or_plan_members_never_produce_a_copy_batch() {
    for mutation in [
        "size",
        "flags",
        "validity",
        "missing",
        "duplicate",
        "method",
        "move",
        "condstore",
        "uidplus",
        "observation",
    ] {
        let (mut operation, mut census) = fixture();
        let before = operation.batches[0].as_copy().unwrap().before().clone();
        match mutation {
            "size" => {
                census
                    .folders
                    .get_mut("INBOX")
                    .unwrap()
                    .messages
                    .get_mut(&4)
                    .unwrap()
                    .size += 1
            }
            "flags" => {
                census
                    .folders
                    .get_mut("INBOX")
                    .unwrap()
                    .messages
                    .get_mut(&4)
                    .unwrap()
                    .flags
                    .insert("$changed".into());
            }
            "validity" => census.folders.get_mut("INBOX").unwrap().uidvalidity += 1,
            "missing" => {
                census.folders.get_mut("INBOX").unwrap().messages.remove(&4);
            }
            "duplicate" => operation
                .plan
                .filing
                .changes
                .push(operation.plan.filing.changes[0].clone()),
            "method" => {
                operation.plan.filing.changes[0].operation = Operation::Imap {
                    method: MoveMethod::UidMove,
                }
            }
            "move" => {
                operation.plan.topology.capabilities.insert("MOVE".into());
            }
            "condstore" => {
                operation.plan.topology.capabilities.remove("CONDSTORE");
            }
            "uidplus" => {
                operation.plan.topology.capabilities.remove("UIDPLUS");
            }
            "observation" => (),
            _ => unreachable!(),
        }
        assert!(
            prepare(
                &operation.plan.filing,
                &operation.plan.topology,
                &census,
                Profile::Copy,
                |_| if mutation == "observation" {
                    Err(AcquisitionError::Incomplete)
                } else {
                    Ok(before.clone())
                }
            )
            .is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn changed_initial_versions_change_exact_confirmation_material() {
    let (operation, census) = fixture();
    let before = operation.batches[0].as_copy().unwrap().before();
    let changed = Readback::new(
        before.identity().clone(),
        ModSeq::new(before.highest_modseq().get() + 1).unwrap(),
        before.messages().clone(),
    )
    .unwrap();
    let batches = prepare(
        &operation.plan.filing,
        &operation.plan.topology,
        &census,
        Profile::Copy,
        |_| Ok(changed.clone()),
    )
    .unwrap();
    assert_ne!(
        serde_json::to_vec(&(&operation.plan, &batches)).unwrap(),
        serde_json::to_vec(&(&operation.plan, &operation.batches)).unwrap()
    );
}

#[test]
fn held_copy_account_actions_deny_before_credentials_or_journal_writes() {
    use crate::acquisition::{CredentialBackend, SecretValue};
    struct NoCredentials;
    impl CredentialBackend for NoCredentials {
        fn get(&self, _: &str) -> Result<SecretValue, AcquisitionError> {
            panic!("Unexpected credential read")
        }
        fn put(&self, _: &str, _: &SecretValue) -> Result<(), AcquisitionError> {
            panic!("Unexpected credential write")
        }
        fn delete(&self, _: &str) -> Result<(), AcquisitionError> {
            panic!("Unexpected credential deletion")
        }
    }
    let (operation, _) = fixture();
    let (_root, original, account, store, mut saved) =
        super::super::copy_checkpoints::tests::fixture(
            operation.batches[0].as_copy().unwrap().clone(),
        );
    let mut repo = Accounts::new(original.paths.clone(), std::sync::Arc::new(NoCredentials));
    repo.backlog_source = original.backlog_source;
    repo.workspace_authority = original.workspace_authority;
    assert!(!repo.backlog_installed(Some(&account.id)));
    let id = saved.protocol_backlog.runs[0].id.clone();
    let before = std::fs::read(store.path()).unwrap();
    assert_eq!(
        repo.backlog_run(
            &account,
            &store,
            &mut saved,
            &id,
            false,
            Cancellation::default(),
            |_| panic!("Unexpected freshness callback")
        ),
        Err(AcquisitionError::InvalidInput)
    );
    assert_eq!(
        repo.backlog_reconcile(&account, &store, &mut saved, &id, Cancellation::default()),
        Err(AcquisitionError::InvalidInput)
    );
    assert!(
        repo.backlog_undo_preview(&account, &store, &saved, &id, Cancellation::default())
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    saved.protocol_backlog.runs.clear();
    store.save_state(&mut saved).unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert_eq!(
        repo.backlog_confirm(
            &account,
            &store,
            &mut saved,
            Options::default(),
            &"0".repeat(64),
            Cancellation::default(),
            |_| panic!("Unexpected freshness callback")
        ),
        Err(AcquisitionError::InvalidInput)
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}
