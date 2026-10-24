use super::*;
use crate::acquisition::Provider;
use crate::rules::workspace::{self, Workspace, WorkspaceInput};
fn input() -> WorkspaceInput {
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    let mut documents: crate::engine::Documents =
        serde_json::from_value(sample["documents"].clone()).unwrap();
    documents.override_yaml = documents.override_yaml.replace(
        "archive@example.net",
        "synthetic-accounting-marker@example.test",
    );
    documents
        .override_yaml
        .push_str("\n# synthetic private workspace document marker\n");
    WorkspaceInput {
        documents,
        pins: vec![],
        as_of: 1792886400,
        baseline: crate::rules::proof::Baseline::Current,
    }
}
#[test]
fn migrates_actual_v1_sqlite_keeps_census_and_adds_no_approval() {
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("migration-fixture").unwrap();
    let secret = LocalSecret::from_bytes(vec![19; 32]).unwrap();
    let store =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    let mut original = Census::new(account.clone());
    original.account_login = Some("synthetic-owner@example.test".into());
    original
        .confirm_address("synthetic-owner@example.test", true)
        .unwrap();
    original
        .reconcile_folder("synthetic-private-folder", 7, [1].into_iter().collect())
        .unwrap();
    original
        .folders
        .get_mut("synthetic-private-folder")
        .unwrap()
        .insert(crate::census::demo::fixture(1).unwrap())
        .unwrap();
    store
        .stage_snapshot(1, &serde_json::to_string(&original).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut migrated = store.load_state().unwrap();
    assert_eq!(migrated.census, original);
    assert!(migrated.workspace.is_none());
    assert!(migrated.exports.is_empty());
    migrated.workspace = Some(Workspace {
        revision: 1,
        input: input(),
    });
    store.save_state(&mut migrated).unwrap();
    let mut census = store.load().unwrap();
    census.account_login = Some("synthetic-second@example.test".into());
    census
        .confirm_address("synthetic-second@example.test", true)
        .unwrap();
    store.save(&census).unwrap();
    assert_eq!(store.load_state().unwrap().workspace.unwrap().revision, 1);
    let mut stale = migrated.clone();
    assert!(store.save_state(&mut stale).is_err());
    drop(store);
    let reopened =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    let state = reopened.load_state().unwrap();
    assert!(
        state
            .census
            .confirmed_own_addresses
            .contains("synthetic-second@example.test")
    );
    assert_eq!(state.workspace.unwrap().input.documents, input().documents);
    let bytes = fs::read(reopened.path()).unwrap();
    for marker in [
        b"synthetic-private-folder".as_slice(),
        b"synthetic-accounting-marker@example.test",
        b"preset_yaml",
        b"synthetic private workspace document marker",
        b"synthetic-second@example.test",
    ] {
        assert!(!bytes.windows(marker.len()).any(|w| w == marker));
    }
    reopened
        .stage_snapshot(2, "{invalid-json")
        .unwrap()
        .persist(reopened.path())
        .unwrap();
    let corrupt = fs::read(reopened.path()).unwrap();
    assert!(matches!(reopened.load_state(), Err(CensusError::Storage)));
    assert!(reopened.save_state(&mut migrated).is_err());
    assert_eq!(fs::read(reopened.path()).unwrap(), corrupt);
    reopened
        .stage_snapshot(999, "{}")
        .unwrap()
        .persist(reopened.path())
        .unwrap();
    assert!(matches!(
        reopened.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    assert!(reopened.save_state(&mut migrated).is_err());
}
#[test]
fn receipts_bind_raw_documents_state_pins_proof_and_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("receipt-fixture").unwrap();
    let store = EncryptedStore::new(
        dir.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    store.save(&Census::new(account.clone())).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: input(),
    });
    store.save_state(&mut state).unwrap();
    let preview = workspace::preview(&state, Provider::MailboxOrg, store.keys()).unwrap();
    let rejects = |state: &mut AccountState| {
        assert!(
            workspace::confirm_export(state, Provider::MailboxOrg, store.keys(), &preview.receipt)
                .is_err()
        );
    };
    let before = state.clone();
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml
        .push_str("\n# formatting only\n");
    assert_eq!(
        workspace::preview(&state, Provider::MailboxOrg, store.keys())
            .unwrap()
            .proof
            .artifact
            .identity(),
        preview.proof.artifact.identity()
    );
    rejects(&mut state);
    state = before.clone();
    state.workspace.as_mut().unwrap().input.as_of += 1;
    rejects(&mut state);
    state = before.clone();
    state.workspace.as_mut().unwrap().revision += 1;
    rejects(&mut state);
    state = before.clone();
    state.revision += 1;
    rejects(&mut state);
    state = before.clone();
    state.census.account_login = Some("synthetic@example.test".into());
    state
        .census
        .confirm_address("synthetic@example.test", true)
        .unwrap();
    rejects(&mut state);
    state = before.clone();
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .pins
        .push(crate::rules::proof::Pin {
            account: account.as_str().into(),
            message_id_hash: "a".repeat(64),
            expected: crate::rules::proof::Destination::Inbox,
        });
    rejects(&mut state);
    let stale = workspace::preview(&state, Provider::MailboxOrg, store.keys()).unwrap();
    assert_eq!(stale.proof.pins[0].status, "stale");
    assert!(stale.download_available);
    state = before.clone();
    assert!(
        workspace::confirm_export(&mut state, Provider::Gmail, store.keys(), &preview.receipt)
            .is_err()
    );
    assert!(
        workspace::confirm_export(
            &mut state,
            Provider::MailboxOrg,
            store.keys(),
            &"0".repeat(64)
        )
        .is_err()
    );
    let export = workspace::confirm_export(
        &mut state,
        Provider::MailboxOrg,
        store.keys(),
        &preview.receipt,
    )
    .unwrap();
    assert_eq!(
        export.proof.artifact.executable().as_bytes(),
        preview.proof.artifact.executable().as_bytes()
    );
    store.save_state(&mut state).unwrap();
    rejects(&mut state);
    let saved = store.load_state().unwrap();
    assert_eq!(
        saved.exports[0].artifact.executable(),
        preview.proof.artifact.executable()
    );
    assert_eq!(
        saved.exports[0].artifact.identity(),
        preview.proof.artifact.identity()
    );
    for _ in 0..12 {
        let fresh = workspace::preview(&state, Provider::MailboxOrg, store.keys()).unwrap();
        workspace::confirm_export(
            &mut state,
            Provider::MailboxOrg,
            store.keys(),
            &fresh.receipt,
        )
        .unwrap();
        store.save_state(&mut state).unwrap();
    }
    assert_eq!(state.exports.len(), workspace::HISTORY_LIMIT);
    state.workspace.as_mut().unwrap().input.baseline = crate::rules::proof::Baseline::Previous;
    assert_eq!(
        workspace::preview(&state, Provider::MailboxOrg, store.keys())
            .unwrap()
            .previous
            .unwrap()
            .identity,
        preview.proof.artifact.identity()
    );
    state.exports.last_mut().unwrap().semantics = "obsolete-test".into();
    assert!(workspace::preview(&state, Provider::MailboxOrg, store.keys()).is_err());
}
#[test]
fn synthetic_lock_worker() {
    let Some(path) = std::env::var_os("MAILROOM_DURABLE_LOCK_TEST") else {
        return;
    };
    let account = AccountId::new("lock-fixture").unwrap();
    assert!(
        EncryptedStore::new(
            PathBuf::from(path),
            CensusKeys::derive(&LocalSecret::from_bytes(vec![21; 32]).unwrap(), &account)
        )
        .is_err()
    );
}
#[test]
fn actual_competing_process_cannot_open_locked_account_and_release_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("lock-fixture").unwrap();
    let secret = LocalSecret::from_bytes(vec![21; 32]).unwrap();
    let store =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    store.save(&Census::new(account.clone())).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "census::store::durable_tests::synthetic_lock_worker",
        ])
        .env("MAILROOM_DURABLE_LOCK_TEST", dir.path())
        .output()
        .unwrap();
    assert!(child.status.success(), "Synthetic lock subprocess failed");
    drop(store);
    let reopened =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    assert_eq!(reopened.load().unwrap().account, account);
}

