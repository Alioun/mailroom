use super::*;
use crate::acquisition::accounts::process_tests::persistent_accounts;

fn fixture() -> (tempfile::TempDir, Accounts, Vec<AccountId>, InstalledLedger) {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let mut ids = vec![];
    for n in 0..3 {
        let a = repo
            .create(
                Provider::MailboxOrg,
                format!("selected-{n}@example.test"),
                &SecretValue::new(b"invented".to_vec()),
            )
            .unwrap();
        let store = repo.store(&a).unwrap();
        let mut saved = store.load_state().unwrap();
        crate::rules::proposals::tests::seed(&mut saved, store.keys());
        saved.winddown.dismissed.insert(format!("decision-{n}"));
        store.save_state(&mut saved).unwrap();
        ids.push(a.id);
    }
    for n in 0..2 {
        let a = repo.load(&ids[n]).unwrap();
        let store = repo.store(&a).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.winddown.settings.comparisons.insert(
            ids[1 - n].as_str().into(),
            BTreeSet::from(["synthetic-comparison".into()]),
        );
        store.save_state(&mut saved).unwrap();
    }
    let ledger = InstalledLedger::install(root.path().join("ledger")).unwrap();
    (root, repo, ids, ledger)
}
fn secret() -> SecretValue {
    SecretValue::new(vec![0x72; 32])
}

#[test]
fn selected_recovery_preserves_transaction_journal_and_cannot_resume_effects() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, fake) = crate::acquisition::sieve_transaction::tests::install(root.path());
    crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
    let ledger = InstalledLedger::install(root.path().join("ledger")).unwrap();
    let (_, bundle) = capture(
        &repo,
        std::slice::from_ref(&account.id),
        &ledger,
        root.path(),
    );
    crate::acquisition::sieve_transaction::tests::later_provider_change(&repo, &account);
    let independent = repo.fixture_sieve_store(&account.id, root.path()).unwrap();
    let later_effects = fs::read(independent.path()).unwrap();
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
    fs::remove_file(fresh_root.path().join("ledger/addresses.yaml")).unwrap();
    let p = fresh
        .selected_restore_preview(&bundle, &secret(), true, Some(&target))
        .unwrap();
    fresh
        .selected_restore(&bundle, &secret(), &p, Some(&target), |_| Ok(()))
        .unwrap();
    fake.install(&mut fresh, &account.id);
    let original = repo.store(&account).unwrap().load_state().unwrap();
    let restored = fresh.load(&account.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    let saved = store.load_state().unwrap();
    assert!(saved.sieve_transactions == original.sieve_transactions);
    assert!(
        fresh
            .transaction_preview(
                &restored,
                &store,
                &saved,
                crate::acquisition::sieve_transaction::Request::Compensation(
                    &original.sieve_transactions.history[0].id
                ),
                crate::acquisition::Cancellation::default()
            )
            .is_err()
    );
    assert!(fs::read(independent.path()).unwrap() == later_effects);
}

