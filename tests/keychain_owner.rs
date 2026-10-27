//! Runs the real owner commands of a hardened, ad-hoc signed copy of the mailroom binary
//! against a throwaway keychain. Every command names that keychain explicitly.
#![cfg(target_os = "macos")]
use core_foundation::{
    array::CFArray,
    base::{CFType, CFTypeRef, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use mailroom::acquisition::{
    AccountPaths, Accounts, AcquisitionError, CredentialBackend, Provider, SecretValue,
};
use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_void},
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecAttrAccess: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecUseKeychain: CFStringRef;
    fn SecKeychainCreate(
        path: *const c_char,
        length: u32,
        password: *const c_void,
        prompt: u8,
        access: *const c_void,
        keychain: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainDelete(keychain: *mut c_void) -> i32;
    fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> i32;
    fn SecAccessCreate(label: CFStringRef, trusted: CFTypeRef, access: *mut *mut c_void) -> i32;
    fn SecTrustedApplicationCreateFromPath(path: *const c_char, app: *mut *mut c_void) -> i32;
}

struct Setup {
    dir: tempfile::TempDir,
    keychain: *mut c_void,
    path: PathBuf,
    binary: PathBuf,
    other: PathBuf,
    dummies: Vec<String>,
}
#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);
impl CredentialBackend for Memory {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        self.0
            .lock()
            .unwrap()
            .get(reference)
            .map(|v| SecretValue::new(v.clone()))
            .ok_or(AcquisitionError::CredentialMissing)
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        self.0
            .lock()
            .unwrap()
            .insert(reference.into(), value.bytes().to_vec());
        Ok(())
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        self.0.lock().unwrap().remove(reference);
        Ok(())
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.assert_no_children();
        unsafe { assert_eq!(SecKeychainDelete(self.keychain), 0) };
        assert!(!self.path.exists());
    }
}

