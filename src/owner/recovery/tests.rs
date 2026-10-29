use super::*;
use crate::acquisition::{Provider, persistent_test_accounts as persistent_accounts};
use crate::confirmation::Backend;
use sha2::Digest;
use std::{collections::VecDeque, fs, os::unix::fs::PermissionsExt};

const DUMMY: &[u8] = b"7272727272727272727272727272727272727272727272727272727272727272";
fn input(twice: bool) -> impl FnMut(&std::ffi::CStr) -> Option<Zeroizing<Vec<u8>>> {
    let mut values = VecDeque::from([
        Some(Zeroizing::new(DUMMY.to_vec())),
        twice.then(|| Zeroizing::new(DUMMY.to_vec())),
    ]);
    move |_| values.pop_front().flatten()
}
fn repository() -> (tempfile::TempDir, Accounts) {
    let root = tempfile::tempdir().unwrap();
    let repo = persistent_accounts(&root.path().canonicalize().unwrap());
    for path in [&repo.paths.config, &repo.paths.data] {
        crate::private_fs::prepare(path).unwrap();
    }
    (root, repo)
}
fn account(repo: &Accounts, n: u8) -> Account {
    repo.create(
        Provider::MailboxOrg,
        format!("dummy-{n}@example.test"),
        &SecretValue::new(b"invented".to_vec()),
    )
    .unwrap()
}
fn bundle(repo: &Accounts) -> (String, PathBuf) {
    let directory = repo.paths.config.join("recovery-transfer/outgoing");
    let files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let id = files[0].file_stem().unwrap().to_str().unwrap().into();
    (id, files[0].clone())
}
fn transfer(target: &Accounts, id: &str, source: &Path) {
    let inbox = target.paths.config.join("recovery-transfer/incoming");
    crate::private_fs::prepare(&inbox).unwrap();
    let path = inbox.join(format!("{id}.bundle"));
    fs::copy(source, &path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
struct Reply(&'static str);
impl Backend for Reply {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        true
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        assert!(!request.windows(DUMMY.len()).any(|w| w == DUMMY));
        assert!(!request.windows(32).any(|w| w == [0x72; 32]));
        let value: serde_json::Value = serde_json::from_slice(request).unwrap();
        assert!(value["actions"].as_array().unwrap().iter().all(|a| {
            a["purpose"] == "recovery"
                && a["consequences"]
                    .as_str()
                    .unwrap()
                    .contains("@example.test")
        }));
        let id = value["request_id"].as_str().unwrap();
        let digest = data_encoding::HEXLOWER.encode(&Sha256::digest(request));
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(challenge).unwrap();
        mac.update(b"mailroom-exact-action-outcome-v1\0");
        mac.update(&serde_json::to_vec(&(id, &digest, self.0)).unwrap());
        Ok(serde_json::to_vec(&serde_json::json!({"request_id":id,"request_digest":digest,"outcome":self.0,"authentication":data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())})).unwrap())
    }
}
fn approve() -> Boundary {
    Boundary::with_backend(Box::new(Reply("approved")))
}

#[test]
fn recovery_owner_parsing_and_hidden_key_fail_closed() {
    for args in [
        &["recovery-capture"][..],
        &["recovery-capture", "a", "A"],
        &["recovery-restore", "../../private"],
        &["recovery-resume", "extra"],
        &["recovery-capture", "../../private"],
    ] {
        assert!(parse(args).is_none());
    }
    assert!(parse(&["recovery-capture", "a", "b"]).is_some());
    assert!(parse(&["recovery-capture-with-ledger", "a"]).is_some());
    assert!(parse(&["recovery-resume"]).is_some());
    assert!(
        parse(&[
            "recovery-capture",
            "a",
            "b",
            "c",
            "d",
            "e",
            "f",
            "g",
            "h",
            "i"
        ])
        .is_none()
    );
    assert!(matches!(key(&mut |_| None, false), Err("no_tty")));
    for raw in [b"short".as_slice(), &[b'G'; 64], &[b' '; 64]] {
        assert!(matches!(
            key(&mut |_| Some(Zeroizing::new(raw.to_vec())), false),
            Err("invalid_input")
        ));
    }
    let mut count = 0;
    assert!(matches!(
        key(
            &mut |_| {
                count += 1;
                Some(Zeroizing::new(if count == 1 {
                    DUMMY.to_vec()
                } else {
                    vec![b'1'; 64]
                }))
            },
            true
        ),
        Err("mismatch")
    ));
    assert!(key(&mut input(true), true).unwrap().bytes() == [0x72; 32]);
}

#[test]
fn recovery_owner_exact_consent_binds_key_set_and_single_use() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    let (manifest, original) = repo
        .selected_capture_preview(&[a.id], false, None, 123)
        .unwrap();
    let binding = Consent::new();
    let secret = key(&mut input(false), false).unwrap();
    let plan = binding
        .plan(original.clone(), &Review::Capture(&manifest), &secret)
        .unwrap();
    let altered_key = binding
        .plan(
            original.clone(),
            &Review::Capture(&manifest),
            &SecretValue::new(vec![3; 32]),
        )
        .unwrap();
    assert!(plan.exact_receipt != altered_key.exact_receipt);
    let mut altered_manifest = manifest.clone();
    altered_manifest.accounts[0].revision += 1;
    let changed = binding
        .plan(
            original.clone(),
            &Review::Capture(&altered_manifest),
            &secret,
        )
        .unwrap();
    assert!(plan.exact_receipt != changed.exact_receipt);
    let mut boundary = approve();
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    assert!(boundary.confirm(&handle, &plan, || Ok(changed)).is_err());
    assert!(
        boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_err()
    );
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    assert!(
        boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_ok()
    );
    assert!(
        boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_err()
    );
}

