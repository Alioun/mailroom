use super::*;
use crate::acquisition::accounts::process_tests::persistent_accounts;
use crate::census::AccountState;

fn secret() -> SecretValue {
    SecretValue::new(vec![0x71; 32])
}

#[test]
fn retained_deployment_artifacts_survive_rotation_and_single_account_recovery_without_exports() {
    let (root, repo, account) = fixture();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    crate::rules::landing::tests::seed_deployment(&mut saved);
    store.save_state(&mut saved).unwrap();
    let before = serde_json::to_vec(&saved.deployment_artifacts).unwrap();
    drop(store);
    let effects = root.path().join("effects");
    fs::create_dir_all(&effects).unwrap();
    repo.rotate(&account.id, &effects).unwrap();
    let current = repo.load(&account.id).unwrap();
    assert_eq!(
        serde_json::to_vec(&state(&repo, &current).deployment_artifacts).unwrap(),
        before
    );
    let (manifest, bundle) = capture(&repo, &current, root.path());
    assert_eq!(manifest.state_schema, crate::census::SCHEMA);
    let target = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(target.path());
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
        .unwrap();
    let restored = fresh.load(&account.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    let saved = store.load_state().unwrap();
    assert!(saved.exports.is_empty());
    assert!(store.recovery_pending());
    assert_eq!(
        serde_json::to_vec(&saved.deployment_artifacts).unwrap(),
        before
    );
    let comparison = serde_json::to_value(crate::rules::landing::compare(&saved, true)).unwrap();
    assert_eq!(comparison["baseline_source"], "retained_deployment");
    assert_eq!(comparison["historical"], true);
}

#[test]
fn transactions_round_trip_without_resumption_authority_under_single_recovery_hold() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, fake) = crate::acquisition::sieve_transaction::tests::install(root.path());
    crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
    let before = state(&repo, &account);
    let (_, bundle) = capture(&repo, &account, root.path());
    crate::acquisition::sieve_transaction::tests::later_provider_change(&repo, &account);
    let independent = repo.fixture_sieve_store(&account.id, root.path()).unwrap();
    let later_effects = fs::read(independent.path()).unwrap();
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
        .unwrap();
    fake.install(&mut fresh, &account.id);
    let restored = fresh.load(&account.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    let mut saved = store.load_state().unwrap();
    assert!(saved.sieve_transactions == before.sieve_transactions);
    assert!(
        fresh
            .transaction_preview(
                &restored,
                &store,
                &saved,
                crate::acquisition::sieve_transaction::Request::Deploy,
                crate::acquisition::Cancellation::default()
            )
            .is_err()
    );
    let id = saved.sieve_transactions.history[0].id.clone();
    assert!(
        fresh
            .transaction_reconcile(
                &restored,
                &store,
                &mut saved,
                &id,
                crate::acquisition::Cancellation::default()
            )
            .is_err()
    );
    assert!(fs::read(independent.path()).unwrap() == later_effects);
}

