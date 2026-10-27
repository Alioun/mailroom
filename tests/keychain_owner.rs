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
use std::{
    ffi::{CString, c_char, c_void},
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
impl Drop for Setup {
    fn drop(&mut self) {
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
    fn add(&self, service: &str, reference: &str, value: &str, trusted: &[&Path]) {
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
                CFData::from_buffer(value.as_bytes()).as_CFType(),
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
import os, pty, select, sys
lines = sys.stdin.read().splitlines()
pid, fd = pty.fork()
if pid == 0:
    os.execv(sys.argv[1], sys.argv[1:])
seen = b""
while True:
    ready, _, _ = select.select([fd], [], [], 30)
    if not ready:
        break
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
        let output = child.wait_with_output().unwrap();
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

    fn assert_no_plaintext(&self) {
        for entry in fs::read_dir(self.dir.path()).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
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
    let (a, c, x) = (s.dummy(), s.dummy(), s.dummy());
    let binary = s.binary.clone();
    let other = s.other.clone();
    s.add("mailroom", "a-census-key", &a, &[&binary]);
    s.add("mailroom", "b-password", &a, &[&binary]);
    s.add("mailroom", "c-census-key", &c, &[&binary]);
    s.add("mailroom-protected-v1", "c-census-key", &x, &[&binary]);
    s.add("mailroom", "d-census-key", &a, &[&other]);
    assert_eq!(
        s.run(&["keychain", "legacy"]),
        "ref=a-census-key class=pinned_self\nref=b-password class=every_access\nref=c-census-key class=pinned_self\nref=d-census-key class=pinned_self\nstatus=ok"
    );
    assert_eq!(
        s.run(&["keychain", "migrate", "a-census-key"]),
        "status=migrated"
    );
    assert_eq!(
        s.run(&["keychain", "migrate", "a-census-key"]),
        "status=matched",
        "a rerun after an interruption converges"
    );
    assert_eq!(
        s.run(&["keychain", "status", "a-census-key"]),
        "status=ok class=pinned_self"
    );
    assert_eq!(
        s.run(&["keychain", "retire-legacy", "a-census-key"]),
        "status=retired"
    );
    assert_eq!(
        s.run(&["keychain", "retire-legacy", "a-census-key"]),
        "status=legacy_missing"
    );
    assert_eq!(s.run(&["keychain", "probe", "a-census-key"]), "status=ok");

    assert_eq!(
        s.run(&["keychain", "migrate", "b-password"]),
        "status=protected_denied",
        "verifying a password needs a person"
    );
    assert_eq!(
        s.run(&["keychain", "status", "b-password"]),
        "status=ok class=every_access"
    );
    assert_eq!(
        s.run(&["keychain", "retire-legacy", "b-password"]),
        "status=protected_denied"
    );

    assert_eq!(
        s.run(&["keychain", "migrate", "c-census-key"]),
        "status=mismatch"
    );
    assert_eq!(
        s.run(&["keychain", "retire-legacy", "c-census-key"]),
        "status=mismatch"
    );

    assert_eq!(
        s.run(&["keychain", "migrate", "d-census-key"]),
        "status=legacy_denied"
    );
    assert_eq!(
        s.run(&["keychain", "status", "d-census-key"]),
        "status=missing class=pinned_self"
    );
    assert_eq!(
        s.run(&["keychain", "migrate", "z-census-key"]),
        "status=legacy_missing"
    );
    assert_eq!(
        s.run(&["keychain", "legacy"]),
        "ref=b-password class=every_access\nref=c-census-key class=pinned_self\nref=d-census-key class=pinned_self\nstatus=ok",
        "only the verified original was deleted"
    );
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
        "status=denied"
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
