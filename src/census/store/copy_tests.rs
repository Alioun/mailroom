use super::*;
use crate::backlog::{
    imap_move::MoveEvidence,
    protocol::{
        self, Batch,
        tests::{Stage, run},
    },
};

fn snapshot(store: &EncryptedStore, version: u32, value: &AccountState) {
    store
        .stage_snapshot(version, &serde_json::to_string(value).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    decoded::forget(&store.path());
}
fn fixture() -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let root = tempfile::tempdir().unwrap();
    let account = AccountId::new("copy-schema").unwrap();
    let store = EncryptedStore::new(
        root.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    let state = AccountState::new(Census::new(account));
    (root, store, state)
}
fn append(store: &EncryptedStore, state: &AccountState, change: patch::Patch) {
    let mut bytes = fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys.encryption.as_ref()).unwrap();
    let base = log::decode(&cipher, store.keys.account.as_str(), &bytes).unwrap();
    let body = serde_json::to_vec(&serde_json::json!({"revision": state.revision + 1, "fields": {"protocol_backlog": change}})).unwrap();
    bytes.extend(
        log::encode_frame(
            &cipher,
            store.keys.account.as_str(),
            &base.base_tag,
            0,
            &body,
        )
        .unwrap(),
    );
    fs::write(store.path(), bytes).unwrap();
    decoded::forget(&store.path());
}

#[test]
fn copy_schema_twenty_six_rejects_older_base_and_frame_records() {
    let (_root, store, original) = fixture();
    let mut current = original.clone();
    current
        .protocol_backlog
        .runs
        .push(run(&current.census.account, Stage::Undone));
    for version in 1..26 {
        snapshot(&store, version, &current);
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "base {version}"
        );
    }
    snapshot(&store, 26, &current);
    assert_eq!(
        serde_json::to_vec(&store.load_state().unwrap().protocol_backlog).unwrap(),
        serde_json::to_vec(&current.protocol_backlog).unwrap()
    );
    for version in 18..26 {
        snapshot(&store, version, &original);
        assert!(store.load_state().is_ok(), "old empty {version}");
        append(
            &store,
            &original,
            patch::Patch::Replace {
                value: serde_json::to_value(&current.protocol_backlog).unwrap(),
            },
        );
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "frame {version}"
        );
    }
}

#[test]
fn move_schema_twenty_five_and_legacy_synthetic_copy_remain_readable() {
    let (_root, store, mut state) = fixture();
    let mut moved = run(&state.census.account, Stage::Prepared);
    moved.plan.filing.semantics = "imap-move-mapping-readback/v1".into();
    moved.plan.topology.capabilities.insert("MOVE".into());
    for c in &mut moved.plan.filing.changes {
        c.operation = crate::backlog::Operation::Imap {
            method: crate::backlog::MoveMethod::UidMove,
        };
    }
    let old = MoveEvidence::intent(moved.batches[0].command().clone());
    let bytes = serde_json::to_vec(&old).unwrap();
    moved.batches = vec![Batch::from(old)];
    state.protocol_backlog.runs = vec![moved.clone()];
    state
        .protocol_backlog
        .validate(&state.census.account)
        .unwrap();
    snapshot(&store, 25, &state);
    let loaded = store.load_state().unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded.protocol_backlog.runs[0].batches[0]).unwrap(),
        bytes
    );
    assert_eq!(
        serde_json::to_vec(&loaded.protocol_backlog).unwrap(),
        serde_json::to_vec(&state.protocol_backlog).unwrap()
    );
    let mut plan = moved.plan.filing;
    plan.semantics = "backlog-flags-preserved-no-copies-v1".into();
    for c in &mut plan.changes {
        c.operation = crate::backlog::Operation::Imap {
            method: crate::backlog::MoveMethod::CopyUidExpunge,
        };
    }
    state.protocol_backlog = protocol::Journal::default();
    state.backlog.push(crate::backlog::Run {
        id: "historical-copy".into(),
        plan,
        status: crate::backlog::Status::Prepared,
        outcomes: vec![],
        undo: vec![],
        undo_requested: false,
        batches: 0,
        error: None,
    });
    snapshot(&store, 25, &state);
    assert_eq!(
        serde_json::to_vec(&store.load_state().unwrap().backlog).unwrap(),
        serde_json::to_vec(&state.backlog).unwrap()
    );
}

#[test]
fn frame_patches_cannot_introduce_copy_semantics_by_splitting_fields() {
    let (_root, store, mut state) = fixture();
    let mut moved = run(&state.census.account, Stage::Prepared);
    moved.plan.filing.semantics = "imap-move-mapping-readback/v1".into();
    moved.plan.topology.capabilities.insert("MOVE".into());
    for c in &mut moved.plan.filing.changes {
        c.operation = crate::backlog::Operation::Imap {
            method: crate::backlog::MoveMethod::UidMove,
        };
    }
    moved.batches = vec![MoveEvidence::intent(moved.batches[0].command().clone()).into()];
    state.protocol_backlog.runs = vec![moved];
    let before = serde_json::to_value(&state.protocol_backlog).unwrap();
    let mut after = before.clone();
    after["runs"][0]["plan"]["filing"]["semantics"] = protocol::COPY_SEMANTICS.into();
    let change = patch::diff(&before, &after).unwrap();
    assert!(!copy_fields(&serde_json::to_value(&change).unwrap()));
    snapshot(&store, 25, &state);
    assert!(store.load_state().is_ok());
    append(&store, &state, change);
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
}

#[test]
fn invalid_copy_journals_never_replace_the_last_valid_ciphertext() {
    let (_root, store, mut state) = fixture();
    state.protocol_backlog.runs = vec![run(&state.census.account, Stage::Undone)];
    store.save_state(&mut state).unwrap();
    let bytes = fs::read(store.path()).unwrap();
    for mutation in 0..3 {
        let mut changed = state.clone();
        match mutation {
            0 => {
                changed.protocol_backlog.runs[0].plan.binding.account =
                    AccountId::new("foreign").unwrap()
            }
            1 => {
                changed.protocol_backlog.runs[0].plan.filing.changes[0]
                    .source
                    .uid = 99
            }
            2 => changed.protocol_backlog.runs[0].reverse[0].forward = 5,
            _ => unreachable!(),
        }
        assert!(store.save_state(&mut changed).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        assert_eq!(store.load_state().unwrap().revision, state.revision);
    }
}