fn signed_copy(dir: &Path, name: &str, identifier: &str) -> PathBuf {
    let path = dir.join(name);
    fs::copy(env!("CARGO_BIN_EXE_mailroom"), &path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let signed = Command::new("/usr/bin/codesign")
        .args(["-s", "-", "-f", "-o", "runtime", "--identifier", identifier])
        .arg(&path)
        .output()
        .unwrap();
    assert!(signed.status.success());
    path
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let binary = signed_copy(dir.path(), "mailroom", "org.mailroom.test");
        let other = signed_copy(dir.path(), "mailroom-earlier", "org.mailroom.earlier");
        let path = dir.path().join("throwaway.keychain-db");
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let password: [u8; 32] = rand::random();
        let mut keychain = std::ptr::null_mut();
        let status = unsafe {
            SecKeychainCreate(
                c_path.as_ptr(),
                32,
                password.as_ptr().cast(),
                0,
                std::ptr::null(),
                &mut keychain,
            )
        };
        assert_eq!(status, 0);
        Self {
            dir,
            keychain,
            path,
            binary,
            other,
            dummies: vec![],
        }
    }

    fn dummy(&mut self) -> String {
        let value = format!("nk-dummy-secret-{:024x}", rand::random::<u128>() >> 32);
        self.dummies.push(value.clone());
        value
    }

    /// Adds an item directly, as an earlier `keyring` build or another binary would have.
    fn add(&self, service: &str, reference: &str, value: impl AsRef<[u8]>, trusted: &[&Path]) {
        let applications: Vec<CFType> = trusted
            .iter()
            .map(|p| {
                let p = CString::new(p.to_str().unwrap()).unwrap();
                let mut app = std::ptr::null_mut();
                assert_eq!(
                    unsafe { SecTrustedApplicationCreateFromPath(p.as_ptr(), &mut app) },
                    0
                );
                unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) }
            })
            .collect();
        let applications = CFArray::from_CFTypes(&applications);
        let mut access = std::ptr::null_mut();
        let label = CFString::new(reference);
        assert_eq!(
            unsafe {
                SecAccessCreate(
                    label.as_concrete_TypeRef(),
                    applications.as_CFTypeRef(),
                    &mut access,
                )
            },
            0
        );
        let k = |s: CFStringRef| unsafe { CFString::wrap_under_get_rule(s) };
        let pairs = [
            (
                k(unsafe { kSecClass }),
                k(unsafe { kSecClassGenericPassword }).as_CFType(),
            ),
            (
                k(unsafe { kSecAttrService }),
                CFString::new(service).as_CFType(),
            ),
            (
                k(unsafe { kSecAttrAccount }),
                CFString::new(reference).as_CFType(),
            ),
            (
                k(unsafe { kSecValueData }),
                CFData::from_buffer(value.as_ref()).as_CFType(),
            ),
            (k(unsafe { kSecAttrAccess }), unsafe {
                CFType::wrap_under_create_rule(access as CFTypeRef)
            }),
            (k(unsafe { kSecUseKeychain }), unsafe {
                CFType::wrap_under_get_rule(self.keychain as CFTypeRef)
            }),
        ];
        let attributes = CFDictionary::from_CFType_pairs(&pairs);
        assert_eq!(
            unsafe { SecItemAdd(attributes.as_CFTypeRef(), std::ptr::null_mut()) },
            0
        );
    }

    fn checked(&self, argv: &[String], stdout: &[u8], stderr: &[u8]) -> String {
        let stdout = String::from_utf8(stdout.to_vec()).unwrap();
        assert!(stderr.is_empty(), "stderr stays empty");
        for line in stdout.lines() {
            let fixed = line.split(' ').all(|field| {
                field.split_once('=').is_some_and(|(k, v)| {
                    ["status", "outcome", "ref", "class"].contains(&k)
                        && !v.is_empty()
                        && v.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                })
            });
            assert!(fixed, "only fixed records are printed: {line}");
        }
        for dummy in &self.dummies {
            assert!(!stdout.contains(dummy.as_str()));
            assert!(
                !argv.iter().any(|a| a.contains(dummy.as_str())),
                "a secret in argv"
            );
        }
        stdout
    }

    fn run_with(&self, binary: &Path, arguments: &[&str]) -> (i32, String) {
        let mut argv: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
        argv.extend(["--keychain".into(), self.path.to_str().unwrap().into()]);
        argv.extend(["--config".into(), self.config().to_str().unwrap().into()]);
        let output = Command::new(binary)
            .args(&argv)
            .env_clear()
            .env("HOME", std::env::var_os("HOME").unwrap())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = self.checked(&argv, &output.stdout, &output.stderr);
        (output.status.code().unwrap(), stdout.trim().to_owned())
    }
    fn run(&self, arguments: &[&str]) -> String {
        self.run_with(&self.binary, arguments).1
    }

    /// Types `lines` into the command through a pseudo-terminal, each after its prompt.
    /// The lines travel on the driver's stdin, never in argv.
    fn typed(&self, arguments: &[&str], lines: &[&str]) -> String {
        const DRIVER: &str = r#"
import os, pty, select, signal, sys
lines = sys.stdin.read().splitlines()
pid, fd = pty.fork()
if pid == 0:
    os.execv(sys.argv[1], sys.argv[1:])
def stop(*_):
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    os._exit(3)
signal.signal(signal.SIGTERM, stop)
seen = b""
while True:
    ready, _, _ = select.select([fd], [], [], 20)
    if not ready:
        stop()
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    seen += chunk
    if lines and (seen.endswith(b"Password: ") or seen.endswith(b"Repeat it: ")):
        os.write(fd, (lines.pop(0) + "\n").encode())
os.waitpid(pid, 0)
sys.stdout.buffer.write(seen)
"#;
        let mut argv: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
        argv.extend(["--keychain".into(), self.path.to_str().unwrap().into()]);
        let mut child = Command::new("/usr/bin/python3")
            .args(["-I", "-c", DRIVER])
            .arg(&self.binary)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
        drop(stdin);
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > std::time::Duration::from_secs(60) {
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                std::thread::sleep(std::time::Duration::from_secs(1));
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        self.assert_no_children();
        assert_eq!(
            output.status.code(),
            Some(0),
            "the driver finished without a timeout"
        );
        for dummy in &self.dummies {
            assert!(
                !output
                    .stdout
                    .windows(dummy.len())
                    .any(|w| w == dummy.as_bytes()),
                "typed secret echoed"
            );
        }
        assert!(output.stderr.is_empty());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| l.starts_with("status="))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// No process started with this test's keychain is still running.
    fn assert_no_children(&self) {
        let found = Command::new("/usr/bin/pgrep")
            .args(["-f", self.path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(found.stdout.is_empty(), "a child outlived its test");
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("accounts")
    }

    /// Creates a real account whose secrets live in memory, then returns its reference
    /// and the genuine census key, as an earlier `keyring` build would have stored them.
    fn account(&self, login: &str) -> (String, Vec<u8>) {
        let config = self.config();
        let backend = Arc::new(Memory::default());
        let repo = Accounts::new(
            AccountPaths {
                data: config.join("data"),
                config,
            },
            backend.clone(),
        );
        let password = SecretValue::new(b"nk-dummy-secret-account".to_vec());
        let account = repo
            .create(Provider::MailboxOrg, login.into(), &password)
            .unwrap();
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        store.save_state(&mut state).unwrap();
        let reference = format!("{}-census-key", account.id.as_str());
        let value = backend.0.lock().unwrap()[&reference].clone();
        (reference, value)
    }

    fn assert_no_plaintext(&self) {
        fn walk(root: &Path, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, files)
                } else {
                    files.push(path)
                }
            }
        }
        let mut files = Vec::new();
        walk(self.dir.path(), &mut files);
        for file in files {
            let bytes = fs::read(file).unwrap();
            for dummy in &self.dummies {
                assert!(
                    !bytes.windows(dummy.len()).any(|w| w == dummy.as_bytes()),
                    "plaintext in a file"
                );
            }
        }
    }
}

