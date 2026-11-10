mod accounts;
pub(crate) mod backlog;
#[cfg(test)]
pub(crate) mod fixture_peer;
pub(crate) mod folder_provisioning;
pub(crate) mod gmail_backlog;
mod imap;
mod managesieve;
mod scan;
mod sieve_discovery;
pub(crate) mod sieve_transaction;
pub(crate) mod synthetic;
pub(crate) mod workspace_authority;
use crate::census::CensusError;
#[cfg(target_os = "macos")]
pub(crate) use accounts::keychain;
pub use accounts::lifecycle;
pub(crate) use accounts::recovery;
pub use accounts::{
    Account, AccountPaths, Accounts, CredentialBackend, OsCredentials, Provider, SecretValue,
};
pub use backlog::compensation_checkpoints::{
    CopyCompensationSlot, CopyCompensationWork, DurableCompensationIntent,
    SavedCopyCompensationCheckpoints,
};
pub use backlog::compensation_review::{
    CopyCompensationActions, CopyCompensationObservation, CopyCompensationRequest,
    CopyCompensationReview,
};
pub(crate) use backlog::copy_continuation::CopyContinuationWork;
pub(crate) use backlog::copy_undo::{CopyUndoTarget, copy_undo_candidates};
pub use backlog::dependent_cleanup::{DependentCleanupRequest, DependentCleanupReview};
pub use scan::{Cancellation, ScanProgress};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionError {
    InvalidInput,
    Credentials,
    CredentialMissing,
    LifecyclePending,
    Storage,
    Tls,
    Authentication,
    Protocol,
    Incomplete,
    Cancelled,
    /// A key change refused because the account already keeps the most rollback copies.
    RollbackCopiesFull,
    /// The local web vault holds every credential and is not unlocked in this process.
    VaultLocked,
}
impl From<CensusError> for AcquisitionError {
    fn from(_: CensusError) -> Self {
        Self::Storage
    }
}
impl std::fmt::Display for AcquisitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "Invalid account input",
            Self::CredentialMissing => "Credential missing. Existing encrypted data was retained.",
            Self::LifecyclePending => "Local lifecycle recovery is required. Existing files were retained.",
            Self::Credentials => "Credential unavailable. Restore the existing credential; saved data was retained.",
            Self::Storage => "Encrypted state unavailable. Retain existing files and restore the original key.",
            Self::Tls => "Secure connection failed. Check connectivity and provider availability.",
            Self::Authentication => "Account authentication failed. Check the existing credential privately.",
            Self::Protocol | Self::Incomplete => "Provider response was incomplete or unsupported. Saved data was retained; retry later.",
            Self::Cancelled => "Census cancelled. Saved checkpoints remain available.",
            Self::VaultLocked => "The Mailroom vault is locked. Unlock it in the browser with the master password.",
            Self::RollbackCopiesFull => "This account already keeps 8 copies from interrupted key changes in one store directory, so a key change can neither start nor roll back. Review the account's .<account>.rollback folder in that store directory and delete it by hand, or forget the account, then retry.",
        })
    }
}
impl std::error::Error for AcquisitionError {}

#[cfg(test)]
pub(crate) use accounts::process_tests::persistent_accounts as persistent_test_accounts;
#[cfg(test)]
pub(crate) use imap::original::tests::{Logs as OriginalLogs, install as install_original_fake};
#[cfg(test)]
pub(crate) use imap::tests::install_provider_fixture;
#[cfg(test)]
pub(crate) use managesieve::tests::Fake as SieveFake;

/// The variant name of an error, without any payload it carries, for test
/// diagnostics at the point where a specific error collapses into `Protocol`.
#[cfg(test)]
pub(crate) fn diagnostic_code(error: &impl std::fmt::Debug) -> String {
    let text = format!("{error:?}");
    text.split(['(', ' ', '{'])
        .next()
        .unwrap_or_default()
        .to_owned()
}
