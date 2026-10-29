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

#[test]
fn compensation_schema_twenty_seven_rejects_older_bases_frames_and_summaries() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, original) = fixture();
    let mut current = original.clone();
    current.protocol_backlog.runs =
        vec![compensation_fixture(&current.census.account, P::Prepared)];
    for version in 1..27 {
        snapshot(&store, version, &current);
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "base {version}"
        );
    }
    snapshot(&store, 27, &current);
    assert!(store.load_state().unwrap().protocol_backlog == current.protocol_backlog);
    for version in 18..27 {
        snapshot(&store, version, &original);
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
    current.protocol_backlog.runs = vec![compensation_fixture(&current.census.account, P::Cleaned)];
    assert!(current.protocol_backlog.compact(u64::MAX));
    snapshot(&store, 26, &current);
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    snapshot(&store, 27, &current);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.summaries[0].copies_removed,
        2
    );
}

#[test]
fn encrypted_compensation_append_updates_and_omission_reopen_exactly() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, mut state) = fixture();
    state.protocol_backlog.runs = vec![run(&state.census.account, Stage::Copied)];
    store.save_state(&mut state).unwrap();
    for phase in [
        P::Prepared,
        P::StoreUncertain,
        P::Marked,
        P::ExpungeUncertain,
        P::Cleaned,
    ] {
        state.protocol_backlog.runs = vec![compensation_fixture(&state.census.account, phase)];
        store.save_state(&mut state).unwrap();
        decoded::forget(&store.path());
        let loaded = store.load_state().unwrap();
        assert!(
            loaded.protocol_backlog == state.protocol_backlog,
            "{phase:?}"
        );
        assert_eq!(loaded.revision, state.revision);
    }
    state.protocol_backlog.runs = vec![run(&state.census.account, Stage::Copied)];
    store.save_state(&mut state).unwrap();
    decoded::forget(&store.path());
    let loaded = store.load_state().unwrap();
    assert!(loaded.protocol_backlog == state.protocol_backlog);
    assert!(
        serde_json::to_value(&loaded.protocol_backlog).unwrap()["runs"][0]
            .get("compensation")
            .is_none()
    );
}

#[test]
fn substituted_compensation_never_replaces_valid_ciphertext() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, mut state) = fixture();
    state.protocol_backlog.runs = vec![compensation_fixture(&state.census.account, P::Prepared)];
    store.save_state(&mut state).unwrap();
    let bytes = fs::read(store.path()).unwrap();
    state.protocol_backlog.runs[0].compensation[0].forward = 99;
    assert!(store.save_state(&mut state).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), bytes);
    assert_eq!(
        store.load_state().unwrap().protocol_backlog.runs[0].compensation[0].forward,
        0
    );
}

#[test]
fn summary_frame_cannot_smuggle_nonzero_removed_copies_into_schema_twenty_six() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, mut state) = fixture();
    state.protocol_backlog.runs = vec![compensation_fixture(&state.census.account, P::Cleaned)];
    state.protocol_backlog.compact(u64::MAX);
    state.protocol_backlog.summaries[0].copies_removed = 0;
    let before = serde_json::to_value(&state.protocol_backlog).unwrap();
    let mut after = before.clone();
    after["summaries"][0]["copies_removed"] = 2.into();
    snapshot(&store, 26, &state);
    assert!(store.load_state().is_ok());
    append(&store, &state, patch::diff(&before, &after).unwrap());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
}

#[test]
fn dependent_cleanup_requires_schema_twenty_eight_in_snapshots_and_frames() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, original) = fixture();
    let mut current = original.clone();
    current.protocol_backlog.runs = vec![compensation_fixture(
        &current.census.account,
        P::AfterClearPrepared,
    )];
    for version in 1..28 {
        snapshot(&store, version, &current);
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "base {version}"
        );
    }
    snapshot(&store, 28, &current);
    assert!(store.load_state().unwrap().protocol_backlog == current.protocol_backlog);
    for version in 18..28 {
        snapshot(&store, version, &original);
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
    current.protocol_backlog.runs = vec![compensation_fixture(&current.census.account, P::Cleared)];
    snapshot(&store, 27, &current);
    assert!(store.load_state().unwrap().protocol_backlog == current.protocol_backlog);
}

