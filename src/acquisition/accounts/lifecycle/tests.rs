use super::*;
use crate::acquisition::accounts::process_tests::persistent_accounts;

fn account(repo: &Accounts) -> Account {
    repo.create(
        Provider::MailboxOrg,
        "owner@example.test".into(),
        &SecretValue::new(b"synthetic-password".to_vec()),
    )
    .unwrap()
}
fn fail(name: Option<&str>) {
    FAIL.with(|f| *f.borrow_mut() = (name.map(str::to_owned), 0));
}
fn snapshots(repo: &Accounts, a: &Account, effects: &Path) -> Vec<Vec<u8>> {
    repo.lock_stores(a, effects)
        .unwrap()
        .iter()
        .map(|s| serde_json::to_vec(&s.load_state().unwrap()).unwrap())
        .collect()
}
fn populate(repo: &Accounts, a: &Account, effects: &Path) {
    for (i, store) in repo.lock_stores(a, effects).unwrap().iter().enumerate() {
        if i > 0 {
            store.save(&Census::new(a.id.clone())).unwrap();
        }
        let mut state = store.load_state().unwrap();
        state
            .census
            .confirmed_own_addresses
            .insert("owner@example.test".into());
        state.census.account_login = Some(format!("synthetic-history-{i}@example.test"));
        state
            .synthetic_uid_next
            .insert("retained-effect-identity".into(), 77);
        state.winddown.settings.winding_down = true;
        state
            .winddown
            .settings
            .comparisons
            .insert("missing-comparison".into(), Default::default());
        store.save_state(&mut state).unwrap();
    }
}
#[test]
fn every_creation_boundary_cleans_only_unpublished_resources_after_backend_reconstruction() {
    for phase in [
        "intent",
        "create_key",
        "create_password",
        "create_data",
        "metadata",
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        fs::create_dir_all(root.path().join("data")).unwrap();
        fs::write(root.path().join("data/unknown"), b"retain").unwrap();
        fail(Some(phase));
        assert!(
            repo.create(
                Provider::Gmail,
                "owner@example.test".into(),
                &SecretValue::new(b"synthetic-password".to_vec())
            )
            .is_err()
        );
        fail(None);
        drop(repo);
        let repo = persistent_accounts(root.path());
        let statuses = repo.recovery_status().unwrap();
        assert_eq!(statuses.len(), 1);
        let id = statuses[0].account.clone();
        repo.recover(&id, &root.path().join("mailboxes")).unwrap();
        repo.recover(&id, &root.path().join("mailboxes")).unwrap();
        if phase == "metadata" {
            let a = repo.load(&id).unwrap();
            repo.store(&a).unwrap().load_state().unwrap();
        } else {
            assert!(repo.list().unwrap().is_empty());
            assert!(
                !root
                    .path()
                    .join("data")
                    .join(format!("{}.census.enc", id.as_str()))
                    .exists()
            );
        }
        assert_eq!(
            fs::read(root.path().join("data/unknown")).unwrap(),
            b"retain"
        );
    }
}
#[test]
fn replacement_publishes_before_retirement_and_preserves_all_ciphertext() {
    for phase in ["intent", "replace_password", "metadata", "retire_password"] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        let before = repo.manifest(&a, &effects).unwrap();
        fail(Some(phase));
        assert!(
            repo.replace_password(
                &a.id,
                &SecretValue::new(b"replacement-synthetic-password".to_vec())
            )
            .is_err()
        );
        fail(None);
        assert!(repo.store(&a).is_err());
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects).unwrap();
        repo.recover(&a.id, &effects).unwrap();
        let current = repo.load(&a.id).unwrap();
        let expected = if matches!(phase, "metadata" | "retire_password") {
            b"replacement-synthetic-password".as_slice()
        } else {
            b"synthetic-password".as_slice()
        };
        assert!(repo.password(&current).unwrap().bytes() == expected);
        for r in before
            .iter()
            .filter(|r| r.path != repo.metadata_path(&a.id))
        {
            assert!(repo.manifest(&current, &effects).unwrap().contains(r));
        }
        assert_eq!(current.local_secret_ref, a.local_secret_ref);
        assert_eq!(current.id, a.id);
    }
}
#[test]
fn mixed_generation_recovery_preserves_all_stores_identity_and_invalidates_approvals() {
    for phase in [
        "intent",
        "rotate_identity",
        "rotate_key",
        "rotate_primary",
        "rotate_mailbox",
        "rotate_invoice",
        "rotate_unsubscribe",
        "metadata",
        "rotate_verified",
        "rotate_retire_data",
        "rotate_retire_data#2",
        "rotate_retire_data#3",
        "rotate_retire_data#4",
        "rotate_retire_key",
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let other = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        let before = snapshots(&repo, &a, &effects);
        let other_bytes = fs::read(repo.store(&other).unwrap().path()).unwrap();
        let (placeholder, subscription, row, approval) = {
            let s = repo.store(&a).unwrap();
            (
                s.keys().placeholder("sender", "a@example.test"),
                s.keys()
                    .identity_binding_for(b"mailroom/subscription/v1", b"x"),
                s.keys().identity_binding_for(b"winddown-row/v1", b"x"),
                s.keys()
                    .binding_for(b"mailroom/invoice-send-confirm/v1", b"x"),
            )
        };
        fail(Some(phase));
        assert!(repo.rotate(&a.id, &effects).is_err(), "{phase}");
        fail(None);
        assert!(repo.store(&a).is_err());
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects)
            .unwrap_or_else(|e| panic!("{phase}: {e}"));
        repo.recover(&a.id, &effects).unwrap();
        let current = repo.load(&a.id).unwrap();
        let after = snapshots(&repo, &current, &effects);
        for (i, (old, new)) in before.iter().zip(&after).enumerate() {
            let mut expected: serde_json::Value = serde_json::from_slice(old).unwrap();
            if i == 0 && current.generation > 0 {
                expected["revision"] =
                    serde_json::json!(expected["revision"].as_u64().unwrap() + 1);
            }
            assert_eq!(
                expected,
                serde_json::from_slice::<serde_json::Value>(new).unwrap(),
                "{phase} store {i}"
            );
        }
        let s = repo.store(&current).unwrap();
        assert_eq!(
            s.keys().placeholder("sender", "a@example.test"),
            placeholder
        );
        assert_eq!(
            s.keys()
                .identity_binding_for(b"mailroom/subscription/v1", b"x"),
            subscription
        );
        assert_eq!(s.keys().identity_binding_for(b"winddown-row/v1", b"x"), row);
        if current.generation > 0 {
            assert_ne!(
                s.keys()
                    .binding_for(b"mailroom/invoice-send-confirm/v1", b"x"),
                approval
            );
            assert!(matches!(
                repo.backend.get(&a.local_secret_ref),
                Err(AcquisitionError::CredentialMissing)
            ));
        }
        assert_eq!(
            fs::read(repo.store(&other).unwrap().path()).unwrap(),
            other_bytes
        );
    }
}
#[test]
fn forget_is_exact_retryable_and_retains_shared_unknown_files_and_lock_inodes() {
    use std::os::unix::fs::MetadataExt;
    for phase in [
        "intent",
        "forget_data",
        "forget_data#2",
        "forget_data#3",
        "forget_data#4",
        "forget_data#5",
        "forget_data#6",
        "forget_data#7",
        "forget_data#8",
        "forget_data#9",
        "forget_data#10",
        "forget_credential",
        "forget_credential#2",
        "forget_metadata",
        "intent#2",
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let other = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        let unknown = effects.join("unknown");
        fs::write(&unknown, b"retain").unwrap();
        let ledger = root.path().join("addresses.yaml");
        fs::write(&ledger, b"owner-authored").unwrap();
        for dir in repo.directories(&effects).into_iter().chain([
            repo.paths.config.clone(),
            repo.paths.config.join("lifecycle"),
        ]) {
            fs::write(
                dir.join(format!(".{}.stage-synthetic", a.id.as_str())),
                b"synthetic interrupted ciphertext",
            )
            .unwrap();
        }
        let preview = repo.forget_preview(&a.id, &effects).unwrap();
        assert_eq!(preview.resources.len(), 11);
        assert_eq!(preview.receipt.len(), 64);
        assert!(repo.forget(&a.id, &effects, &preview, false).is_err());
        assert!(repo.forget(&other.id, &effects, &preview, true).is_err());
        let lock = root
            .path()
            .join("data")
            .join(format!("{}.lock", a.id.as_str()));
        let ino = fs::metadata(&lock).unwrap().ino();
        fail(Some(phase));
        assert!(repo.forget(&a.id, &effects, &preview, true).is_err());
        fail(None);
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects).unwrap();
        repo.recover(&a.id, &effects).unwrap();
        for resource in preview.resources {
            assert!(!resource.path.exists());
        }
        for reference in preview.credential_references {
            assert!(matches!(
                repo.backend.get(&reference),
                Err(AcquisitionError::CredentialMissing)
            ));
        }
        assert_eq!(fs::metadata(&lock).unwrap().ino(), ino);
        assert_eq!(fs::read(unknown).unwrap(), b"retain");
        assert_eq!(fs::read(ledger).unwrap(), b"owner-authored");
        repo.store(&repo.load(&other.id).unwrap())
            .unwrap()
            .load_state()
            .unwrap();
        assert!(repo.store(&a).is_err());
        assert!(
            repo.forget(
                &a.id,
                &effects,
                &repo.forget_preview(&other.id, &effects).unwrap(),
                true
            )
            .is_err()
        );
    }
}
#[test]
fn stale_receipts_missing_keys_and_authentication_errors_never_reset_data() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    let mut preview = repo.forget_preview(&a.id, &effects).unwrap();
    preview.consequences.en.push_str(" altered");
    assert!(repo.forget(&a.id, &effects, &preview, true).is_err());
    let preview = repo.forget_preview(&a.id, &effects).unwrap();
    {
        let s = repo.store(&a).unwrap();
        let c = s.load().unwrap();
        s.save(&c).unwrap();
    }
    assert!(repo.forget(&a.id, &effects, &preview, true).is_err());
    let path = repo.store(&a).unwrap().path();
    let original = fs::read(&path).unwrap();
    repo.backend.delete(&a.local_secret_ref).unwrap();
    assert!(repo.rotate(&a.id, &effects).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(repo.recovery_status().unwrap().is_empty());
}
#[test]
fn lifecycle_process_worker() {
    let Some(root) = std::env::var_os("MAILROOM_LIFECYCLE_PROCESS_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let repo = persistent_accounts(&root);
    let effects = root.join("mailboxes");
    match std::env::var("MAILROOM_LIFECYCLE_PROCESS_ACTION")
        .unwrap()
        .as_str()
    {
        "create" => {
            let a = account(&repo);
            populate(&repo, &a, &effects);
            fs::write(root.join("id"), a.id.as_str()).unwrap();
        }
        action => {
            let id = AccountId::new(&fs::read_to_string(root.join("id")).unwrap()).unwrap();
            match action {
                "hold" => {
                    let a = repo.load(&id).unwrap();
                    let _s = repo.store(&a).unwrap();
                    fs::write(root.join("locked"), b"ready").unwrap();
                    while !root.join("release").exists() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                "excluded" => {
                    assert!(repo.rotate(&id, &effects).is_err());
                    assert!(
                        repo.replace_password(
                            &id,
                            &SecretValue::new(b"synthetic-replacement".to_vec())
                        )
                        .is_err()
                    );
                    assert!(repo.forget_preview(&id, &effects).is_err());
                    assert!(
                        EncryptedStore::lock_directory(&repo.paths.data, &id).is_err(),
                        "The actively held store lock must retain its inode even after ciphertext deletion"
                    );
                }
                "rotate" => {
                    repo.rotate(&id, &effects).unwrap();
                }
                "recover" => {
                    repo.recover(&id, &effects).unwrap();
                    let a = repo.load(&id).unwrap();
                    assert_eq!(snapshots(&repo, &a, &effects).len(), 4);
                }
                _ => panic!("Unknown synthetic action"),
            }
        }
    }
}
#[test]
fn interrupted_fake_credential_staging_is_attributable_and_recovered() {
    let root = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "acquisition::accounts::lifecycle::tests::lifecycle_process_worker",
        ])
        .env("MAILROOM_LIFECYCLE_PROCESS_DIR", root.path())
        .env("MAILROOM_LIFECYCLE_PROCESS_ACTION", "create")
        .env("MAILROOM_DURABLE_BROWSER_FIXTURE", root.path())
        .env("MAILROOM_LIFECYCLE_BOUNDARY", "credential_staged")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..1000 {
        if root.path().join("reached-credential_staged").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let reached = root.path().join("reached-credential_staged").exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(reached);
    let credentials = root.path().join("fake-credentials");
    assert_eq!(fs::read_dir(&credentials).unwrap().count(), 1);
    let unknown = credentials.join(".unrelated.stage-synthetic");
    fs::write(&unknown, b"retain unrelated synthetic staging").unwrap();
    let repo = persistent_accounts(root.path());
    let status = repo.recovery_status().unwrap();
    assert_eq!(status.len(), 1);
    repo.recover(&status[0].account, &root.path().join("mailboxes"))
        .unwrap();
    assert_eq!(fs::read_dir(&credentials).unwrap().count(), 1);
    assert!(unknown.exists());
    assert!(repo.list().unwrap().is_empty());
}

#[test]
fn cross_process_exclusion_uses_the_same_lock_after_reconstruction() {
    let root = tempfile::tempdir().unwrap();
    let spawn = |action: &str| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "acquisition::accounts::lifecycle::tests::lifecycle_process_worker",
            ])
            .env("MAILROOM_LIFECYCLE_PROCESS_DIR", root.path())
            .env("MAILROOM_LIFECYCLE_PROCESS_ACTION", action)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    assert!(spawn("create").wait().unwrap().success());
    let mut holder = spawn("hold");
    for _ in 0..1000 {
        if root.path().join("locked").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(root.path().join("locked").exists());
    assert!(spawn("excluded").wait().unwrap().success());
    fs::write(root.path().join("release"), b"done").unwrap();
    assert!(holder.wait().unwrap().success());
    assert!(spawn("rotate").wait().unwrap().success());
    assert!(spawn("recover").wait().unwrap().success());
}