#[test]
fn reopened_store_rejects_old_census_without_a_current_load_even_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("stale-census-fixture").unwrap();
    let secret = LocalSecret::from_bytes(vec![22; 32]).unwrap();
    let store =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    let old = Census::new(account.clone());
    store.save(&old).unwrap();
    let mut current = store.load().unwrap();
    current.account_login = Some("synthetic-current@example.test".into());
    store.save(&current).unwrap();
    drop(store);
    let store =
        EncryptedStore::new(dir.path().into(), CensusKeys::derive(&secret, &account)).unwrap();
    assert!(store.save(&old).is_err());
    assert!(store.save(&old).is_err());
    assert_eq!(store.load().unwrap(), current);
}

#[test]
fn migrates_v2_without_inventing_runs_and_preserves_prior_fields() {
    let dir = tempfile::tempdir().unwrap();
    let id = AccountId::new("synthetic-v2-migration").unwrap();
    let store = EncryptedStore::new(
        dir.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &id),
    )
    .unwrap();
    let census = Census::new(id);
    let mut state = AccountState::new(census);
    state.revision = 9;
    state.workspace = Some(Workspace {
        revision: 3,
        input: input(),
    });
    let mut old = serde_json::to_value(&state).unwrap();
    old.as_object_mut().unwrap().remove("backlog");
    old.as_object_mut().unwrap().remove("synthetic_effects");
    old.as_object_mut().unwrap().remove("synthetic_uid_next");
    old["census"]
        .as_object_mut()
        .unwrap()
        .remove("folder_roles");
    store
        .stage_snapshot(2, &serde_json::to_string(&old).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut migrated = store.load_state().unwrap();
    assert!(migrated.backlog.is_empty());
    assert!(migrated.synthetic_effects.is_empty());
    assert_eq!(migrated.revision, 9);
    assert_eq!(migrated.workspace.as_ref().unwrap().revision, 3);
    store.save_state(&mut migrated).unwrap();
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.revision, 10);
    assert!(reopened.backlog.is_empty());
    assert_eq!(
        reopened.workspace.unwrap().input.documents,
        input().documents
    );
}
