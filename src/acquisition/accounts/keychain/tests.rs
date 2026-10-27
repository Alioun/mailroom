use super::*;
use crate::acquisition::{AccountPaths, Accounts, Provider};
use crate::native::{
    Pin,
    tests::{Fixture, cdhash},
};
use std::{
    ffi::{CString, c_void},
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, OnceLock},
};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainCreate(
        path: *const std::ffi::c_char,
        password_length: u32,
        password: *const c_void,
        prompt_user: u8,
        initial_access: *const c_void,
        keychain: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainDelete(keychain: *mut c_void) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *mut c_void);
}

/// A keychain file created for one test and deleted when it ends. It is never added to the
/// user's search list, and its own unlock password is random and never leaves this process.
struct Throwaway {
    handle: *mut c_void,
    path: PathBuf,
}
unsafe impl Send for Throwaway {}
impl Throwaway {
    fn new(fixture: &Fixture) -> Self {
        let path = fixture.dir.path().join("throwaway.keychain-db");
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let mut password = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut password[..]);
        let mut handle = std::ptr::null_mut();
        let status = unsafe {
            SecKeychainCreate(
                c_path.as_ptr(),
                32,
                password.as_ptr().cast(),
                0,
                std::ptr::null(),
                &mut handle,
            )
        };
        assert_eq!(status, 0, "throwaway keychain");
        Self { handle, path }
    }
}
impl Drop for Throwaway {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(SecKeychainDelete(self.handle), 0);
            CFRelease(self.handle);
        }
        assert!(!self.path.exists());
    }
}

/// Builds the real Swift helper once per test process.
fn built_helper() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("native/macos");
        let status = Command::new("/usr/bin/xcrun")
            .args([
                "swift",
                "build",
                "-c",
                "release",
                "--product",
                "mailroom-keychain",
                "--package-path",
            ])
            .arg(&package)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "swift build: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        package.join(".build/release/mailroom-keychain")
    })
}

fn sign(path: &Path, identifier: &str) -> Helper {
    let signed = Command::new("/usr/bin/codesign")
        .args(["-s", "-", "-f", "-o", "runtime", "--identifier", identifier])
        .arg(path)
        .output()
        .unwrap();
    assert!(signed.status.success());
    Helper {
        pin: Pin::AdHocCdHash(cdhash(path)),
        path: path.to_owned(),
    }
}

struct Setup {
    fixture: Fixture,
    keychain: Option<Throwaway>,
    helper: Helper,
}
impl Setup {
    fn new() -> Self {
        let fixture = Fixture::new();
        let copy = fixture.copy("mailroom-keychain", built_helper());
        let helper = sign(&copy, "org.mailroom.keychain");
        let keychain = Some(Throwaway::new(&fixture));
        Self {
            fixture,
            keychain,
            helper,
        }
    }
    fn path(&self) -> PathBuf {
        self.keychain.as_ref().unwrap().path.clone()
    }
    fn backend(&self) -> Keychain<Helper> {
        Keychain::new(self.helper.clone(), self.path())
    }
    /// No file in the test directory, keychain included, holds the dummy or its encodings.
    fn assert_no_plaintext(&self, dummy: &[u8]) {
        let encoded = data_encoding::BASE64.encode(dummy);
        for entry in walk(self.fixture.dir.path()) {
            let bytes = fs::read(entry).unwrap();
            for needle in [dummy, encoded.as_bytes()] {
                assert!(
                    !bytes.windows(needle.len()).any(|w| w == needle),
                    "plaintext in a file"
                );
            }
        }
    }
}
impl Drop for Setup {
    fn drop(&mut self) {
        drop(self.keychain.take());
    }
}

fn dummy() -> SecretValue {
    let mut random = [0u8; 12];
    OsRng.fill_bytes(&mut random);
    SecretValue::new(
        format!(
            "nk-dummy-secret-{}",
            data_encoding::HEXLOWER.encode(&random)
        )
        .into_bytes(),
    )
}

fn assert_clean(error: AcquisitionError, dummy: &SecretValue) {
    let text = format!("{error} {error:?}");
    assert!(!text.contains(dummy.text().unwrap()), "secret in an error");
    assert!(
        !text.contains("nk-dummy-secret"),
        "secret prefix in an error"
    );
}

#[test]
fn pinned_local_keys_round_trip_without_overwrite_and_delete_idempotently() {
    let setup = Setup::new();
    let backend = setup.backend();
    let value = dummy();
    backend.put("acct-census-key", &value).unwrap();
    assert_eq!(
        backend.protection("acct-census-key").unwrap(),
        Some(Protection::PinnedHelper)
    );
    assert_eq!(
        backend.get("acct-census-key").unwrap().bytes(),
        value.bytes()
    );
    let overwrite = backend.put("acct-census-key", &dummy()).unwrap_err();
    assert_eq!(overwrite, AcquisitionError::Credentials);
    assert_eq!(
        backend.get("acct-census-key").unwrap().bytes(),
        value.bytes()
    );
    backend.delete("acct-census-key").unwrap();
    assert_eq!(
        backend.get("acct-census-key").unwrap_err(),
        AcquisitionError::CredentialMissing
    );
    backend.delete("acct-census-key").unwrap();
    setup.assert_no_plaintext(value.bytes());
    assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
}

