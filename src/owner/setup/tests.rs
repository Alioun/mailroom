use super::*;
use crate::confirmation::{Backend, SyntheticBackend};
use sha2::Digest;
use std::{
    collections::{BTreeMap, VecDeque},
    os::unix::fs::{PermissionsExt, symlink},
    sync::Mutex,
};

const DUMMY: &[u8] = b"ACCOUNT-SETUP-DUMMY-32d746";
#[derive(Default)]
struct Memory(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl CredentialBackend for Memory {
    fn get(&self, r: &str) -> Result<SecretValue, AcquisitionError> {
        self.0
            .lock()
            .unwrap()
            .get(r)
            .map(|v| SecretValue::new(v.to_vec()))
            .ok_or(AcquisitionError::CredentialMissing)
    }
    fn put(&self, r: &str, v: &SecretValue) -> Result<(), AcquisitionError> {
        self.0
            .lock()
            .unwrap()
            .insert(r.into(), Zeroizing::new(v.bytes().to_vec()));
        Ok(())
    }
    fn delete(&self, r: &str) -> Result<(), AcquisitionError> {
        self.0.lock().unwrap().remove(r);
        Ok(())
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    repo: Accounts,
    memory: Arc<Memory>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let paths = AccountPaths {
            config: path.join("config"),
            data: path.join("data"),
        };
        for path in [&paths.config, &paths.data] {
            crate::private_fs::prepare(path).unwrap();
        }
        let memory = Arc::new(Memory::default());
        Self {
            repo: Accounts::new(paths, memory.clone()),
            memory,
            _root: root,
        }
    }
    fn empty(&self) {
        assert!(self.repo.list().unwrap().is_empty());
        assert!(self.memory.0.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&self.repo.paths.data).unwrap().count(), 0);
        let records = self.repo.paths.config.join("lifecycle");
        if records.exists() {
            assert!(
                std::fs::read_dir(records)
                    .unwrap()
                    .all(|e| e.unwrap().path().extension().unwrap() == "lock")
            );
        }
    }
}
fn input(values: &[&[u8]]) -> impl FnMut(&std::ffi::CStr) -> Option<Zeroizing<Vec<u8>>> + use<> {
    let mut values: VecDeque<_> = values.iter().map(|v| Zeroizing::new(v.to_vec())).collect();
    move |_| values.pop_front()
}
struct Reply {
    outcome: &'static str,
    malformed: bool,
}
impl Backend for Reply {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        true
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        assert!(!request.windows(DUMMY.len()).any(|w| w == DUMMY));
        let plain_digest = data_encoding::HEXLOWER.encode(&Sha256::digest(DUMMY));
        assert!(!String::from_utf8_lossy(request).contains(&plain_digest));
        if self.malformed {
            return Ok(b"not-authenticated".to_vec());
        }
        let value: serde_json::Value = serde_json::from_slice(request).unwrap();
        assert!(matches!(
            value["actions"][0]["purpose"].as_str(),
            Some("local_account" | "recovery")
        ));
        assert_eq!(value["actions"].as_array().unwrap().len(), 1);
        if value["actions"][0]["purpose"] == "recovery" {
            assert!(
                value["actions"][0]["consequences"]
                    .as_str()
                    .unwrap()
                    .contains("owner@example.test")
            );
        }
        let id = value["request_id"].as_str().unwrap();
        let digest = data_encoding::HEXLOWER.encode(&Sha256::digest(request));
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(challenge).unwrap();
        mac.update(b"mailroom-exact-action-outcome-v1\0");
        mac.update(&serde_json::to_vec(&(id, &digest, self.outcome)).unwrap());
        Ok(serde_json::to_vec(&serde_json::json!({"request_id":id,"request_digest":digest,"outcome":self.outcome,"authentication":data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())})).unwrap())
    }
}
fn approve() -> Boundary {
    Boundary::with_backend(Box::new(Reply {
        outcome: "approved",
        malformed: false,
    }))
}

#[test]
fn account_setup_rejects_extra_arguments_and_unqualified_path_resolution() {
    for tail in [
        &["add", "gmail", "private-extra"][..],
        &["add", "unknown"],
        &["replace-password"],
        &["replace-password", "../../account"],
        &["add", "gmail", "--config", "/unused"],
    ] {
        assert!(parse(tail).is_none());
    }
    assert!(parse(&["add", "gmail"]).is_some());
    assert!(parse(&["replace-password", "saved-account"]).is_some());
    let helper = crate::native::Helper {
        path: "/unused/helper".into(),
        pin: crate::native::Pin::AdHocCdHash([8; 20]),
    };
    for core in [false, true] {
        for hardened in [false, true] {
            assert_eq!(qualified(core, hardened, Some(&helper)), core && hardened);
            assert!(!qualified(core, hardened, None));
        }
    }
    assert!(repository_if(false, || panic!("unqualified setup accessed paths")).is_err());
}

