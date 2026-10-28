use super::*;
use crate::{
    acquisition::lifecycle::LifecycleState,
    confirmation::{Boundary, ExactAction, Plan},
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Forget,
    Rotate,
    Recover,
}
pub(super) struct Command {
    pub operation: Operation,
    pub account: AccountId,
}
pub(super) fn parse(tail: &[&str]) -> Option<Command> {
    let [verb, id] = tail else {
        return None;
    };
    let operation = match *verb {
        "forget" => Operation::Forget,
        "rotate-key" => Operation::Rotate,
        "recover-local" => Operation::Recover,
        _ => return None,
    };
    Some(Command {
        operation,
        account: AccountId::new(id).ok()?,
    })
}
fn plan(id: &AccountId, operation: &str, data: &[u8], consequences: &str) -> Plan {
    let mut digest = Sha256::new();
    digest.update(b"mailroom/local-maintenance-consent/v1\0");
    for value in [
        id.as_str().as_bytes(),
        operation.as_bytes(),
        data,
        consequences.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    let receipt = data_encoding::HEXLOWER.encode(&digest.finalize());
    Plan {
        exact_receipt: receipt.clone(),
        actions: vec![ExactAction::Recovery {
            account: id.as_str().into(),
            operation: operation.into(),
            manifest_digest: receipt,
            consequences: consequences.into(),
        }],
    }
}
fn consent(
    id: &AccountId,
    operation: &str,
    data: &[u8],
    consequences: &str,
    boundary: &mut Boundary,
) -> Result<(), AcquisitionError> {
    let current = plan(id, operation, data, consequences);
    let (handle, _) = boundary
        .offer(current.clone())
        .map_err(|_| AcquisitionError::Credentials)?;
    boundary
        .confirm(&handle, &current, || {
            Ok(plan(id, operation, data, consequences))
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
fn identity(account: &Account) -> String {
    format!(
        "For {} at {}.",
        account.login,
        match account.provider {
            crate::acquisition::Provider::Gmail => "Gmail",
            crate::acquisition::Provider::MailboxOrg => "mailbox.org",
        }
    )
}
fn pending_consequences(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Creating => {
            "Reconcile interrupted local creation. Verify a published account, or delete recorded unpublished local data and credentials. No provider connection, send or mailbox change."
        }
        LifecycleState::Replacing => {
            "Reconcile interrupted password replacement. Keep the selected password reference and retire only the unused recorded reference. Saved local history stays. No provider connection."
        }
        LifecycleState::Forgetting => {
            "Finish the recorded local deletion, including credentials, saved data, decisions, undo and reconciliation history. Shared addresses.yaml and other accounts stay. No remote undo or account shutdown. Exported copies and backups remain outside this deletion."
        }
        LifecycleState::Rotating => {
            "Reconcile interrupted local key rotation. Verify the selected generation and finish retiring old resources, or roll back an unpublished generation while retaining its files in the account rollback folder. No provider connection or operation resumption."
        }
        LifecycleState::Forgotten => "No unfinished local operation remains.",
    }
}
pub(super) fn perform(
    repo: &Accounts,
    command: Command,
    boundary: &mut Boundary,
) -> Result<&'static str, &'static str> {
    let effects = repo.paths.data.join("mailboxes");
    for directory in [
        &effects,
        &effects.join("invoice-transport"),
        &effects.join("unsubscribe-transport"),
    ] {
        crate::private_fs::prepare(directory).map_err(|_| "storage_unavailable")?;
    }
    let id = &command.account;
    let result = match command.operation {
        Operation::Forget => repo.forget_reviewed(id, &effects, |account, preview| {
            let bytes = Zeroizing::new(serde_json::to_vec(&(account,preview)).map_err(|_| AcquisitionError::Storage)?);
            consent(id, "Forget this local account", &bytes, &format!("{} {} Local preview covers {} recorded resources and {} bytes. All deletion is local.",identity(account),preview.consequences.en, preview.resources.len(), preview.resources.iter().map(|r|r.bytes).sum::<u64>()), boundary)
        }).map(|_| "forgotten"),
        Operation::Rotate => repo.rotate_reviewed(id, &effects, |account, revision, resources| {
            let bytes = Zeroizing::new(serde_json::to_vec(&(account,revision,resources)).map_err(|_| AcquisitionError::Storage)?);
            consent(id, "Rotate local encryption keys", &bytes, &format!("{} Preserve local rules, decisions, undo, reconciliation history and stable operation identities. Outstanding approvals become invalid. Verify the new generation before retiring the previous keys. Restored-account holds stay. No provider connection or mailbox change.",identity(account)), boundary)
        }).map(|_| "rotated"),
        Operation::Recover => repo.recover_reviewed(id, &effects, |account, state, data| consent(id, "Reconcile the recorded local lifecycle operation", data, &format!("{} {}",identity(account),pending_consequences(state)), boundary)).map(|_| "recovered"),
    };
    result.map_err(|e| match e {
        AcquisitionError::Cancelled => "cancelled",
        AcquisitionError::Credentials | AcquisitionError::CredentialMissing => "denied",
        AcquisitionError::InvalidInput => "invalid_input",
        AcquisitionError::LifecyclePending => "recovery_required",
        AcquisitionError::RollbackCopiesFull => "rollback_copies_full",
        _ => "storage_unavailable",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_maintenance_exact_plans_reject_changed_sources_actions_or_consequences() {
        let id = AccountId::new("invented-account").unwrap();
        let original = plan(
            &id,
            "Rotate local encryption keys",
            b"private-dummy-version-1",
            "Keep history",
        );
        for changed in [
            plan(
                &id,
                "Rotate local encryption keys",
                b"private-dummy-version-2",
                "Keep history",
            ),
            plan(
                &id,
                "Forget this local account",
                b"private-dummy-version-1",
                "Keep history",
            ),
            plan(
                &id,
                "Rotate local encryption keys",
                b"private-dummy-version-1",
                "Discard history",
            ),
            plan(
                &AccountId::new("other-account").unwrap(),
                "Rotate local encryption keys",
                b"private-dummy-version-1",
                "Keep history",
            ),
        ] {
            assert!(original.exact_receipt != changed.exact_receipt);
            let mut boundary = Boundary::synthetic();
            let (handle, _) = boundary.offer(original.clone()).unwrap();
            assert!(
                boundary
                    .confirm(&handle, &original, || Ok(changed))
                    .is_err()
            );
            assert!(
                boundary
                    .confirm(&handle, &original, || Ok(original.clone()))
                    .is_err()
            );
        }
    }
}
