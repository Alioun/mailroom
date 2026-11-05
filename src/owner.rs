//! Owner commands for the hardened mailroom binary. Every line printed is a fixed record:
//! `status=<code>`, `outcome=<code>` or `ref=<name> class=<class>`. No value, OS text or
//! error detail is ever printed, and nothing is written to stderr.
use crate::acquisition::{
    Account, AccountPaths, Accounts, AcquisitionError, CredentialBackend, SecretValue,
    keychain::{self, Class, Gathered, Keychain, Migration, Status},
};
use crate::census::AccountId;
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
mod maintenance;
mod recovery;
mod setup;

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

/// Runs the fixed-record owner CLI. Returns the process exit code.
pub fn run(arguments: &[String]) -> u8 {
    if !crate::native::core_dumps_disabled() {
        return status("core_dumps_enabled", false);
    }
    let Some(options) = parse(arguments) else {
        return status("usage", false);
    };
    match (options.positional.as_slice(), &options.keychain) {
        (["native-check"], None) if options.config.is_none() => native_check(),
        (["account", tail @ ..], None) if options.config.is_none() => setup::run(tail),
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

/// What a set of key values does to the account they belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreCheck {
    /// The values open the account's existing authenticated state, and the identity
    /// bytes, if any, match the commitment saved under its encryption key.
    Verified,
    /// The state does not decrypt, or the identity bytes do not match the commitment.
    Mismatch,
    /// The account has no saved state to authenticate the values against.
    NoState,
    /// The account's identity predates commitments, so nothing can authenticate it.
    Uncommitted,
}
impl StoreCheck {
    fn code(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Mismatch => "mismatch",
            Self::NoState => "no_state",
            Self::Uncommitted => "uncommitted",
        }
    }
}

/// Answers `get` with fixed key values and nothing else.
struct Values(Vec<(String, Zeroizing<Vec<u8>>)>);
impl Values {
    fn of(keys: &[Gathered]) -> Arc<Self> {
        Arc::new(Self(
            keys.iter()
                .map(|k| (k.reference.clone(), Zeroizing::new(k.value.to_vec())))
                .collect(),
        ))
    }
}
impl CredentialBackend for Values {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        self.0
            .iter()
            .find(|(r, _)| r == reference)
            .map(|(_, v)| SecretValue::new(v.to_vec()))
            .ok_or(AcquisitionError::CredentialMissing)
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

fn account(paths: &AccountPaths, id: &str) -> Option<Account> {
    let id = AccountId::new(id).ok()?;
    Accounts::new(paths.clone(), Arc::new(Refusing))
        .load(&id)
        .ok()
}

/// Opens the account's saved state with `keys` answering for all of its keys at once.
pub(crate) fn verify(
    paths: &AccountPaths,
    account: &Account,
    keys: Arc<dyn CredentialBackend>,
) -> StoreCheck {
    if account.uncommitted() {
        return StoreCheck::Uncommitted;
    }
    let Ok(store) = Accounts::new(paths.clone(), keys).store(account) else {
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

/// Moves the keys of one account to this build. Each key is gathered from wherever it is,
/// and the whole set is checked against the account's saved state and identity commitment
/// before the first key moves. The moves then run one key at a time, so an interruption
/// can leave some keys moved; a rerun gathers them again and finishes. `adopt` first
/// commits an account whose identity predates commitments to the identity bytes found now.
pub(crate) fn settle(
    keychain: &Arc<Keychain>,
    paths: &AccountPaths,
    id: &str,
    adopt: bool,
) -> Result<&'static str, &'static str> {
    let mut account = account(paths, id).ok_or("no_account")?;
    if adopt != account.uncommitted() {
        return Err(if adopt { "refused" } else { "uncommitted" });
    }
    let keys = account
        .key_references()
        .into_iter()
        .map(|reference| keychain.gather(reference))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Status::code)?;
    if adopt {
        let repo = Accounts::new(paths.clone(), Values::of(&keys));
        match repo.adopt_identity(&account.id) {
            Ok(true) => {}
            Ok(false) => return Err("no_state"),
            Err(_) => return Err("mismatch"),
        }
        account = repo.load(&account.id).map_err(|_| "failed")?;
    }
    let moves = keys.iter().any(Gathered::moves);
    match (verify(paths, &account, Values::of(&keys)), moves) {
        (StoreCheck::Verified, true) => {}
        (StoreCheck::Verified | StoreCheck::NoState, false) => return Ok("unchanged"),
        (checked, _) => return Err(checked.code()),
    }
    for key in &keys {
        keychain.settle(key).map_err(Status::code)?;
    }
    match verify(paths, &account, keychain.clone()) {
        StoreCheck::Verified => Ok("settled"),
        _ => Err("failed"),
    }
}

fn item(
    keychain: &Arc<Keychain>,
    paths: Option<AccountPaths>,
    command: &str,
    reference: &str,
) -> u8 {
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
            let Some((paths, account)) = paths
                .as_ref()
                .and_then(|p| owner_of(p, reference).map(|a| (p, a)))
            else {
                return status("no_account", false);
            };
            let mut keys = Vec::new();
            for key in account.key_references() {
                match keychain.read(key) {
                    Ok(value) => keys.push((key.to_owned(), value)),
                    Err(observed) => return status(observed.code(), false),
                }
            }
            let checked = verify(paths, &account, Arc::new(Values(keys)));
            status(checked.code(), checked == StoreCheck::Verified)
        }
        "settle" | "adopt" => {
            let settled = match &paths {
                Some(paths) => settle(keychain, paths, reference, command == "adopt"),
                None => Err("no_account"),
            };
            match settled {
                Ok(code) => status(code, true),
                Err(code) => status(code, false),
            }
        }
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
        _ => status("usage", false),
    }
}