#[test]
fn account_setup_private_directories_reject_permissions_symlinks_and_parent_writes() {
    let f = Fixture::new();
    let root = f._root.path().canonicalize().unwrap();
    let wrong = root.join("wrong");
    std::fs::create_dir(&wrong).unwrap();
    std::fs::set_permissions(&wrong, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(crate::private_fs::prepare(&wrong).is_err());
    assert_eq!(
        std::fs::metadata(&wrong).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let alias = root.join("alias");
    symlink(&f.repo.paths.config, &alias).unwrap();
    assert!(crate::private_fs::prepare(&alias).is_err());
    std::fs::set_permissions(&wrong, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(crate::private_fs::prepare(&wrong.join("new")).is_err());
    assert!(!wrong.join("new").exists());
    assert!(crate::private_fs::prepare(&root.join("one/two")).is_ok());
}

#[test]
fn account_setup_input_failures_publish_no_account_or_credential() {
    for values in [
        vec![],
        vec![b"owner@example.test".as_slice()],
        vec![b"bad\nlogin", DUMMY, DUMMY],
        vec![b"owner@example.test", DUMMY, b"different"],
        vec![b"owner@example.test", b"", b""],
        vec![b"owner@example.test", b"bad\npassword", b"bad\npassword"],
    ] {
        let f = Fixture::new();
        assert!(
            perform(
                &f.repo,
                Command::Add(Provider::Gmail),
                &mut input(&values),
                &mut approve()
            )
            .is_err()
        );
        f.empty();
    }
}

#[test]
fn account_setup_cancelled_and_refused_consent_name_the_refusal_and_write_nothing() {
    for (mut boundary, code) in [
        (
            Boundary::with_backend(Box::new(Reply {
                outcome: "cancelled",
                malformed: false,
            })),
            "cancelled",
        ),
        (
            Boundary::with_backend(Box::new(Reply {
                outcome: "approved",
                malformed: true,
            })),
            "denied_malformed",
        ),
        (Boundary::native(), "unavailable"),
    ] {
        let f = Fixture::new();
        let result = perform(
            &f.repo,
            Command::Add(Provider::Gmail),
            &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
            &mut boundary,
        );
        assert_eq!(result, Err(code));
        f.empty();
    }
}

#[test]
fn account_setup_reports_a_keychain_failure_after_consent_apart_from_a_refusal() {
    let f = Fixture::new();
    let failing = Accounts::new(
        f.repo.paths.clone(),
        Arc::new(FailPassword(f.memory.clone())),
    );
    let mut boundary = approve();
    assert_eq!(
        perform(
            &failing,
            Command::Add(Provider::Gmail),
            &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
            &mut boundary
        ),
        Err("keychain_failed")
    );
    assert_eq!(boundary.accesses, 1);
    assert!(f.repo.list().unwrap().is_empty());
    assert_eq!(f.repo.recovery_status().unwrap().len(), 1);
    assert!(
        f.memory
            .0
            .lock()
            .unwrap()
            .values()
            .all(|v| v.as_slice() != DUMMY)
    );
}

#[test]
fn password_replacement_reports_an_unreadable_key_before_any_consent() {
    let f = Fixture::new();
    perform(
        &f.repo,
        Command::Add(Provider::MailboxOrg),
        &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
        &mut approve(),
    )
    .unwrap();
    let account = f.repo.list().unwrap().remove(0);
    f.memory.delete(account.key_references()[0]).unwrap();
    let mut boundary = approve();
    assert_eq!(
        perform(
            &f.repo,
            Command::Replace(account.id.clone()),
            &mut input(&[b"REPLACEMENT-DUMMY-1c4e", b"REPLACEMENT-DUMMY-1c4e"]),
            &mut boundary
        ),
        Err("keychain_failed")
    );
    assert_eq!(boundary.accesses, 0);
    assert!(
        f.memory
            .get(account.credential_references()[0])
            .unwrap()
            .bytes()
            == DUMMY
    );
}

#[test]
fn account_setup_creates_unverified_and_refuses_duplicate_under_transaction_lock() {
    let f = Fixture::new();
    assert_eq!(
        perform(
            &f.repo,
            Command::Add(Provider::Gmail),
            &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
            &mut approve()
        ),
        Ok("created")
    );
    let account = f.repo.list().unwrap().remove(0);
    assert_eq!(
        serde_json::to_value(&account).unwrap()["connection"],
        "unverified"
    );
    assert!(f.repo.store(&account).unwrap().load_state().is_ok());
    let count = f.memory.0.lock().unwrap().len();
    let mut unavailable = Boundary::with_backend(Box::new(Reply {
        outcome: "approved",
        malformed: true,
    }));
    assert_eq!(
        perform(
            &f.repo,
            Command::Add(Provider::Gmail),
            &mut input(&[b"OWNER@example.test", DUMMY, DUMMY]),
            &mut unavailable
        ),
        Err("invalid_input")
    );
    assert_eq!(unavailable.accesses, 0);
    assert_eq!(f.repo.list().unwrap().len(), 1);
    assert_eq!(f.memory.0.lock().unwrap().len(), count);
}

#[test]
fn account_setup_replacement_preserves_all_saved_history_and_denial_preserves_password() {
    let f = Fixture::new();
    perform(
        &f.repo,
        Command::Add(Provider::MailboxOrg),
        &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
        &mut approve(),
    )
    .unwrap();
    let account = f.repo.list().unwrap().remove(0);
    let store = f.repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved
        .synthetic_uid_next
        .insert("preserved-history".into(), 73);
    store.save_state(&mut saved).unwrap();
    let before = serde_json::to_vec(&saved).unwrap();
    drop(store);
    let changed = b"REPLACEMENT-DUMMY-d60f7a";
    let mut cancel = Boundary::with_backend(Box::new(Reply {
        outcome: "cancelled",
        malformed: false,
    }));
    assert_eq!(
        perform(
            &f.repo,
            Command::Replace(account.id.clone()),
            &mut input(&[changed, changed]),
            &mut cancel
        ),
        Err("cancelled")
    );
    let old_ref = account.credential_references()[0].to_owned();
    assert!(f.memory.get(&old_ref).unwrap().bytes() == DUMMY);
    assert_eq!(
        perform(
            &f.repo,
            Command::Replace(account.id.clone()),
            &mut input(&[changed, changed]),
            &mut approve()
        ),
        Ok("replaced")
    );
    let current = f.repo.load(&account.id).unwrap();
    assert!(current.credential_references()[0] != old_ref);
    assert!(f.memory.get(&old_ref).is_err());
    assert!(
        f.memory
            .get(current.credential_references()[0])
            .unwrap()
            .bytes()
            == changed
    );
    assert!(
        serde_json::to_vec(&f.repo.store(&current).unwrap().load_state().unwrap()).unwrap()
            == before
    );
}

#[test]
fn account_setup_receipts_bind_password_identity_provider_operation_and_revision() {
    let f = Fixture::new();
    perform(
        &f.repo,
        Command::Add(Provider::Gmail),
        &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
        &mut approve(),
    )
    .unwrap();
    let account = f.repo.list().unwrap().remove(0);
    let key = Binding::new();
    let password = SecretValue::new(DUMMY.to_vec());
    let original = key
        .plan(&account, LocalAccountOperation::Create, 1, &password)
        .unwrap();
    let mut other = account.clone();
    other.login = "other@example.test".into();
    let mut service = account.clone();
    service.provider = Provider::MailboxOrg;
    let variants = [
        key.plan(&account, LocalAccountOperation::Create, 2, &password)
            .unwrap(),
        key.plan(
            &account,
            LocalAccountOperation::ReplacePassword,
            1,
            &password,
        )
        .unwrap(),
        key.plan(&other, LocalAccountOperation::Create, 1, &password)
            .unwrap(),
        key.plan(&service, LocalAccountOperation::Create, 1, &password)
            .unwrap(),
        key.plan(
            &account,
            LocalAccountOperation::Create,
            1,
            &SecretValue::new(b"OTHER-DUMMY".to_vec()),
        )
        .unwrap(),
        Binding::new()
            .plan(&account, LocalAccountOperation::Create, 1, &password)
            .unwrap(),
    ];
    for variant in variants {
        assert!(variant.exact_receipt != original.exact_receipt);
        let mut boundary = Boundary::with_backend(Box::new(SyntheticBackend));
        let (handle, _) = boundary.offer(original.clone()).unwrap();
        assert!(
            boundary
                .confirm(&handle, &original, || Ok(variant))
                .is_err()
        );
        assert!(
            boundary
                .confirm(&handle, &original, || Ok(original.clone()))
                .is_err()
        );
    }
}

fn saved_account(f: &Fixture) -> Account {
    perform(
        &f.repo,
        Command::Add(Provider::Gmail),
        &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
        &mut approve(),
    )
    .unwrap();
    f.repo.list().unwrap().remove(0)
}
fn maintenance(
    f: &Fixture,
    operation: super::super::maintenance::Operation,
    id: &AccountId,
    boundary: &mut Boundary,
) -> Result<&'static str, &'static str> {
    super::super::maintenance::perform(
        &f.repo,
        super::super::maintenance::Command {
            operation,
            account: id.clone(),
        },
        boundary,
    )
}
#[test]
fn local_maintenance_parses_only_fixed_operations_and_safe_account_ids() {
    for verb in ["forget", "rotate-key", "recover-local"] {
        assert!(super::super::maintenance::parse(&[verb, "saved-account"]).is_some());
        assert!(super::super::maintenance::parse(&[verb, "../account"]).is_none());
        assert!(super::super::maintenance::parse(&[verb, "saved-account", "--yes"]).is_none());
    }
    assert!(super::super::maintenance::parse(&["resume-provider", "saved-account"]).is_none());
}
#[test]
fn local_maintenance_cancelled_or_forged_forget_and_rotation_leave_state_and_keys_unchanged() {
    use super::super::maintenance::Operation;
    for operation in [Operation::Forget, Operation::Rotate] {
        for reply in [
            Reply {
                outcome: "cancelled",
                malformed: false,
            },
            Reply {
                outcome: "approved",
                malformed: true,
            },
        ] {
            let f = Fixture::new();
            let account = saved_account(&f);
            let metadata = serde_json::to_vec(&account).unwrap();
            let cipher = std::fs::read(
                f.repo
                    .paths
                    .data
                    .join(format!("{}.census.enc", account.id.as_str())),
            )
            .unwrap();
            let count = f.memory.0.lock().unwrap().len();
            let mut boundary = Boundary::with_backend(Box::new(reply));
            assert!(maintenance(&f, operation, &account.id, &mut boundary).is_err());
            assert_eq!(boundary.accesses, 1);
            assert!(serde_json::to_vec(&f.repo.load(&account.id).unwrap()).unwrap() == metadata);
            assert!(
                std::fs::read(
                    f.repo
                        .paths
                        .data
                        .join(format!("{}.census.enc", account.id.as_str()))
                )
                .unwrap()
                    == cipher
            );
            assert_eq!(f.memory.0.lock().unwrap().len(), count);
            assert!(f.repo.recovery_status().unwrap().is_empty());
        }
    }
}
#[test]
fn local_maintenance_busy_account_denies_before_confirmation_and_unpinned_helper_denies_before_intent()
 {
    use super::super::maintenance::Operation;
    let f = Fixture::new();
    let account = saved_account(&f);
    let store = f.repo.store(&account).unwrap();
    for operation in [Operation::Forget, Operation::Rotate, Operation::Recover] {
        let mut boundary = approve();
        assert_eq!(
            maintenance(&f, operation, &account.id, &mut boundary),
            Err("recovery_required")
        );
        assert_eq!(boundary.accesses, 0);
    }
    drop(store);
    let restore = f.repo.paths.config.join("recovery-selected.restore");
    std::fs::write(&restore, b"DUMMY-RESTORE-HOLD").unwrap();
    let mut held = approve();
    assert_eq!(
        maintenance(&f, Operation::Recover, &account.id, &mut held),
        Err("recovery_required")
    );
    assert_eq!(held.accesses, 0);
    std::fs::remove_file(restore).unwrap();

    let mut boundary = Boundary::native();
    assert_eq!(
        maintenance(&f, Operation::Forget, &account.id, &mut boundary),
        Err("denied")
    );
    assert!(f.repo.recovery_status().unwrap().is_empty());
    assert!(f.repo.store(&account).unwrap().load_state().is_ok());
}
#[test]
fn local_maintenance_rotation_preserves_exact_state_and_forget_preserves_ledger_and_other_accounts()
{
    use super::super::maintenance::Operation;
    let f = Fixture::new();
    let account = saved_account(&f);
    let other = f
        .repo
        .create(
            Provider::MailboxOrg,
            "other@example.test".into(),
            &SecretValue::new(b"OTHER-DUMMY-7eaa".to_vec()),
        )
        .unwrap();
    let other_metadata = serde_json::to_vec(&other).unwrap();
    let other_state =
        serde_json::to_vec(&f.repo.store(&other).unwrap().load_state().unwrap()).unwrap();
    let ledger = crate::ledger::InstalledLedger::install_under(&f.repo.paths.data).unwrap();
    let ledger_bytes = std::fs::read(f.repo.paths.data.join("ledger/addresses.yaml")).unwrap();
    drop(ledger);
    let store = f.repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved
        .synthetic_uid_next
        .insert("operation-identity-survives".into(), 92);
    saved.winddown.settings.winding_down = true;
    store.save_state(&mut saved).unwrap();
    let before = serde_json::to_vec(&saved).unwrap();
    drop(store);
    assert_eq!(
        maintenance(&f, Operation::Rotate, &account.id, &mut approve()),
        Ok("rotated")
    );
    let rotated = f.repo.load(&account.id).unwrap();
    assert_ne!(rotated.key_references(), account.key_references());
    let mut reopened = f.repo.store(&rotated).unwrap().load_state().unwrap();
    assert_eq!(reopened.revision, saved.revision + 1);
    reopened.revision = saved.revision;
    assert!(serde_json::to_vec(&reopened).unwrap() == before);
    assert!(f.memory.get(account.key_references()[0]).is_err());
    assert_eq!(
        maintenance(&f, Operation::Forget, &account.id, &mut approve()),
        Ok("forgotten")
    );
    assert!(f.repo.load(&account.id).is_err());
    for reference in rotated.credential_references() {
        assert!(f.memory.get(reference).is_err());
    }
    assert!(
        std::fs::read(f.repo.paths.data.join("ledger/addresses.yaml")).unwrap() == ledger_bytes
    );
    assert!(serde_json::to_vec(&f.repo.load(&other.id).unwrap()).unwrap() == other_metadata);
    assert!(
        serde_json::to_vec(&f.repo.store(&other).unwrap().load_state().unwrap()).unwrap()
            == other_state
    );
}
struct FailPassword(Arc<Memory>);
impl CredentialBackend for FailPassword {
    fn get(&self, r: &str) -> Result<SecretValue, AcquisitionError> {
        self.0.get(r)
    }
    fn put(&self, r: &str, v: &SecretValue) -> Result<(), AcquisitionError> {
        if r.ends_with("-password") {
            Err(AcquisitionError::Credentials)
        } else {
            self.0.put(r, v)
        }
    }
    fn delete(&self, r: &str) -> Result<(), AcquisitionError> {
        self.0.delete(r)
    }
}
#[test]
fn local_maintenance_recovery_requires_consent_before_cleaning_unpublished_creation_and_is_idempotent()
 {
    use super::super::maintenance::Operation;
    let f = Fixture::new();
    let failing = Accounts::new(
        f.repo.paths.clone(),
        Arc::new(FailPassword(f.memory.clone())),
    );
    assert!(
        failing
            .create(
                Provider::Gmail,
                "owner@example.test".into(),
                &SecretValue::new(DUMMY.to_vec())
            )
            .is_err()
    );
    let pending = f.repo.recovery_status().unwrap();
    assert_eq!(pending.len(), 1);
    let id = &pending[0].account;
    let path = f
        .repo
        .paths
        .config
        .join("lifecycle")
        .join(format!("{}.json", id.as_str()));
    let intent = std::fs::read(&path).unwrap();
    let count = f.memory.0.lock().unwrap().len();
    assert_eq!(count, 1);
    let mut cancel = Boundary::with_backend(Box::new(Reply {
        outcome: "cancelled",
        malformed: false,
    }));
    assert_eq!(
        maintenance(&f, Operation::Recover, id, &mut cancel),
        Err("cancelled")
    );
    assert!(std::fs::read(&path).unwrap() == intent);
    assert_eq!(f.memory.0.lock().unwrap().len(), count);
    assert_eq!(
        maintenance(&f, Operation::Recover, id, &mut approve()),
        Ok("recovered")
    );
    assert!(!path.exists());
    assert!(f.memory.0.lock().unwrap().is_empty());
    assert!(f.repo.list().unwrap().is_empty());
    let mut no_work = Boundary::native();
    assert_eq!(
        maintenance(&f, Operation::Recover, id, &mut no_work),
        Ok("recovered")
    );
    assert_eq!(no_work.accesses, 0);
}
