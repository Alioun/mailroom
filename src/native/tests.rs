use super::*;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

const DEADLINE: Duration = Duration::from_secs(20);

/// Kernel-visible cdhash of the native slice, read from codesign's report.
pub(crate) fn cdhash(path: &Path) -> [u8; 20] {
    let report = Command::new("/usr/bin/codesign")
        .args([
            "-d",
            "-vvv",
            "--arch",
            if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x86_64"
            },
        ])
        .arg(path)
        .output()
        .expect("codesign");
    let text = String::from_utf8_lossy(&report.stderr);
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("CDHash="))
        .expect("CDHash line");
    parse_cdhash(line).expect("cdhash hex")
}

/// A fake helper compiled once per test process: echoes stdin unless it starts with a command.
const FAKE: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
extern char **environ;
int main(int argc, char **argv) {
  static char in[70000]; size_t n = fread(in, 1, sizeof in - 1, stdin); in[n] = 0;
  if (!strcmp(in, "argv")) { int e = 0; while (environ[e]) e++; printf("%s|%d|%d", argv[0], argc, e); return 0; }
  if (!strcmp(in, "sleep")) { sleep(30); return 0; }
  if (!strcmp(in, "exit3")) return 3;
  if (!strcmp(in, "flood")) { for (int i = 0; i < 70000; i++) putchar('y'); return 0; }
  if (!strncmp(in, "touch:", 6)) { FILE *f = fopen(in + 6, "w"); if (f) fclose(f); return 0; }
  fwrite(in, 1, n, stdout); return 0;
}
"#;

pub(crate) fn fake_helper() -> &'static Path {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("mailroom-fake-helper")
            .tempdir()
            .unwrap()
            .keep();
        fs::write(dir.join("fake.c"), FAKE).unwrap();
        let binary = dir.join("fake");
        assert!(
            Command::new("/usr/bin/clang")
                .arg("-o")
                .arg(&binary)
                .arg(dir.join("fake.c"))
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["-s", "-", "-f", "-o", "runtime"])
                .arg(&binary)
                .output()
                .unwrap()
                .status
                .success()
        );
        binary
    })
}

pub(crate) struct Fixture {
    pub dir: tempfile::TempDir,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }
    /// Copies a signed binary into the private directory, keeping its signature.
    pub fn copy(&self, name: &str, from: &Path) -> PathBuf {
        let path = self.dir.path().join(name);
        // A fresh inode each time: macOS caches a signature per vnode and kills images rewritten in place.
        let _ = fs::remove_file(&path);
        fs::copy(from, &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    pub fn pinned(&self, name: &str) -> Helper {
        let path = self.copy(name, fake_helper());
        Helper {
            pin: Pin::AdHocCdHash(cdhash(&path)),
            path,
        }
    }
}

fn fake(
    fixture: &Fixture,
    command: &str,
    limit: usize,
    deadline: Duration,
) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
    exchange(&fixture.pinned("fake"), command.as_bytes(), limit, deadline)
}

#[test]
fn pinned_helper_receives_input_only_on_stdin_with_fixed_argv_and_empty_environment() {
    let fixture = Fixture::new();
    let echoed = fake(&fixture, "nk-dummy-secret-stdin", 4096, DEADLINE).unwrap();
    assert_eq!(&echoed[..], b"nk-dummy-secret-stdin");
    let seen = fake(&fixture, "argv", 4096, DEADLINE).unwrap();
    assert_eq!(&seen[..], b"mailroom-helper|1|0");
}

#[test]
fn wrong_pin_substitution_and_missing_hardened_runtime_never_run() {
    let fixture = Fixture::new();
    let marker = fixture.dir.path().join("ran");
    let script = format!("touch:{}", marker.display());
    let mut helper = fixture.pinned("fake");
    let pin = helper.pin.clone();
    helper.pin = Pin::AdHocCdHash([0; 20]);
    assert_eq!(
        exchange(&helper, script.as_bytes(), 64, DEADLINE),
        Err(LaunchError::IdentityMismatch)
    );
    helper.pin = pin;
    let substitute = fixture.copy("other", Path::new("/bin/cat"));
    let replaced = exchange_with(&helper, script.as_bytes(), 64, DEADLINE, || {
        fs::rename(&substitute, &helper.path).unwrap()
    });
    assert_eq!(replaced, Err(LaunchError::IdentityMismatch));
    let unhardened = fixture.copy("adhoc", fake_helper());
    let signed = Command::new("/usr/bin/codesign")
        .args(["-s", "-", "-f", "--identifier", "unhardened"])
        .arg(&unhardened)
        .output()
        .unwrap();
    assert!(signed.status.success());
    let unhardened = Helper {
        pin: Pin::AdHocCdHash(cdhash(&unhardened)),
        path: unhardened,
    };
    assert_eq!(
        exchange(&unhardened, script.as_bytes(), 64, DEADLINE),
        Err(LaunchError::IdentityMismatch)
    );
    assert!(!marker.exists(), "no rejected helper executed");
}

