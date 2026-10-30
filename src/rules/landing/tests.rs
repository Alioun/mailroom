use super::*;
use crate::{
    acquisition::Provider,
    census::{
        CensusKeys, EncryptedStore, LocalSecret,
        sieve::{self, Body, Context, ScriptName},
    },
    engine::{self, PreviewInput},
    rules::workspace::{ExportRecord, Workspace, WorkspaceInput},
};
use sha2::{Digest, Sha256};

fn fixture(
    foreign: &str,
    placement: composition::Placement,
) -> (tempfile::TempDir, EncryptedStore, AccountState) {
    fixture_for("synthetic-rules", foreign, placement)
}

pub(crate) fn fixture_for(
    account: &str,
    foreign: &str,
    placement: composition::Placement,
) -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let mut input: PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.context.account = account.into();
    input.documents.preset_yaml = "id: placement\nmajor: 1\nrules:\n  - id: sender.vendor\n    category: orders\n    stage: from\n    folder: Vendor\n    when: [{kind: from_domain, value: vendor.example.test}]\n".into();
    input.documents.override_yaml =
        "preset: placement\nmajor: 1\nlayout:\n  language: de\n  personal_root: Privat\n  business_root: Geschäftlich\n  new_addresses: Neue Adressen\n  retired_addresses: Stillgelegte Adressen\nroles:\n  domains: {home.example.test: personal}\n".into();
    input.messages = serde_json::from_value(serde_json::json!([
        {"id":"match", "headers":"From: sender@vendor.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: match\r\n", "current_folder":"Privat/Vendor"},
        {"id":"elsewhere", "headers":"From: sender@vendor.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: elsewhere\r\n", "current_folder":"INBOX"},
        {"id":"existing", "headers":"From: sender@vendor.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: existing\r\n", "current_folder":"Prior"},
        {"id":"hold", "headers":"From: sender@vendor.example.test\r\nDelivered-To: owner@unknown.example.test\r\nSubject: hold\r\n", "current_folder":"INBOX"}
    ])).unwrap();
    let artifact = engine::artifact(&input.documents, &input.context).unwrap();
    let mut census = engine::synthetic_census(&input).unwrap();
    census.last_completed_at = Some(input.as_of);
    let directory = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        directory.path().into(),
        CensusKeys::derive(
            &LocalSecret::from_bytes(vec![7; 32]).unwrap(),
            &census.account,
        ),
    )
    .unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    let target = "Open-Xchange";
    let script = composition::parse_foreign(foreign, target).unwrap();
    let composed =
        composition::compose(foreign, &script, &placement, artifact.executable(), target).unwrap();
    let image = |bytes| sieve_transaction::Image {
        capabilities: BTreeMap::from([("VERSION".into(), "1.0".into())]),
        names: vec![ScriptName {
            name: target.into(),
            active: true,
        }],
        bodies: vec![Body::new(target.into(), bytes)],
    };
    let before = image(foreign.as_bytes().to_vec());
    let mut after = image(composed.host);
    if let Some((name, bytes)) = composed.owned {
        after.names.push(ScriptName {
            name: name.clone(),
            active: false,
        });
        after.bodies.push(Body::new(name, bytes));
        after.names.sort_by(|a, b| a.name.cmp(&b.name));
        after.bodies.sort_by(|a, b| a.name.cmp(&b.name));
    }
    state.exports.push(ExportRecord {
        context_binding: Some("constructed-test-context".into()),
        semantics: program::SEMANTICS.into(),
        proof_semantics: proof::SEMANTICS.into(),
        artifact: artifact.clone(),
        workspace: Workspace {
            revision: 1,
            input: WorkspaceInput {
                documents: input.documents,
                pins: vec![],
                as_of: input.as_of,
                baseline: proof::Baseline::Current,
            },
        },
        receipt: "0".repeat(64),
        exported_at: 1,
        provenance: "Constructed local download. Not a deployment.".into(),
    });
    state
        .sieve_transactions
        .history
        .push(sieve_transaction::Transaction {
            id: "1".repeat(64),
            compensates: None,
            context: Context {
                account: census.account,
                provider: Provider::MailboxOrg,
                profile: "installed-loopback-starttls-plain".into(),
                source_epoch: "fixture".into(),
                tls_identity: "fixture".into(),
                authority: "fixture".into(),
                adapter: sieve::ADAPTER.into(),
            },
            target: target.into(),
            ownership_epoch: "fixture".into(),
            artifact: artifact.identity().into(),
            proof_binding: "2".repeat(64),
            receipt: "3".repeat(64),
            before: before.clone(),
            after: after.clone(),
            status: sieve_transaction::Status::Complete,
            validation: Some(Ok(())),
            space: Some(Ok(())),
            steps: vec![sieve_transaction::Step {
                effect: sieve_transaction::Effect::Upload,
                before,
                after,
                outcome: sieve_transaction::Outcome::Observed,
                acknowledgment: true,
                failure: None,
            }],
            composition: Some(sieve_transaction::Composition {
                purpose: sieve_transaction::Purpose::Deploy,
                semantics: composition::SEMANTICS.into(),
                owned: placement.owned().map(Into::into),
                placement: Some(placement),
                foreign_sha256: data_encoding::HEXLOWER.encode(&Sha256::digest(foreign.as_bytes())),
                existing_rules: vec![],
                precedence: None,
            }),
        });
    (directory, store, state)
}

