//! Owner commands for the hardened mailroom binary. Every line printed is a fixed record:
//! `status=<code>`, `outcome=<code>` or `ref=<name> class=<class>`. No value, OS text or
//! error detail is ever printed, and nothing is written to stderr.
use crate::acquisition::{
    Account, AccountPaths, Accounts, AcquisitionError, CredentialBackend, SecretValue,
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

/// Parsed command line. Each option appears at most once and always with a value.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options<'a> {
    pub positional: Vec<&'a str>,
    pub keychain: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// Rejects missing values, repeated options and unknown options, before any backend opens.
pub(crate) fn parse(arguments: &[String]) -> Option<Options<'_>> {
    let mut options = Options::default();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let slot = match argument.as_str() {
            "--keychain" => &mut options.keychain,
            "--config" => &mut options.config,
            other if other.starts_with('-') => return None,
            other => {
                options.positional.push(other);
                continue;
            }
        };
        let value = rest
            .next()
            .filter(|v| !v.is_empty() && !v.starts_with('-'))?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return None;
        }
    }
    Some(options)
}

/// Runs `mailroom keychain …` or `mailroom native-check`. Returns the process exit code.
pub fn run(arguments: &[String]) -> u8 {
    if !crate::native::core_dumps_disabled() {
        return status("core_dumps_enabled", false);
    }
    let Some(options) = parse(arguments) else {
        return status("usage", false);
    };
    match (options.positional.as_slice(), &options.keychain) {
        (["native-check"], None) if options.config.is_none() => native_check(),
        (["keychain", "refs"], None) => references(options.config),
        (["keychain", command, tail @ ..], keychain_path) => {
            let usable = match (*command, tail) {
                ("legacy", []) => true,
                (_, [reference]) => keychain::valid_reference(reference),
                _ => false,
            };
            if !usable {
                return status("usage", false);
            }
            if !crate::native::own_process_hardened() {
                return status("unhardened", false);
            }
            let opened = match keychain_path {
                Some(path) => keychain::explicit(path).map_err(|_| ()),
                None => keychain::installed().map_err(|_| ()),
            };
            let Ok(keychain) = opened else {
                return status("keychain_unavailable", false);
            };
            let keychain = Arc::new(keychain);
            let paths = match options.config {
                Some(config) => Some(AccountPaths {
                    data: config.join("data"),
                    config,
                }),
                None => AccountPaths::per_user().ok(),
            };
            match (*command, tail) {
                ("legacy", []) => legacy(&keychain),
                (command, [reference]) => item(&keychain, paths, command, reference),
                _ => status("usage", false),
            }
        }
        _ => status("usage", false),
    }
}

/// What a candidate key value does to the account that owns the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreCheck {
    /// The value opens the account's existing authenticated state.
    Verified,
    /// The account's state does not decrypt under the value.
    Mismatch,
    /// The account has no saved state to authenticate the value against.
    NoState,
    /// No saved account uses this reference.
    NoAccount,
}
impl StoreCheck {
    fn code(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Mismatch => "mismatch",
            Self::NoState => "no_state",
            Self::NoAccount => "no_account",
        }
    }
}

/// Answers `get` for one reference with a candidate value and every other from Keychain.
struct Candidate {
    keychain: Arc<Keychain>,
    reference: String,
    value: Zeroizing<Vec<u8>>,
}
impl CredentialBackend for Candidate {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        if reference == self.reference {
            Ok(SecretValue::new(self.value.to_vec()))
        } else {
            self.keychain.get(reference)
        }
    }
    fn put(&self, _: &str, _: &SecretValue) -> Result<(), AcquisitionError> {
        Err(AcquisitionError::Credentials)
    }
    fn delete(&self, _: &str) -> Result<(), AcquisitionError> {
        Err(AcquisitionError::Credentials)
    }
}

fn owner_of(paths: &AccountPaths, reference: &str) -> Option<Account> {
    let accounts = Accounts::new(paths.clone(), Arc::new(Refusing))
        .list()
        .ok()?;
    accounts
        .into_iter()
        .find(|a| a.credential_references().contains(&reference))
}

/// Opens the owning account's encrypted state with `value` in place of `reference`.
pub(crate) fn check_store(
    keychain: &Arc<Keychain>,
    paths: &AccountPaths,
    reference: &str,
    value: &[u8],
) -> StoreCheck {
    let Some(account) = owner_of(paths, reference) else {
        return StoreCheck::NoAccount;
    };
    let backend = Candidate {
        keychain: keychain.clone(),
        reference: reference.into(),
        value: Zeroizing::new(value.to_vec()),
    };
    let repo = Accounts::new(paths.clone(), Arc::new(backend));
    let Ok(store) = repo.store(&account) else {
        return StoreCheck::Mismatch;
    };
    if !store.path().exists() {
        return StoreCheck::NoState;
    }
    match store.load_state() {
        Ok(_) => StoreCheck::Verified,
        Err(_) => StoreCheck::Mismatch,
    }
}

fn item(
    keychain: &Arc<Keychain>,
    paths: Option<AccountPaths>,
    command: &str,
    reference: &str,
) -> u8 {
    let verify = |value: &[u8]| {
        paths
            .as_ref()
            .is_some_and(|p| check_store(keychain, p, reference, value) == StoreCheck::Verified)
    };
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
        "check-store" => {
            if Class::of(reference) != Class::PinnedSelf {
                return status("refused", false);
            }
            let Some(paths) = &paths else {
                return status("no_account", false);
            };
            match keychain.read(reference) {
                Ok(value) => {
                    let checked = check_store(keychain, paths, reference, &value);
                    status(checked.code(), checked == StoreCheck::Verified)
                }
                Err(observed) => status(observed.code(), false),
            }
        }
        "enroll" => enroll(keychain, reference),
        "migrate" => match keychain.migrate(reference, &verify) {
            Ok(Status::Ok) => status("migrated", true),
            Ok(_) => status("matched", true),
            Err(failure) => migration(failure),
        },
        "retire-legacy" => match keychain.retire_legacy(reference) {
            Ok(()) => status("retired", true),
            Err(failure) => migration(failure),
        },
        "retrust" => match keychain.retrust(reference, &verify) {
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

#[cfg(test)]
mod tests;