#[test]
fn encrypted_dependent_cleanup_keeps_each_prerequisite_and_uncertain_effect_on_reopen() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, mut state) = fixture();
    for phase in [
        P::Cleared,
        P::AfterClearPrepared,
        P::AfterClearStoreUncertain,
        P::AfterClearMarked,
        P::AfterClearExpungeUncertain,
        P::AfterClearCleaned,
    ] {
        state.protocol_backlog.runs = vec![compensation_fixture(&state.census.account, phase)];
        store.save_state(&mut state).unwrap();
        decoded::forget(&store.path());
        let reopened = store.load_state().unwrap();
        assert!(
            reopened.protocol_backlog == state.protocol_backlog,
            "{phase:?}"
        );
        let evidence = &reopened.protocol_backlog.runs[0].compensation[0].evidence;
        assert_eq!(evidence.clear(0).unwrap().verified().len(), 2);
        assert_eq!(
            evidence.fully_reversed(),
            matches!(phase, P::AfterClearCleaned)
        );
    }
}

#[test]
fn dependent_approvals_require_schema_twenty_nine_and_preserve_legacy_absence() {
    use protocol::{CompensationPhase as P, DependentCleanupApproval, compensation_fixture};
    let (_root, store, original) = fixture();
    let mut current = original.clone();
    let mut run = compensation_fixture(&current.census.account, P::AfterClearPrepared);
    run.compensation[0]
        .after_clear_approvals
        .push(DependentCleanupApproval {
            clear: 0,
            receipt: "a".repeat(64),
            binding: "b".repeat(64),
        });
    current.protocol_backlog.runs = vec![run];
    for version in 1..29 {
        snapshot(&store, version, &current);
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "base {version}"
        );
    }
    snapshot(&store, 29, &current);
    assert!(store.load_state().unwrap().protocol_backlog == current.protocol_backlog);
    for version in 18..29 {
        snapshot(&store, version, &original);
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
    current.protocol_backlog.runs[0].compensation[0]
        .after_clear_approvals
        .clear();
    snapshot(&store, 28, &current);
    assert!(store.load_state().unwrap().protocol_backlog == current.protocol_backlog);
    assert!(
        serde_json::to_value(&current.protocol_backlog).unwrap()["runs"][0]["compensation"][0]
            .get("after_clear_approvals")
            .is_none()
    );
    let mut next = current.clone();
    next.protocol_backlog.runs[0].compensation[0]
        .after_clear_approvals
        .push(DependentCleanupApproval {
            clear: 0,
            receipt: "a".repeat(64),
            binding: "b".repeat(64),
        });
    let before = serde_json::to_value(&current.protocol_backlog).unwrap();
    let after = serde_json::to_value(&next.protocol_backlog).unwrap();
    snapshot(&store, 28, &current);
    append(&store, &current, patch::diff(&before, &after).unwrap());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    snapshot(&store, 29, &current);
    append(&store, &current, patch::diff(&before, &after).unwrap());
    assert!(store.load_state().unwrap().protocol_backlog == next.protocol_backlog);
}

#[test]
fn old_schema_cannot_append_a_dependent_plan_to_existing_compensation() {
    use protocol::{CompensationPhase as P, compensation_fixture};
    let (_root, store, mut state) = fixture();
    state.protocol_backlog.runs = vec![compensation_fixture(&state.census.account, P::Cleared)];
    snapshot(&store, 27, &state);
    assert!(store.load_state().is_ok());
    let before = serde_json::to_value(&state.protocol_backlog).unwrap();
    let mut after = state.clone();
    after.protocol_backlog.runs = vec![compensation_fixture(
        &state.census.account,
        P::AfterClearPrepared,
    )];
    let change = patch::diff(
        &before,
        &serde_json::to_value(after.protocol_backlog).unwrap(),
    )
    .unwrap();
    append(&store, &state, change);
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
}