#[test]
fn lifecycle_snapshot_worker() {
    use sha2::{Digest, Sha256};
    let Some(root) = std::env::var_os("MAILROOM_LIFECYCLE_SNAPSHOT_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let repo = persistent_accounts(&root);
    let id = AccountId::new(&std::env::var("MAILROOM_LIFECYCLE_ACCOUNT").unwrap()).unwrap();
    let _lease = repo.lock_account(&id).unwrap();
    let account = repo.load_metadata(&id).unwrap();
    let mut hashes = std::collections::BTreeMap::new();
    let mut histories = std::collections::BTreeMap::new();
    let mut identity = None;
    for (kind, dir) in repo.directories(&root.join("mailboxes")).iter().enumerate() {
        if !cipher_path(dir, &id, account.generation).exists() {
            continue;
        }
        let store = repo.open_store(&account, dir, kind).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.revision = 0;
        hashes.insert(
            kind,
            data_encoding::HEXLOWER.encode(&Sha256::digest(serde_json::to_vec(&saved).unwrap())),
        );
        histories.insert(kind,serde_json::json!({"exports":saved.exports.len(),"backlog":saved.backlog.len(),"invoice_sends":saved.invoices.sends.len(),"unsubscribe":saved.unsubscribe.history.len(),"effects":saved.synthetic_effects.len(),"invoice_effects":saved.invoices.synthetic_received.len(),"unsubscribe_effects":saved.unsubscribe.synthetic_acknowledgments.len(),"winddown_decisions":saved.winddown.history.len()}));
        if kind == 0 {
            identity = Some(
                serde_json::json!({"placeholder":store.keys().placeholder("sender","news@shop.example"),"subscription":store.keys().identity_binding_for(b"mailroom/subscription/v1",b"fixture"),"winddown_row":store.keys().identity_binding_for(b"winddown-row/v1",b"fixture")}),
            );
        }
    }
    assert!(hashes.contains_key(&0));
    fs::write(
        root.join("lifecycle-observation.json"),
        serde_json::to_vec(
            &serde_json::json!({"hashes":hashes,"histories":histories,"identity":identity}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn unreadable_dependent_store_keeps_the_old_key_rolls_back_and_rotates_once_bytes_return() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    populate(&repo, &a, &effects);
    let baseline = snapshots(&repo, &a, &effects);
    let path = cipher_path(&effects.join("invoice-transport"), &a.id, 0);
    let original = fs::read(&path).unwrap();
    let mut damaged = original.clone();
    // Inside the base snapshot. Damage that reaches the end of the file is
    // read as an uncommitted frame tail, and census saves now append frames.
    damaged[100] ^= 1;
    fs::write(&path, &damaged).unwrap();
    assert!(repo.rotate(&a.id, &effects).is_err());
    assert!(repo.backend.get(&a.local_secret_ref).is_ok());
    assert_eq!(repo.load_metadata(&a.id).unwrap().generation, 0);
    assert_eq!(fs::read(&path).unwrap(), damaged);
    repo.recover(&a.id, &effects).unwrap();
    assert!(
        !repo.intent_path(&a.id).exists(),
        "the unpublished rotation is undone"
    );
    assert!(repo.backend.get(&a.local_secret_ref).is_ok());
    assert_eq!(repo.load_metadata(&a.id).unwrap().generation, 0);
    assert_eq!(
        fs::read(&path).unwrap(),
        damaged,
        "rolling back reads and writes no earlier data"
    );
    fs::write(&path, &original).unwrap();
    drop(repo);
    let repo = persistent_accounts(root.path());
    repo.rotate(&a.id, &effects).unwrap();
    let current = repo.load(&a.id).unwrap();
    let after = snapshots(&repo, &current, &effects);
    for (i, (before, after)) in baseline.iter().zip(after).enumerate() {
        let mut expected: serde_json::Value = serde_json::from_slice(before).unwrap();
        if i == 0 {
            expected["revision"] = serde_json::json!(expected["revision"].as_u64().unwrap() + 1);
        }
        assert_eq!(
            expected,
            serde_json::from_slice::<serde_json::Value>(&after).unwrap()
        );
    }
}

#[test]
fn legacy_metadata_and_failed_connection_state_remain_explicit() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(repo.metadata_path(&a.id)).unwrap()).unwrap();
    for key in ["generation", "identity_ref", "connection"] {
        metadata.as_object_mut().unwrap().remove(key);
    }
    fs::write(
        repo.metadata_path(&a.id),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let legacy = repo.load(&a.id).unwrap();
    assert_eq!(legacy.generation, 0);
    assert!(matches!(legacy.connection, ConnectionState::Unverified));
    repo.store(&legacy).unwrap().load_state().unwrap();
    repo.connection_observed(&a.id, false).unwrap();
    assert!(matches!(
        repo.load(&a.id).unwrap().connection,
        ConnectionState::Failed
    ));
}

#[test]
fn credential_errors_and_failed_retirement_retain_retry_information_without_false_success() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Faults {
        inner: crate::acquisition::accounts::process_tests::FileFake,
        get: AtomicBool,
        put: AtomicBool,
        delete: AtomicBool,
    }
    impl CredentialBackend for Faults {
        fn get(&self, r: &str) -> Result<SecretValue, AcquisitionError> {
            if self.get.load(Ordering::SeqCst) {
                Err(AcquisitionError::Credentials)
            } else {
                self.inner.get(r)
            }
        }
        fn put(&self, r: &str, v: &SecretValue) -> Result<(), AcquisitionError> {
            if self.put.load(Ordering::SeqCst) {
                Err(AcquisitionError::Credentials)
            } else {
                self.inner.put(r, v)
            }
        }
        fn delete(&self, r: &str) -> Result<(), AcquisitionError> {
            if self.delete.load(Ordering::SeqCst) {
                Err(AcquisitionError::Credentials)
            } else {
                self.inner.delete(r)
            }
        }
        fn delete_owned(
            &self,
            r: &str,
            owned: &dyn Fn(&[u8]) -> bool,
        ) -> Result<bool, AcquisitionError> {
            if self.delete.load(Ordering::SeqCst) {
                Err(AcquisitionError::Credentials)
            } else {
                self.inner.delete_owned(r, owned)
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(Faults {
        inner: crate::acquisition::accounts::process_tests::FileFake(
            root.path().join("fake-credentials"),
        ),
        get: AtomicBool::new(false),
        put: AtomicBool::new(false),
        delete: AtomicBool::new(false),
    });
    let repo = Accounts::new(
        AccountPaths {
            config: root.path().join("config"),
            data: root.path().join("data"),
        },
        backend.clone(),
    );
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    let original = fs::read(repo.store(&a).unwrap().path()).unwrap();
    backend.get.store(true, Ordering::SeqCst);
    assert!(matches!(repo.store(&a), Err(AcquisitionError::Credentials)));
    backend.get.store(false, Ordering::SeqCst);
    backend.put.store(true, Ordering::SeqCst);
    assert!(
        repo.replace_password(&a.id, &SecretValue::new(b"synthetic-new".to_vec()))
            .is_err()
    );
    backend.put.store(false, Ordering::SeqCst);
    repo.recover(&a.id, &effects).unwrap();
    assert_eq!(repo.load(&a.id).unwrap().password_ref, a.password_ref);
    backend.delete.store(true, Ordering::SeqCst);
    assert!(
        repo.replace_password(&a.id, &SecretValue::new(b"synthetic-new".to_vec()))
            .is_err()
    );
    assert!(repo.recover(&a.id, &effects).is_err());
    assert_eq!(repo.recovery_status().unwrap().len(), 1);
    assert!(repo.store(&a).is_err());
    backend.delete.store(false, Ordering::SeqCst);
    repo.recover(&a.id, &effects).unwrap();
    let current = repo.load(&a.id).unwrap();
    assert_ne!(current.password_ref, a.password_ref);
    assert_eq!(
        fs::read(repo.store(&current).unwrap().path()).unwrap(),
        original
    );
    let preview = repo.forget_preview(&a.id, &effects).unwrap();
    backend.delete.store(true, Ordering::SeqCst);
    assert!(repo.forget(&a.id, &effects, &preview, true).is_err());
    assert!(repo.recover(&a.id, &effects).is_err());
    assert!(repo.metadata_path(&a.id).exists());
    backend.delete.store(false, Ordering::SeqCst);
    repo.recover(&a.id, &effects).unwrap();
    assert!(!repo.metadata_path(&a.id).exists());
}

#[cfg(unix)]
#[test]
fn released_locks_do_not_remain_owned_by_an_unrelated_child_before_exec() {
    use std::{
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
    };

    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    populate(&repo, &a, &effects);
    let before = snapshots(&repo, &a, &effects);
    for kind in ["lifecycle", "store", "recovery_store"] {
        let lease: Box<dyn std::any::Any> = match kind {
            "lifecycle" => Box::new(repo.lock_account(&a.id).unwrap()),
            "store" => Box::new(repo.store(&a).unwrap()),
            _ => Box::new(EncryptedStore::lock_directory(&repo.paths.data, &a.id).unwrap()),
        };
        let (mut parent, child) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let child_fd = child.as_raw_fd();
        let spawning = std::thread::spawn(move || {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "acquisition::accounts::lifecycle::tests::lifecycle_process_worker",
                ])
                .env_remove("MAILROOM_LIFECYCLE_PROCESS_DIR")
                .stdout(std::process::Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    let mut byte = 1u8;
                    if libc::write(child_fd, (&byte as *const u8).cast(), 1) != 1
                        || libc::read(child_fd, (&mut byte as *mut u8).cast(), 1) != 1
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let result = command.status();
            drop(child);
            result.unwrap()
        });
        let mut ready = [0];
        parent.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [1]);
        assert!(
            repo.rotate(&a.id, &effects).is_err(),
            "{kind} must exclude a real competing operation"
        );
        drop(lease);
        let recovery = repo.recover(&a.id, &effects);
        let reopened = repo.lock_stores(&a, &effects);
        parent.write_all(&[1]).unwrap();
        assert!(spawning.join().unwrap().success());
        recovery
            .unwrap_or_else(|e| panic!("{kind} remains locked after its owner released it: {e:?}"));
        let after: Vec<Vec<u8>> = reopened
            .unwrap()
            .iter()
            .map(|s| serde_json::to_vec(&s.load_state().unwrap()).unwrap())
            .collect();
        assert_eq!(after, before, "{kind}");
    }
}

/// Removes identity commitments from an account's metadata and pending intent, as a build
/// from before commitments would have written them.
fn strip_commitments(repo: &Accounts, id: &AccountId, roles: &[&str]) {
    let strip = |path: PathBuf, pointers: &[String]| {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for pointer in pointers {
            if let Some(object) = value.pointer_mut(pointer).and_then(|v| v.as_object_mut()) {
                object.remove("identity_commitment");
            }
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    };
    strip(
        repo.intent_path(id),
        &roles
            .iter()
            .map(|r| format!("/intent/{r}"))
            .collect::<Vec<_>>(),
    );
    strip(repo.metadata_path(id), &[String::new()]);
}

/// Every persisted rotation phase with its recovery rule from the receipt's state table.
const ROTATION_PHASES: [(&str, bool); 14] = [
    ("intent", false),
    ("rotate_identity", false),
    ("rotate_key", false),
    ("rotate_primary", false),
    ("rotate_mailbox", false),
    ("rotate_invoice", false),
    ("rotate_unsubscribe", false),
    ("metadata", true),
    ("rotate_verified", true),
    ("rotate_retire_data", true),
    ("rotate_retire_data#2", true),
    ("rotate_retire_data#3", true),
    ("rotate_retire_data#4", true),
    ("rotate_retire_key", true),
];

fn plant(repo: &Accounts, reference: &str) {
    repo.backend.delete(reference).unwrap();
    repo.backend
        .put(reference, &SecretValue::new(vec![0x5a; 32]))
        .unwrap();
}

#[test]
fn rotation_recovery_follows_the_state_table_and_never_uses_an_unauthenticated_next_key() {
    for (phase, published) in ROTATION_PHASES {
        for variant in [
            "genuine",
            "pre_commitment",
            "metadata_only",
            "planted",
            "pre_commitment_planted",
        ] {
            let case = format!("{phase} {variant}");
            let root = tempfile::tempdir().unwrap();
            let repo = persistent_accounts(root.path());
            let a = account(&repo);
            let effects = root.path().join("mailboxes");
            populate(&repo, &a, &effects);
            let placeholder = repo
                .store(&a)
                .unwrap()
                .keys()
                .placeholder("sender", "a@example.test");
            let earlier_path = cipher_path(&repo.paths.data, &a.id, 0);
            let later_path = cipher_path(&repo.paths.data, &a.id, 1);
            let earlier = fs::read(&earlier_path).unwrap();
            let next_key = format!("{}-key-g1", a.id.as_str());
            let identity = format!("{}-identity", a.id.as_str());
            fail(Some(phase));
            assert!(repo.rotate(&a.id, &effects).is_err(), "{case}");
            fail(None);
            match variant {
                "pre_commitment" | "pre_commitment_planted" => {
                    strip_commitments(&repo, &a.id, &["after"])
                }
                "metadata_only" => strip_commitments(&repo, &a.id, &[]),
                _ => {}
            }
            if variant.ends_with("planted") {
                plant(&repo, &next_key);
            }
            let later = fs::read(&later_path).ok();
            let later_files: Vec<Option<Vec<u8>>> = repo
                .directories(&effects)
                .iter()
                .map(|dir| fs::read(cipher_path(dir, &a.id, 1)).ok())
                .collect();
            let earlier_now = fs::read(&earlier_path).ok();
            drop(repo);
            let repo = persistent_accounts(root.path());
            let repairs = matches!(variant, "pre_commitment" | "metadata_only");
            let forward = published
                && (variant == "genuine"
                    || (repairs && matches!(phase, "metadata" | "rotate_verified")));
            if published && repairs && forward {
                fail(Some("metadata"));
                assert!(
                    repo.recover(&a.id, &effects).is_err(),
                    "{case}: repair interrupted"
                );
                fail(None);
            }
            let outcome = repo.recover(&a.id, &effects);
            if !published {
                outcome.unwrap_or_else(|e| panic!("{case}: {e}"));
                let current = repo.load(&a.id).unwrap();
                assert_eq!(current.generation, 0, "{case}: rolled back");
                assert!(!repo.intent_path(&a.id).exists(), "{case}");
                assert_eq!(
                    fs::read(&earlier_path).unwrap(),
                    earlier,
                    "{case}: earlier data untouched"
                );
                // Rollback never unlinks a file: every next-generation entry is moved,
                // byte for byte, into quarantine beside it.
                for (dir, before_rollback) in repo.directories(&effects).iter().zip(&later_files) {
                    assert!(!cipher_path(dir, &a.id, 1).exists(), "{case}: name freed");
                    if let Some(bytes) = before_rollback {
                        assert!(
                            quarantined(dir, &a.id).contains(bytes),
                            "{case}: next-generation data kept in quarantine"
                        );
                    }
                }
                assert_eq!(
                    repo.backend.get(&identity).err(),
                    Some(AcquisitionError::CredentialMissing),
                    "{case}: the identity the rotation derived is deleted"
                );
                // Only a key that reproduces the recorded commitment is provably the
                // rotation's own; a planted key, or any key of a pre-commitment intent, stays.
                let next = repo.backend.get(&next_key);
                match variant {
                    "genuine" | "metadata_only" => assert_eq!(
                        next.err(),
                        Some(AcquisitionError::CredentialMissing),
                        "{case}: the rotation's own next key is deleted"
                    ),
                    "planted" | "pre_commitment_planted" => {
                        assert_eq!(
                            next.unwrap().bytes(),
                            &[0x5a; 32][..],
                            "{case}: a planted key is kept"
                        )
                    }
                    _ => assert_eq!(
                        next.is_ok(),
                        !matches!(phase, "intent" | "rotate_identity"),
                        "{case}: an unprovable key is kept"
                    ),
                }
                let s = repo.store(&current).unwrap();
                assert_eq!(
                    s.keys().placeholder("sender", "a@example.test"),
                    placeholder,
                    "{case}"
                );
            } else if forward {
                outcome.unwrap_or_else(|e| panic!("{case}: {e}"));
                repo.recover(&a.id, &effects).unwrap();
                let current = repo.load(&a.id).unwrap();
                assert_eq!(current.generation, 1, "{case}");
                assert!(!current.uncommitted(), "{case}: committed");
                let s = repo.store(&current).unwrap();
                assert_eq!(
                    s.keys().placeholder("sender", "a@example.test"),
                    placeholder,
                    "{case}"
                );
            } else {
                assert!(outcome.is_err(), "{case}: refused");
                assert!(
                    repo.intent_path(&a.id).exists(),
                    "{case}: the intent is kept"
                );
                assert_eq!(
                    fs::read(&later_path).ok(),
                    later,
                    "{case}: next-generation data unchanged"
                );
                assert_eq!(
                    fs::read(&earlier_path).ok(),
                    earlier_now,
                    "{case}: earlier data unchanged"
                );
                assert!(
                    repo.backend.get(&identity).is_ok(),
                    "{case}: the identity is kept"
                );
                if variant.ends_with("planted") {
                    let planted = repo.backend.get(&next_key).unwrap();
                    assert_eq!(planted.bytes(), &[0x5a; 32][..], "{case}");
                }
            }
        }
    }

    // A second rotation: the earlier account already has an identity, which rollback keeps.
    for (phase, published) in [
        ("rotate_key", false),
        ("rotate_primary", false),
        ("metadata", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        repo.rotate(&a.id, &effects).unwrap();
        let identity = format!("{}-identity", a.id.as_str());
        fail(Some(phase));
        assert!(repo.rotate(&a.id, &effects).is_err());
        fail(None);
        strip_commitments(&repo, &a.id, &["before", "after"]);
        plant(&repo, &format!("{}-key-g2", a.id.as_str()));
        let outcome = repo.recover(&a.id, &effects);
        assert_eq!(outcome.is_ok(), !published, "{phase}");
        assert!(
            repo.backend.get(&identity).is_ok(),
            "{phase}: the earlier identity is kept"
        );
        if published {
            assert!(repo.intent_path(&a.id).exists(), "{phase}");
        } else {
            assert_eq!(repo.load(&a.id).unwrap().generation, 1, "{phase}");
        }
    }
}

/// Answers the first read of one reference genuinely and every later read with a plant, as
/// a process replacing the item between two reads would.
struct Swapping {
    inner: Arc<dyn CredentialBackend>,
    reference: String,
    plant: Vec<u8>,
    reads: std::sync::atomic::AtomicUsize,
}
impl CredentialBackend for Swapping {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        if reference == self.reference
            && self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1
        {
            return Ok(SecretValue::new(self.plant.clone()));
        }
        self.inner.get(reference)
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        self.inner.put(reference, value)
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        self.inner.delete(reference)
    }
    fn delete_owned(
        &self,
        reference: &str,
        owned: &dyn Fn(&[u8]) -> bool,
    ) -> Result<bool, AcquisitionError> {
        self.inner.delete_owned(reference, owned)
    }
}

#[test]
fn repair_commits_only_the_identity_it_authenticated_even_if_the_item_changes_between_reads() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    populate(&repo, &a, &effects);
    let placeholder = repo
        .store(&a)
        .unwrap()
        .keys()
        .placeholder("sender", "a@example.test");
    fail(Some("metadata"));
    assert!(repo.rotate(&a.id, &effects).is_err());
    fail(None);
    strip_commitments(&repo, &a.id, &["after"]);
    let files = |repo: &Accounts| {
        [0, 1].map(|g| fs::read(cipher_path(&repo.paths.data, &a.id, g)).unwrap())
    };
    let before_files = files(&repo);
    let identity = format!("{}-identity", a.id.as_str());
    let plant = vec![0x6b; 128];
    let swapping = Accounts::new(
        repo.paths.clone(),
        Arc::new(Swapping {
            inner: repo.backend.clone(),
            reference: identity.clone(),
            plant: plant.clone(),
            reads: Default::default(),
        }),
    );
    assert!(
        swapping.recover(&a.id, &effects).is_err(),
        "the planted identity is refused"
    );
    assert!(repo.intent_path(&a.id).exists(), "the intent is kept");
    assert_eq!(
        files(&repo),
        before_files,
        "both generations are kept unchanged"
    );
    let saved = repo.load_metadata(&a.id).unwrap();
    let next = repo.backend.get(&saved.local_secret_ref).unwrap();
    let with_plant = CensusKeys::rotated(
        &LocalSecret::from_bytes(next.bytes().to_vec()).unwrap(),
        &a.id,
        &plant,
        1,
    )
    .unwrap();
    assert!(
        !with_plant.commits_to(saved.identity_commitment.as_deref()),
        "no commitment authenticating the plant was published"
    );
    repo.recover(&a.id, &effects).unwrap();
    let current = repo.load(&a.id).unwrap();
    assert_eq!(current.generation, 1);
    assert_eq!(
        repo.store(&current)
            .unwrap()
            .keys()
            .placeholder("sender", "a@example.test"),
        placeholder,
        "the genuine identity carries on"
    );
}

#[test]
fn rotation_never_deletes_an_entry_it_did_not_create() {
    // An entry that exists before a rotation, at each place the rotation would create one,
    // refuses the rotation before anything is recorded, and survives recovery.
    for place in [
        "identity",
        "next_key",
        "data",
        "mailbox",
        "invoice",
        "unsubscribe",
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        let foreign = SecretValue::new(vec![0x33; 32]);
        let dirs = repo.directories(&effects);
        let file = |kind: usize| cipher_path(&dirs[kind], &a.id, 1);
        let check: Box<dyn Fn(&Accounts) -> bool> = match place {
            "identity" | "next_key" => {
                let reference = if place == "identity" {
                    format!("{}-identity", a.id.as_str())
                } else {
                    format!("{}-key-g1", a.id.as_str())
                };
                repo.backend.put(&reference, &foreign).unwrap();
                Box::new(move |r: &Accounts| {
                    r.backend
                        .get(&reference)
                        .is_ok_and(|v| v.bytes() == &[0x33; 32][..])
                })
            }
            _ => {
                let kind = ["data", "mailbox", "invoice", "unsubscribe"]
                    .iter()
                    .position(|p| *p == place)
                    .unwrap();
                fs::create_dir_all(&dirs[kind]).unwrap();
                fs::write(file(kind), b"pre-existing").unwrap();
                let path = file(kind);
                Box::new(move |_: &Accounts| fs::read(&path).is_ok_and(|b| b == b"pre-existing"))
            }
        };
        assert!(repo.rotate(&a.id, &effects).is_err(), "{place}: refused");
        assert!(
            !repo.intent_path(&a.id).exists(),
            "{place}: nothing was recorded"
        );
        assert!(check(&repo), "{place}: kept");
        repo.recover(&a.id, &effects).unwrap();
        assert!(check(&repo), "{place}: kept after recovery");
        assert_eq!(repo.load(&a.id).unwrap().generation, 0, "{place}");
    }

    // Entries that appear after the intent, in place of the rotation's own, are not
    // provably its own either, so the rollback keeps them.
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    populate(&repo, &a, &effects);
    fail(Some("intent"));
    assert!(repo.rotate(&a.id, &effects).is_err());
    fail(None);
    let identity = format!("{}-identity", a.id.as_str());
    let next_key = format!("{}-key-g1", a.id.as_str());
    repo.backend
        .put(&identity, &SecretValue::new(vec![0x44; 128]))
        .unwrap();
    repo.backend
        .put(&next_key, &SecretValue::new(vec![0x55; 32]))
        .unwrap();
    let foreign_file = cipher_path(&repo.paths.data, &a.id, 1);
    fs::write(&foreign_file, b"foreign").unwrap();
    repo.recover(&a.id, &effects).unwrap();
    assert!(!repo.intent_path(&a.id).exists());
    assert_eq!(
        repo.backend.get(&identity).unwrap().bytes(),
        &[0x44; 128][..]
    );
    assert_eq!(
        repo.backend.get(&next_key).unwrap().bytes(),
        &[0x55; 32][..]
    );
    assert!(!foreign_file.exists());
    assert!(
        quarantined(&repo.paths.data, &a.id).contains(&b"foreign".to_vec()),
        "the foreign file is moved intact, not deleted"
    );
}

#[test]
fn an_interrupted_rollback_resumes_and_removes_only_the_rotations_own_entries() {
    for phase in ["rollback_data", "rollback_keys"] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        let earlier = fs::read(cipher_path(&repo.paths.data, &a.id, 0)).unwrap();
        fail(Some("rotate_invoice"));
        assert!(repo.rotate(&a.id, &effects).is_err());
        fail(Some(phase));
        assert!(repo.recover(&a.id, &effects).is_err(), "{phase}");
        fail(None);
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects).unwrap();
        assert!(!repo.intent_path(&a.id).exists(), "{phase}");
        assert_eq!(
            fs::read(cipher_path(&repo.paths.data, &a.id, 0)).unwrap(),
            earlier,
            "{phase}"
        );
        for dir in repo.directories(&effects) {
            assert!(!cipher_path(&dir, &a.id, 1).exists(), "{phase}");
        }
        for reference in [
            format!("{}-key-g1", a.id.as_str()),
            format!("{}-identity", a.id.as_str()),
        ] {
            assert_eq!(
                repo.backend.get(&reference).err(),
                Some(AcquisitionError::CredentialMissing),
                "{phase}"
            );
        }
        assert!(repo.store(&repo.load(&a.id).unwrap()).is_ok(), "{phase}");
    }
}

/// The contents of every file rollback moved into quarantine in `dir` for account `id`.
fn quarantined(dir: &Path, id: &AccountId) -> Vec<Vec<u8>> {
    let mut found = vec![];
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.starts_with(&format!(".{}.rollback-", id.as_str())) {
            for file in fs::read_dir(&path).unwrap() {
                found.push(fs::read(file.unwrap().path()).unwrap());
            }
        }
    }
    found
}