#[test]
fn schema_ten_protocol_and_proposal_sieve_facts_round_trip_with_single_account_recovery() {
    let (root, mut repo, account) = fixture();
    crate::acquisition::install_provider_fixture(&mut repo, root.path());
    let fake = crate::acquisition::SieveFake::start("normal", None);
    repo.install_sieve_fixture(&fake);
    {
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        crate::acquisition::sieve_discovery::tests::seed(&mut saved, store.keys());
        store.save_state(&mut saved).unwrap();
    }
    repo.scan(
        &account.id,
        crate::acquisition::Cancellation::default(),
        |_| {},
    )
    .unwrap();
    repo.discover_sieve(&account.id, crate::acquisition::Cancellation::default())
        .unwrap();
    let before = state(&repo, &account);
    assert!(before.census.acquisition.is_protocol());
    let (manifest, bundle) = capture(&repo, &account, root.path());
    assert_eq!(manifest.state_schema, crate::census::SCHEMA);
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    let restored = fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
        .unwrap();
    assert!(restored.reconciliation_required);
    let account = fresh.load(&account.id).unwrap();
    let store = fresh.store(&account).unwrap();
    let mut after = store.load_state().unwrap();
    assert_eq!(
        encoded(&before).unwrap().as_slice(),
        encoded(&after).unwrap().as_slice()
    );
    assert!(after.sieve.last_successful.is_some());
    assert!(store.save_state(&mut after).is_err());
    drop(store);
    fresh.install_sieve_fixture(&fake);
    let commands = fake.commands.lock().unwrap().len();
    assert_eq!(
        fresh.discover_sieve(&account.id, crate::acquisition::Cancellation::default()),
        Err(AcquisitionError::LifecyclePending)
    );
    assert_eq!(fake.commands.lock().unwrap().len(), commands);
}
fn fixture() -> (tempfile::TempDir, Accounts, Account) {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = repo
        .create(
            Provider::MailboxOrg,
            "recovery@example.test".into(),
            &SecretValue::new(b"invented-password".to_vec()),
        )
        .unwrap();
    let store = repo.store(&a).unwrap();
    let mut saved = store.load_state().unwrap();
    crate::rules::proposals::tests::seed(&mut saved, store.keys());
    saved
        .winddown
        .dismissed
        .insert("synthetic-stable-row".into());
    saved.winddown.settings.comparisons.insert(
        "uncaptured-peer".into(),
        BTreeSet::from(["synthetic-address".into()]),
    );
    store.save_state(&mut saved).unwrap();
    drop(store);
    (root, repo, a)
}
fn capture(repo: &Accounts, a: &Account, root: &Path) -> (Manifest, PathBuf) {
    let (preview, _) = repo.capture_preview(&a.id, 1792915200).unwrap();
    let id = repo
        .capture(&preview, &secret(), &root.join("bundles"), |_| Ok(()))
        .unwrap();
    (preview, bundle_file(&root.join("bundles"), &id).unwrap())
}
fn state(repo: &Accounts, a: &Account) -> AccountState {
    repo.store(a).unwrap().load_state().unwrap()
}

