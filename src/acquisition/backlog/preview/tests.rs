use super::*;
use crate::{
    backlog::{
        MoveMethod, Operation, Scope,
        imap_copy::{ModSeq, VersionedMetadata},
        protocol::tests::{Stage, run},
    },
    census::{Direction, MessageRecord, parse_headers},
};

pub(in crate::acquisition) fn fixture(
    profile: Profile,
) -> (
    tempfile::TempDir,
    Accounts,
    Account,
    EncryptedStore,
    AccountState,
    Inventory,
    Readback,
) {
    let seed = run(&AccountId::new("preview-input").unwrap(), Stage::Prepared);
    let evidence = seed.batches[0].as_copy().unwrap().clone();
    let (root, mut repo, account, store, mut saved) =
        super::super::copy_checkpoints::tests::fixture(evidence.clone());
    let mut topology = saved.protocol_backlog.runs.remove(0).plan.topology;
    if profile == Profile::Move {
        std::sync::Arc::get_mut(repo.backlog_source.as_mut().unwrap())
            .unwrap()
            .authority = "fixture-message-move/v1".into();
        topology.capabilities.insert("MOVE".into());
    }
    saved.workspace.as_mut().unwrap().input.documents = crate::engine::Documents {
        preset_yaml: "id: preview\nmajor: 1\nrules: []\n".into(),
        override_yaml: "preset: preview\nmajor: 1\nlayout:\n  language: de\n  personal_root: Privat\n  business_root: Geschäftlich\n  new_addresses: Neue Adressen\n  retired_addresses: Stillgelegte Adressen\nroles:\n  domains: {home.example.test: personal}\nadditions:\n  - id: user.news\n    category: newsletters\n    stage: from\n    when: [{kind: from_address, value: news@example.test}]\n    folder: News\n".into(),
    };
    let mut observed = Census::new(account.id.clone());
    observed
        .reconcile_folder("INBOX", 101, [4, 8].into())
        .unwrap();
    observed
        .reconcile_folder("Privat/News", 202, BTreeSet::new())
        .unwrap();
    for (uid, metadata) in evidence.intent().messages() {
        let headers = parse_headers(
            b"From: news@example.test\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: Saved rules preview\r\nMessage-ID: <conditional@example.test>\r\nDate: Sat, 24 Oct 2026 12:00:00 +0000\r\n\r\n",
            Direction::Received,
        ).unwrap();
        observed
            .folders
            .get_mut("INBOX")
            .unwrap()
            .insert(MessageRecord {
                delivery: Default::default(),
                uid: uid.get(),
                size: 100,
                internal_date: metadata.internal_date.clone(),
                flags_observed: true,
                flags: metadata.flags.clone(),
                gmail_message_id: None,
                gmail_thread_id: None,
                gmail_labels: BTreeSet::new(),
                headers,
            })
            .unwrap();
    }
    let versions = observed.folders["INBOX"]
        .messages
        .iter()
        .map(|(uid, message)| {
            (
                Uid::new(*uid).unwrap(),
                VersionedMetadata {
                    metadata: Metadata {
                        message_id_hash: message.headers.message_id_hash.clone().unwrap(),
                        size: message.size.try_into().unwrap(),
                        internal_date: message.internal_date.clone(),
                        flags: message.flags.clone(),
                    },
                    modseq: ModSeq::new(9_007_199_254_740_993).unwrap(),
                },
            )
        })
        .collect();
    let before = Readback::new(
        evidence.intent().source().clone(),
        ModSeq::new(9_007_199_254_740_993).unwrap(),
        versions,
    )
    .unwrap();
    saved.census = observed.into();
    store.save_state(&mut saved).unwrap();
    (root, repo, account, store, saved, topology, before)
}

pub(in crate::acquisition) fn review(
    repo: &Accounts,
    account: &Account,
    store: &EncryptedStore,
    saved: &AccountState,
    topology: Inventory,
    persistent: impl FnMut(&MoveIntent) -> Result<Readback, AcquisitionError>,
) -> Result<Preview, AcquisitionError> {
    let context = repo.workspace_context(account, store)?;
    let saved_proof = workspace::preview(saved, &context, store.keys()).map_err(failure)?;
    assert!(saved_proof.download_available);
    prepare(
        saved,
        store,
        Options {
            scope: Scope::Inbox,
            ..Options::default()
        },
        Evidence {
            saved_proof,
            binding: repo.backlog_binding(account, store, saved)?,
            topology,
            observed: (*saved.census).clone(),
            profile: repo.backlog_source(account)?.profile().map_err(failure)?,
        },
        persistent,
    )
}