#[test]
fn migration_retirement_and_their_failures_through_the_real_binary() {
    let mut s = Setup::new();
    let (a, a_key) = s.account("a@example.test");
    let (b, b_key) = s.account("b@example.test");
    let (x, other_value) = (s.dummy(), s.dummy());
    let binary = s.binary.clone();
    let other = s.other.clone();
    s.add("mailroom", &a, &a_key, &[&binary]);
    s.add("mailroom", "b-password", &x, &[&binary]);
    s.add("mailroom", &b, &b_key, &[&binary]);
    s.add("mailroom-protected-v1", &b, &other_value, &[&binary]);
    s.add("mailroom", "d-census-key", &x, &[&other]);
    s.add("mailroom", "p-census-key", &x, &[&binary]);
    let listed = s.run(&["keychain", "legacy"]);
    assert_eq!(listed.lines().count(), 6, "five names and the status");
    assert_eq!(s.run(&["keychain", "migrate", &a]), "status=migrated");
    assert_eq!(
        s.run(&["keychain", "migrate", &a]),
        "status=matched",
        "a rerun after an interruption converges"
    );
    assert_eq!(
        s.run(&["keychain", "status", &a]),
        "status=ok class=pinned_self"
    );
    assert_eq!(s.run(&["keychain", "check-store", &a]), "status=verified");
    assert_eq!(s.run(&["keychain", "retire-legacy", &a]), "status=retired");
    assert_eq!(
        s.run(&["keychain", "retire-legacy", &a]),
        "status=legacy_missing"
    );
    assert_eq!(s.run(&["keychain", "probe", &a]), "status=ok");

    assert_eq!(
        s.run(&["keychain", "migrate", "b-password"]),
        "status=protected_denied",
        "verifying a password needs a person"
    );
    assert_eq!(
        s.run(&["keychain", "retire-legacy", "b-password"]),
        "status=protected_denied"
    );

    assert_eq!(
        s.run(&["keychain", "migrate", &b]),
        "status=mismatch",
        "a planted protected key collides"
    );
    assert_eq!(s.run(&["keychain", "retire-legacy", &b]), "status=mismatch");
    assert_eq!(
        s.run(&["keychain", "check-store", &b]),
        "status=mismatch",
        "the planted key does not open the account"
    );

    assert_eq!(
        s.run(&["keychain", "migrate", "d-census-key"]),
        "status=legacy_denied"
    );
    assert_eq!(
        s.run(&["keychain", "migrate", "p-census-key"]),
        "status=protected_unverified",
        "a planted legacy key with no account"
    );
    assert_eq!(
        s.run(&["keychain", "status", "p-census-key"]),
        "status=missing class=pinned_self"
    );
    assert_eq!(
        s.run(&["keychain", "migrate", "z-census-key"]),
        "status=legacy_missing"
    );
    assert_eq!(
        s.run(&["keychain", "legacy"]).lines().count(),
        5,
        "only the verified original was deleted"
    );
    s.assert_no_plaintext();
}