#[test]
fn rollback_preserves_whatever_file_holds_a_next_generation_name_across_retries() {
    for interrupt in [None, Some("rollback_keys")] {
        let root = tempfile::tempdir().unwrap();
        let repo = persistent_accounts(root.path());
        let a = account(&repo);
        let effects = root.path().join("mailboxes");
        populate(&repo, &a, &effects);
        fail(Some("rotate_unsubscribe"));
        assert!(repo.rotate(&a.id, &effects).is_err());
        fail(None);
        // Another process swaps a different file in under the rotation's own name.
        let dirs = repo.directories(&effects);
        let genuine: Vec<Vec<u8>> = dirs[1..]
            .iter()
            .map(|dir| fs::read(cipher_path(dir, &a.id, 1)).unwrap())
            .collect();
        let swapped = cipher_path(&repo.paths.data, &a.id, 1);
        fs::rename(&swapped, root.path().join("moved-away")).unwrap();
        fs::write(&swapped, b"unrelated bytes moved into this name").unwrap();
        if let Some(phase) = interrupt {
            fail(Some(phase));
            assert!(repo.recover(&a.id, &effects).is_err());
            fail(None);
        }
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects).unwrap();
        assert!(
            quarantined(&repo.paths.data, &a.id)
                .contains(&b"unrelated bytes moved into this name".to_vec()),
            "{interrupt:?}: the swapped-in file survives"
        );
        assert!(fs::read(root.path().join("moved-away")).is_ok());
        for (dir, bytes) in dirs[1..].iter().zip(&genuine) {
            assert!(quarantined(dir, &a.id).contains(bytes), "{interrupt:?}");
        }
        assert_eq!(repo.load(&a.id).unwrap().generation, 0);
        let again = repo.rotate(&a.id, &effects);
        assert!(
            again.is_ok(),
            "{interrupt:?}: quarantine frees the names for a later rotation"
        );
    }
}