#[test]
fn mailbox_passwords_trust_no_application_and_need_a_person_at_every_read() {
    let setup = Setup::new();
    let backend = setup.backend();
    let value = dummy();
    backend.put("acct-password", &value).unwrap();
    assert_eq!(
        backend.protection("acct-password").unwrap(),
        Some(Protection::EveryAccess)
    );
    for _ in 0..2 {
        let denied = backend.get("acct-password").unwrap_err();
        assert_eq!(denied, AcquisitionError::Credentials);
        assert_clean(denied, &value);
    }
    backend.delete("acct-password").unwrap();
    setup.assert_no_plaintext(value.bytes());
}

#[test]
fn another_helper_build_cannot_read_items_pinned_to_the_original() {
    let setup = Setup::new();
    let value = dummy();
    setup.backend().put("acct-key-g2", &value).unwrap();
    let rebuilt = setup.fixture.copy("rebuilt", built_helper());
    let rebuilt = Keychain::new(sign(&rebuilt, "org.mailroom.rebuilt"), setup.path());
    let denied = rebuilt.get("acct-key-g2").unwrap_err();
    assert_eq!(denied, AcquisitionError::Credentials);
    assert_clean(denied, &value);
    assert_eq!(
        setup.backend().get("acct-key-g2").unwrap().bytes(),
        value.bytes()
    );
}

#[test]
fn substituted_unpinned_or_answerless_helpers_deny_before_any_keychain_access() {
    let setup = Setup::new();
    let value = dummy();
    let mut wrong = setup.helper.clone();
    wrong.pin = Pin::AdHocCdHash([1; 20]);
    let echo = setup.fixture.pinned("echo");
    assert!(installed().is_err(), "this test build pins no helper");
    let os = crate::acquisition::OsCredentials;
    assert_eq!(
        os.get("acct-census-key").err(),
        Some(AcquisitionError::Credentials)
    );
    assert_eq!(
        os.put("acct-census-key", &value).err(),
        Some(AcquisitionError::Credentials)
    );
    assert_eq!(
        os.delete("acct-census-key").err(),
        Some(AcquisitionError::Credentials)
    );
    for helper in [wrong, echo] {
        let backend = Keychain::new(helper, setup.path());
        for error in [
            backend.put("acct-census-key", &value).unwrap_err(),
            backend.get("acct-census-key").unwrap_err(),
            backend.delete("acct-census-key").unwrap_err(),
        ] {
            assert_eq!(error, AcquisitionError::Credentials);
            assert_clean(error, &value);
        }
    }
    assert_eq!(
        setup.backend().get("acct-census-key").unwrap_err(),
        AcquisitionError::CredentialMissing
    );
}

/// Records each exchange and lets a test rewrite or replay the helper's reply.
struct Tamper {
    inner: Helper,
    edit: fn(&mut serde_json::Value, Option<&serde_json::Value>),
    last: Mutex<Option<serde_json::Value>>,
}
impl Channel for Tamper {
    fn exchange(&self, input: &[u8]) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
        let reply = self.inner.exchange(input)?;
        let mut value: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        let previous = self.last.lock().unwrap().replace(value.clone());
        (self.edit)(&mut value, previous.as_ref());
        Ok(Zeroizing::new(serde_json::to_vec(&value).unwrap()))
    }
}

#[test]
fn altered_replayed_or_unauthenticated_replies_deny() {
    let setup = Setup::new();
    let value = dummy();
    setup.backend().put("acct-census-key", &value).unwrap();
    let edits: [fn(&mut serde_json::Value, Option<&serde_json::Value>); 6] = [
        |v, _| {
            v["secret"] = data_encoding::BASE64
                .encode(b"nk-dummy-secret-swapped")
                .into()
        },
        |v, _| v["status"] = "missing".into(),
        |v, _| v["authentication"] = "0".repeat(64).into(),
        |v, _| v["extra"] = true.into(),
        |v, _| {
            v.as_object_mut().unwrap().remove("secret");
        },
        |v, previous| {
            if let Some(previous) = previous {
                *v = previous.clone();
            }
        },
    ];
    for (index, edit) in edits.into_iter().enumerate() {
        let tamper = Tamper {
            inner: setup.helper.clone(),
            edit,
            last: Mutex::new(None),
        };
        let backend = Keychain::new(tamper, setup.path());
        if index == 5 {
            assert_eq!(
                backend.get("acct-census-key").unwrap().bytes(),
                value.bytes()
            );
        }
        let denied = backend.get("acct-census-key").unwrap_err();
        assert_eq!(denied, AcquisitionError::Credentials, "edit {index}");
        assert_clean(denied, &value);
    }
    assert_eq!(
        setup.backend().get("acct-census-key").unwrap().bytes(),
        value.bytes()
    );
}

#[test]
fn account_lifecycle_runs_on_the_keychain_backend_and_never_reads_the_password_silently() {
    let setup = Setup::new();
    let root = setup.fixture.dir.path().join("accounts");
    let repo = Accounts::new(
        AccountPaths {
            config: root.join("config"),
            data: root.join("data"),
        },
        Arc::new(setup.backend()),
    );
    let password = dummy();
    let account = repo
        .create(Provider::MailboxOrg, "login@example.test".into(), &password)
        .unwrap();
    repo.store(&account).unwrap();
    let denied = repo.password(&account).err().unwrap();
    assert_eq!(denied, AcquisitionError::Credentials);
    assert_clean(denied, &password);
    let backend = setup.backend();
    assert_eq!(
        backend.protection(&account.password_ref).unwrap(),
        Some(Protection::EveryAccess)
    );
    assert_eq!(
        backend.protection(&account.local_secret_ref).unwrap(),
        Some(Protection::PinnedHelper)
    );
    setup.assert_no_plaintext(password.bytes());
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