#[test]
fn a_planted_staged_copy_is_refused_and_a_genuine_one_recovers() {
    let mut s = Setup::new();
    let (a, a_key) = s.account("a@example.test");
    let planted = s.dummy();
    let binary = s.binary.clone();
    let staged = format!("{a}-retrust");
    s.add("mailroom-protected-v1", &staged, &planted, &[&binary]);
    assert_eq!(s.run(&["keychain", "retrust", &a]), "status=unverified");
    assert_eq!(
        s.run(&["keychain", "status", &a]),
        "status=missing class=pinned_self",
        "nothing was published"
    );
    let (b, b_key) = s.account("b@example.test");
    s.add(
        "mailroom-protected-v1",
        &format!("{b}-retrust"),
        &b_key,
        &[&binary],
    );
    assert_eq!(
        s.run(&["keychain", "retrust", &b]),
        "status=retrusted",
        "an interrupted move finishes"
    );
    assert_eq!(s.run(&["keychain", "check-store", &b]), "status=verified");
    assert_eq!(s.run(&["keychain", "retrust", &b]), "status=unchanged");
    s.add(
        "mailroom-protected-v1",
        &format!("{b}-retrust"),
        &b_key,
        &[&binary],
    );
    assert_eq!(
        s.run(&["keychain", "retrust", &b]),
        "status=unchanged",
        "a leftover equal copy is cleaned"
    );
    assert_eq!(
        s.run(&["keychain", "status", &format!("{b}-retrust")]),
        "status=missing class=every_access"
    );
    let _ = a_key;
    s.assert_no_plaintext();
}

#[test]
fn an_earlier_build_is_detected_and_its_keys_need_a_person_to_move() {
    let mut s = Setup::new();
    let value = s.dummy();
    let other = s.other.clone();
    s.add("mailroom-protected-v1", "e-census-key", &value, &[&other]);
    s.add("mailroom-protected-v1", "f-password", &value, &[&other]);
    assert_eq!(
        s.run(&["keychain", "status", "e-census-key"]),
        "status=other_binary class=pinned_self"
    );
    assert_eq!(
        s.run(&["keychain", "probe", "e-census-key"]),
        "status=other_binary"
    );
    assert_eq!(
        s.run(&["keychain", "retrust", "e-census-key"]),
        "status=denied",
        "reading an earlier build's key needs a person"
    );
    assert_eq!(
        s.run(&["keychain", "status", "e-census-key"]),
        "status=other_binary class=pinned_self",
        "nothing moved or deleted"
    );
    assert_eq!(
        s.run(&["keychain", "status", "f-password"]),
        "status=changed class=every_access"
    );
    assert_eq!(
        s.run(&["keychain", "probe", "f-password"]),
        "status=changed",
        "a granted password is refused, not read"
    );
    assert_eq!(
        s.run_with(&other, &["keychain", "probe", "e-census-key"]).1,
        "status=ok",
        "its own build still reads it"
    );
    s.assert_no_plaintext();
}