#[test]
fn symlinked_shared_or_writable_helpers_are_refused_before_spawn() {
    let fixture = Fixture::new();
    let helper = fixture.pinned("fake");
    let link = fixture.dir.path().join("link");
    std::os::unix::fs::symlink(&helper.path, &link).unwrap();
    let linked = Helper {
        path: link,
        pin: helper.pin.clone(),
    };
    assert_eq!(
        exchange(&linked, b"x", 64, DEADLINE),
        Err(LaunchError::UntrustedFile)
    );
    fs::set_permissions(&helper.path, fs::Permissions::from_mode(0o720)).unwrap();
    assert_eq!(
        exchange(&helper, b"x", 64, DEADLINE),
        Err(LaunchError::UntrustedFile)
    );
    fs::set_permissions(&helper.path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(fixture.dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        exchange(&helper, b"x", 64, DEADLINE),
        Err(LaunchError::UntrustedFile)
    );
    fs::set_permissions(fixture.dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let missing = Helper {
        path: fixture.dir.path().join("absent"),
        pin: helper.pin.clone(),
    };
    assert_eq!(
        exchange(&missing, b"x", 64, DEADLINE),
        Err(LaunchError::UntrustedFile)
    );
    assert_eq!(
        exchange(&helper, b"x", 64, DEADLINE).unwrap().as_slice(),
        b"x"
    );
}

#[test]
fn slow_failing_and_oversized_helpers_deny() {
    let fixture = Fixture::new();
    let started = Instant::now();
    assert_eq!(
        fake(&fixture, "sleep", 64, Duration::from_millis(300)),
        Err(LaunchError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        fake(&fixture, "exit3", 64, DEADLINE),
        Err(LaunchError::Failed)
    );
    assert_eq!(
        fake(&fixture, "flood", 1000, DEADLINE),
        Err(LaunchError::Oversized)
    );
    let big = vec![b'#'; REQUEST_LIMIT + 1];
    assert_eq!(
        exchange(&fixture.pinned("fake"), &big, 64, DEADLINE),
        Err(LaunchError::Oversized)
    );
}

#[test]
fn pins_parse_strictly_and_developer_id_is_a_denied_slot() {
    let hash = "00112233445566778899aabbccddeeff00112233";
    assert!(matches!(
        pinned(Some("/opt/h"), Some(hash), None),
        Ok(Helper {
            pin: Pin::AdHocCdHash(_),
            ..
        })
    ));
    assert_eq!(pinned(None, Some(hash), None), Err(LaunchError::NotPinned));
    assert_eq!(
        pinned(Some("relative/h"), Some(hash), None),
        Err(LaunchError::NotPinned)
    );
    assert_eq!(
        pinned(Some("/opt/h"), Some("00"), None),
        Err(LaunchError::NotPinned)
    );
    assert_eq!(
        pinned(Some("/opt/h"), Some(hash), Some("anchor apple")),
        Err(LaunchError::NotPinned)
    );
    assert_eq!(
        pinned(Some("/opt/h"), None, None),
        Err(LaunchError::NotPinned)
    );
    let developer = pinned(Some("/opt/h"), None, Some("anchor apple generic")).unwrap();
    assert_eq!(
        exchange(&developer, b"x", 64, DEADLINE),
        Err(LaunchError::UnsupportedPin)
    );
}

#[test]
fn this_test_build_pins_no_helper() {
    assert_eq!(installed(Kind::Confirm), Err(LaunchError::NotPinned));
    assert_eq!(installed(Kind::Keychain), Err(LaunchError::NotPinned));
}