#[test]
fn recovery_owner_capture_restore_preserves_selected_history_and_read_only_hold() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    let _excluded = account(&repo, 2);
    let before = {
        let store = repo.store(&a).unwrap();
        let mut state = store.load_state().unwrap();
        state
            .winddown
            .dismissed
            .insert("dummy-owner-decision".into());
        store.save_state(&mut state).unwrap();
        serde_json::to_vec(&state).unwrap()
    };
    assert!(
        perform(
            &repo,
            Command::Capture {
                accounts: vec![a.id.clone()],
                ledger: false
            },
            &mut input(true),
            &mut approve()
        ) == Ok("captured")
    );
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    assert!(
        perform(
            &fresh,
            Command::Restore {
                bundle: id.clone(),
                ledger: false
            },
            &mut input(false),
            &mut approve()
        ) == Ok("restored_read_only")
    );
    let restored = fresh.load(&a.id).unwrap();
    assert!(
        serde_json::to_value(&restored).unwrap()["restored"]["reconciliation_required"] == true
    );
    assert!(fresh.password(&restored).is_err());
    assert!(
        serde_json::to_vec(&fresh.store(&restored).unwrap().load_state().unwrap()).unwrap()
            == before
    );
    let store = fresh.store(&restored).unwrap();
    assert!(store.save_state(&mut store.load_state().unwrap()).is_err());
    drop(store);
    assert_eq!(fresh.list().unwrap().len(), 1);
    assert!(!fresh.paths.data.join("ledger/addresses.yaml").exists());
    assert!(
        perform(
            &fresh,
            Command::Restore {
                bundle: id,
                ledger: false
            },
            &mut input(false),
            &mut approve()
        )
        .is_err()
    );
}

#[test]
fn recovery_owner_cancel_wrong_key_and_unsafe_import_publish_nothing() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    assert!(
        perform(
            &repo,
            Command::Capture {
                accounts: vec![a.id.clone()],
                ledger: false
            },
            &mut input(true),
            &mut Boundary::with_backend(Box::new(Reply("cancelled")))
        ) == Err("cancelled")
    );
    assert_eq!(
        fs::read_dir(repo.paths.config.join("recovery-transfer/outgoing"))
            .unwrap()
            .count(),
        0
    );
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id],
            ledger: false,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    assert!(
        perform(
            &fresh,
            Command::Restore {
                bundle: id.clone(),
                ledger: false
            },
            &mut |_| Some(Zeroizing::new(vec![b'1'; 64])),
            &mut approve()
        ) == Err("authentication_failed")
    );
    assert!(
        perform(
            &fresh,
            Command::Restore {
                bundle: id.clone(),
                ledger: false
            },
            &mut input(false),
            &mut Boundary::with_backend(Box::new(Reply("cancelled")))
        ) == Err("cancelled")
    );
    assert!(fresh.list().unwrap().is_empty());
    assert!(fresh.selected_pending().unwrap().is_empty());
    let import = fresh
        .paths
        .config
        .join("recovery-transfer/incoming")
        .join(format!("{id}.bundle"));
    fs::remove_file(&import).unwrap();
    std::os::unix::fs::symlink(path, &import).unwrap();
    assert!(
        perform(
            &fresh,
            Command::Restore {
                bundle: id,
                ledger: false
            },
            &mut input(false),
            &mut approve()
        )
        .is_err()
    );
    assert!(fresh.list().unwrap().is_empty());
}

