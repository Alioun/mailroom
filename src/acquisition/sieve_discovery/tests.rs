use super::*;
use crate::acquisition::{
    Provider, SecretValue,
    managesieve::tests::{Fake, PRIVATE_SCRIPT},
    persistent_test_accounts,
};
use std::sync::Arc;
#[test]
fn encrypted_actual_discovery_refresh_failures_reopen_and_exact_aggregate_preservation() {
    let root = tempfile::tempdir().unwrap();
    let mode = root.path().join("sieve-mode");
    let fake = Fake::start("normal", Some(mode.clone()));
    let mut repo = persistent_test_accounts(root.path());
    let source = super::super::managesieve::InstalledSource {
        address: fake.source.address,
        tls_name: fake.source.tls_name.clone(),
        root: fake.source.root.clone(),
        epoch: fake.source.epoch.clone(),
        command_timeout: fake.source.command_timeout,
        attempt_timeout: fake.source.attempt_timeout,
    };
    repo.sieve_source = Some(Arc::new(source));
    let account = repo
        .create(
            Provider::MailboxOrg,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut initial = store.load_state().unwrap();
    seed(&mut initial, store.keys());
    store.save_state(&mut initial).unwrap();
    let mut before = serde_json::to_value(&initial).unwrap();
    before.as_object_mut().unwrap().remove("revision");
    before.as_object_mut().unwrap().remove("sieve");
    drop(store);
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    assert_eq!(store.schema_version().unwrap(), 14);
    let baseline = state.sieve.last_successful.clone().unwrap();
    assert!(baseline.bodies[0].bytes == PRIVATE_SCRIPT);
    assert!(
        state
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
    assert!(
        !std::fs::read(store.path())
            .unwrap()
            .windows(20)
            .any(|w| w == &PRIVATE_SCRIPT[..20])
    );
    drop(store);
    for scenario in [
        "auth_denied",
        "duplicate_caps",
        "legacy",
        "partial_list",
        "two_active",
        "duplicate_names",
        "bye",
        "missing",
        "denied_get",
        "overflow",
        "oversize",
        "eof",
        "unstable",
        "unstable_names",
        "unstable_caps",
        "excess_caps",
        "bad_escape",
        "bad_crlf",
        "control_name",
        "too_many_names",
        "leading_zero",
        "malformed_code",
        "attempt_limit",
        "partial_bodies",
        "line_separator",
        "paragraph_separator",
        "bom_name",
        "unassigned_name",
        "long_quoted_wire",
        "bad_quoted_utf8",
        "error_literal",
        "post_starttls",
        "unsupported_sasl",
        "many_caps",
        "cap_count",
    ] {
        std::fs::write(&mode, scenario).unwrap();
        assert!(
            repo.discover_sieve(&account.id, Cancellation::default())
                .is_err(),
            "{scenario}"
        );
        let store = repo.store(&account).unwrap();
        let state = store.load_state().unwrap();
        assert_eq!(
            state.sieve.latest.as_ref().unwrap().outcome,
            Outcome::Failed,
            "{scenario}"
        );
        assert_eq!(
            state.sieve.last_successful.as_ref().unwrap(),
            &baseline,
            "{scenario}"
        );
        assert!(
            !state
                .sieve
                .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
        );
        let mut unrelated = serde_json::to_value(state).unwrap();
        unrelated.as_object_mut().unwrap().remove("revision");
        unrelated.as_object_mut().unwrap().remove("sieve");
        assert_eq!(unrelated, before, "{scenario}");
    }
    std::fs::write(&mode, "two_bodies").unwrap();
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    assert_eq!(
        repo.store(&account)
            .unwrap()
            .load_state()
            .unwrap()
            .sieve
            .last_successful
            .unwrap()
            .bodies
            .len(),
        2
    );
    for scenario in ["changed", "empty"] {
        std::fs::write(&mode, scenario).unwrap();
        repo.discover_sieve(&account.id, Cancellation::default())
            .unwrap();
        let state = repo.store(&account).unwrap().load_state().unwrap();
        assert!(state.sieve.last_successful.unwrap().bodies.is_empty());
    }
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    state.sieve.latest.as_mut().unwrap().outcome = Outcome::Running;
    state.sieve.latest.as_mut().unwrap().finished_at = None;
    store.save_state(&mut state).unwrap();
    drop(store);
    let reopened = persistent_test_accounts(root.path());
    let state = reopened
        .store(&reopened.load(&account.id).unwrap())
        .unwrap()
        .load_state()
        .unwrap();
    assert_eq!(state.sieve.latest.unwrap().outcome, Outcome::Interrupted);
    assert!(
        fake.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("AUTHENTICATE"))
            .count()
            >= 17
    );
}

pub(crate) fn seed(state: &mut AccountState, keys: &crate::census::CensusKeys) {
    use crate::{
        backlog, invoice,
        rules::{proof, workspace},
        unsubscribe, winddown,
    };
    use std::collections::{BTreeMap, BTreeSet};
    if state.proposals.history.is_empty() {
        crate::rules::proposals::tests::seed(state, keys);
    }
    let (folder_name, folder) = state
        .census
        .folders
        .iter()
        .find(|(_, f)| !f.messages.is_empty())
        .unwrap();
    let message = folder.messages.values().next().unwrap();
    let identity = backlog::Identity {
        folder: folder_name.clone(),
        uidvalidity: folder.uidvalidity,
        uid: message.uid,
        message_id_hash: message.headers.message_id_hash.clone().unwrap(),
        gmail_id: None,
    };
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .pins
        .push(proof::Pin {
            account: state.census.account.as_str().into(),
            message_id_hash: identity.message_id_hash.clone(),
            expected: proof::Destination::Inbox,
        });
    let export = workspace::preview(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
    )
    .unwrap();
    workspace::confirm_export(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        &export.receipt,
    )
    .unwrap();
    let plan = backlog::preview(
        state,
        Provider::MailboxOrg,
        keys,
        backlog::Options::default(),
        None,
    )
    .unwrap()
    .plan;
    state.backlog.push(backlog::Run {
        id: "invented-completed-run".into(),
        plan,
        status: backlog::Status::Completed,
        outcomes: vec![],
        undo: vec![],
        undo_requested: false,
        batches: 1,
        error: None,
    });
    state.synthetic_effects.insert(
        "independent-effect".into(),
        backlog::Observation {
            destination: identity,
            commands: vec!["invented-effect".into()],
        },
    );
    state.synthetic_uid_next.insert("retained-uid".into(), 321);
    state.ledger.source_version = "invented-ledger-version".into();
    state.ledger.document = Some(Default::default());
    state.winddown.dismissed.insert("retained-dismissal".into());
    state.winddown.history.push(winddown::DecisionRecord {
        receipt: "retained-receipt".into(),
        input_version: "retained-version".into(),
        changed: winddown::Change::Dismiss {
            rows: BTreeSet::from(["retained-dismissal".into()]),
            dismissed: true,
        },
    });
    invoice::initialize_synthetic(state);
    let source = state.invoices.sources.values().next().unwrap().clone();
    let plan = invoice::Plan {
        account: state.census.account.as_str().into(),
        state_revision: state.revision,
        report_version: "invented-report".into(),
        selection: source.id.clone(),
        source,
        content_digest: "invented-digest".into(),
        target_name: "invented-target".into(),
        target: "sink@example.test".into(),
        sending_address: "invented@example.test".into(),
        semantics: invoice::SEMANTICS.into(),
        confirmation_action: "invented-completed".into(),
        mode: invoice::SendMode::SyntheticTransport,
    };
    state.invoices.sends.push(invoice::Send {
        id: "retained-send".into(),
        plan: plan.clone(),
        status: invoice::Status::Completed,
    });
    state
        .invoices
        .synthetic_received
        .insert("independent-send".into(), plan);
    state.unsubscribe.history.push(unsubscribe::Entry {
        id: "retained-unsubscribe".into(),
        subscription: "invented-list".into(),
        sender_key: "invented-sender".into(),
        method: unsubscribe::Method::Keep,
        time: 1792915200,
        status: unsubscribe::Status::Kept,
        host: None,
        payload_digest: "invented-digest".into(),
        source_ids: vec!["invented-source".into()],
        http_status: None,
        success: "local retention".into(),
    });
    state.unsubscribe.synthetic_acknowledgments.insert(
        "independent-unsubscribe".into(),
        unsubscribe::Acknowledgment {
            payload_digest: "invented-digest".into(),
            host: None,
            http_status: None,
            mail_handoff: false,
            verb: "none".into(),
            form: None,
            headers: BTreeMap::new(),
        },
    );
}

#[test]
fn sieve_currentness_password_key_source_rotation_and_census_save_preserve_history() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::start("normal", None);
    let mut repo = persistent_test_accounts(root.path());
    repo.install_sieve_fixture(&fake);
    let mut account = repo
        .create(
            Provider::MailboxOrg,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    {
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        seed(&mut saved, store.keys());
        store.save_state(&mut saved).unwrap();
    }
    let old_proof = {
        let store = repo.store(&account).unwrap();
        crate::rules::workspace::preview(
            &store.load_state().unwrap(),
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
        )
        .unwrap()
        .receipt
    };
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let good = saved.sieve.clone();
    assert!(
        crate::rules::workspace::confirm_export(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &old_proof
        )
        .is_err()
    );
    let before = serde_json::to_value(&saved).unwrap();
    store.save(&saved.census).unwrap();
    assert_eq!(store.load_state().unwrap().sieve, good);
    let raw = std::fs::read(store.path()).unwrap();
    saved = store.load_state().unwrap();
    saved.sieve.last_successful.as_mut().unwrap().bodies[0].sha256 = "0".repeat(64);
    assert!(store.save_state(&mut saved).is_err());
    assert!(std::fs::read(store.path()).unwrap() == raw);
    drop(store);
    repo.replace_password(
        &account.id,
        &SecretValue::new(b"replacement-invented".to_vec()),
    )
    .unwrap();
    account = repo.load(&account.id).unwrap();
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert_eq!(saved.sieve, good);
    assert!(
        !saved
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
    drop(store);
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let before_rotate = repo.store(&account).unwrap().load_state().unwrap();
    repo.rotate(&account.id, &root.path().join("mailboxes"))
        .unwrap();
    account = repo.load(&account.id).unwrap();
    let store = repo.store(&account).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(after.sieve, before_rotate.sieve);
    assert!(
        !after
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
    let mut unrelated = serde_json::to_value(after).unwrap();
    unrelated.as_object_mut().unwrap().remove("sieve");
    unrelated.as_object_mut().unwrap().remove("revision");
    let mut before = before;
    before.as_object_mut().unwrap().remove("sieve");
    before.as_object_mut().unwrap().remove("revision");
    assert_eq!(unrelated, before);
    drop(store);
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    {
        let store = repo.store(&account).unwrap();
        assert!(
            store
                .load_state()
                .unwrap()
                .sieve
                .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
        );
    }
    let changed = Fake::start("normal", None);
    repo.install_sieve_fixture(&changed);
    let store = repo.store(&account).unwrap();
    assert!(
        !store
            .load_state()
            .unwrap()
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
}

#[test]
fn sieve_independent_reopen_worker() {
    use sha2::{Digest, Sha256};
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let repo = persistent_test_accounts(root);
    let account = repo
        .load(&AccountId::new(&std::env::var("MAILROOM_SIEVE_ACCOUNT").unwrap()).unwrap())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    let mut unrelated = serde_json::to_value(&saved).unwrap();
    unrelated.as_object_mut().unwrap().remove("sieve");
    unrelated.as_object_mut().unwrap().remove("revision");
    let output = serde_json::json!({"schema":store.schema_version().unwrap(),"unrelated_sha256": data_encoding::HEXLOWER.encode(&Sha256::digest(serde_json::to_vec(&unrelated).unwrap())),"latest":saved.sieve.latest.as_ref().map(|a| a.outcome),"success":saved.sieve.last_successful.as_ref().map(|b| &b.attempt),"body_digests":saved.sieve.last_successful.as_ref().map(|b| b.bodies.iter().map(|s| &s.sha256).collect::<Vec<_>>())});
    std::fs::write(
        root.join("sieve-reopen.json"),
        serde_json::to_vec(&output).unwrap(),
    )
    .unwrap();
}

#[test]
fn sieve_seed_worker() {
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_FIXTURE") else {
        return;
    };
    let repo = persistent_test_accounts(std::path::Path::new(&root));
    let id = AccountId::new(&std::env::var("MAILROOM_SIEVE_ACCOUNT").unwrap()).unwrap();
    let account = repo.load(&id).unwrap();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    seed(&mut saved, store.keys());
    store.save_state(&mut saved).unwrap();
}

#[test]
fn sieve_evidence_invalidates_download_and_proposal_receipts_without_compiler_authority() {
    use crate::rules::{
        proposals::{self, Disposition, Selected, Selection},
        workspace,
    };
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::start("normal", None);
    let mut repo = persistent_test_accounts(root.path());
    repo.install_sieve_fixture(&fake);
    let account = repo
        .create(
            Provider::MailboxOrg,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    seed(&mut saved, store.keys());
    saved
        .proposals
        .history
        .retain(|d| d.disposition == Disposition::Accept);
    store.save_state(&mut saved).unwrap();
    let old_proof = workspace::preview(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
    )
    .unwrap();
    assert!(old_proof.download_available);
    let generated = proposals::generate(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
    )
    .unwrap();
    let candidate = generated.candidates.first().unwrap();
    let selected = Selection {
        account: generated.account,
        generation: generated.generation,
        candidates: vec![Selected {
            id: candidate.id.clone(),
            change: candidate.change.clone(),
        }],
    };
    let old_review = proposals::review(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
        &selected,
        Disposition::Reject,
    )
    .unwrap();
    drop(store);
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    assert!(
        workspace::confirm_export(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &old_proof.receipt
        )
        .is_err()
    );
    assert!(
        proposals::confirm(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &selected,
            Disposition::Reject,
            &old_review.receipt
        )
        .is_err()
    );
    assert!(
        proposals::confirm(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &selected,
            Disposition::Reject,
            &old_proof.receipt
        )
        .is_err()
    );
    let fresh = workspace::preview(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
    )
    .unwrap();
    assert_ne!(fresh.receipt, old_proof.receipt);
    assert_eq!(
        fresh.proof.artifact.identity(),
        old_proof.proof.artifact.identity()
    );
    assert!(fresh.proof.artifact.context().capabilities.is_empty());
}

#[test]
fn missing_password_and_pre_cancelled_attempt_persist_before_any_connection() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::start("normal", None);
    let mut repo = persistent_test_accounts(root.path());
    repo.install_sieve_fixture(&fake);
    let account = repo
        .create(
            Provider::MailboxOrg,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let good = repo
        .store(&account)
        .unwrap()
        .load_state()
        .unwrap()
        .sieve
        .last_successful;
    let commands = fake.commands.lock().unwrap().len();
    std::fs::remove_file(
        root.path()
            .join("fake-credentials")
            .join(format!("{}-password", account.id.as_str())),
    )
    .unwrap();
    assert!(
        repo.discover_sieve(&account.id, Cancellation::default())
            .is_err()
    );
    let failed = repo.store(&account).unwrap().load_state().unwrap().sieve;
    assert_eq!(
        failed.latest.unwrap().failure,
        Some(super::super::managesieve::Error::CredentialUnavailable)
    );
    assert_eq!(failed.last_successful, good);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        repo.discover_sieve(&account.id, cancel),
        Err(AcquisitionError::Cancelled)
    );
    let failed = repo.store(&account).unwrap().load_state().unwrap().sieve;
    assert_eq!(failed.latest.unwrap().outcome, Outcome::Cancelled);
    assert_eq!(failed.last_successful, good);
    assert_eq!(fake.commands.lock().unwrap().len(), commands);
}

#[test]
fn successful_large_baseline_never_prunes_to_fit_recovery_resource_limit() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::start("max_body", None);
    let mut repo = persistent_test_accounts(root.path());
    repo.install_sieve_fixture(&fake);
    let account = repo
        .create(
            Provider::MailboxOrg,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert_eq!(
        saved.sieve.last_successful.as_ref().unwrap().bodies[0]
            .bytes
            .len(),
        1048576
    );
    let before = std::fs::read(store.path()).unwrap();
    drop(store);
    assert!(repo.capture_preview(&account.id, 1792915200).is_err());
    let store = repo.store(&account).unwrap();
    assert!(std::fs::read(store.path()).unwrap() == before);
    assert_eq!(store.load_state().unwrap().sieve, saved.sieve);
}
