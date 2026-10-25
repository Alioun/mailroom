use super::*;
use crate::acquisition::accounts::process_tests::persistent_accounts;
use crate::census::AccountState;

fn secret() -> SecretValue {
    SecretValue::new(vec![0x71; 32])
}

#[test]
fn schema_eight_protocol_facts_round_trip_with_single_account_recovery() {
    let (root, mut repo, account) = fixture();
    crate::acquisition::install_provider_fixture(&mut repo, root.path());
    repo.scan(
        &account.id,
        crate::acquisition::Cancellation::default(),
        |_| {},
    )
    .unwrap();
    let before = state(&repo, &account);
    assert!(before.census.acquisition.is_protocol());
    let (manifest, bundle) = capture(&repo, &account, root.path());
    assert_eq!(manifest.state_schema, 8);
    let fresh_root = tempfile::tempdir().unwrap();
    let fresh = persistent_accounts(fresh_root.path());
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
    assert!(store.save_state(&mut after).is_err());
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
    (preview, root.join("bundles").join(format!("{id}.jwe")))
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
    assert!(open(&bytes, &SecretValue::new(vec![4; 32])).is_err());
    for cut in [0, 10, bytes.len() - 1] {
        assert!(open(&bytes[..cut], &secret()).is_err());
    }
    let mut corrupt = bytes.clone();
    let n = corrupt.len() - 7;
    corrupt[n] ^= 1;
    assert!(open(&corrupt, &secret()).is_err());
    for attack in [
        "schema",
        "path",
        "extra",
        "identity",
        "key",
        "ciphertext",
        "inventory",
    ] {
        let mut c = open(&bytes, &secret()).unwrap();
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
                c.ciphertext[0] ^= 1;
                c.manifest.resources = inventory(&c.account, &c.ciphertext).unwrap();
            }
            "inventory" => c.manifest.key_material.clear(),
            _ => c.manifest.account = AccountId::new("other-account").unwrap(),
        }
        assert!(
            open(&seal(&c, &secret()).unwrap(), &secret()).is_err(),
            "{attack}"
        );
    }
    assert!(repo.restore_preview(&bundle, &secret()).is_err());
    assert_eq!(fs::read(bundle).unwrap(), bytes);
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
    for phase in ["recovery_staged#2", "recovery_bundle_published"] {
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
            assert_eq!(path.extension().unwrap(), "jwe");
            assert!(open(&fs::read(path).unwrap(), &secret()).is_ok());
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