#[test]
fn recovery_owner_explicit_ledger_restore_rejects_changed_owner_bytes() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    let ledger = InstalledLedger::install_under(&repo.paths.data).unwrap();
    let original = fs::read(repo.paths.data.join("ledger/addresses.yaml")).unwrap();
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id.clone()],
            ledger: true,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    drop(ledger);
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    perform(
        &fresh,
        Command::Restore {
            bundle: id.clone(),
            ledger: true,
        },
        &mut input(false),
        &mut approve(),
    )
    .unwrap();
    assert!(fs::read(fresh.paths.data.join("ledger/addresses.yaml")).unwrap() == original);
    let (_other_root, other) = repository();
    transfer(&other, &id, &path);
    let _other_ledger = InstalledLedger::install_under(&other.paths.data).unwrap();
    let target = other.paths.data.join("ledger/addresses.yaml");
    let changed = [original, b"\n# owner change\n".to_vec()].concat();
    fs::write(&target, &changed).unwrap();
    assert!(
        perform(
            &other,
            Command::Restore {
                bundle: id,
                ledger: true
            },
            &mut input(false),
            &mut approve()
        )
        .is_err()
    );
    assert!(fs::read(target).unwrap() == changed);
    assert!(other.list().unwrap().is_empty());
}

#[test]
fn recovery_owner_import_keeps_the_checked_file_after_path_replacement() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id.clone()],
            ledger: false,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    let inbox = fresh.paths.config.join("recovery-transfer/incoming");
    let checked = incoming(&inbox, &id).unwrap();
    let imported = inbox.join(format!("{id}.bundle"));
    fs::rename(&imported, inbox.join("retained-ciphertext")).unwrap();
    let other = inbox.join("different-input");
    fs::write(&other, b"not the checked recovery bundle").unwrap();
    std::os::unix::fs::symlink(&other, &imported).unwrap();
    let secret = key(&mut input(false), false).unwrap();
    let preview = fresh
        .selected_restore_preview_file(&checked, &secret, false, None)
        .unwrap();
    assert_eq!(preview.bundle, id);
    fresh
        .selected_restore_file(&checked, &secret, &preview, None, |plan| {
            Consent::new().authorize(plan, &Review::Restore(&preview), &secret, &mut approve())
        })
        .unwrap();
    assert_eq!(fresh.load(&a.id).unwrap().login, a.login);
    assert!(
        fresh
            .store(&fresh.load(&a.id).unwrap())
            .unwrap()
            .load_state()
            .is_ok()
    );
}

#[test]
fn recovery_owner_native_review_names_the_ledger_restore_choice() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    let _ledger = InstalledLedger::install_under(&repo.paths.data).unwrap();
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id],
            ledger: true,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    let file = incoming(&fresh.paths.config.join("recovery-transfer/incoming"), &id).unwrap();
    let ledger = InstalledLedger::recovery_under(&fresh.paths.data).unwrap();
    let secret = key(&mut input(false), false).unwrap();
    let binding = Consent::new();
    let mut receipts = vec![];
    for (restore, description) in [
        (false, "Preserve the installed ledger"),
        (
            true,
            "Restore the bundled authoritative ledger into the absent ledger target",
        ),
    ] {
        let preview = fresh
            .selected_restore_preview_file(&file, &secret, restore, Some(&ledger))
            .unwrap();
        let plan = binding
            .plan(
                recovery::selected::selected_restore_plan(&preview).unwrap(),
                &Review::Restore(&preview),
                &secret,
            )
            .unwrap();
        receipts.push(plan.exact_receipt.clone());
        let (_, request) = Boundary::synthetic().offer(plan).unwrap();
        let json = serde_json::to_value(request).unwrap();
        assert!(json["display"].as_str().unwrap().contains(description));
    }
    assert!(receipts[0] != receipts[1]);
}

