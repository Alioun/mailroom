use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Preview,
    SyntheticCensus,
    Connect,
    Census,
    UploadRules,
    ActivateRules,
    CreateFolder,
    MoveMessages,
    ChangeFlags,
    ChangeForwarding,
    SendMessage,
    Unsubscribe,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    SyntheticRead,
    AccountRead,
    AccountWrite,
}

#[derive(Serialize)]
pub struct Availability {
    pub operation: Operation,
    pub effect: Effect,
    pub allowed: bool,
    pub reason: &'static str,
}

pub const OPERATIONS: [Operation; 12] = [
    Operation::Preview,
    Operation::SyntheticCensus,
    Operation::Connect,
    Operation::Census,
    Operation::UploadRules,
    Operation::ActivateRules,
    Operation::CreateFolder,
    Operation::MoveMessages,
    Operation::ChangeFlags,
    Operation::ChangeForwarding,
    Operation::SendMessage,
    Operation::Unsubscribe,
];

pub fn authorize(operation: Operation) -> Availability {
    let effect = match operation {
        Operation::Preview | Operation::SyntheticCensus => Effect::SyntheticRead,
        Operation::Connect | Operation::Census => Effect::AccountRead,
        Operation::UploadRules
        | Operation::ActivateRules
        | Operation::CreateFolder
        | Operation::MoveMessages
        | Operation::ChangeFlags
        | Operation::ChangeForwarding
        | Operation::SendMessage
        | Operation::Unsubscribe => Effect::AccountWrite,
    };
    let (allowed, reason) = match effect {
        Effect::SyntheticRead => (true, "Bundled synthetic data only"),
        Effect::AccountRead => (true, "Owner-run TLS IMAP discovery and header reads only"),
        Effect::AccountWrite => (
            false,
            "Denied: all real-account mutations are locked in this unit",
        ),
    };
    Availability {
        operation,
        effect,
        allowed,
        reason,
    }
}

// Only this module issues the provider-bound read capability. HTTP bodies cannot deserialize it.
pub(crate) struct ReadPermit(crate::acquisition::Provider);
impl ReadPermit {
    pub(crate) fn provider(&self) -> crate::acquisition::Provider {
        self.0
    }
}
pub(crate) fn read_permit(
    provider: crate::acquisition::Provider,
) -> Result<ReadPermit, crate::acquisition::AcquisitionError> {
    if !authorize(Operation::Census).allowed {
        return Err(crate::acquisition::AcquisitionError::InvalidInput);
    }
    Ok(ReadPermit(provider))
}