#[test]
fn schema_ten_protocol_and_proposal_sieve_facts_round_trip_with_selected_recovery() {
    let (root, mut repo, ids, ledger) = fixture();
    crate::acquisition::install_provider_fixture(&mut repo, root.path());
    let fake = crate::acquisition::SieveFake::start("normal", None);
    repo.install_sieve_fixture(&fake);
    for id in &ids[..2] {
        {
            let store = repo.store(&repo.load(id).unwrap()).unwrap();
            let mut saved = store.load_state().unwrap();
            crate::acquisition::sieve_discovery::tests::seed(&mut saved, store.keys());
            store.save_state(&mut saved).unwrap();
        }
        repo.scan(id, crate::acquisition::Cancellation::default(), |_| {})
            .unwrap();
    }
    for id in &ids[..2] {
        repo.discover_sieve(id, crate::acquisition::Cancellation::default())
            .unwrap();
    }
    let (manifest, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    assert!(
        manifest
            .accounts
            .iter()
            .all(|m| m.state_schema == crate::census::SCHEMA)
    );
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
    fs::remove_file(fresh_root.path().join("ledger/addresses.yaml")).unwrap();
    let preview = fresh
        .selected_restore_preview(&bundle, &secret(), true, Some(&target))
        .unwrap();
    fresh
        .selected_restore(&bundle, &secret(), &preview, Some(&target), |_| Ok(()))
        .unwrap();
    for id in &ids[..2] {
        let before = repo.store(&repo.load(id).unwrap()).unwrap();
        let after = fresh.store(&fresh.load(id).unwrap()).unwrap();
        assert_eq!(
            encoded(&before.load_state().unwrap()).unwrap().as_slice(),
            encoded(&after.load_state().unwrap()).unwrap().as_slice()
        );
        assert!(after.load().unwrap().acquisition.is_protocol());
        assert!(after.recovery_pending());
        assert!(after.load_state().unwrap().sieve.last_successful.is_some());
    }
    fresh.install_sieve_fixture(&fake);
    let commands = fake.commands.lock().unwrap().len();
    for id in &ids[..2] {
        assert_eq!(
            fresh.discover_sieve(id, crate::acquisition::Cancellation::default()),
            Err(AcquisitionError::LifecyclePending)
        );
    }
    assert_eq!(fake.commands.lock().unwrap().len(), commands);
}
fn capture(
    repo: &Accounts,
    ids: &[AccountId],
    ledger: &InstalledLedger,
    root: &Path,
) -> (SelectedManifest, PathBuf) {
    let (p, _) = repo
        .selected_capture_preview(ids, true, Some(ledger), 1792915200)
        .unwrap();
    let id = repo
        .selected_capture(&p, &secret(), &root.join("bundles"), Some(ledger), |_| {
            Ok(())
        })
        .unwrap();
    (p, bundle_file(&root.join("bundles"), &id).unwrap())
}
#[test]
fn selected_loss_restore_every_boundary_preserves_bytes_histories_and_authority() {
    for phase in [
        "selected_restore_intent",
        "selected_restore_ledger",
        "selected_restore_credentials",
        "selected_restore_data",
        "selected_restore_metadata",
        "selected_restore_first_account",
        "selected_restore_second_credentials",
        "selected_restore_second_data",
        "selected_restore_second_metadata",
        "selected_restore_committed",
    ] {
        let (root, repo, ids, ledger) = fixture();
        let (manifest, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
        assert_eq!(manifest.excluded_accounts, vec![ids[2].clone()]);
        let fresh_root = tempfile::tempdir().unwrap();
        let fresh = persistent_accounts(fresh_root.path());
        let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
        fs::remove_file(fresh_root.path().join("ledger/addresses.yaml")).unwrap();
        let preview = fresh
            .selected_restore_preview(&bundle, &secret(), true, Some(&target))
            .unwrap();
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (Some(phase.into()), 0));
        assert!(
            fresh
                .selected_restore(&bundle, &secret(), &preview, Some(&target), |_| Ok(()))
                .is_err(),
            "{phase}"
        );
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (None, 0));
        if phase != "selected_restore_committed" {
            for id in &ids[..2] {
                assert!(fresh.require_ready(id).is_err());
            }
            assert!(
                fresh
                    .selected_resume(&SecretValue::new(vec![9; 32]), Some(&target))
                    .is_err()
            );
            let before = serde_json::to_vec(&fresh.recovery_view().unwrap()).unwrap();
            let mut confirmations = 0;
            assert!(matches!(
                fresh.selected_resume_reviewed(&secret(), Some(&target), |plan, reviewed| {
                    confirmations += 1;
                    assert_eq!(plan.actions.len(), 2);
                    assert!(reviewed.manifest == manifest);
                    assert!(fresh.lock_account(&ids[0]).is_err());
                    Err(AcquisitionError::Cancelled)
                }),
                Err(AcquisitionError::Cancelled)
            ));
            assert_eq!(confirmations, 1);
            assert!(serde_json::to_vec(&fresh.recovery_view().unwrap()).unwrap() == before);
            fresh.selected_resume_reviewed(&secret(), Some(&target), |plan, _| {
                confirmations += 1;
                assert!(plan.actions.iter().all(|a| matches!(a, crate::confirmation::ExactAction::Recovery { operation, .. } if operation == "resume_selected_local_restore")));
                Ok(())
            }).unwrap();
            assert_eq!(confirmations, 2);
        }
        for id in &ids[..2] {
            let before = repo.store(&repo.load(id).unwrap()).unwrap();
            let after = fresh.store(&fresh.load(id).unwrap()).unwrap();
            assert_eq!(
                fs::read(before.path()).unwrap(),
                fs::read(after.path()).unwrap()
            );
            assert_eq!(
                encoded(&before.load_state().unwrap()).unwrap().as_slice(),
                encoded(&after.load_state().unwrap()).unwrap().as_slice()
            );
            assert_eq!(
                before.keys().identity_binding_for(b"stable", b"op"),
                after.keys().identity_binding_for(b"stable", b"op")
            );
            assert_ne!(
                before.keys().binding_for(b"approval", b"op"),
                after.keys().binding_for(b"approval", b"op")
            );
            assert!(after.save_state(&mut after.load_state().unwrap()).is_err());
        }
        assert_eq!(
            ledger.lock().unwrap().raw().unwrap(),
            target.lock().unwrap().raw().unwrap()
        );
        assert!(
            fresh
                .selected_restore_preview(&bundle, &secret(), true, Some(&target))
                .is_err()
        );
    }
}
#[test]
fn selected_stale_set_ledger_owner_edits_and_collisions_publish_nothing() {
    let (root, repo, ids, ledger) = fixture();
    assert!(
        repo.selected_capture_preview(&[], false, Some(&ledger), 1)
            .is_err()
    );
    assert!(
        repo.selected_capture_preview(&[ids[0].clone(), ids[0].clone()], false, Some(&ledger), 1)
            .is_err()
    );
    let (p, _) = repo
        .selected_capture_preview(&ids[..2], true, Some(&ledger), 1)
        .unwrap();
    fs::write(
        root.path().join("ledger/addresses.yaml"),
        b"version: 1\nenabled: false\nentries: []\n# new owner edit\n",
    )
    .unwrap();
    assert!(
        repo.selected_capture(
            &p,
            &secret(),
            &root.path().join("bundles"),
            Some(&ledger),
            |_| panic!("stale authorization")
        )
        .is_err()
    );
    let (_, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
    let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
    assert!(
        fresh
            .selected_restore_preview(&bundle, &secret(), true, Some(&target))
            .is_err()
    );
    let p = fresh
        .selected_restore_preview(&bundle, &secret(), false, Some(&target))
        .unwrap();
    let preserved = target.lock().unwrap().raw().unwrap();
    fresh
        .create(
            Provider::MailboxOrg,
            repo.load(&ids[1]).unwrap().login,
            &SecretValue::new(b"unrelated-collision".to_vec()),
        )
        .unwrap();
    assert!(
        fresh
            .selected_restore(&bundle, &secret(), &p, Some(&target), |_| panic!(
                "collision authorization"
            ))
            .is_err()
    );
    assert!(!fresh.metadata_path(&ids[0]).exists());
    assert!(!fresh.selected_marker().exists());
    assert_eq!(target.lock().unwrap().raw().unwrap(), preserved);
}
#[test]
fn selected_authenticates_every_resource_rejects_unknown_and_overlapping_identities() {
    let (root, repo, ids, ledger) = fixture();
    let (_, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    let altered = root.path().join("altered");
    for attack in [
        "duplicate",
        "overlap",
        "ledger",
        "second_cipher",
        "second_key",
        "schema",
    ] {
        let (mut c, _) = selected_open(&bundle, &secret()).unwrap();
        match attack {
            "duplicate" => c.manifest.accounts[1] = c.manifest.accounts[0].clone(),
            "overlap" => {
                c.manifest.excluded_accounts = vec![c.manifest.accounts[0].account.clone()]
            }
            "ledger" => c.ledger.as_mut().unwrap().0[0] ^= 1,
            "second_cipher" => {
                let path = c.accounts[1].resource.path().to_path_buf();
                let mut file = fs::read(&path).unwrap();
                file[100] ^= 1;
                fs::write(&path, &file).unwrap();
                c.accounts[1].manifest.resources =
                    inventory(&c.accounts[1].account, &stream::file_digest(&path).unwrap())
                        .unwrap();
                c.manifest.accounts[1] = c.accounts[1].manifest.clone();
            }
            "second_key" => c.accounts[1].key.0[0] ^= 1,
            _ => c.manifest.version = 999,
        }
        let body = Body::Selected {
            manifest: c.manifest.clone(),
            accounts: c.accounts.iter().map(Captured::header).collect(),
            ledger: c.ledger.clone(),
        };
        let files: Vec<_> = c.accounts.iter().map(|a| a.resource.path()).collect();
        let resealed = reseal(&altered, &secret(), &body, &files);
        assert!(selected_open(&resealed, &secret()).is_err(), "{attack}");
        let legacy = LegacySelected {
            manifest: c.manifest.clone(),
            accounts: c.accounts.iter().map(Captured::legacy).collect(),
            ledger: c.ledger.clone(),
        };
        let path = altered.join("legacy.jwe");
        fs::write(&path, seal_legacy(&legacy, &secret())).unwrap();
        assert!(selected_open(&path, &secret()).is_err(), "legacy {attack}");
    }
    let (preview, _) = repo.capture_preview(&ids[0], 1792915200).unwrap();
    let legacy = repo
        .capture(
            &preview,
            &secret(),
            &root.path().join("legacy-bundles"),
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(
        selected_open(
            &bundle_file(&root.path().join("legacy-bundles"), &legacy).unwrap(),
            &secret()
        )
        .unwrap()
        .0
        .accounts
        .len(),
        1
    );
}

#[test]
fn selected_unrelated_fixture_worker() {
    let Some(root) = std::env::var_os("MAILROOM_SELECTED_UNRELATED_INSTALLATION") else {
        return;
    };
    let repo = persistent_accounts(Path::new(&root));
    let account = repo
        .create(
            Provider::MailboxOrg,
            "unrelated-selected-recovery@example.test".into(),
            &SecretValue::new(b"invented-fixture-password".to_vec()),
        )
        .unwrap();
    fs::write(
        Path::new(&root).join("unrelated-fixture-id"),
        account.id.as_str(),
    )
    .unwrap();
}

#[test]
fn selected_ledger_restore_choices_preserve_owner_bytes_and_exact_target_identity() {
    let (root, repo, ids, ledger) = fixture();
    let (_, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    for include in [false, true] {
        let target_root = tempfile::tempdir().unwrap();
        let target_repo = persistent_accounts(target_root.path());
        let target = InstalledLedger::install(target_root.path().join("ledger")).unwrap();
        let path = target_root.path().join("ledger/addresses.yaml");
        if !include {
            fs::write(
                &path,
                b"version: 1\nenabled: false\nentries: []\n# owner-specific newer bytes\n",
            )
            .unwrap();
        }
        let before = fs::read(&path).unwrap();
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&path).unwrap().ino()
        };
        let p = target_repo
            .selected_restore_preview(&bundle, &secret(), include, Some(&target))
            .unwrap();
        target_repo
            .selected_restore(&bundle, &secret(), &p, Some(&target), |_| Ok(()))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        }
    }
    let target_root = tempfile::tempdir().unwrap();
    let target_repo = persistent_accounts(target_root.path());
    let target = InstalledLedger::install(target_root.path().join("ledger")).unwrap();
    let path = target_root.path().join("ledger/addresses.yaml");
    fs::remove_file(&path).unwrap();
    let p = target_repo
        .selected_restore_preview(&bundle, &secret(), true, Some(&target))
        .unwrap();
    fs::write(
        &path,
        b"version: 1\nenabled: false\nentries: []\n# concurrent owner edit\n",
    )
    .unwrap();
    assert!(
        target_repo
            .selected_restore(&bundle, &secret(), &p, Some(&target), |_| panic!(
                "stale target authorization"
            ))
            .is_err()
    );
    assert!(!target_repo.selected_marker().exists());
    for id in &ids[..2] {
        assert!(!target_repo.metadata_path(id).exists());
    }
}

#[cfg(unix)]
#[test]
fn selected_existing_generations_and_dangling_targets_reject_before_publication() {
    use std::os::unix::fs::symlink;
    let (root, repo, ids, ledger) = fixture();
    let (_, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    for metadata in [false, true] {
        let target_root = tempfile::tempdir().unwrap();
        let target = persistent_accounts(target_root.path());
        private_directory(&target.paths.data).unwrap();
        private_directory(&target.paths.config).unwrap();
        let path = if metadata {
            target.metadata_path(&ids[1])
        } else {
            cipher_path(&target.paths.data, &ids[1], 17)
        };
        symlink(target_root.path().join("unknown-owner-resource"), &path).unwrap();
        assert!(
            target
                .selected_restore_preview(&bundle, &secret(), false, None)
                .is_err()
        );
        assert!(!target.metadata_path(&ids[0]).exists());
        assert!(!target.selected_marker().exists());
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
}

#[test]
fn selected_installation_lease_excludes_disjoint_restore_and_creation_without_changing_inode() {
    let (root, repo, ids, ledger) = fixture();
    let (_, bundle) = capture(&repo, &ids[..2], &ledger, root.path());
    let target_root = tempfile::tempdir().unwrap();
    let target = persistent_accounts(target_root.path());
    let lease = target.lock_installation().unwrap();
    assert!(
        target
            .selected_restore_preview(&bundle, &secret(), false, None)
            .is_err()
    );
    assert!(
        target
            .create(
                Provider::Gmail,
                "unrelated@example.test".into(),
                &SecretValue::new(b"invented".to_vec())
            )
            .is_err()
    );
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(target.paths.config.join("account-installation.lock"))
            .unwrap()
            .ino()
    };
    drop(lease);
    target
        .selected_restore_preview(&bundle, &secret(), false, None)
        .unwrap();
    target
        .create(
            Provider::Gmail,
            "unrelated@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(target.paths.config.join("account-installation.lock"))
                .unwrap()
                .ino(),
            inode
        );
    }
}

#[test]
fn selected_folder_journal_restores_held_and_retains_later_independent_effects() {
    let root = tempfile::tempdir().unwrap();
    let (mut repo, account, fake) =
        crate::acquisition::folder_provisioning::tests::install(root.path());
    crate::acquisition::folder_provisioning::tests::commit_saved_workspace_folders(
        &mut repo,
        &account,
        root.path(),
    );
    let before = repo.store(&account).unwrap().load_state().unwrap();
    let ledger = InstalledLedger::install(root.path().join("ledger")).unwrap();
    let (_, bundle) = capture(
        &repo,
        std::slice::from_ref(&account.id),
        &ledger,
        root.path(),
    );
    let effects = crate::acquisition::sieve_transaction::tests::independent_effects(
        root.path(),
        &repo.store(&account).unwrap(),
    );
    crate::acquisition::folder_provisioning::tests::later_folder_change(
        &repo,
        &account,
        root.path(),
    );
    let independent = repo.fixture_sieve_store(&account.id, root.path()).unwrap();
    let later = fs::read(independent.path()).unwrap();
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
    fs::remove_file(fresh_root.path().join("ledger/addresses.yaml")).unwrap();
    let p = fresh
        .selected_restore_preview(&bundle, &secret(), true, Some(&target))
        .unwrap();
    fresh
        .selected_restore(&bundle, &secret(), &p, Some(&target), |_| Ok(()))
        .unwrap();
    fake.install(&mut fresh);
    let account = fresh.load(&account.id).unwrap();
    let store = fresh.store(&account).unwrap();
    let mut restored = store.load_state().unwrap();
    assert!(serde_json::to_value(&restored).unwrap() == serde_json::to_value(&before).unwrap());
    assert_eq!(
        fs::read(fresh_root.path().join("ledger/addresses.yaml")).unwrap(),
        fs::read(root.path().join("ledger/addresses.yaml")).unwrap()
    );
    assert!(store.recovery_pending());
    assert!(
        fresh
            .folder_preview(
                &account,
                &store,
                &restored,
                crate::acquisition::Cancellation::default()
            )
            .is_err()
    );
    let id = restored.folder_provisioning.history[0].id.clone();
    assert!(
        fresh
            .folder_reconcile(
                &account,
                &store,
                &mut restored,
                &id,
                crate::acquisition::Cancellation::default()
            )
            .is_err()
    );
    assert_eq!(fs::read(independent.path()).unwrap(), later);
    drop(independent);
    assert_eq!(
        crate::acquisition::sieve_transaction::tests::independent_effects(root.path(), &store),
        effects
    );
}

#[test]
fn selected_schema_fourteen_keeps_complete_delivery_state_and_restored_holds() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _folder, _sieve) =
        crate::acquisition::workspace_authority::envelope_tests::install(root.path());
    let ledger = InstalledLedger::install(root.path().join("ledger")).unwrap();
    let before = repo.store(&account).unwrap().load_state().unwrap();
    let (manifest, bundle) = capture(
        &repo,
        std::slice::from_ref(&account.id),
        &ledger,
        root.path(),
    );
    assert_eq!(manifest.accounts[0].state_schema, crate::census::SCHEMA);
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let target = InstalledLedger::install(fresh_root.path().join("ledger")).unwrap();
    fs::remove_file(fresh_root.path().join("ledger/addresses.yaml")).unwrap();
    let preview = fresh
        .selected_restore_preview(&bundle, &secret(), true, Some(&target))
        .unwrap();
    fresh
        .selected_restore(&bundle, &secret(), &preview, Some(&target), |_| Ok(()))
        .unwrap();
    let restored = fresh.load(&account.id).unwrap();
    fresh.install_synthetic_envelope(&restored, "delivery-fixture-v1");
    let store = fresh.store(&restored).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(
        encoded(&before).unwrap().as_slice(),
        encoded(&after).unwrap().as_slice()
    );
    assert!(store.recovery_pending());
    let p = crate::rules::workspace::preview(
        &after,
        &fresh.workspace_context(&restored, &store).unwrap(),
        store.keys(),
    )
    .unwrap();
    assert_eq!(
        p.proof.artifact.context().recipient_evidence,
        crate::rules::EvidenceSource::Unknown
    );
    assert!(
        p.proof
            .decisions
            .iter()
            .all(|d| d.review && d.keyword_additions.is_empty())
    );
}

