use super::*;
use crate::{
    acquisition::Provider,
    confirmation::{Boundary, ExactAction, LocalAccountOperation, Plan},
};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub(super) enum Command {
    Add(Provider),
    Replace(AccountId),
}
fn parse(tail: &[&str]) -> Option<Command> {
    match tail {
        ["add", "gmail"] => Some(Command::Add(Provider::Gmail)),
        ["add", "mailbox_org"] => Some(Command::Add(Provider::MailboxOrg)),
        ["replace-password", id] => AccountId::new(id).ok().map(Command::Replace),
        _ => None,
    }
}
fn qualified(core: bool, hardened: bool, helper: Option<&crate::native::Helper>) -> bool {
    core && hardened && helper.is_some_and(|h| matches!(h.pin, crate::native::Pin::AdHocCdHash(_)))
}
fn repository_if(
    ready: bool,
    paths: impl FnOnce() -> Result<AccountPaths, AcquisitionError>,
) -> Result<Accounts, &'static str> {
    if !ready {
        return Err("protection_unavailable");
    }
    let paths = paths().map_err(|_| "storage_unavailable")?;
    for path in [&paths.config, &paths.data, &paths.config.join("lifecycle")] {
        crate::private_fs::prepare(path).map_err(|_| "storage_unavailable")?;
    }
    Ok(Accounts::new(
        paths,
        Arc::new(crate::acquisition::OsCredentials),
    ))
}
pub(super) fn run(tail: &[&str]) -> u8 {
    let Some(command) = parse(tail) else {
        return status("usage", false);
    };
    let helper = crate::native::installed().ok();
    let repository = repository_if(
        qualified(
            crate::native::core_dumps_disabled(),
            crate::native::own_process_hardened(),
            helper.as_ref(),
        ),
        AccountPaths::per_user,
    );
    let Ok(repository) = repository else {
        return status(repository.err().unwrap(), false);
    };
    match perform(&repository, command, &mut hidden, &mut Boundary::native()) {
        Ok(code) => status(code, true),
        Err(code) => status(code, false),
    }
}

/// A fresh private HMAC key binds the password without exposing a reusable password digest.
struct Binding(Zeroizing<[u8; 32]>);
impl Binding {
    fn new() -> Self {
        let mut key = Zeroizing::new([0; 32]);
        OsRng.fill_bytes(key.as_mut());
        Self(key)
    }
    fn plan(
        &self,
        account: &Account,
        operation: LocalAccountOperation,
        revision: u64,
        password: &SecretValue,
    ) -> Result<Plan, AcquisitionError> {
        let metadata = Zeroizing::new(
            serde_json::to_vec(&(account, operation, revision))
                .map_err(|_| AcquisitionError::Storage)?,
        );
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_ref()).expect("HMAC key");
        mac.update(b"mailroom/local-account-consent/v1\0");
        mac.update(&(metadata.len() as u64).to_be_bytes());
        mac.update(&metadata);
        mac.update(&(password.bytes().len() as u64).to_be_bytes());
        mac.update(password.bytes());
        let receipt = data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes());
        Ok(Plan {
            exact_receipt: receipt.clone(),
            actions: vec![ExactAction::LocalAccount {
                account: account.id.as_str().into(),
                operation,
                provider: account.provider,
                login: account.login.clone(),
                plan_receipt: receipt,
            }],
        })
    }
    fn authorize(
        &self,
        account: &Account,
        operation: LocalAccountOperation,
        revision: u64,
        password: &SecretValue,
        boundary: &mut Boundary,
    ) -> Result<(), AcquisitionError> {
        let plan = self.plan(account, operation, revision, password)?;
        let (handle, _) = boundary
            .offer(plan.clone())
            .map_err(|_| AcquisitionError::Credentials)?;
        boundary
            .confirm(&handle, &plan, || {
                self.plan(account, operation, revision, password)
                    .map_err(|_| "Local account plan unavailable".into())
            })
            .map(|_| ())
            .map_err(|e| {
                if e == crate::confirmation::CANCELLED {
                    AcquisitionError::Cancelled
                } else {
                    AcquisitionError::Credentials
                }
            })
    }
}

type Reader<'a> = dyn FnMut(&std::ffi::CStr) -> Option<Zeroizing<Vec<u8>>> + 'a;
fn password(input: &mut Reader<'_>) -> Result<SecretValue, &'static str> {
    let mut first = input(c"Password: ").ok_or("no_tty")?;
    let second = input(c"Password again: ").ok_or("no_tty")?;
    if first.is_empty() {
        return Err("invalid_input");
    }
    if !bool::from(first.as_slice().ct_eq(second.as_slice())) {
        return Err("mismatch");
    }
    if first.len() > 1022
        || first.iter().any(|b| b.is_ascii_control())
        || std::str::from_utf8(&first).is_err()
    {
        return Err("invalid_input");
    }
    Ok(SecretValue::new(std::mem::take(&mut *first)))
}
fn perform(
    repo: &Accounts,
    command: Command,
    input: &mut Reader<'_>,
    boundary: &mut Boundary,
) -> Result<&'static str, &'static str> {
    let login = if matches!(command, Command::Add(_)) {
        let raw = input(c"Account login: ").ok_or("no_tty")?;
        let login = std::str::from_utf8(&raw).map_err(|_| "invalid_input")?;
        if login.is_empty()
            || login.len() > 320
            || login.chars().any(char::is_whitespace)
            || login.chars().any(char::is_control)
        {
            return Err("invalid_input");
        }
        Some(login.to_owned())
    } else {
        None
    };
    let password = password(input)?;
    let binding = Binding::new();
    let result = match command {
        Command::Add(provider) => repo
            .create_reviewed(provider, login.unwrap(), &password, |a| {
                binding.authorize(a, LocalAccountOperation::Create, 0, &password, boundary)
            })
            .map(|_| "created"),
        Command::Replace(id) => repo
            .replace_password_reviewed(&id, &password, |a, saved, _| {
                binding.authorize(
                    a,
                    LocalAccountOperation::ReplacePassword,
                    saved.revision,
                    &password,
                    boundary,
                )
            })
            .map(|_| "replaced"),
    };
    result.map_err(|e| match e {
        AcquisitionError::Cancelled => "cancelled",
        AcquisitionError::Credentials | AcquisitionError::CredentialMissing => "denied",
        AcquisitionError::InvalidInput => "invalid_input",
        AcquisitionError::LifecyclePending => "recovery_required",
        _ => "storage_unavailable",
    })
}

#[cfg(test)]
mod tests;