#[test]
fn lost_key_fresh_install_preserves_exact_state_and_stable_identity_but_invalidates_approvals() {
    for rotate in [false, true] {
        let (original, repo, mut a) = fixture();
        if rotate {
            repo.rotate(&a.id, &original.path().join("external-reality"))
                .unwrap();
            a = repo.load(&a.id).unwrap();
        }
        let before = state(&repo, &a);
        let store = repo.store(&a).unwrap();
        let identity = store
            .keys()
            .identity_binding_for(b"stable-operation", b"invented-op");
        let approval = store.keys().binding_for(b"approval", b"exact-action");
        let raw = fs::read(store.path()).unwrap();
        drop(store);
        let (manifest, bundle) = capture(&repo, &a, original.path());
        assert!(manifest.dependencies.contains("uncaptured-peer"));
        let independent = original.path().join("external-reality");
        private_directory(&independent).unwrap();
        fs::write(
            independent.join("accepted-after-snapshot"),
            b"send accepted once; unsubscribe uncertain",
        )
        .unwrap();
        drop(repo);
        fs::rename(
            original.path().join("fake-credentials"),
            original.path().join("lost-credentials-preserved"),
        )
        .unwrap();
        fs::rename(
            original.path().join("data"),
            original.path().join("lost-data-preserved"),
        )
        .unwrap();
        fs::rename(
            original.path().join("config"),
            original.path().join("lost-config-preserved"),
        )
        .unwrap();
        let fresh = persistent_accounts(original.path());
        assert!(matches!(
            fresh.backend.get(&a.local_secret_ref),
            Err(AcquisitionError::CredentialMissing)
        ));
        let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
        let restored = fresh
            .restore(&bundle, &secret(), &preview, |_| Ok(()))
            .unwrap();
        assert!(restored.reconciliation_required);
        let a = fresh.load(&a.id).unwrap();
        assert!(matches!(
            fresh.password(&a),
            Err(AcquisitionError::CredentialMissing)
        ));
        let store = fresh.store(&a).unwrap();
        assert_eq!(fs::read(store.path()).unwrap(), raw);
        let mut after = store.load_state().unwrap();
        assert_eq!(
            encoded(&before).unwrap().as_slice(),
            encoded(&after).unwrap().as_slice()
        );
        assert_eq!(
            store
                .keys()
                .identity_binding_for(b"stable-operation", b"invented-op"),
            identity
        );
        assert_ne!(
            store.keys().binding_for(b"approval", b"exact-action"),
            approval
        );
        assert!(store.save_state(&mut after).is_err());
        assert_eq!(
            fs::read(independent.join("accepted-after-snapshot")).unwrap(),
            b"send accepted once; unsubscribe uncertain"
        );
    }
}
#[test]
fn tampering_wrong_secret_truncation_unknown_schema_and_paths_reject_without_changes() {
    let (root, repo, a) = fixture();
    let (_, bundle) = capture(&repo, &a, root.path());
    let bytes = fs::read(&bundle).unwrap();
    assert!(bytes.starts_with(stream::MAGIC));
    assert!(open(&bundle, &SecretValue::new(vec![4; 32])).is_err());
    let altered = root.path().join("altered");
    fs::create_dir_all(&altered).unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let path = altered.join(name);
        fs::write(&path, bytes).unwrap();
        path
    };
    for cut in [0, 10, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            open(&write("cut", &bytes[..cut]), &secret()).is_err(),
            "{cut}"
        );
    }
    assert!(open(&write("extended", &[&bytes[..], b"x"].concat()), &secret()).is_err());
    for at in [bytes.len() - 7, bytes.len() / 2, stream::MAGIC.len() + 20] {
        let mut corrupt = bytes.clone();
        corrupt[at] ^= 1;
        assert!(
            open(&write("corrupt", &corrupt), &secret()).is_err(),
            "{at}"
        );
    }
    for attack in [
        "schema",
        "path",
        "extra",
        "identity",
        "key",
        "ciphertext",
        "inventory",
    ] {
        let (mut c, _) = open(&bundle, &secret()).unwrap();
        match attack {
            "schema" => c.manifest.state_schema = 999,
            "path" => c.manifest.resources[0].resource = "../../outside".into(),
            "extra" => c.manifest.resources.push(Inventory {
                resource: "unknown".into(),
                bytes: 0,
                sha256: digest(b""),
            }),
            "key" => c.key.0[0] ^= 1,
            "ciphertext" => {
                let path = c.resource.path().to_path_buf();
                let mut file = fs::read(&path).unwrap();
                file[100] ^= 1;
                fs::write(&path, &file).unwrap();
                c.manifest.resources =
                    inventory(&c.account, &stream::file_digest(&path).unwrap()).unwrap();
            }
            "inventory" => c.manifest.key_material.clear(),
            _ => c.manifest.account = AccountId::new("other-account").unwrap(),
        }
        let body = Body::Single {
            captured: c.header(),
        };
        let resealed = reseal(&altered, &secret(), &body, &[c.resource.path()]);
        assert!(open(&resealed, &secret()).is_err(), "{attack}");
        let legacy = write("legacy.jwe", &seal_legacy(&c.legacy(), &secret()));
        assert!(open(&legacy, &secret()).is_err(), "legacy {attack}");
    }
    let (c, _) = open(&bundle, &secret()).unwrap();
    let foreign = Body::Single {
        captured: c.header(),
    };
    let swapped = reseal(
        &altered,
        &secret(),
        &foreign,
        &[c.resource.path(), c.resource.path()],
    );
    assert!(
        open(&swapped, &secret()).is_err(),
        "an extra file is refused"
    );
    assert!(repo.restore_preview(&bundle, &secret()).is_err());
    assert_eq!(fs::read(bundle).unwrap(), bytes);
}

