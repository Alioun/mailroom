use super::*;
use std::collections::BTreeSet;

#[test]
fn sender_sets_require_schema_twenty_four_in_base_and_frames() {
    use crate::rules::{program, proof, workspace};
    let input: crate::engine::PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    let census = crate::engine::synthetic_census(&input).unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        root.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &census.account),
    )
    .unwrap();
    let original = AccountState::new(census);
    let mut current = original.clone();
    let artifact = crate::engine::artifact(&input.documents, &input.context).unwrap();
    current.exports.push(workspace::ExportRecord {
        context_binding: None,
        semantics: program::SEMANTICS.into(),
        proof_semantics: proof::SEMANTICS.into(),
        artifact: artifact.clone(),
        workspace: workspace::Workspace {
            revision: 1,
            input: workspace::WorkspaceInput {
                documents: input.documents,
                pins: vec![],
                as_of: input.as_of,
                baseline: proof::Baseline::Current,
            },
        },
        receipt: "a".repeat(64),
        exported_at: 1,
        provenance: "Synthetic source-format fixture".into(),
    });
    for version in [13, 18, 21, 23] {
        store
            .stage_snapshot(version, &serde_json::to_string(&current).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }
    store
        .stage_snapshot(24, &serde_json::to_string(&current).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(
        store.load_state().unwrap().exports[0].artifact.identity(),
        artifact.identity()
    );
    store
        .stage_snapshot(23, &serde_json::to_string(&original).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut loaded = store.load_state().unwrap();
    let file = fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys().encryption.as_ref()).unwrap();
    let decoded = log::decode(&cipher, store.keys().account.as_str(), &file).unwrap();
    store.persisted.borrow_mut().as_mut().unwrap().tail = Some(Tail {
        base_tag: decoded.base_tag,
        base_len: decoded.base_len,
        valid_end: decoded.valid_end,
        next_sequence: 0,
    });
    loaded.exports = current.exports;
    store.save_state(&mut loaded).unwrap();
    assert!(fs::metadata(store.path()).unwrap().len() > decoded.valid_end);
    decoded::forget(&store.path());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
}

#[test]
fn old_schemas_reject_protocol_and_archive_semantics_in_base_and_frames() {
    let root = tempfile::tempdir().unwrap();
    let input: crate::engine::PreviewInput =
        serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json")).unwrap();
    let census = crate::engine::synthetic_census(&input).unwrap();
    let store = EncryptedStore::new(
        root.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &census.account),
    )
    .unwrap();
    let mut state = AccountState::new(census);
    state.backlog.push(crate::backlog::Run {
        id: "historical-legacy-run".into(),
        plan: crate::backlog::Plan {
            account: state.census.account.as_str().into(),
            provider: crate::acquisition::Provider::MailboxOrg,
            source: "synthetic".into(),
            state_revision: state.revision,
            ledger_version: String::new(),
            mailbox_revision: None,
            semantics: "imap-move-mapping-readback/v1".into(),
            artifact: crate::engine::artifact(&input.documents, &input.context).unwrap(),
            proof_gate: "pass".into(),
            options: crate::backlog::Options::default(),
            changes: vec![],
            excluded: Default::default(),
            unmatched: 0,
            review: 0,
            groups: Default::default(),
            stopped_folders: Default::default(),
            diagnostics: vec![],
        },
        status: crate::backlog::Status::Completed,
        outcomes: vec![],
        undo: vec![],
        undo_requested: false,
        batches: 0,
        error: None,
    });
    let mut value = serde_json::to_value(state).unwrap();
    value.as_object_mut().unwrap().remove("protocol_backlog");
    let mut legacy = value.clone();
    legacy["backlog"][0]["plan"]["semantics"] = "backlog-flags-preserved-no-copies-v1".into();
    store
        .stage_snapshot(14, &serde_json::to_string(&legacy).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(
        serde_json::to_value(store.load_state().unwrap().backlog).unwrap(),
        legacy["backlog"]
    );
    store
        .stage_snapshot(14, &serde_json::to_string(&value).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    let original: AccountState = serde_json::from_value(legacy).unwrap();
    let mut archived = original.clone();
    archived.backlog[0].plan.options.intent = crate::backlog::Intent::ArchiveUnmatched;
    archived.backlog[0].plan.options.scope = crate::backlog::Scope::Inbox;
    for version in [18, 21, 24] {
        store
            .stage_snapshot(version, &serde_json::to_string(&archived).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }
    store
        .stage_snapshot(SCHEMA, &serde_json::to_string(&archived).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(
        serde_json::to_value(store.load_state().unwrap().backlog).unwrap(),
        serde_json::to_value(&archived.backlog).unwrap()
    );
    store
        .stage_snapshot(24, &serde_json::to_string(&original).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut loaded = store.load_state().unwrap();
    assert_eq!(
        loaded.backlog[0].plan.options.intent,
        crate::backlog::Intent::Sort
    );
    let file = fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys().encryption.as_ref()).unwrap();
    let decoded = log::decode(&cipher, store.keys().account.as_str(), &file).unwrap();
    store.persisted.borrow_mut().as_mut().unwrap().tail = Some(Tail {
        base_tag: decoded.base_tag,
        base_len: decoded.base_len,
        valid_end: decoded.valid_end,
        next_sequence: 0,
    });
    loaded.backlog = archived.backlog;
    store.save_state(&mut loaded).unwrap();
    assert!(fs::metadata(store.path()).unwrap().len() > decoded.valid_end);
    decoded::forget(&store.path());
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
}

#[test]
fn protocol_schema_fifteen_keeps_old_absence_and_rejects_nested_authority() {
    for version in 1..=14 {
        let root = tempfile::tempdir().unwrap();
        let input: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json"))
                .unwrap();
        let census = crate::engine::synthetic_census(&input).unwrap();
        let store = EncryptedStore::new(
            root.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &census.account),
        )
        .unwrap();
        let mut value = if version == 1 {
            serde_json::to_value(census).unwrap()
        } else {
            serde_json::to_value(AccountState::new(census)).unwrap()
        };
        value.as_object_mut().unwrap().remove("protocol_backlog");
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(store.load_state().unwrap().protocol_backlog.runs.is_empty());
        for injection in [
            serde_json::json!({"protocol_backlog":{"runs":[{}]}}),
            serde_json::json!({"protocol_backlog":{"runs":[],"source":"forged"}}),
            serde_json::json!({"backlog":[{"plan":{"execution_proof":"forged"}}]}),
            serde_json::json!({"exports":[{"artifact":{"move_evidence":{}}}]}),
            serde_json::json!({"backlog":[{"plan":{"semantics":"imap-move-mapping-readback/v1"}}]}),
            serde_json::json!({"exports":[{"artifact":{"semantics":"imap-move-mapping-readback/v1"}}]}),
        ] {
            let mut changed = value.clone();
            changed
                .as_object_mut()
                .unwrap()
                .extend(injection.as_object().unwrap().clone());
            store
                .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(
                matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
                "schema {version}"
            );
        }
    }
}

#[test]
fn gmail_schema_eighteen_keeps_old_absence_and_rejects_label_semantics() {
    for version in 1..=18 {
        let root = tempfile::tempdir().unwrap();
        let input: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json"))
                .unwrap();
        let census = crate::engine::synthetic_census(&input).unwrap();
        let store = EncryptedStore::new(
            root.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &census.account),
        )
        .unwrap();
        let value = if version == 1 {
            serde_json::to_value(census).unwrap()
        } else {
            serde_json::to_value(AccountState::new(census)).unwrap()
        };
        assert!(
            value.get("gmail_backlog").is_none(),
            "Empty journal is omitted"
        );
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut loaded = store.load_state().unwrap();
        assert!(loaded.gmail_backlog.runs.is_empty());
        if version > 1 {
            store.save_state(&mut loaded).unwrap();
            assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
            assert_eq!(
                serde_json::to_value(&store.load_state().unwrap().protocol_backlog).unwrap(),
                serde_json::to_value(&loaded.protocol_backlog).unwrap()
            );
        }
        for injection in [
            serde_json::json!({"gmail_backlog":{"runs":[]}}),
            serde_json::json!({"gmail_backlog":{"runs":[{}]}}),
            serde_json::json!({"backlog":[{"plan":{"semantics":"gmail-label-readback/v1"}}]}),
            serde_json::json!({"exports":[{"artifact":{"semantics":"gmail-label-readback/v1"}}]}),
        ] {
            let mut changed = value.clone();
            changed
                .as_object_mut()
                .unwrap()
                .extend(injection.as_object().unwrap().clone());
            store
                .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let expected_storage = version == 18;
            let result = store.load_state();
            assert!(
                if expected_storage {
                    injection
                        .get("gmail_backlog")
                        .is_some_and(|j| j["runs"].as_array().unwrap().is_empty())
                        && result.is_ok()
                        || matches!(result, Err(CensusError::Storage))
                } else {
                    matches!(result, Err(CensusError::UnsupportedVersion))
                },
                "schema {version} {injection}"
            );
        }
    }
}

#[test]
fn delivery_schema_fourteen_preserves_legacy_absence_and_rejects_nested_smuggling() {
    for version in 1..=13 {
        let root = tempfile::tempdir().unwrap();
        let input: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json"))
                .unwrap();
        let census = crate::engine::synthetic_census(&input).unwrap();
        let store = EncryptedStore::new(
            root.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &census.account),
        )
        .unwrap();
        let value = if version == 1 {
            serde_json::to_value(&census).unwrap()
        } else {
            serde_json::to_value(AccountState::new(census)).unwrap()
        };
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        let loaded = store.load_state().unwrap();
        assert!(
            loaded
                .census
                .folders
                .values()
                .flat_map(|f| f.messages.values())
                .all(|m| m.delivery == Default::default())
        );
        for injected in ["message", "context", "tag", "yaml", "backlog"] {
            let mut changed = value.clone();
            if version == 1 {
                changed["folders"]["INBOX"]["messages"]["1"]["delivery"] =
                    serde_json::json!({"status":"ambiguous"});
            } else {
                match injected {
                    "message" => {
                        changed["census"]["folders"]["INBOX"]["messages"]["1"]["delivery"] =
                            serde_json::json!({"status":"unsupported"})
                    }
                    "context" => {
                        changed["exports"] = serde_json::json!([{"artifact":{"context":{"recipient_evidence":{"synthetic_envelope":{"source":{}}}}}}])
                    }
                    "tag" => {
                        changed["exports"] = serde_json::json!([{"artifact":{"effective":[{"definition":{"kind":"delivery_tag"}}]}}])
                    }
                    "backlog" => {
                        changed["backlog"] = serde_json::json!([{"plan":{"artifact":{"context":{"recipient_evidence":{"synthetic_envelope":{"source":{}}}}}}}])
                    }
                    _ => {
                        changed["workspace"] = serde_json::json!({"input":{"documents":{"override_yaml":"delivery_tags: [{id: user.injected}]"}}})
                    }
                }
            }
            store
                .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(
                matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
                "schema {version} {injected}"
            );
        }
        store
            .stage_state(&loaded)
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
        assert_eq!(
            serde_json::to_value(store.load_state().unwrap()).unwrap(),
            serde_json::to_value(loaded).unwrap()
        );
    }
}

#[test]
fn obsolete_export_bytes_and_versions_survive_without_recompilation() {
    for (schema, version, executable) in [
        (
            13,
            "v3",
            "# Historical export retains its original line endings.\r\nkeep;\r\n",
        ),
        (
            14,
            "v4",
            "require [\"envelope\", \"imap4flags\"];\n\n#@ system.role_safety\nif true {\n  keep;\n  stop;\n}\n\n#@ user.route\nif envelope :all :is \"to\" \"route@business.test\" {\n  addflag \"$route\";\n}\n",
        ),
        (
            15,
            "v4",
            "require [\"envelope\", \"imap4flags\"];\r\n#@ system.role_safety\r\nif true {\r\n  keep;\r\n  stop;\r\n}\r\n",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        historical_fixture(&repo, &account);
        let store = repo.store(&account).unwrap();
        let mut historical = serde_json::to_value(store.load_state().unwrap()).unwrap();
        before_composition(&mut historical);
        for export in historical["exports"].as_array_mut().unwrap() {
            export["semantics"] = serde_json::json!(format!("mailroom-sieve-subset-{version}"));
            export["proof_semantics"] =
                serde_json::json!(format!("parsed-emitted-sieve-subset-{version}"));
            export["artifact"]["executable"] = serde_json::json!(executable);
        }
        let expected = historical["exports"].clone();
        store
            .stage_snapshot(schema, &serde_json::to_string(&historical).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut loaded = store.load_state().unwrap();
        let context = repo.workspace_context(&account, &store).unwrap();
        assert!(
            crate::rules::workspace::view(&loaded, &context)
                .exports
                .iter()
                .all(|e| !e.usable),
            "{schema} {version}"
        );
        loaded.workspace.as_mut().unwrap().input.baseline = crate::rules::proof::Baseline::Previous;
        assert!(crate::rules::workspace::preview(&loaded, &context, store.keys()).is_err());
        store.save_state(&mut loaded).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
        assert_eq!(
            serde_json::to_value(store.load_state().unwrap().exports).unwrap(),
            expected,
            "{schema} {version}"
        );
    }
}

#[test]
fn pre_thirteen_exports_default_to_unusable_and_reject_authority_smuggling() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) =
        crate::acquisition::folder_provisioning::tests::install(root.path());
    historical_fixture(&repo, &account);
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert!(!saved.exports.is_empty());
    let mut legacy = serde_json::to_value(&saved).unwrap();
    before_composition(&mut legacy);
    for export in legacy["exports"].as_array_mut().unwrap() {
        export.as_object_mut().unwrap().remove("context_binding");
    }
    for version in [11, 12] {
        let mut compatible = legacy.clone();
        if version < 12 {
            compatible
                .as_object_mut()
                .unwrap()
                .remove("folder_provisioning");
            compatible
                .as_object_mut()
                .unwrap()
                .remove("synthetic_folders");
        }
        store
            .stage_snapshot(version, &serde_json::to_string(&compatible).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        let loaded = store.load_state().unwrap();
        assert!(loaded.exports.iter().all(|e| e.context_binding.is_none()));
        let context = repo.workspace_context(&account, &store).unwrap();
        assert!(
            crate::rules::workspace::view(&loaded, &context)
                .exports
                .iter()
                .all(|e| !e.usable)
        );
        compatible["exports"][0]["context_binding"] = serde_json::json!("caller-claim");
        store
            .stage_snapshot(version, &serde_json::to_string(&compatible).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }
    store
        .stage_state(&saved)
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    assert_eq!(
        serde_json::to_value(store.load_state().unwrap().exports).unwrap(),
        serde_json::to_value(&saved.exports).unwrap()
    );
}

#[test]
fn old_schemas_reject_folder_semantics_and_current_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let (mut repo, account, _fake) =
        crate::acquisition::folder_provisioning::tests::install(root.path());
    crate::acquisition::folder_provisioning::tests::commit_saved_workspace_folders(
        &mut repo,
        &account,
        root.path(),
    );
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    for version in 1..=11 {
        let mut value = if version == 1 {
            serde_json::to_value(&saved.census).unwrap()
        } else {
            serde_json::to_value(&saved).unwrap()
        };
        if let Some(exports) = value
            .get_mut("exports")
            .and_then(serde_json::Value::as_array_mut)
        {
            for export in exports {
                export.as_object_mut().unwrap().remove("context_binding");
            }
        }
        value["folder_provisioning"] = serde_json::to_value(&saved.folder_provisioning).unwrap();
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }
    store
        .stage_state(&saved)
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    assert!(store.load_state().unwrap().folder_provisioning == saved.folder_provisioning);
    let mut bad = saved.clone();
    bad.folder_provisioning.history[0].plan.entries[0]
        .name
        .wire
        .push('x');
    assert!(store.stage_state(&bad).is_err());
    let mut bad = saved.clone();
    bad.folder_provisioning.history[0].steps[0]
        .name
        .decoded
        .push('x');
    assert!(store.stage_state(&bad).is_err());
}

#[test]
fn old_schemas_reject_real_transaction_semantics_and_eleven_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = crate::acquisition::sieve_transaction::tests::install(root.path());
    crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    for version in 1..=10 {
        let mut value = if version == 1 {
            serde_json::to_value(&state.census).unwrap()
        } else {
            serde_json::to_value(&state).unwrap()
        };
        if version == 1 {
            value["sieve_transactions"] = serde_json::to_value(&state.sieve_transactions).unwrap();
        }
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }
    store
        .stage_state(&state)
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    assert!(store.load_state().unwrap().sieve_transactions == state.sieve_transactions);
    let mut malformed = state.clone();
    malformed.sieve_transactions.history[0].after.bodies[1]
        .bytes
        .push(1);
    assert!(store.stage_state(&malformed).is_err());
    assert!(store.load_state().unwrap().sieve_transactions == state.sieve_transactions);
}

/// The shared account fixtures export and plan `examples/sample.json`,
/// whose core sender categories need schema 24, and record the deployed
/// artifact, which needs schema 30. This replaces the exports and backlog
/// plans with ones of the pre-core sample and drops the deployment record,
/// so the state fits the historical schemas these cases stage. The
/// workspace stays as it is, as after an edit since that export.
fn historical_fixture(repo: &crate::acquisition::Accounts, account: &crate::acquisition::Account) {
    use crate::rules::workspace::{self, WorkspaceContext};
    let store = repo.store(account).unwrap();
    let mut state = store.load_state().unwrap();
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/pre-core-sample.json")).unwrap();
    let mut exported = state.clone();
    exported.workspace.as_mut().unwrap().input.documents =
        serde_json::from_value(sample["documents"].clone()).unwrap();
    exported.exports.clear();
    let context = WorkspaceContext::local(account.provider);
    let proof = workspace::preview(&exported, &context, store.keys()).unwrap();
    workspace::confirm_export(&mut exported, &context, store.keys(), &proof.receipt).unwrap();
    for run in &mut state.backlog {
        run.plan = crate::backlog::preview(
            &exported,
            account.provider,
            store.keys(),
            run.plan.options.clone(),
            None,
        )
        .unwrap()
        .plan;
    }
    state.exports = exported.exports;
    state.deployment_artifacts = Default::default();
    assert!(!uses_sender_sets(&state));
    store.save_state(&mut state).unwrap();
}

/// Schemas before 16 cannot carry a placement choice or composed transactions.
fn before_composition(state: &mut serde_json::Value) {
    state.as_object_mut().unwrap().remove("sieve_placement");
    state["sieve_transactions"] = serde_json::json!({"history": []});
}

#[test]
fn composition_needs_schema_sixteen_and_tampered_foreign_bytes_never_load() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = crate::acquisition::sieve_transaction::tests::install(root.path());
    crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
    historical_fixture(&repo, &account);
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    assert!(state.sieve_transactions.history[0].composition.is_some());
    let mut placement_only = state.clone();
    placement_only.sieve_transactions = Default::default();
    let mut composition_only = state.clone();
    composition_only.sieve_placement = None;
    for version in [11, 15] {
        for candidate in [&state, &placement_only, &composition_only] {
            store
                .stage_snapshot(version, &serde_json::to_string(candidate).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(matches!(
                store.load_state(),
                Err(CensusError::UnsupportedVersion)
            ));
        }
    }
    let mut legacy = placement_only.clone();
    legacy.sieve_placement = None;
    store
        .stage_snapshot(15, &serde_json::to_string(&legacy).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    assert_eq!(store.schema_version().unwrap(), 15);
    let mut reopened = store.load_state().unwrap();
    assert!(reopened.sieve_placement.is_none());
    store.save_state(&mut reopened).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    let mut tampered = state.clone();
    let t = &mut tampered.sieve_transactions.history[0];
    for image in std::iter::once(&mut t.after).chain(t.steps.iter_mut().map(|s| &mut s.after)) {
        let host = image
            .bodies
            .iter_mut()
            .find(|b| b.name == "Open-Xchange")
            .unwrap();
        let bytes = String::from_utf8(host.bytes.clone())
            .unwrap()
            .replace("Reisen", "Moved")
            .into_bytes();
        *host = super::super::sieve::Body::new(host.name.clone(), bytes);
    }
    assert!(store.stage_state(&tampered).is_err());
}

#[test]
fn copy_first_composition_needs_schema_seventeen_and_legacy_records_stay_readable() {
    use crate::rules::composition::{Counts, LEGACY_SEMANTICS, SEMANTICS};
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = crate::acquisition::sieve_transaction::tests::install(root.path());
    crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
    historical_fixture(&repo, &account);
    let store = repo.store(&account).unwrap();
    let current = store.load_state().unwrap();
    let composition = current.sieve_transactions.history[0]
        .composition
        .as_ref()
        .unwrap();
    assert_eq!(composition.semantics, SEMANTICS);
    assert!(composition.precedence.as_ref().unwrap().copies.is_some());
    let mut legacy = current.clone();
    let c = legacy.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap();
    c.semantics = LEGACY_SEMANTICS.into();
    c.precedence = Some(Counts {
        suppressed_copies: Some(0),
        copies: None,
        ..c.precedence.clone().unwrap()
    });
    let stage = |version, state: &AccountState| {
        store
            .stage_snapshot(version, &serde_json::to_string(state).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
    };
    stage(16, &current);
    assert!(matches!(
        store.load_state(),
        Err(CensusError::UnsupportedVersion)
    ));
    stage(16, &legacy);
    let mut reopened = store.load_state().unwrap();
    assert!(reopened.sieve_transactions == legacy.sieve_transactions);
    store.save_state(&mut reopened).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    assert!(store.load_state().unwrap().sieve_transactions == legacy.sieve_transactions);
    for mixed in [
        Counts {
            suppressed_copies: Some(0),
            ..Default::default()
        },
        Counts {
            copies: None,
            ..Default::default()
        },
    ] {
        let mut bad = current.clone();
        bad.sieve_transactions.history[0]
            .composition
            .as_mut()
            .unwrap()
            .precedence = Some(mixed);
        assert!(store.stage_state(&bad).is_err());
    }
    let mut bad = legacy.clone();
    bad.sieve_transactions.history[0]
        .composition
        .as_mut()
        .unwrap()
        .precedence = Some(Counts {
        suppressed_copies: Some(0),
        copies: Some(Default::default()),
        ..Default::default()
    });
    assert!(store.stage_state(&bad).is_err());
}

#[test]
fn historical_schemas_decode_without_inventing_acquisition_evidence() {
    for version in 1..=10 {
        let directory = tempfile::tempdir().unwrap();
        let account = AccountId::new("historical-schema").unwrap();
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let census = Census::new(account);
        let mut value = if version == 1 {
            serde_json::to_value(&census).unwrap()
        } else {
            serde_json::to_value(AccountState::new(census)).unwrap()
        };
        if version == 1 {
            value.as_object_mut().unwrap().remove("acquisition");
        } else {
            value["census"]
                .as_object_mut()
                .unwrap()
                .remove("acquisition");
        }
        if version > 1 {
            value.as_object_mut().unwrap().remove("proposals");
            value.as_object_mut().unwrap().remove("sieve");
        }
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut state = store.load_state().unwrap();
        assert_eq!(state.census.acquisition, Default::default());
        assert!(state.exports.is_empty());
        assert!(state.proposals.history.is_empty());
        store.save_state(&mut state).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
        assert_eq!(store.load().unwrap().acquisition, Default::default());
    }
}

#[test]
fn sieve_semantics_cannot_be_smuggled_under_old_schema_or_invented_in_ten() {
    for version in [1, 2, 8, 9, 10] {
        let directory = tempfile::tempdir().unwrap();
        let account = AccountId::new("sieve-schema-boundary").unwrap();
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let mut saved = AccountState::new(Census::new(account));
        saved.sieve.version = 1;
        let value = if version == 1 {
            let mut value = serde_json::to_value(&saved.census).unwrap();
            value["sieve"] = serde_json::to_value(&saved.sieve).unwrap();
            value
        } else {
            serde_json::to_value(&saved).unwrap()
        };
        store
            .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(store.load_state().is_err());
    }
}

#[test]
fn interruption_before_atomic_replacement_retains_the_previous_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("interruption-test").unwrap();
    let store = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    let original = Census::new(account);
    store.save(&original).unwrap();
    let before = fs::read(store.path()).unwrap();
    let mut next = original.clone();
    next.reconcile_folder("private-folder-name", 1, BTreeSet::from([1]))
        .unwrap();
    let staged = store.stage(&next).unwrap();
    let staged_bytes = fs::read(staged.path()).unwrap();
    assert!(
        !staged_bytes
            .windows(19)
            .any(|w| w == b"private-folder-name")
    );
    let (file, orphan) = staged.keep().unwrap();
    drop(file);
    assert_eq!(fs::read(store.path()).unwrap(), before);
    assert_eq!(store.load().unwrap(), original);
    // A crash can also leave an incomplete encrypted staging file.
    fs::write(&orphan, &staged_bytes[..20]).unwrap();
    assert_eq!(store.load().unwrap(), original);
    store.save(&next).unwrap();
    assert_eq!(
        store.load().unwrap().folders["private-folder-name"].pending(10),
        vec![1]
    );
}
