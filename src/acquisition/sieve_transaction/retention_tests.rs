use super::*;

fn preview(state: &mut AccountState) -> Preview {
    let mut transaction = state.sieve_transactions.history.pop().unwrap();
    transaction.status = Status::Running;
    transaction.steps.clear();
    let artifact = state.exports[0].artifact.clone();
    state.exports.clear();
    Preview {
        receipt: transaction.receipt.clone(),
        transaction,
        proof: serde_json::json!({}),
        precedence: None,
        effects: vec![Effect::Upload],
        limitations: LIMITATIONS,
        deployment_artifact: Some(artifact),
    }
}

#[test]
fn confirmed_intent_persists_exact_artifact_before_effects_without_exporting() {
    let (_root, store, mut state) =
        crate::rules::landing::tests::fixture_for("intent-account", "", Placement::Block {});
    let reviewed = preview(&mut state);
    let identity = reviewed.transaction.artifact.clone();
    let operation = reviewed.transaction.id.clone();
    let serialized = serde_json::to_value(&reviewed).unwrap();
    assert!(serialized.get("deployment_artifact").is_none());
    assert!(state.deployment_artifacts.is_empty());
    persist_confirmed_intent(&store, &mut state, reviewed).unwrap();
    let reopened = store.load_state().unwrap();
    assert!(reopened.exports.is_empty());
    assert!(reopened.sieve_transactions.history[0].steps.is_empty());
    assert_eq!(
        reopened.sieve_transactions.history[0].status,
        Status::Stopped
    );
    assert_eq!(
        reopened
            .deployment_artifacts
            .get(&operation)
            .unwrap()
            .artifact
            .identity(),
        identity
    );
    assert_eq!(
        serde_json::to_value(crate::rules::landing::compare(&reopened, false)).unwrap()["reason"],
        "transaction_unsettled"
    );
}

#[test]
fn missing_material_refuses_before_journal_or_ciphertext_change() {
    let (_root, store, mut state) =
        crate::rules::landing::tests::fixture_for("missing-account", "", Placement::Block {});
    let mut reviewed = preview(&mut state);
    reviewed.deployment_artifact = None;
    let before = std::fs::read(store.path()).unwrap();
    assert!(persist_confirmed_intent(&store, &mut state, reviewed).is_err());
    assert!(state.sieve_transactions.history.is_empty());
    assert!(state.deployment_artifacts.is_empty());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn active_observation_records_actual_local_time_and_readback_or_reconciliation_provenance() {
    for via in [
        crate::census::deployment::ObservationVia::Readback,
        crate::census::deployment::ObservationVia::Reconciliation,
    ] {
        let (_root, store, mut saved) =
            crate::rules::landing::tests::fixture_for("clock-account", "", Placement::Block {});
        crate::rules::landing::tests::seed_deployment(&mut saved);
        let before = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        record_active_observation(&mut saved, 0, via).unwrap();
        save(&store, &mut saved).unwrap();
        let reopened = store.load_state().unwrap();
        let observation = reopened
            .deployment_artifacts
            .get(&reopened.sieve_transactions.history[0].id)
            .unwrap()
            .active_observation
            .unwrap();
        assert_eq!(observation.via, via);
        assert!(
            (before
                ..=i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                )
                .unwrap())
                .contains(&observation.at)
        );
        assert_eq!(
            reopened.sieve_transactions.history[0].status,
            Status::Complete
        );
    }
}
