use super::*;
use crate::rules::agent_edits::Rejection;

fn rejection() -> Rejection {
    Rejection {
        edit: "a".repeat(64),
        target: "sender.vendor".into(),
        summary: "b".repeat(64),
        as_of: 0,
    }
}

fn stage(store: &EncryptedStore, version: u32, state: &AccountState) {
    store
        .stage_snapshot(version, &serde_json::to_string(state).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
}

#[test]
fn agent_rejections_require_schema_thirty_three_in_snapshots_and_frames() {
    let (_root, store, saved) = crate::rules::landing::tests::fixture_for(
        "agent-rejection-schema",
        "",
        crate::rules::composition::Placement::Block {},
    );
    let mut rejected = saved.clone();
    rejected.proposals.agent_rejections.push(rejection());

    stage(&store, 32, &saved);
    let mut migrated = store.load_state().expect("schema 32 without rejections");
    assert!(migrated.proposals.agent_rejections.is_empty());

    stage(&store, 32, &rejected);
    assert!(
        matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
        "schema 32 snapshot with rejections"
    );

    stage(&store, 32, &saved);
    let mut bytes = fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys.encryption.as_ref()).unwrap();
    let base = log::decode(&cipher, store.keys.account.as_str(), &bytes).unwrap();
    let change = patch::diff(
        &serde_json::to_value(&saved.proposals).unwrap(),
        &serde_json::to_value(&rejected.proposals).unwrap(),
    )
    .unwrap();
    let frame = serde_json::to_vec(
        &serde_json::json!({"revision":saved.revision+1,"fields":{"proposals":change}}),
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
    assert!(
        matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
        "schema 32 frame adding rejections"
    );

    for state in [&saved, &rejected] {
        stage(&store, 33, state);
        assert_eq!(
            store.load_state().unwrap().proposals.agent_rejections.len(),
            state.proposals.agent_rejections.len()
        );
    }

    stage(&store, 32, &saved);
    migrated = store.load_state().unwrap();
    migrated.proposals.agent_rejections.push(rejection());
    store.save_state(&mut migrated).unwrap();
    assert_eq!(store.schema_version().unwrap(), 33);
    assert_eq!(crate::census::SCHEMA, 33);
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.proposals.agent_rejections.len(), 1);
    assert_eq!(reopened.proposals.agent_rejections[0].edit, "a".repeat(64));
}