#[test]
fn the_file_backend_deletes_only_the_entry_it_checked() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let backend = repo.backend.clone();
    backend
        .put("x-key-g1", &SecretValue::new(vec![1; 32]))
        .unwrap();
    assert!(
        !backend.delete_owned("x-key-g1", &|v| v == [2; 32]).unwrap(),
        "unequal is kept"
    );
    assert_eq!(backend.get("x-key-g1").unwrap().bytes(), &[1; 32][..]);
    assert!(
        !backend.delete_owned("x-absent", &|_| true).unwrap(),
        "missing deletes nothing"
    );
    // The check itself replaces the named entry; the replacement must survive.
    let replace = |value: &[u8]| {
        backend.delete("x-key-g1").unwrap();
        backend
            .put("x-key-g1", &SecretValue::new(vec![9; 32]))
            .unwrap();
        value == [1; 32]
    };
    assert!(
        backend.delete_owned("x-key-g1", &replace).unwrap(),
        "the checked entry is deleted"
    );
    assert_eq!(
        backend.get("x-key-g1").unwrap().bytes(),
        &[9; 32][..],
        "the replacement survives"
    );
}

#[test]
fn the_file_backend_never_unlinks_and_never_replaces_a_name_taken_meanwhile() {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let backend = repo.backend.clone();
    backend
        .put("y-key-g1", &SecretValue::new(vec![1; 32]))
        .unwrap();
    // Not owned, and the name is taken again during the check: the new entry keeps the
    // name, and the checked entry stays discarded rather than replacing it.
    let take = |_: &[u8]| {
        backend
            .put("y-key-g1", &SecretValue::new(vec![7; 32]))
            .unwrap();
        false
    };
    assert!(!backend.delete_owned("y-key-g1", &take).unwrap());
    assert_eq!(backend.get("y-key-g1").unwrap().bytes(), &[7; 32][..]);
    let discarded: Vec<Vec<u8>> = fs::read_dir(root.path().join("fake-credentials"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".y-key-g1.discarded-")
        })
        .map(|p| fs::read(p.join("y-key-g1")).unwrap())
        .collect();
    assert_eq!(
        discarded,
        vec![vec![1; 32]],
        "the checked entry is kept, not destroyed"
    );
}