#[test]
fn selected_schema_twenty_four_preserves_sender_sets_exports_and_recovery_hold() {
    use crate::rules::{config, workspace};
    let (root, repo, ids, ledger) = fixture();
    let account = repo.load(&ids[0]).unwrap();
    let before = {
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let input = &mut state.workspace.as_mut().unwrap().input;
        let (mut preset, mut overrides) =
            config::documents(&input.documents.preset_yaml, &input.documents.override_yaml)
                .unwrap();
        preset.core_senders = Some(config::CoreSenders { major: 1 });
        input.documents.preset_yaml = serde_yaml_ng::to_string(&preset).unwrap();
        overrides.patches.push(config::Patch {
            id: "sender.core.money".into(),
            enabled: None,
            folder: None,
            when: None,
            extend: vec![config::Condition::FromDomain {
                value: "recovery.example.test".into(),
            }],
            accounts: None,
            mark_read: None,
        });
        input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
        let context = workspace::WorkspaceContext::local(Provider::MailboxOrg);
        let preview = workspace::preview(&state, &context, store.keys()).unwrap();
        workspace::confirm_export(&mut state, &context, store.keys(), &preview.receipt).unwrap();
        assert!(
            state
                .exports
                .last()
                .unwrap()
                .artifact
                .effective()
                .iter()
                .any(|effective| {
                    matches!(&effective.definition,
                crate::rules::program::EffectiveDefinition::Filing { rule }
                if rule.when.iter().any(|condition| matches!(condition,
                    config::Condition::SenderSet { domains, .. }
                    if domains.contains("recovery.example.test"))))
                })
        );
        store.save_state(&mut state).unwrap();
        state
    };
    let (manifest, bundle) = capture(&repo, &ids[..1], &ledger, root.path());
    assert_eq!(manifest.accounts[0].state_schema, 24);
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
    let preview = fresh
        .selected_restore_preview(&bundle, &secret(), false, None)
        .unwrap();
    fresh
        .selected_restore(&bundle, &secret(), &preview, None, |_| Ok(()))
        .unwrap();
    let restored = fresh.load(&ids[0]).unwrap();
    let store = fresh.store(&restored).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(
        encoded(&before).unwrap().as_slice(),
        encoded(&after).unwrap().as_slice()
    );
    assert_eq!(
        after.exports.last().unwrap().artifact.identity(),
        before.exports.last().unwrap().artifact.identity()
    );
    assert!(store.recovery_pending());
    let context = fresh.workspace_context(&restored, &store).unwrap();
    let preview = workspace::preview(&after, &context, store.keys()).unwrap();
    assert_eq!(
        preview.proof.artifact.context().recipient_evidence,
        crate::rules::EvidenceSource::Unknown
    );
    assert!(
        preview
            .proof
            .decisions
            .iter()
            .all(|d| d.review && d.copies.is_empty() && !d.mark_read)
    );
}