fn migration(failure: Migration) -> u8 {
    match failure {
        Migration::Refused => status("refused", false),
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
    let (code, success) = native_outcome(crate::confirmation::Boundary::native());
    record(&format!("outcome={code}"), success)
}

fn native_outcome(mut boundary: crate::confirmation::Boundary) -> (&'static str, bool) {
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
    let Ok((handle, _)) = boundary.offer(plan.clone()) else {
        return ("unavailable", false);
    };
    match boundary.confirm(&handle, &plan, || Ok(plan.clone())) {
        Ok(_) => ("approved", true),
        Err(error) if error == crate::confirmation::CANCELLED => ("cancelled", true),
        Err(error) => (denial(&error), false),
    }
}

/// Names the first point that refused the answer. Only an exact known error gets its own
/// code, so no error text, whatever it holds, reaches the record. `account add` and
/// `account replace-password` report a refused consent with the same codes.
fn denial(error: &str) -> &'static str {
    use crate::confirmation::{CHANGED_AFTER, MALFORMED, OVERSIZED, UNAUTHENTICATED};
    use crate::native::LaunchError;
    let launch = |e: LaunchError| e.to_string() == error;
    match error {
        CHANGED_AFTER => "denied_expired",
        MALFORMED => "denied_malformed",
        UNAUTHENTICATED => "denied_unauthenticated",
        OVERSIZED => "denied_oversized",
        _ if launch(LaunchError::Failed) => "denied_failed",
        _ if launch(LaunchError::Timeout) => "denied_timeout",
        _ if launch(LaunchError::Channel) => "denied_channel",
        _ if launch(LaunchError::Oversized) => "denied_oversized",
        _ if [
            LaunchError::UntrustedFile,
            LaunchError::Spawn,
            LaunchError::IdentityMismatch,
            LaunchError::UnsupportedPin,
        ]
        .into_iter()
        .any(launch) =>
        {
            "denied_launch"
        }
        _ => "denied_other",
    }
}

#[cfg(test)]
mod tests;