/// Interrupts a first rotation of a populated account after its next-generation files exist.
fn pending_rotation() -> (tempfile::TempDir, Accounts, Account, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(root.path());
    let a = account(&repo);
    let effects = root.path().join("mailboxes");
    populate(&repo, &a, &effects);
    fail(Some("rotate_unsubscribe"));
    assert!(repo.rotate(&a.id, &effects).is_err());
    fail(None);
    (root, repo, a, effects)
}

fn set_after_create(hook: Option<Box<dyn FnMut(&str)>>) {
    crate::acquisition::accounts::exclusive::AFTER_CREATE.with(|h| *h.borrow_mut() = hook);
}

#[test]
fn rollback_refuses_rather_than_overwrite_a_planted_destination_or_follow_a_substituted_folder() {
    for plant in ["file", "symlink"] {
        let (root, repo, a, effects) = pending_rotation();
        let data = repo.paths.data.clone();
        let canonical = cipher_path(&data, &a.id, 1);
        let genuine = fs::read(&canonical).unwrap();
        let entry = canonical.file_name().unwrap().to_str().unwrap().to_owned();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep"), b"unrelated file B").unwrap();
        let planted = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let seen = planted.clone();
        let (store, file_name, target) = (data.clone(), entry.clone(), elsewhere.clone());
        set_after_create(Some(Box::new(move |folder: &str| {
            let folder = store.join(folder);
            if seen.borrow().is_some() || !folder.starts_with(&store) {
                return;
            }
            if plant == "file" {
                fs::write(folder.join(&file_name), b"unrelated file B").unwrap();
            } else {
                fs::remove_dir(&folder).unwrap();
                std::os::unix::fs::symlink(&target, &folder).unwrap();
            }
            *seen.borrow_mut() = Some(folder);
        })));
        assert!(repo.recover(&a.id, &effects).is_err(), "{plant}: refused");
        set_after_create(None);
        let folder = planted.borrow().clone().unwrap();
        assert_eq!(
            fs::read(&canonical).unwrap(),
            genuine,
            "{plant}: the source stays"
        );
        if plant == "file" {
            assert_eq!(
                fs::read(folder.join(&entry)).unwrap(),
                b"unrelated file B",
                "B survives"
            );
        } else {
            let names: Vec<_> = fs::read_dir(&elsewhere)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(
                names,
                vec![std::ffi::OsString::from("keep")],
                "nothing moved through the link"
            );
        }
        assert!(
            repo.intent_path(&a.id).exists(),
            "{plant}: the rollback is still pending"
        );
        drop(repo);
        let repo = persistent_accounts(root.path());
        repo.recover(&a.id, &effects).unwrap();
        assert!(
            quarantined(&data, &a.id).contains(&genuine),
            "{plant}: a retry moves it"
        );
        assert_eq!(
            fs::read(elsewhere.join("keep")).unwrap(),
            b"unrelated file B"
        );
        if plant == "file" {
            assert_eq!(fs::read(folder.join(&entry)).unwrap(), b"unrelated file B");
        }
    }
}