/// Bundles written by earlier builds, one JWE with the account file inline,
/// restore exactly as before into a fresh installation, held.
#[test]
fn version_two_jwe_bundles_still_restore_exactly() {
    let (root, repo, a) = fixture();
    let before = state(&repo, &a);
    let raw = fs::read(repo.store(&a).unwrap().path()).unwrap();
    let (_, bundle) = capture(&repo, &a, root.path());
    let (c, _) = open(&bundle, &secret()).unwrap();
    let legacy = seal_legacy(&c.legacy(), &secret());
    let directory = root.path().join("legacy");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}.jwe", digest(&legacy)));
    fs::write(&path, &legacy).unwrap();
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&path, &secret()).unwrap();
    assert_eq!(preview.bundle, digest(&legacy));
    fresh
        .restore(&path, &secret(), &preview, |_| Ok(()))
        .unwrap();
    let restored = fresh.load(&a.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    assert_eq!(fs::read(store.path()).unwrap(), raw);
    assert!(store.recovery_pending(), "restored accounts stay held");
    assert_eq!(
        serde_json::to_value(store.load_state().unwrap().protocol_backlog).unwrap(),
        serde_json::to_value(before.protocol_backlog).unwrap()
    );
}
#[test]
fn stale_capture_and_no_clobber_preserve_source_and_existing_bundle() {
    let (root, repo, a) = fixture();
    let (preview, bundle) = capture(&repo, &a, root.path());
    let bytes = fs::read(&bundle).unwrap();
    let store = repo.store(&a).unwrap();
    let mut saved = store.load_state().unwrap();
    store.save_state(&mut saved).unwrap();
    drop(store);
    assert!(
        repo.capture(
            &preview,
            &secret(),
            &root.path().join("bundles"),
            |_| panic!("stale approval reached")
        )
        .is_err()
    );
    assert!(publish_new(&bundle, b"partial").is_err());
    assert_eq!(fs::read(&bundle).unwrap(), bytes);
}
#[test]
fn every_restore_boundary_recovers_without_overwriting_unrelated_accounts_or_ledger() {
    for phase in [
        "restore_intent",
        "restore_key",
        "restore_identity",
        "restore_authenticated",
        "restore_data",
        "restore_metadata",
        "restore_committed",
    ] {
        let (source, repo, a) = fixture();
        let (_, bundle) = capture(&repo, &a, source.path());
        let fresh_root = tempfile::tempdir().unwrap();
        let fresh = persistent_accounts(fresh_root.path());
        let other = fresh
            .create(
                Provider::Gmail,
                "unrelated@example.test".into(),
                &SecretValue::new(b"invented".to_vec()),
            )
            .unwrap();
        let other_before = encoded(&state(&fresh, &other)).unwrap();
        fs::write(
            fresh_root.path().join("addresses.yaml"),
            b"owner-edited-after-capture",
        )
        .unwrap();
        let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (Some(phase.into()), 0));
        assert!(
            fresh
                .restore(&bundle, &secret(), &preview, |_| Ok(()))
                .is_err(),
            "{phase}"
        );
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (None, 0));
        drop(fresh);
        let fresh = persistent_accounts(fresh_root.path());
        if phase != "restore_committed" {
            assert!(fresh.require_ready(&a.id).is_err());
            assert!(
                fresh
                    .resume_restore(&a.id, &SecretValue::new(vec![8; 32]))
                    .is_err()
            );
            fresh.resume_restore(&a.id, &secret()).unwrap();
        }
        let restored = fresh.load(&a.id).unwrap();
        assert_eq!(
            encoded(&state(&fresh, &restored)).unwrap().as_slice(),
            encoded(&state(&repo, &a)).unwrap().as_slice()
        );
        assert_eq!(
            other_before.as_slice(),
            encoded(&state(&fresh, &other)).unwrap().as_slice()
        );
        assert_eq!(
            fs::read(fresh_root.path().join("addresses.yaml")).unwrap(),
            b"owner-edited-after-capture"
        );
        assert!(fresh.restore_preview(&bundle, &secret()).is_err());
    }
}

