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

#[derive(Clone, Copy, Serialize)]
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
    authorize_with_mode(operation, false)
}

pub fn authorize_with_mode(operation: Operation, live_writes: bool) -> Availability {
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
        Effect::AccountWrite if live_writes => (
            true,
            "Live provider writes armed for this session; exact preview and confirmation required",
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

/// Proof that one provider write is authorized. Only this module issues it, and every real
/// send, one-click and mail-app adapter requires it, so a route cannot reach one while
/// `authorize` denies the operation.
pub(crate) struct WritePermit(Operation);
impl WritePermit {
    pub(crate) fn operation(&self) -> Operation {
        self.0
    }
}
/// Issues a write permit only when `authorize` allows `operation` as an account write.
#[cfg(test)]
pub(crate) fn write_permit(operation: Operation) -> Result<WritePermit, &'static str> {
    write_permit_with_mode(operation, false)
}
/// Issues a write permit when `live_writes` has been explicitly armed for the running session.
pub(crate) fn write_permit_with_mode(
    operation: Operation,
    live_writes: bool,
) -> Result<WritePermit, &'static str> {
    let availability = authorize_with_mode(operation, live_writes);
    match availability.effect {
        Effect::AccountWrite if availability.allowed => Ok(WritePermit(operation)),
        Effect::AccountWrite => Err(availability.reason),
        _ => Err("Not an account write"),
    }
}
/// Names the test-only authorization, so a release binary can be checked for its absence.
#[cfg(test)]
pub(crate) const FAKE_WRITES: &str = "mailroom-test-only-fake-endpoint-write-authorization";
/// Authorizes writes to fake endpoints that a test installed. It is compiled only into test
/// builds, and it refuses any endpoint off loopback.
#[cfg(test)]
pub(crate) fn fake_write_permit(
    operation: Operation,
    endpoints: &[std::net::SocketAddr],
) -> Result<WritePermit, &'static str> {
    if !matches!(authorize(operation).effect, Effect::AccountWrite)
        || endpoints.is_empty()
        || endpoints.iter().any(|e| !e.ip().is_loopback())
    {
        return Err(FAKE_WRITES);
    }
    Ok(WritePermit(operation))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_issues_no_write_permit_and_fakes_must_be_loopback() {
        let loopback: std::net::SocketAddr = "127.0.0.1:2525".parse().unwrap();
        let public: std::net::SocketAddr = "93.184.216.34:465".parse().unwrap();
        for operation in OPERATIONS {
            let write = matches!(authorize(operation).effect, Effect::AccountWrite);
            assert!(write_permit(operation).is_err(), "{operation:?}");
            if write {
                assert_eq!(
                    write_permit(operation).err(),
                    Some("Denied: all real-account mutations are locked in this unit")
                );
            }
            assert_eq!(fake_write_permit(operation, &[loopback]).is_ok(), write);
            assert!(fake_write_permit(operation, &[loopback, public]).is_err());
            assert!(fake_write_permit(operation, &[]).is_err());
        }
    }
}