#[test]
fn a_fresh_folder_must_not_already_exist() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("taken")).unwrap();
    fs::write(root.path().join("taken/keep"), b"keep").unwrap();
    let dir = crate::acquisition::accounts::exclusive::Dir::open(root.path()).unwrap();
    assert!(dir.create_fresh("taken").is_err());
    assert_eq!(fs::read(root.path().join("taken/keep")).unwrap(), b"keep");
}

#[test]
fn rollback_syncs_both_directories_of_every_move_before_it_is_done() {
    use std::os::unix::fs::MetadataExt;
    let (_root, repo, a, effects) = pending_rotation();
    crate::acquisition::accounts::exclusive::SYNCED.with(|s| s.borrow_mut().clear());
    repo.recover(&a.id, &effects).unwrap();
    let synced = crate::acquisition::accounts::exclusive::SYNCED.with(|s| s.borrow().clone());
    let folders = repo.rollback_directories(&a.id, &effects).unwrap();
    assert_eq!(folders.len(), 4, "one move per recorded store");
    for folder in folders {
        for dir in [folder.clone(), folder.parent().unwrap().to_path_buf()] {
            let m = fs::metadata(&dir).unwrap();
            assert!(
                synced.contains(&(m.dev() as u64, m.ino())),
                "{} synced",
                dir.display()
            );
        }
    }
}

