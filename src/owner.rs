//! Owner commands for the hardened mailroom binary. Every line printed is a fixed record:
//! `status=<code>`, `outcome=<code>` or `ref=<name> class=<class>`. No value, OS text or
//! error detail is ever printed, and nothing is written to stderr.
use crate::acquisition::{
    AccountPaths, Accounts, AcquisitionError, CredentialBackend, SecretValue,
    keychain::{self, Class, Keychain, Migration, Status},
};
use std::{path::PathBuf, sync::Arc};
use zeroize::Zeroizing;

unsafe extern "C" {
    fn readpassphrase(
        prompt: *const std::ffi::c_char,
        buffer: *mut std::ffi::c_char,
        size: usize,
        flags: std::ffi::c_int,
    ) -> *mut std::ffi::c_char;
}
const RPP_REQUIRE_TTY: std::ffi::c_int = 0x02;

fn record(line: &str, success: bool) -> u8 {
    println!("{line}");
    if success { 0 } else { 1 }
}
fn status(code: &str, success: bool) -> u8 {
    record(&format!("status={code}"), success)
}

/// Runs `mailroom keychain …` or `mailroom native-check`. Returns the process exit code.
pub fn run(arguments: &[String]) -> u8 {
    if !crate::native::core_dumps_disabled() {
        return status("core_dumps_enabled", false);
    }
    let mut positional = Vec::new();
    let mut keychain_path = None;
    let mut config = None;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--keychain" => keychain_path = rest.next().map(PathBuf::from),
            "--config" => config = rest.next().map(PathBuf::from),
            _ => positional.push(argument.as_str()),
        }
    }
    match positional.as_slice() {
        ["native-check"] => native_check(),
        ["keychain", "refs"] => references(config),
        ["keychain", command, tail @ ..] => {
            if !crate::native::own_process_hardened() {
                return status("unhardened", false);
            }
            let opened = match &keychain_path {
                Some(path) => keychain::explicit(path).map_err(|_| ()),
                None => keychain::installed().map_err(|_| ()),
            };
            let Ok(keychain) = opened else {
                return status("keychain_unavailable", false);
            };
            match (*command, tail) {
                ("legacy", []) => legacy(&keychain),
                (command, [reference]) if keychain::valid_reference(reference) => {
                    item(&keychain, command, reference)
                }
                _ => status("usage", false),
            }
        }
        _ => status("usage", false),
    }
}

fn item(keychain: &Keychain, command: &str, reference: &str) -> u8 {
    match command {
        "status" => {
            let observed = keychain.status(reference);
            record(
                &format!(
                    "status={} class={}",
                    observed.code(),
                    Class::of(reference).code()
                ),
                observed == Status::Ok,
            )
        }
        "probe" => match keychain.read(reference) {
            Ok(_) => status("ok", true),
            Err(observed) => status(observed.code(), false),
        },
        "enroll" => enroll(keychain, reference),
        "migrate" => match keychain.migrate(reference) {
            Ok(Status::Ok) => status("migrated", true),
            Ok(_) => status("matched", true),
            Err(failure) => migration(failure),
        },
        "retire-legacy" => match keychain.retire_legacy(reference) {
            Ok(()) => status("retired", true),
            Err(failure) => migration(failure),
        },
        "retrust" => match keychain.retrust(reference) {
            Ok(Status::Ok) => status("retrusted", true),
            Ok(_) => status("unchanged", true),
            Err(observed) => status(observed.code(), false),
        },
        _ => status("usage", false),
    }
}

fn migration(failure: Migration) -> u8 {
    match failure {
        Migration::Mismatch => status("mismatch", false),
        Migration::Legacy(observed) => status(&format!("legacy_{}", observed.code()), false),
        Migration::Protected(observed) => status(&format!("protected_{}", observed.code()), false),
    }
}