#[test]
fn recovery_owner_eight_account_review_fits_native_display_and_request_limits() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    let (mut manifest, _) = repo
        .selected_capture_preview(&[a.id], false, None, 123)
        .unwrap();
    let sample = manifest.accounts[0].clone();
    manifest.accounts = (0..8)
        .map(|i| {
            let mut captured = sample.clone();
            captured.account = AccountId::new(format!("account-{i}").as_str()).unwrap();
            captured.login = format!("{}@example.test", "a".repeat(307));
            captured.generation = u64::MAX;
            captured.revision = u64::MAX;
            captured
        })
        .collect();
    let original = Plan {
        exact_receipt: "a".repeat(64),
        actions: manifest
            .accounts
            .iter()
            .map(|a| ExactAction::Recovery {
                account: a.account.as_str().into(),
                operation: "capture_selected".into(),
                manifest_digest: "a".repeat(64),
                consequences: recovery::selected::SELECTED_LIMITATIONS.into(),
            })
            .collect(),
    };
    let secret = key(&mut input(false), false).unwrap();
    let plan = Consent::new()
        .plan(original, &Review::Capture(&manifest), &secret)
        .unwrap();
    let (_, request) = Boundary::synthetic().offer(plan).unwrap();
    let json = serde_json::to_value(&request).unwrap();
    let display = json["display"].as_str().unwrap();
    assert!(display.len() <= 8192);
    assert!(serde_json::to_vec(&request).unwrap().len() <= 49152);
    assert_eq!(display.matches("Provider passwords").count(), 1);
    for account in &manifest.accounts {
        assert!(display.contains(account.account.as_str()));
    }
}

#[test]
fn recovery_owner_prepare_does_not_read_a_key_or_request_consent() {
    let (_root, repo) = repository();
    let mut boundary = approve();
    assert!(parse(&["recovery-prepare"]).is_some());
    assert!(parse(&["recovery-prepare", "extra"]).is_none());
    assert!(
        perform(
            &repo,
            Command::Prepare,
            &mut |_| panic!("Preparation must not request private input"),
            &mut boundary
        ) == Ok("recovery_prepared")
    );
    assert_eq!(boundary.accesses, 0);
    assert!(
        repo.paths
            .config
            .join("recovery-transfer/incoming")
            .is_dir()
    );
    assert!(
        repo.paths
            .config
            .join("recovery-transfer/outgoing")
            .is_dir()
    );
    assert!(repo.list().unwrap().is_empty());
}

#[test]
fn recovery_owner_interrupted_restore_requires_new_exact_consent_before_resuming() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id.clone()],
            ledger: false,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    crate::acquisition::lifecycle::inject_failure(Some("selected_restore_intent"));
    let interrupted = perform(
        &fresh,
        Command::Restore {
            bundle: id,
            ledger: false,
        },
        &mut input(false),
        &mut approve(),
    );
    crate::acquisition::lifecycle::inject_failure(None);
    assert!(interrupted == Err("storage_unavailable"));
    assert!(fresh.selected_pending().unwrap() == vec![a.id.clone()]);
    assert!(fresh.list().unwrap().is_empty());
    assert!(
        perform(
            &fresh,
            Command::Resume,
            &mut input(false),
            &mut Boundary::with_backend(Box::new(Reply("cancelled")))
        ) == Err("cancelled")
    );
    assert!(fresh.selected_pending().unwrap() == vec![a.id.clone()]);
    assert!(fresh.list().unwrap().is_empty());
    assert!(
        perform(&fresh, Command::Resume, &mut input(false), &mut approve())
            == Ok("restored_read_only")
    );
    assert!(fresh.selected_pending().unwrap().is_empty());
    let restored = fresh.load(&a.id).unwrap();
    let store = fresh.store(&restored).unwrap();
    assert!(store.save_state(&mut store.load_state().unwrap()).is_err());
    assert!(fresh.password(&restored).is_err());
}

#[test]
fn recovery_owner_without_ledger_preserves_first_start_ledger_bootstrap() {
    let (_root, repo) = repository();
    let a = account(&repo, 1);
    perform(
        &repo,
        Command::Capture {
            accounts: vec![a.id.clone()],
            ledger: false,
        },
        &mut input(true),
        &mut approve(),
    )
    .unwrap();
    let (id, path) = bundle(&repo);
    let (_fresh_root, fresh) = repository();
    transfer(&fresh, &id, &path);
    perform(
        &fresh,
        Command::Restore {
            bundle: id,
            ledger: false,
        },
        &mut input(false),
        &mut approve(),
    )
    .unwrap();
    assert!(fresh.load(&a.id).is_ok());
    assert!(!fresh.paths.data.join("ledger/addresses.yaml").exists());
    let installed = InstalledLedger::install_under(&fresh.paths.data).unwrap();
    assert!(
        installed.lock().unwrap().read().is_ok(),
        "Normal startup must still initialize the disabled empty ledger after ledger-excluded recovery"
    );
}

#[test]
fn recovery_owner_failed_resume_preserves_first_start_ledger_bootstrap() {
    let (_root, repo) = repository();
    assert!(perform(&repo, Command::Resume, &mut input(false), &mut approve()).is_err());
    let installed = InstalledLedger::install_under(&repo.paths.data).unwrap();
    assert!(
        installed.lock().unwrap().read().is_ok(),
        "A failed recovery action must not prevent normal ledger initialization"
    );
}