#[test]
fn saved_rules_and_observed_versions_produce_an_exact_consumable_copy_preview() {
    let (_root, mut repo, account, store, mut saved, topology, before) = fixture(Profile::Copy);
    let credentials = repo.track_credential_accesses();
    let ciphertext = std::fs::read(store.path()).unwrap();
    let mut reads = 0;
    let preview = review(
        &repo,
        &account,
        &store,
        &saved,
        topology.clone(),
        |intent| {
            reads += 1;
            assert_eq!(intent.source(), before.identity());
            assert_eq!(intent.messages().len(), before.messages().len());
            Ok(before.clone())
        },
    )
    .unwrap();
    assert_eq!(reads, 1);
    assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
    assert_eq!(preview.operation.plan.filing.changes.len(), 2);
    assert!(
        preview
            .operation
            .plan
            .filing
            .changes
            .iter()
            .all(|change| change.destination == "Privat/News"
                && change.operation
                    == Operation::Imap {
                        method: MoveMethod::CopyUidExpunge
                    })
    );
    assert_eq!(preview.operation.plan.saved_proof, preview.saved.receipt);
    assert!(preview.operation.plan.filing.artifact == preview.execution.artifact);
    let repeated = review(&repo, &account, &store, &saved, topology, |_| {
        Ok(before.clone())
    })
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&preview).unwrap(),
        serde_json::to_vec(&repeated).unwrap()
    );
    println!(
        "MAILROOM_FORWARD_PREVIEW_UI_FIXTURE {}",
        serde_json::json!({"method":"copy","account":account.id,"preview":preview})
    );
    let receipt = preview.receipt.clone();
    let id = super::super::copy_start::tests::confirm_prepared(
        &repo,
        &account,
        &store,
        &mut saved,
        preview.operation,
        &receipt,
    )
    .unwrap();
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.protocol_backlog.runs[0].id, id);
    assert_eq!(reopened.protocol_backlog.runs[0].receipt, receipt);
    assert!(!reopened.protocol_backlog.runs[0].batches[0].attempted());
    assert_eq!(credentials.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!repo.backlog_installed(Some(&account.id)));
}

#[test]
fn saved_rule_move_preview_retains_legacy_batch_shape_and_never_requests_versions() {
    let (_root, repo, account, store, saved, topology, _before) = fixture(Profile::Move);
    let preview = review(&repo, &account, &store, &saved, topology, |_| {
        panic!("MOVE requested persistent versions")
    })
    .unwrap();
    assert_eq!(preview.operation.plan.filing.changes.len(), 2);
    assert!(
        preview
            .operation
            .batches
            .iter()
            .all(|batch| !batch.is_copy())
    );
    for batch in &preview.operation.batches {
        assert_eq!(
            serde_json::to_vec(batch).unwrap(),
            serde_json::to_vec(&MoveEvidence::intent(batch.command().clone())).unwrap()
        );
    }
    println!(
        "MAILROOM_FORWARD_PREVIEW_UI_FIXTURE {}",
        serde_json::json!({"method":"move","account":account.id,"preview":preview})
    );
}

#[test]
fn changed_persistent_metadata_or_versions_cannot_reuse_the_saved_rule_copy_receipt() {
    let (_root, repo, account, store, saved, topology, before) = fixture(Profile::Copy);
    let preview = review(&repo, &account, &store, &saved, topology.clone(), |_| {
        Ok(before.clone())
    })
    .unwrap();
    let mut versions = before.messages().clone();
    for message in versions.values_mut() {
        message.modseq = ModSeq::new(message.modseq.get() + 1).unwrap();
    }
    let updated = Readback::new(
        before.identity().clone(),
        ModSeq::new(before.highest_modseq().get() + 1).unwrap(),
        versions,
    )
    .unwrap();
    let next = review(&repo, &account, &store, &saved, topology.clone(), |_| {
        Ok(updated.clone())
    })
    .unwrap();
    assert_ne!(next.receipt, preview.receipt);
    let mut versions = before.messages().clone();
    versions.values_mut().next().unwrap().metadata.size += 1;
    let changed =
        Readback::new(before.identity().clone(), before.highest_modseq(), versions).unwrap();
    let ciphertext = std::fs::read(store.path()).unwrap();
    assert!(
        review(&repo, &account, &store, &saved, topology, |_| Ok(
            changed.clone()
        ))
        .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
}