#[test]
fn recovery_snapshot_worker() {
    let Some(root) = std::env::var_os("MAILROOM_RECOVERY_INSTALLATION") else {
        return;
    };
    let root = PathBuf::from(root);
    let repo = persistent_accounts(&root);
    let id = AccountId::new(&std::env::var("MAILROOM_RECOVERY_ACCOUNT").unwrap()).unwrap();
    let a = repo.load(&id).unwrap();
    let reality = PathBuf::from(std::env::var_os("MAILROOM_RECOVERY_REALITY").unwrap());
    let dirs = [
        root.join("data"),
        reality.clone(),
        reality.join("invoice-transport"),
        reality.join("unsubscribe-transport"),
    ];
    let mut hashes = std::collections::BTreeMap::new();
    let mut histories = std::collections::BTreeMap::new();
    let mut identity = None;
    for (kind, dir) in dirs.iter().enumerate() {
        let store = repo.open_store(&a, dir, kind).unwrap();
        if !store.path().exists() {
            continue;
        }
        let saved = store.load_state().unwrap();
        hashes.insert(kind, digest(&encoded(&saved).unwrap()));
        histories.insert(kind,serde_json::json!({"backlog":saved.backlog,"invoice_sends":saved.invoices.sends,"unsubscribe":saved.unsubscribe.history,"winddown":saved.winddown.history,"exports":saved.exports,"effects":saved.synthetic_effects.len(),"invoice_effects":saved.invoices.synthetic_received.len(),"unsubscribe_effects":saved.unsubscribe.synthetic_acknowledgments.len()}));
        if kind == 0 {
            identity = Some(
                serde_json::json!({"stable":store.keys().identity_binding_for(b"stable-recovery-test",b"effect-id"),"approval":store.keys().binding_for(b"recovery-test",b"approval"),"placeholder":store.keys().placeholder("sender","news@shop.example")}),
            );
        }
    }
    fs::write(
        root.join("recovery-observation.json"),
        serde_json::to_vec(
            &serde_json::json!({"hashes":hashes,"histories":histories,"identity":identity}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn partial_bundle_publication_and_oversized_input_preserve_usable_copies_and_source() {
    let (root, repo, a) = fixture();
    let (_, usable) = capture(&repo, &a, root.path());
    let usable_bytes = fs::read(&usable).unwrap();
    let source = fs::read(repo.store(&a).unwrap().path()).unwrap();
    for phase in ["recovery_staged", "recovery_bundle_published"] {
        let (preview, _) = repo.capture_preview(&a.id, 1792915201).unwrap();
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (Some(phase.into()), 0));
        assert!(
            repo.capture(
                &preview,
                &secret(),
                &root.path().join("bundles"),
                |_| Ok(())
            )
            .is_err()
        );
        lifecycle::FAIL.with(|f| *f.borrow_mut() = (None, 0));
        assert_eq!(fs::read(&usable).unwrap(), usable_bytes);
        assert_eq!(fs::read(repo.store(&a).unwrap().path()).unwrap(), source);
        for entry in fs::read_dir(root.path().join("bundles")).unwrap() {
            let path = entry.unwrap().path();
            assert_eq!(path.extension().unwrap(), stream::EXTENSION);
            assert!(open(&path, &secret()).is_ok());
        }
    }
    let oversized = root.path().join("oversized.jwe");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_BUNDLE + 1)
        .unwrap();
    let fresh = persistent_accounts(&root.path().join("fresh"));
    assert!(fresh.restore_preview(&oversized, &secret()).is_err());
    assert!(!fresh.paths.config.exists());
    assert!(!fresh.paths.data.exists());
}

#[test]
fn organisational_decisions_survive_real_rotation_and_recovery_without_approval_authority() {
    use crate::rules::proposals::{self, Disposition, Selected, Selection};
    let (root, repo, account) = fixture();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&3)
        .unwrap()
        .headers
        .from[0]
        .address = "robot@new-service.example".into();
    store.save_state(&mut saved).unwrap();
    let g = proposals::generate(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
    )
    .unwrap();
    let selection = Selection {
        account: g.account,
        generation: g.generation,
        candidates: vec![Selected {
            id: g
                .candidates
                .iter()
                .find(|c| !c.rejected)
                .unwrap()
                .id
                .clone(),
            change: g
                .candidates
                .iter()
                .find(|c| !c.rejected)
                .unwrap()
                .change
                .clone(),
        }],
    };
    let old = proposals::review(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
        &selection,
        Disposition::Accept,
    )
    .unwrap();
    let decisions = serde_json::to_value(&saved.proposals).unwrap();
    drop(store);
    repo.rotate(&account.id, &root.path().join("external-reality"))
        .unwrap();
    let account = repo.load(&account.id).unwrap();
    let store = repo.store(&account).unwrap();
    saved = store.load_state().unwrap();
    assert_eq!(serde_json::to_value(&saved.proposals).unwrap(), decisions);
    assert!(
        proposals::confirm(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &selection,
            Disposition::Accept,
            &old.receipt
        )
        .is_err()
    );
    let current = proposals::review(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(account.provider),
        store.keys(),
        &selection,
        Disposition::Accept,
    )
    .unwrap();
    drop(store);
    let (_, bundle) = capture(&repo, &account, root.path());
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
        .unwrap();
    let restored = fresh.load(&account.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    let mut saved = store.load_state().unwrap();
    assert_eq!(serde_json::to_value(&saved.proposals).unwrap(), decisions);
    assert!(
        proposals::confirm(
            &mut saved,
            &crate::rules::workspace::WorkspaceContext::local(account.provider),
            store.keys(),
            &selection,
            Disposition::Accept,
            &current.receipt
        )
        .is_err()
    );
    assert!(store.recovery_pending());
    assert!(store.save_state(&mut saved).is_err());
}

#[test]
fn folder_journal_restores_held_without_reverting_later_independent_topology() {
    let root = tempfile::tempdir().unwrap();
    let (mut repo, account, fake) =
        crate::acquisition::folder_provisioning::tests::install(root.path());
    crate::acquisition::folder_provisioning::tests::commit_saved_workspace_folders(
        &mut repo,
        &account,
        root.path(),
    );
    let before = state(&repo, &account);
    let (manifest, bundle) = capture(&repo, &account, root.path());
    assert_eq!(manifest.state_schema, crate::census::SCHEMA);
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
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
        .unwrap();
    fake.install(&mut fresh);
    let account = fresh.load(&account.id).unwrap();
    let store = fresh.store(&account).unwrap();
    let mut restored = store.load_state().unwrap();
    assert!(serde_json::to_value(&restored).unwrap() == serde_json::to_value(&before).unwrap());
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
fn schema_fourteen_delivery_round_trips_complete_state_without_recovering_authority() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _folder, _sieve) =
        crate::acquisition::workspace_authority::envelope_tests::install(root.path());
    let before = state(&repo, &account);
    let (manifest, bundle) = capture(&repo, &account, root.path());
    assert_eq!(manifest.state_schema, crate::census::SCHEMA);
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
    fresh
        .restore(&bundle, &secret(), &preview, |_| Ok(()))
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
            .all(|d| d.review && d.keyword_additions.is_empty() && d.copies.is_empty())
    );
}

