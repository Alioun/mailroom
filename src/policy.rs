use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Preview,
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

pub const OPERATIONS: [Operation; 11] = [
    Operation::Preview,
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
        Operation::Preview => Effect::SyntheticRead,
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
        Effect::SyntheticRead => (true, "Synthetic preview only"),
        Effect::AccountRead => (
            false,
            "Unavailable: no account adapters or credentials in this unit",
        ),
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