const FOREIGN: &str = "require [\"fileinto\"];\nif header :contains \"subject\" \"existing\" { fileinto \"Prior\"; stop; }\n";

pub(crate) fn seed_deployment(saved: &mut AccountState) {
    let (_directory, _store, mut fixture) = fixture_for(
        saved.census.account.as_str(),
        FOREIGN,
        composition::Placement::Block {},
    );
    fixture
        .deployment_artifacts
        .insert(
            &fixture.sieve_transactions.history[0],
            fixture.exports[0].artifact.clone(),
        )
        .unwrap();
    saved.census = fixture.census;
    saved.sieve_transactions = fixture.sieve_transactions;
    saved.deployment_artifacts = fixture.deployment_artifacts;
    saved.exports.clear();
}

fn value(state: &AccountState) -> serde_json::Value {
    serde_json::to_value(compare(state, false)).unwrap()
}

#[test]
fn exact_deployment_compares_saved_folders_with_existing_rule_precedence_without_mutation() {
    let (_directory, store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    store.save_state(&mut state).unwrap();
    let encoded = std::fs::read(store.path()).unwrap();
    let saved = serde_json::to_vec(&state).unwrap();
    let report = value(&state);
    assert_eq!(report["kind"], "compared");
    assert_eq!(
        report["counts"],
        serde_json::json!({"matches":2,"elsewhere":1,"additional_folders":0,"uncheckable":1})
    );
    let rows = report["rows"].as_array().unwrap();
    let existing = rows.iter().find(|r| r["subject"] == "existing").unwrap();
    assert_eq!(existing["prediction"]["destination"]["name"], "Prior");
    assert_eq!(existing["prediction"]["origin"], "existing_rule");
    assert_eq!(serde_json::to_vec(&state).unwrap(), saved);
    assert_eq!(std::fs::read(store.path()).unwrap(), encoded);
    assert_eq!(value(&store.load_state().unwrap()), report);
    println!(
        "MAILROOM_LANDING_FIXTURE {}",
        serde_json::json!({"compared":report})
    );
}

#[test]
fn export_alone_or_missing_artifact_never_substitutes_current_editable_rules() {
    let (_directory, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    let transaction = state.sieve_transactions.history.pop().unwrap();
    assert_eq!(value(&state)["reason"], "no_recorded_deployment");
    state.sieve_transactions.history.push(transaction);
    state.workspace = Some(state.exports[0].workspace.clone());
    state.exports.clear();
    assert_eq!(value(&state)["reason"], "artifact_not_retained");
}

#[test]
fn altered_bytes_and_include_substitution_refuse_comparison() {
    let (_directory, _store, mut state) = fixture(
        FOREIGN,
        composition::Placement::Include {
            script: "mailroom-owned".into(),
        },
    );
    assert_eq!(value(&state)["kind"], "compared");
    let transaction = &mut state.sieve_transactions.history[0];
    let body = transaction
        .after
        .bodies
        .iter_mut()
        .find(|b| b.name == "mailroom-owned")
        .unwrap();
    *body = Body::new(body.name.clone(), b"keep;".to_vec());
    transaction.steps[0].after = transaction.after.clone();
    assert_eq!(value(&state)["reason"], "recorded_bytes_differ");
}

#[test]
fn historical_or_unsettled_records_and_absent_refresh_remain_unavailable() {
    let (_directory, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    state.sieve_transactions.history[0].status = sieve_transaction::Status::Uncertain;
    assert_eq!(value(&state)["reason"], "transaction_unsettled");
    state.sieve_transactions.history[0].status = sieve_transaction::Status::Complete;
    state.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap()
        .semantics = composition::LEGACY_SEMANTICS.into();
    assert_eq!(value(&state)["reason"], "historical_semantics");
    state.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap()
        .semantics = composition::SEMANTICS.into();
    state.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap()
        .purpose = sieve_transaction::Purpose::Removal;
    assert_eq!(value(&state)["reason"], "no_active_composed_deployment");
    state.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap()
        .purpose = sieve_transaction::Purpose::Deploy;
    state.census.last_completed_at = None;
    assert_eq!(value(&state)["reason"], "no_completed_refresh");
}

#[test]
fn refresh_recomputes_observation_and_preserves_partial_and_restored_status() {
    let (_directory, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    let before = value(&state);
    let mut folder = state.census.folders.remove("Prior").unwrap();
    for record in folder.messages.values_mut() {
        record.headers.subject = "later move".into();
    }
    state.census.folders.insert("Moved Later".into(), folder);
    state.census.acquisition_state = ScanState::Failed;
    let after = serde_json::to_value(compare(&state, true)).unwrap();
    assert_ne!(before["census_sha256"], after["census_sha256"]);
    assert_eq!(after["historical"], true);
    assert_eq!(after["scan_state"], "failed");
    assert_eq!(after["artifact"], before["artifact"]);
    assert_eq!(after["counts"]["elsewhere"], 2);
}

#[test]
fn nonfiling_existing_actions_do_not_become_missing_folder_failures() {
    let (_directory, _store, state) = fixture(
        "if header :contains \"subject\" \"existing\" { discard; stop; }\n",
        composition::Placement::Block {},
    );
    let report = value(&state);
    assert_eq!(report["counts"]["uncheckable"], 2);
    let row = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["subject"] == "existing")
        .unwrap();
    assert_eq!(row["prediction"]["reason"], "not_a_folder_destination");
}

#[test]
fn artifact_from_a_different_account_cannot_explain_this_accounts_observations() {
    let (_directory, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    let mut input: PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.context.account = "another-synthetic-account".into();
    let artifact =
        engine::artifact(&state.exports[0].workspace.input.documents, &input.context).unwrap();
    state.sieve_transactions.history[0].artifact = artifact.identity().into();
    state.exports[0].artifact = artifact;
    assert_eq!(value(&state)["reason"], "artifact_not_retained");
}

#[test]
fn retained_deployment_survives_export_eviction_and_encrypted_reopen_without_a_download() {
    let (_directory, store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    seed_deployment(&mut state);
    assert!(state.exports.is_empty());
    store.save_state(&mut state).unwrap();
    let report = value(&state);
    assert_eq!(report["baseline_source"], "retained_deployment");
    assert_eq!(report["counts"]["matches"], 2);
    assert_eq!(value(&store.load_state().unwrap()), report);
    println!(
        "MAILROOM_RETAINED_DEPLOYMENT_FIXTURE {}",
        serde_json::json!({"compared":report})
    );
}

#[test]
fn retained_artifact_is_immutable_and_orphaned_or_mismatched_records_are_rejected() {
    let (_directory, store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    let transaction = state.sieve_transactions.history[0].clone();
    let artifact = state.exports[0].artifact.clone();
    state
        .deployment_artifacts
        .insert(&transaction, artifact.clone())
        .unwrap();
    let before = serde_json::to_vec(&state.deployment_artifacts).unwrap();
    assert!(
        state
            .deployment_artifacts
            .insert(&transaction, artifact)
            .is_err()
    );
    assert_eq!(
        serde_json::to_vec(&state.deployment_artifacts).unwrap(),
        before
    );
    store.save_state(&mut state).unwrap();
    let encrypted = std::fs::read(store.path()).unwrap();
    state.sieve_transactions.history.clear();
    assert!(store.save_state(&mut state).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), encrypted);
    state.sieve_transactions.history.push(transaction);
    state.sieve_transactions.history[0].artifact = "0".repeat(64);
    assert!(store.save_state(&mut state).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), encrypted);
}

pub(crate) fn observe_active(state: &mut AccountState, at: i64) {
    state
        .deployment_artifacts
        .observe_active(
            &state.sieve_transactions.history[0],
            crate::census::deployment::ActiveObservation {
                at,
                via: crate::census::deployment::ObservationVia::Readback,
            },
        )
        .unwrap();
}

#[test]
fn mailbox_date_window_uses_internaldate_not_header_date_and_keeps_full_comparison() {
    let (_root, store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    seed_deployment(&mut state);
    let through = state.census.last_completed_at.unwrap();
    let after = through - 3600;
    observe_active(&mut state, after);
    let stamp = |at| {
        chrono::DateTime::from_timestamp(at, 0)
            .unwrap()
            .format("%d-%b-%Y %H:%M:%S %z")
            .to_string()
    };
    for record in state
        .census
        .folders
        .values_mut()
        .flat_map(|f| f.messages.values_mut())
    {
        record.internal_date = match record.headers.subject.as_str() {
            "match" => stamp(after + 1),
            "elsewhere" => stamp(after),
            "existing" => stamp(through),
            _ => "invalid".into(),
        };
        record.headers.date = Some("Wed, 01 Jan 1997 00:00:00 +0000".into());
    }
    let report = value(&state);
    assert_eq!(report["arrival_window"]["kind"], "timed");
    assert_eq!(report["arrival_window"]["after"], after);
    assert_eq!(report["arrival_window"]["through"], through);
    assert_eq!(report["arrival_window"]["counts"]["matches"], 2);
    assert_eq!(report["counts"]["elsewhere"], 1);
    let rows = report["rows"].as_array().unwrap();
    let period = |subject| {
        rows.iter().find(|r| r["subject"] == subject).unwrap()["arrival_period"]
            .as_str()
            .unwrap()
    };
    assert_eq!(period("elsewhere"), "before_or_at_observation");
    assert_eq!(period("existing"), "after_observation");
    assert_eq!(period("hold"), "invalid_date");
    store.save_state(&mut state).unwrap();
    assert_eq!(value(&store.load_state().unwrap()), report);
    println!("MAILROOM_ARRIVAL_WINDOW_FIXTURE {}", report);
    for record in state
        .census
        .folders
        .values_mut()
        .flat_map(|f| f.messages.values_mut())
    {
        record.internal_date = stamp(through + 1);
    }
    let future = value(&state);
    assert!(
        future["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["arrival_period"] == "after_refresh")
    );
    assert_eq!(future["arrival_window"]["counts"]["matches"], 0);
}

#[test]
fn legacy_and_reversed_chronology_never_invent_a_window() {
    let (_root, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    assert_eq!(
        value(&state)["arrival_window"]["reason"],
        "no_active_observation"
    );
    seed_deployment(&mut state);
    assert_eq!(
        value(&state)["arrival_window"]["reason"],
        "no_active_observation"
    );
    let observed = state.census.last_completed_at.unwrap() + 1;
    observe_active(&mut state, observed);
    let report = value(&state);
    assert_eq!(
        report["arrival_window"]["reason"],
        "refresh_predates_observation"
    );
    assert!(
        report["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["arrival_period"] == "unavailable")
    );
}

#[test]
fn active_observation_is_first_only_and_survives_later_context_conflict() {
    let (_root, store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    seed_deployment(&mut state);
    observe_active(&mut state, 123);
    let before = serde_json::to_vec(&state.deployment_artifacts).unwrap();
    state
        .deployment_artifacts
        .observe_active(
            &state.sieve_transactions.history[0],
            crate::census::deployment::ActiveObservation {
                at: 456,
                via: crate::census::deployment::ObservationVia::Reconciliation,
            },
        )
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&state.deployment_artifacts).unwrap(),
        before
    );
    state.sieve_transactions.history[0].status = sieve_transaction::Status::Conflict;
    store.save_state(&mut state).unwrap();
    let loaded = store.load_state().unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded.deployment_artifacts).unwrap(),
        before
    );
    assert_eq!(value(&loaded)["reason"], "transaction_unsettled");
}

#[test]
fn incomplete_unobserved_or_inactive_transactions_cannot_acquire_timing() {
    let (_root, _store, mut state) = fixture(FOREIGN, composition::Placement::Block {});
    seed_deployment(&mut state);
    let original = state.sieve_transactions.history[0].clone();
    for failure in 0..4 {
        let mut t = original.clone();
        match failure {
            0 => t.status = sieve_transaction::Status::Uncertain,
            1 => t.steps[0].outcome = sieve_transaction::Outcome::Acknowledged,
            2 => t.after.names[0].active = false,
            _ => t.steps.clear(),
        }
        assert!(
            state
                .deployment_artifacts
                .observe_active(
                    &t,
                    crate::census::deployment::ActiveObservation {
                        at: 123,
                        via: crate::census::deployment::ObservationVia::Readback
                    }
                )
                .is_err()
        );
        assert!(
            state
                .deployment_artifacts
                .get(&original.id)
                .unwrap()
                .active_observation
                .is_none()
        );
    }
}