/// Captures a personal-scale account into a streamed bundle and restores it
/// into a fresh installation whose key, data and metadata were lost.
#[test]
#[ignore = "personal-scale measurement; run explicitly"]
fn scale_recovery_capture() {
    use crate::census::scale;
    let (root, repo, a) = fixture();
    let setup = scale::inflate_store(&repo.store(&a).unwrap(), false);
    scale::report("recovery.setup", setup, Default::default());
    let raw = fs::read(repo.store(&a).unwrap().path()).unwrap();
    let before = encoded(&state(&repo, &a)).unwrap();
    let preview = scale::measure("recovery.capture_preview", serde_json::json!({}), || {
        repo.capture_preview(&a.id, 1792915200)
    });
    let (preview, _) = preview.unwrap();
    let id = scale::measure("recovery.capture", serde_json::json!({}), || {
        repo.capture(
            &preview,
            &secret(),
            &root.path().join("bundles"),
            |_| Ok(()),
        )
    })
    .unwrap();
    let bundle = bundle_file(&root.path().join("bundles"), &id).unwrap();
    scale::report(
        "recovery.bundle",
        serde_json::json!({"bundle_bytes": fs::metadata(&bundle).unwrap().len(), "account_file_bytes": raw.len()}),
        Default::default(),
    );
    drop(repo);
    for lost in ["fake-credentials", "data", "config"] {
        fs::rename(
            root.path().join(lost),
            root.path().join(format!("lost-{lost}")),
        )
        .unwrap();
    }
    let fresh = persistent_accounts(root.path());
    let restore = scale::measure("recovery.restore_preview", serde_json::json!({}), || {
        fresh.restore_preview(&bundle, &secret())
    })
    .unwrap();
    scale::measure("recovery.restore", serde_json::json!({}), || {
        fresh.restore(&bundle, &secret(), &restore, |_| Ok(()))
    })
    .unwrap();
    let restored = fresh.load(&a.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    let exact = fs::read(store.path()).unwrap() == raw;
    let same_state = encoded(&store.load_state().unwrap()).unwrap().as_slice() == before.as_slice();
    scale::report(
        "recovery.result",
        serde_json::json!({"exact_file": exact, "same_state": same_state, "held": store.recovery_pending()}),
        Default::default(),
    );
    assert!(exact && same_state && store.recovery_pending());
}

/// A rotated account captured before identity commitments, or a bundle whose identity no
/// longer matches its commitment, is refused before any key is imported. The
/// commitment-free capture is otherwise consistent: its manifest inventory is recomputed
/// for the metadata without the commitment, and its preview and pending marker are its own.
/// A restore left pending by an earlier build, with keys already imported, refuses on
/// resume and keeps those keys, the marker and the staged bundle.
#[test]
fn rotated_bundles_without_a_matching_identity_commitment_are_refused_before_import() {
    let (root, repo, a) = fixture();
    repo.rotate(&a.id, &root.path().join("external-reality"))
        .unwrap();
    let a = repo.load(&a.id).unwrap();
    let (_, bundle) = capture(&repo, &a, root.path());
    let (c, _) = open(&bundle, &secret()).unwrap();
    let directory = root.path().join("legacy");
    fs::create_dir_all(&directory).unwrap();
    let write = |captured: &LegacyCaptured| {
        let sealed = seal_legacy(captured, &secret());
        let path = directory.join(format!("{}.jwe", digest(&sealed)));
        fs::write(&path, &sealed).unwrap();
        (path, sealed)
    };
    let resource = stream::file_digest(c.resource.path()).unwrap();
    let mut uncommitted = c.legacy();
    uncommitted.account.identity_commitment = None;
    uncommitted.manifest.resources = inventory(&uncommitted.account, &resource).unwrap();
    assert!(
        uncommitted.manifest.resources != c.manifest.resources,
        "the inventory follows the metadata"
    );
    let mut planted = c.legacy();
    planted.identity = Some(SecretBytes(Zeroizing::new(vec![0x5a; 128])));
    let identity_ref = a.identity_ref.clone().unwrap();

    for (case, captured) in [("uncommitted", uncommitted), ("planted identity", planted)] {
        let (path, sealed) = write(&captured);
        let id = digest(&sealed);
        let fresh_root = tempfile::tempdir().unwrap();
        let fresh = persistent_accounts(fresh_root.path());
        let preview = RestorePreview {
            manifest: captured.manifest.clone(),
            bundle: id.clone(),
            target: fresh.paths.config.to_string_lossy().into_owned(),
            account_absent: true,
            shared_state: "excluded_preserve_current".into(),
            consequences: LIMITATIONS.into(),
        };
        assert!(fresh.restore_preview(&path, &secret()).is_err(), "{case}");
        assert!(
            fresh
                .restore(&path, &secret(), &preview, |_| Ok(()))
                .is_err(),
            "{case}"
        );
        for reference in [&a.local_secret_ref, &identity_ref, &a.password_ref] {
            assert_eq!(
                fresh.backend.get(reference).err(),
                Some(AcquisitionError::CredentialMissing),
                "{case}: no key was imported"
            );
        }
        assert!(!fresh.restore_marker(&a.id).exists(), "{case}");
        assert!(!fresh.metadata_path(&a.id).exists(), "{case}");
        assert_eq!(
            fs::read(&path).unwrap(),
            sealed,
            "{case}: the source bundle is kept"
        );

        // As an earlier build left it: bundle staged, marker written, keys imported.
        fresh.stage_bundle(&path, &id).unwrap();
        let tx = Transaction {
            version: 1,
            bundle: id.clone(),
            authority: nonce(),
            preview,
        };
        write_json(&fresh.restore_marker(&a.id), &tx).unwrap();
        fresh
            .put_exact(&a.local_secret_ref, &captured.key.0)
            .unwrap();
        fresh
            .put_exact(&identity_ref, &captured.identity.as_ref().unwrap().0)
            .unwrap();
        assert!(fresh.resume_restore(&a.id, &secret()).is_err(), "{case}");
        assert!(
            fresh.restore_marker(&a.id).exists(),
            "{case}: the marker is kept"
        );
        assert_eq!(
            fs::read(fresh.staged_bundle(&id).unwrap()).unwrap(),
            sealed,
            "{case}: the staged bundle is kept"
        );
        assert!(
            !fresh.metadata_path(&a.id).exists(),
            "{case}: nothing is installed"
        );
        assert_eq!(
            fresh.backend.get(&a.local_secret_ref).unwrap().bytes(),
            &captured.key.0[..],
            "{case}: imported keys are kept"
        );
        assert_eq!(
            fresh.backend.get(&identity_ref).unwrap().bytes(),
            &captured.identity.as_ref().unwrap().0[..],
            "{case}"
        );
        assert_eq!(
            fresh.backend.get(&a.password_ref).err(),
            Some(AcquisitionError::CredentialMissing),
            "{case}: resuming adds no key"
        );
    }
    let (committed, _) = write(&c.legacy());
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
    let preview = fresh.restore_preview(&committed, &secret()).unwrap();
    fresh
        .restore(&committed, &secret(), &preview, |_| Ok(()))
        .unwrap();
    assert!(
        fresh.store(&fresh.load(&a.id).unwrap()).is_ok(),
        "the committed bundle restores"
    );
}

#[test]
fn copy_phase_history_survives_single_recovery_without_execution_authority() {
    use crate::backlog::protocol::tests::{Stage, run};
    for stage in [
        Stage::Copied,
        Stage::Marked,
        Stage::StoreUncertain,
        Stage::Undone,
    ] {
        let (root, repo, account) = fixture();
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.protocol_backlog.runs = vec![run(&account.id, stage)];
        store.save_state(&mut saved).unwrap();
        let expected = serde_json::to_vec(&saved.protocol_backlog).unwrap();
        let ciphertext = fs::read(store.path()).unwrap();
        drop(store);
        let (manifest, bundle) = capture(&repo, &account, root.path());
        assert_eq!(manifest.state_schema, crate::census::SCHEMA);
        let destination = tempfile::tempdir().unwrap();
        let fresh = persistent_accounts(destination.path());
        let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
        fresh
            .restore(&bundle, &secret(), &preview, |_| Ok(()))
            .unwrap();
        let restored = fresh.load(&account.id).unwrap();
        let store = fresh.store(&restored).unwrap();
        let mut after = store.load_state().unwrap();
        assert_eq!(
            serde_json::to_vec(&after.protocol_backlog).unwrap(),
            expected,
            "{stage:?}"
        );
        assert_eq!(fs::read(store.path()).unwrap(), ciphertext);
        assert!(store.recovery_pending());
        assert!(!fresh.backlog_installed(Some(&account.id)));
        assert!(store.save_state(&mut after).is_err());
        assert!(
            fresh
                .backlog_run(
                    &restored,
                    &store,
                    &mut after,
                    &"a".repeat(64),
                    false,
                    crate::acquisition::Cancellation::default(),
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), ciphertext);
    }
}

#[test]
fn compensation_history_survives_single_recovery_without_execution_authority() {
    use crate::backlog::protocol::{CompensationPhase as Phase, compensation_fixture};
    for stage in [
        Phase::Prepared,
        Phase::StoreUncertain,
        Phase::Marked,
        Phase::ExpungeUncertain,
        Phase::Cleaned,
        Phase::ClearUncertain,
        Phase::Cleared,
        Phase::RestorePrepared,
        Phase::Restored,
        Phase::AfterClearPrepared,
        Phase::AfterClearStoreUncertain,
        Phase::AfterClearMarked,
        Phase::AfterClearExpungeUncertain,
        Phase::AfterClearCleaned,
        Phase::AfterClearConflict,
    ] {
        let (root, repo, account) = fixture();
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.protocol_backlog.runs = vec![compensation_fixture(&account.id, stage)];
        for entry in &mut saved.protocol_backlog.runs[0].compensation {
            entry.after_clear_approvals = entry
                .evidence
                .dependent_clears()
                .map(|clear| crate::backlog::protocol::DependentCleanupApproval {
                    clear,
                    receipt: "a".repeat(64),
                    binding: "b".repeat(64),
                })
                .collect();
        }
        store.save_state(&mut saved).unwrap();
        let expected = serde_json::to_vec(&saved.protocol_backlog).unwrap();
        let ciphertext = fs::read(store.path()).unwrap();
        drop(store);
        let (manifest, bundle) = capture(&repo, &account, root.path());
        assert_eq!(manifest.state_schema, crate::census::SCHEMA);
        let destination = tempfile::tempdir().unwrap();
        let fresh = persistent_accounts(destination.path());
        let preview = fresh.restore_preview(&bundle, &secret()).unwrap();
        fresh
            .restore(&bundle, &secret(), &preview, |_| Ok(()))
            .unwrap();
        let restored = fresh.load(&account.id).unwrap();
        let store = fresh.store(&restored).unwrap();
        let mut after = store.load_state().unwrap();
        assert_eq!(
            serde_json::to_vec(&after.protocol_backlog).unwrap(),
            expected,
            "{stage:?}"
        );
        assert_eq!(fs::read(store.path()).unwrap(), ciphertext);
        assert!(store.recovery_pending());
        assert!(!fresh.backlog_installed(Some(&account.id)));
        assert!(store.save_state(&mut after).is_err());
        assert!(
            fresh
                .backlog_run(
                    &restored,
                    &store,
                    &mut after,
                    &"a".repeat(64),
                    false,
                    crate::acquisition::Cancellation::default(),
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), ciphertext);
    }
}
