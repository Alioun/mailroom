use super::*;

#[test]
fn deployment_artifacts_require_schema_thirty_in_snapshots_and_frames() {
    let (_root, store, mut saved) = crate::rules::landing::tests::fixture_for(
        "retention-schema",
        "",
        crate::rules::composition::Placement::Block {},
    );
    crate::rules::landing::tests::seed_deployment(&mut saved);
    for version in 1..30 {
        store
            .stage_snapshot(version, &serde_json::to_string(&saved).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        decoded::forget(&store.path());
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "snapshot {version}"
        );
    }
    store
        .stage_snapshot(30, &serde_json::to_string(&saved).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
    let loaded = store.load_state().unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded.deployment_artifacts).unwrap(),
        serde_json::to_vec(&saved.deployment_artifacts).unwrap()
    );
    saved.deployment_artifacts = Default::default();
    for version in 18..30 {
        store
            .stage_snapshot(version, &serde_json::to_string(&saved).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        decoded::forget(&store.path());
        let mut bytes = fs::read(store.path()).unwrap();
        let cipher = Aes256Gcm::new_from_slice(store.keys.encryption.as_ref()).unwrap();
        let base = log::decode(&cipher, store.keys.account.as_str(), &bytes).unwrap();
        let change = patch::Patch::Replace {
            value: serde_json::json!({"records":[]}),
        };
        let frame = serde_json::to_vec(&serde_json::json!({"revision":saved.revision+1,"fields":{"deployment_artifacts":change}})).unwrap();
        bytes.extend(
            log::encode_frame(
                &cipher,
                store.keys.account.as_str(),
                &base.base_tag,
                0,
                &frame,
            )
            .unwrap(),
        );
        fs::write(store.path(), bytes).unwrap();
        decoded::forget(&store.path());
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "frame {version}"
        );
    }
}

#[test]
fn schema_twenty_nine_absence_stays_empty_and_new_saves_own_the_field() {
    let account = AccountId::new("legacy-retention").unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        root.path().into(),
        CensusKeys::derive(&LocalSecret::from_bytes(vec![3; 32]).unwrap(), &account),
    )
    .unwrap();
    let saved = AccountState::new(Census::new(account));
    store
        .stage_snapshot(29, &serde_json::to_string(&saved).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut loaded = store.load_state().unwrap();
    assert!(loaded.deployment_artifacts.is_empty());
    assert!(
        serde_json::to_value(&loaded)
            .unwrap()
            .get("deployment_artifacts")
            .is_none()
    );
    crate::rules::landing::tests::seed_deployment(&mut loaded);
    store.save_state(&mut loaded).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    assert_eq!(
        serde_json::to_value(crate::rules::landing::compare(
            &store.load_state().unwrap(),
            false
        ))
        .unwrap()["baseline_source"],
        "retained_deployment"
    );
}

#[test]
fn active_observation_requires_schema_thirty_one_in_snapshot_and_incremental_frame() {
    let (_root, store, mut saved) = crate::rules::landing::tests::fixture_for(
        "observation-schema",
        "",
        crate::rules::composition::Placement::Block {},
    );
    crate::rules::landing::tests::seed_deployment(&mut saved);
    let legacy = saved.clone();
    crate::rules::landing::tests::observe_active(&mut saved, 123);
    store
        .stage_snapshot(30, &serde_json::to_string(&saved).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    store
        .stage_snapshot(30, &serde_json::to_string(&legacy).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
    let loaded = store.load_state().unwrap();
    assert!(
        loaded
            .deployment_artifacts
            .get(&saved.sieve_transactions.history[0].id)
            .unwrap()
            .active_observation
            .is_none()
    );
    let mut bytes = fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys.encryption.as_ref()).unwrap();
    let base = log::decode(&cipher, store.keys.account.as_str(), &bytes).unwrap();
    let change = patch::diff(
        &serde_json::to_value(&legacy.deployment_artifacts).unwrap(),
        &serde_json::to_value(&saved.deployment_artifacts).unwrap(),
    )
    .unwrap();
    let frame = serde_json::to_vec(
        &serde_json::json!({"revision":saved.revision+1,"fields":{"deployment_artifacts":change}}),
    )
    .unwrap();
    bytes.extend(
        log::encode_frame(
            &cipher,
            store.keys.account.as_str(),
            &base.base_tag,
            0,
            &frame,
        )
        .unwrap(),
    );
    fs::write(store.path(), bytes).unwrap();
    decoded::forget(&store.path());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    store
        .stage_snapshot(
            crate::census::SCHEMA,
            &serde_json::to_string(&saved).unwrap(),
        )
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
    assert_eq!(
        serde_json::to_vec(&store.load_state().unwrap().deployment_artifacts).unwrap(),
        serde_json::to_vec(&saved.deployment_artifacts).unwrap()
    );
}
