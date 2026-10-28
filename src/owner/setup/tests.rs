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
        assert_eq!(value["actions"][0]["purpose"], "local_account");
        assert_eq!(value["actions"].as_array().unwrap().len(), 1);
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
fn account_setup_cancelled_and_unauthenticated_consent_write_no_intent_or_credential() {
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
        let mut boundary = Boundary::with_backend(Box::new(reply));
        let result = perform(
            &f.repo,
            Command::Add(Provider::Gmail),
            &mut input(&[b"owner@example.test", DUMMY, DUMMY]),
            &mut boundary,
        );
        assert!(result.is_err());
        f.empty();
    }
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
