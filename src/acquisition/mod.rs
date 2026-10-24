mod accounts;
mod imap;
mod scan;
pub(crate) mod synthetic;
use crate::census::CensusError;
pub use accounts::{
    Account, AccountPaths, Accounts, CredentialBackend, OsCredentials, Provider, SecretValue,
};
pub use scan::{Cancellation, ScanProgress};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionError {
    InvalidInput,
    Credentials,
    Storage,
    Tls,
    Authentication,
    Protocol,
    Incomplete,
    Cancelled,
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
            Self::Credentials => "Credential unavailable. Restore the existing credential; saved data was retained.",
            Self::Storage => "Encrypted state unavailable. Retain existing files and restore the original key.",
            Self::Tls => "Secure connection failed. Check connectivity and provider availability.",
            Self::Authentication => "Account authentication failed. Check the existing credential privately.",
            Self::Protocol | Self::Incomplete => "Provider response was incomplete or unsupported. Saved data was retained; retry later.",
            Self::Cancelled => "Census cancelled. Saved checkpoints remain available.",
        })
    }
}
impl std::error::Error for AcquisitionError {}