#[test]
fn rollback_copies_are_bounded_listed_by_forget_and_removed_by_forget_rotation_or_discard() {
    let (_root, repo, a, effects) = pending_rotation();
    repo.recover(&a.id, &effects).unwrap();
    assert_eq!(repo.rollback_directories(&a.id, &effects).unwrap().len(), 4);

    // An explicit discard removes them.
    assert_eq!(repo.discard_rollback_copies(&a.id, &effects).unwrap(), 4);
    assert!(
        repo.rollback_directories(&a.id, &effects)
            .unwrap()
            .is_empty()
    );

    // At the bound a rotation refuses before recording anything.
    for i in 0..MAX_ROLLBACK_DIRECTORIES {
        fs::create_dir(
            repo.paths
                .data
                .join(format!(".{}.rollback-x{i}", a.id.as_str())),
        )
        .unwrap();
    }
    assert!(repo.rotate(&a.id, &effects).is_err());
    assert!(!repo.intent_path(&a.id).exists());
    repo.discard_rollback_copies(&a.id, &effects).unwrap();

    // A successful rotation removes those left by an earlier interrupted one.
    fail(Some("rotate_primary"));
    assert!(repo.rotate(&a.id, &effects).is_err());
    fail(None);
    repo.recover(&a.id, &effects).unwrap();
    assert!(
        !repo
            .rollback_directories(&a.id, &effects)
            .unwrap()
            .is_empty()
    );
    repo.rotate(&a.id, &effects).unwrap();
    assert!(
        repo.rollback_directories(&a.id, &effects)
            .unwrap()
            .is_empty()
    );

    // Forget lists them in its preview, refuses a changed folder, and removes them.
    fail(Some("rotate_primary"));
    assert!(repo.rotate(&a.id, &effects).is_err());
    fail(None);
    repo.recover(&a.id, &effects).unwrap();
    let folders = repo.rollback_directories(&a.id, &effects).unwrap();
    assert!(!folders.is_empty());
    let preview = repo.forget_preview(&a.id, &effects).unwrap();
    for folder in &folders {
        assert!(
            preview.resources.iter().any(|r| &r.path == folder),
            "{} listed",
            folder.display()
        );
    }
    fs::write(folders[0].join("added-later"), b"x").unwrap();
    assert!(
        repo.forget(&a.id, &effects, &preview, true).is_err(),
        "a changed folder refuses"
    );
    fs::remove_file(folders[0].join("added-later")).unwrap();
    let preview = repo.forget_preview(&a.id, &effects).unwrap();
    repo.forget(&a.id, &effects, &preview, true).unwrap();
    for folder in folders {
        assert!(
            fs::symlink_metadata(&folder).is_err(),
            "{} removed",
            folder.display()
        );
    }
}