#[test]
fn enrollment_reads_only_a_terminal_and_never_echoes() {
    let mut s = Setup::new();
    let (value, wrong) = (s.dummy(), s.dummy());
    assert_eq!(
        s.typed(&["keychain", "enroll", "g-password"], &[&value, &value]),
        "status=ok"
    );
    assert_eq!(
        s.run(&["keychain", "status", "g-password"]),
        "status=ok class=every_access"
    );
    assert_eq!(s.run(&["keychain", "probe", "g-password"]), "status=denied");
    assert_eq!(
        s.typed(&["keychain", "enroll", "g-password"], &[]),
        "status=exists"
    );
    assert_eq!(
        s.typed(&["keychain", "enroll", "h-password"], &[&value, &wrong]),
        "status=mismatch"
    );
    assert_eq!(
        s.run(&["keychain", "status", "h-password"]),
        "status=missing class=every_access"
    );
    assert_eq!(
        s.run(&["keychain", "enroll", "h-password"]),
        "status=no_tty"
    );
    assert_eq!(
        s.run(&["keychain", "enroll", "h-census-key"]),
        "status=refused"
    );
    assert_eq!(
        s.run(&["keychain", "status", "h-password"]),
        "status=missing class=every_access"
    );
    s.assert_no_plaintext();
}

#[test]
fn unhardened_builds_unknown_keychains_and_bad_arguments_are_refused() {
    let s = Setup::new();
    let unhardened = PathBuf::from(env!("CARGO_BIN_EXE_mailroom"));
    assert_eq!(
        s.run_with(&unhardened, &["keychain", "status", "a-password"]),
        (1, "status=unhardened".into())
    );
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for keychain in [
        home.join("Library/Keychains/login.keychain-db"),
        s.dir.path().join("absent"),
    ] {
        let output = Command::new(&s.binary)
            .args(["keychain", "status", "a-password", "--keychain"])
            .arg(&keychain)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "status=keychain_unavailable"
        );
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        s.run_with(&s.binary, &["keychain", "status", "../x"]),
        (1, "status=usage".into())
    );
    for malformed in [
        &["keychain", "retire-legacy", "a-census-key", "--keychain"][..],
        &[
            "keychain",
            "status",
            "a-password",
            "--keychain",
            "/tmp/x",
            "--keychain",
        ],
        &["keychain", "status", "a-password", "--keychain", "--config"],
        &["keychain", "status", "a-password", "--unknown", "x"],
    ] {
        let output = Command::new(&s.binary).args(malformed).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "status=usage",
            "{malformed:?}"
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        s.run_with(&s.binary, &["keychain", "dump"]),
        (1, "status=usage".into())
    );
    assert_eq!(
        s.run_with(&s.binary, &["keychain", "export", "a-password"]),
        (1, "status=usage".into())
    );
}

#[test]
fn references_come_from_metadata_and_native_check_is_unavailable_without_a_pin() {
    let s = Setup::new();
    let config = s.dir.path().join("config");
    fs::create_dir(&config).unwrap();
    fs::write(
        config.join("acct1.json"),
        r#"{"id":"acct1","provider":"mailbox_org","login":"x@example.test","password_ref":"acct1-password","local_secret_ref":"acct1-census-key"}"#,
    )
    .unwrap();
    let output = Command::new(&s.binary)
        .args(["keychain", "refs", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ref=acct1-password class=every_access\nref=acct1-census-key class=pinned_self\nstatus=ok"
    );
    let output = Command::new(&s.binary)
        .arg("native-check")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "outcome=unavailable"
    );
    assert!(output.stderr.is_empty());
}
