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
fn unreadable_dependent_store_keeps_the_old_key_and_recovers_only_after_original_bytes_return() {
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
    assert!(repo.recover(&a.id, &effects).is_err());
    assert!(repo.backend.get(&a.local_secret_ref).is_ok());
    fs::write(&path, &original).unwrap();
    drop(repo);
    let repo = persistent_accounts(root.path());
    repo.recover(&a.id, &effects).unwrap();
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