fn legacy(keychain: &Keychain) -> u8 {
    match keychain.legacy_references() {
        Ok(names) => {
            for name in names {
                println!("ref={name} class={}", Class::of(&name).code());
            }
            status("ok", true)
        }
        Err(observed) => status(observed.code(), false),
    }
}

struct Refusing;
impl CredentialBackend for Refusing {
    fn get(&self, _: &str) -> Result<SecretValue, AcquisitionError> {
        Err(AcquisitionError::Credentials)
    }
    fn put(&self, _: &str, _: &SecretValue) -> Result<(), AcquisitionError> {
        Err(AcquisitionError::Credentials)
    }
    fn delete(&self, _: &str) -> Result<(), AcquisitionError> {
        Err(AcquisitionError::Credentials)
    }
}

/// Lists the Keychain references saved accounts use, from their metadata files only.
fn references(config: Option<PathBuf>) -> u8 {
    let paths = match config {
        Some(config) => Ok(AccountPaths {
            data: config.join("data"),
            config,
        }),
        None => AccountPaths::per_user(),
    };
    let Ok(paths) = paths else {
        return status("failed", false);
    };
    if !paths.config.exists() {
        return status("ok", true);
    }
    match Accounts::new(paths, Arc::new(Refusing)).list() {
        Ok(accounts) => {
            for account in &accounts {
                for reference in account.credential_references() {
                    println!("ref={reference} class={}", Class::of(reference).code());
                }
            }
            status("ok", true)
        }
        Err(_) => status("failed", false),
    }
}

/// Reads a line from the controlling terminal with echo off. Never from a pipe.
fn hidden(prompt: &std::ffi::CStr) -> Option<Zeroizing<Vec<u8>>> {
    let mut buffer = Zeroizing::new([0u8; 1024]);
    let read = unsafe {
        readpassphrase(
            prompt.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            RPP_REQUIRE_TTY,
        )
    };
    if read.is_null() {
        return None;
    }
    let length = buffer.iter().position(|b| *b == 0)?;
    (length < buffer.len() - 1).then(|| Zeroizing::new(buffer[..length].to_vec()))
}

fn enroll(keychain: &Keychain, reference: &str) -> u8 {
    if Class::of(reference) != Class::EveryAccess {
        return status("refused", false);
    }
    match keychain.status(reference) {
        Status::Missing => {}
        Status::Ok => return status("exists", false),
        observed => return status(observed.code(), false),
    }
    let Some(first) = hidden(c"Password: ") else {
        return status("no_tty", false);
    };
    let Some(second) = hidden(c"Repeat it: ") else {
        return status("no_tty", false);
    };
    if first.is_empty() {
        return status("empty", false);
    }
    if !bool::from(subtle::ConstantTimeEq::ct_eq(
        first.as_slice(),
        second.as_slice(),
    )) {
        return status("mismatch", false);
    }
    let created = keychain.create(reference, &first);
    status(created.code(), created == Status::Ok)
}

/// Shows one invented action through the pinned helper and reports only its verified outcome.
fn native_check() -> u8 {
    let plan = crate::confirmation::Plan {
        exact_receipt: "0".repeat(64),
        actions: vec![crate::confirmation::ExactAction::InvoiceSend {
            account: "wizard-check".into(),
            revision: 1,
            report_version: "check".into(),
            target_name: "nobody".into(),
            target: "nobody@example.invalid".into(),
            sending_identity: "check@example.invalid".into(),
            source: "none".into(),
            content_digest: "0".repeat(64),
        }],
    };
    let mut boundary = crate::confirmation::Boundary::native();
    let Ok((handle, _)) = boundary.offer(plan.clone()) else {
        return record("outcome=unavailable", false);
    };
    match boundary.confirm(&handle, &plan, || Ok(plan.clone())) {
        Ok(_) => record("outcome=approved", true),
        Err(error) if error == crate::confirmation::CANCELLED => record("outcome=cancelled", true),
        Err(_) => record("outcome=denied", false),
    }
}
