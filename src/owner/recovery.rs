use super::*;
use crate::{
    acquisition::recovery::{
        self,
        selected::{LedgerSelection, LedgerTarget, SelectedManifest, SelectedRestorePreview},
    },
    confirmation::{Boundary, ExactAction, Plan},
    ledger::InstalledLedger,
};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use sha2::Sha256;
use std::{
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use subtle::ConstantTimeEq;

pub(super) enum Command {
    Prepare,
    Capture {
        accounts: Vec<AccountId>,
        ledger: bool,
    },
    Restore {
        bundle: String,
        ledger: bool,
    },
    Resume,
}
pub(super) fn parse(tail: &[&str]) -> Option<Command> {
    let (verb, rest) = tail.split_first()?;
    match *verb {
        "recovery-prepare" if rest.is_empty() => Some(Command::Prepare),
        "recovery-capture" | "recovery-capture-with-ledger" => {
            if rest.is_empty() || rest.len() > 8 {
                return None;
            }
            let accounts: Vec<_> = rest
                .iter()
                .map(|v| AccountId::new(v))
                .collect::<Result<_, _>>()
                .ok()?;
            let unique: std::collections::BTreeSet<_> = accounts
                .iter()
                .map(|a| a.as_str().to_ascii_lowercase())
                .collect();
            (unique.len() == accounts.len()).then_some(Command::Capture {
                accounts,
                ledger: *verb == "recovery-capture-with-ledger",
            })
        }
        "recovery-restore" | "recovery-restore-with-ledger"
            if rest.len() == 1 && recovery::valid_digest(rest[0]) =>
        {
            Some(Command::Restore {
                bundle: rest[0].into(),
                ledger: *verb == "recovery-restore-with-ledger",
            })
        }
        "recovery-resume" if rest.is_empty() => Some(Command::Resume),
        _ => None,
    }
}
type Reader<'a> = dyn FnMut(&std::ffi::CStr) -> Option<Zeroizing<Vec<u8>>> + 'a;
fn key(input: &mut Reader<'_>, twice: bool) -> Result<SecretValue, &'static str> {
    let first = input(c"Recovery key, 64 lowercase hex characters: ").ok_or("no_tty")?;
    if first.len() != 64
        || first
            .iter()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(b))
    {
        return Err("invalid_input");
    }
    if twice {
        let second = input(c"Recovery key again: ").ok_or("no_tty")?;
        if !bool::from(first.as_slice().ct_eq(second.as_slice())) {
            return Err("mismatch");
        }
    }
    let mut decoded = Zeroizing::new(vec![0; 32]);
    data_encoding::HEXLOWER
        .decode_mut(&first, &mut decoded)
        .map_err(|_| "invalid_input")?;
    Ok(SecretValue::new(std::mem::take(&mut *decoded)))
}

enum Review<'a> {
    Capture(&'a SelectedManifest),
    Restore(&'a SelectedRestorePreview),
}
impl Review<'_> {
    fn manifest(&self) -> &SelectedManifest {
        match self {
            Self::Capture(manifest) => manifest,
            Self::Restore(preview) => &preview.manifest,
        }
    }
    fn ledger_action(&self) -> String {
        match self {
            Self::Capture(manifest) => match &manifest.ledger {
                LedgerSelection::Included { entries, .. } => {
                    format!("Include the authoritative ledger with {entries} entries.")
                }
                LedgerSelection::Excluded { .. } => "Exclude the authoritative ledger.".into(),
            },
            Self::Restore(preview) => match &preview.ledger_target {
                LedgerTarget::PreserveCurrent { .. } => {
                    "Preserve the installed ledger. Do not restore ledger data from the bundle."
                        .into()
                }
                LedgerTarget::Absent => {
                    "Restore the bundled authoritative ledger into the absent ledger target.".into()
                }
                LedgerTarget::Identical { .. } => {
                    "Keep the installed authoritative ledger, whose bytes match the bundle.".into()
                }
            },
        }
    }
}
struct Consent(Zeroizing<[u8; 32]>);
impl Consent {
    fn new() -> Self {
        let mut value = Zeroizing::new([0; 32]);
        OsRng.fill_bytes(value.as_mut());
        Self(value)
    }
    fn plan(
        &self,
        mut plan: Plan,
        review: &Review<'_>,
        secret: &SecretValue,
    ) -> Result<Plan, AcquisitionError> {
        let manifest = review.manifest();
        for (index, action) in plan.actions.iter_mut().enumerate() {
            let ExactAction::Recovery {
                account,
                consequences,
                operation,
                ..
            } = action
            else {
                return Err(AcquisitionError::InvalidInput);
            };
            let captured = manifest
                .accounts
                .iter()
                .find(|a| a.account.as_str() == account)
                .ok_or(AcquisitionError::InvalidInput)?;
            let provider = match captured.provider {
                crate::acquisition::Provider::Gmail => "Gmail",
                crate::acquisition::Provider::MailboxOrg => "mailbox.org",
            };
            *operation = match operation.as_str() {
                "capture_selected" => "Create an encrypted recovery bundle",
                "restore_selected_absent_accounts" => "Restore absent local accounts",
                "resume_selected_local_restore" => "Resume the interrupted local restore",
                _ => return Err(AcquisitionError::InvalidInput),
            }
            .into();
            let shared = if index == 0 {
                format!(
                    " {} {} other accounts excluded. {}",
                    review.ledger_action(),
                    manifest.excluded_accounts.len(),
                    consequences
                )
            } else {
                String::new()
            };
            let captured_at = chrono::DateTime::from_timestamp(
                i64::try_from(manifest.captured_at).map_err(|_| AcquisitionError::InvalidInput)?,
                0,
            )
            .ok_or(AcquisitionError::InvalidInput)?
            .to_rfc3339();
            *consequences = format!(
                "For {} at {}. Captured {}. Schema {}, generation {}, revision {}. {} captured account resources.{}",
                captured.login,
                provider,
                captured_at,
                captured.state_schema,
                captured.generation,
                captured.revision,
                captured.resources.len(),
                shared
            );
        }
        let bytes = Zeroizing::new(
            serde_json::to_vec(&(&plan.actions, &plan.exact_receipt))
                .map_err(|_| AcquisitionError::Storage)?,
        );
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_ref()).expect("HMAC key");
        mac.update(b"mailroom/owner-recovery-consent/v1\0");
        mac.update(&(bytes.len() as u64).to_be_bytes());
        mac.update(&bytes);
        mac.update(secret.bytes());
        plan.exact_receipt = data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes());
        Ok(plan)
    }
    fn authorize(
        &self,
        plan: Plan,
        review: &Review<'_>,
        secret: &SecretValue,
        boundary: &mut Boundary,
    ) -> Result<(), AcquisitionError> {
        let current = self.plan(plan.clone(), review, secret)?;
        let (handle, _) = boundary
            .offer(current.clone())
            .map_err(|_| AcquisitionError::Credentials)?;
        boundary
            .confirm(&handle, &current, || {
                self.plan(plan, review, secret)
                    .map_err(|_| "Recovery plan unavailable".into())
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
fn incoming(directory: &Path, id: &str) -> Result<std::fs::File, AcquisitionError> {
    if !recovery::valid_digest(id) {
        return Err(AcquisitionError::InvalidInput);
    }
    let path = directory.join(format!("{id}.bundle"));
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|_| AcquisitionError::Storage)?;
    let m = file.metadata().map_err(|_| AcquisitionError::Storage)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
    {
        return Err(AcquisitionError::InvalidInput);
    }
    crate::private_fs::no_granting_acl(&file).map_err(|_| AcquisitionError::Storage)?;
    Ok(file)
}
pub(super) fn perform(
    repo: &Accounts,
    command: Command,
    input: &mut Reader<'_>,
    boundary: &mut Boundary,
) -> Result<&'static str, &'static str> {
    let transfer = repo.paths.config.join("recovery-transfer");
    let outgoing = transfer.join("outgoing");
    let inbox = transfer.join("incoming");
    for directory in [&transfer, &outgoing, &inbox] {
        crate::private_fs::prepare(directory).map_err(|_| "storage_unavailable")?;
    }
    if matches!(command, Command::Prepare) {
        return Ok("recovery_prepared");
    }
    let secret = key(input, matches!(&command, Command::Capture { .. }))?;
    let ledger =
        InstalledLedger::recovery_under(&repo.paths.data).map_err(|_| "storage_unavailable")?;
    let consent = Consent::new();
    let result = match command {
        Command::Prepare => unreachable!("preparation completed before key entry"),
        Command::Capture {
            accounts,
            ledger: include,
        } => repo
            .selected_capture_preview(&accounts, include, Some(&ledger), recovery::now())
            .and_then(|(preview, _)| {
                repo.selected_capture(&preview, &secret, &outgoing, Some(&ledger), |plan| {
                    consent.authorize(plan, &Review::Capture(&preview), &secret, boundary)
                })
            })
            .map(|_| "captured"),
        Command::Restore {
            bundle,
            ledger: restore,
        } => incoming(&inbox, &bundle)
            .and_then(|path| {
                let preview =
                    repo.selected_restore_preview_file(&path, &secret, restore, Some(&ledger))?;
                if preview.bundle != bundle {
                    return Err(AcquisitionError::Authentication);
                }
                repo.selected_restore_file(&path, &secret, &preview, Some(&ledger), |plan| {
                    consent.authorize(plan, &Review::Restore(&preview), &secret, boundary)
                })
            })
            .map(|_| "restored_read_only"),
        Command::Resume => repo
            .selected_resume_reviewed(&secret, Some(&ledger), |plan, preview| {
                consent.authorize(plan, &Review::Restore(&preview), &secret, boundary)
            })
            .map(|_| "restored_read_only"),
    };
    result.map_err(|e| match e {
        AcquisitionError::Cancelled => "cancelled",
        AcquisitionError::Credentials | AcquisitionError::CredentialMissing => "denied",
        AcquisitionError::Authentication => "authentication_failed",
        AcquisitionError::InvalidInput => "invalid_input",
        AcquisitionError::LifecyclePending => "recovery_required",
        _ => "storage_unavailable",
    })
}

#[cfg(test)]
mod tests;
