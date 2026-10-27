//! macOS credentials through the pinned `mailroom-keychain` helper.
//!
//! The helper owns every Security framework call. Mailbox passwords are stored with no
//! trusted application, so macOS asks the person at every read. Local census keys trust only
//! the pinned helper's cdhash. Secrets cross only the helper's private stdin and stdout pipes.
use super::{AcquisitionError, CredentialBackend, SecretValue};
use crate::native::{self, Helper, LaunchError};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// A person may need time to answer the system prompt.
const DEADLINE: Duration = Duration::from_secs(120);
const REPLY_LIMIT: usize = 32768;
const SECRET_LIMIT: usize = 8192;

pub(crate) trait Channel: Send + Sync {
    fn exchange(&self, input: &[u8]) -> Result<Zeroizing<Vec<u8>>, LaunchError>;
}
impl Channel for Helper {
    fn exchange(&self, input: &[u8]) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
        native::exchange(self, input, REPLY_LIMIT, DEADLINE)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Get,
    Put,
    Delete,
    Status,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Ok,
    Missing,
    Exists,
    Denied,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Protection {
    EveryAccess,
    PinnedHelper,
}

#[derive(Serialize)]
struct Request<'a> {
    op: Operation,
    keychain: Option<&'a str>,
    interactive: bool,
    reference: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<&'a str>,
    challenge: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply<'a> {
    status: Status,
    #[serde(borrow)]
    secret: Option<&'a str>,
    protection: Option<Protection>,
    authentication: &'a str,
}

fn name<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Must match `channelTag` in the Swift helper.
pub(crate) fn tag(
    challenge: &[u8; 32],
    op: Operation,
    reference: &str,
    status: Status,
    protection: Option<Protection>,
    secret: &[u8],
) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(challenge).expect("HMAC key length");
    let protection = protection.map(name).unwrap_or_default();
    mac.update(
        format!(
            "mailroom-keychain-v1\0{}\0{reference}\0{}\0{protection}\0",
            name(op),
            name(status)
        )
        .as_bytes(),
    );
    mac.update(
        data_encoding::HEXLOWER
            .encode(&Sha256::digest(secret))
            .as_bytes(),
    );
    data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
}

pub(crate) struct Keychain<C> {
    channel: C,
    /// None is the user's default keychain. Tests always name a throwaway keychain.
    keychain: Option<PathBuf>,
    interactive: bool,
}

/// The production backend, available only when the helper was pinned into this build.
pub(crate) fn installed() -> Result<Keychain<Helper>, AcquisitionError> {
    let helper =
        native::installed(native::Kind::Keychain).map_err(|_| AcquisitionError::Credentials)?;
    Ok(Keychain {
        channel: helper,
        keychain: None,
        interactive: true,
    })
}

struct Answer {
    status: Status,
    secret: Option<SecretValue>,
    #[cfg_attr(not(test), allow(dead_code))]
    protection: Option<Protection>,
}

impl<C: Channel> Keychain<C> {
    #[cfg(test)]
    pub(crate) fn new(channel: C, keychain: PathBuf) -> Self {
        Self {
            channel,
            keychain: Some(keychain),
            interactive: false,
        }
    }

    fn call(
        &self,
        op: Operation,
        reference: &str,
        secret: Option<&SecretValue>,
    ) -> Result<Answer, AcquisitionError> {
        let fail = AcquisitionError::Credentials;
        let mut challenge = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut challenge[..]);
        let encoded_challenge = data_encoding::BASE64.encode(&challenge[..]);
        let encoded_secret = match secret {
            Some(value) if value.bytes().is_empty() || value.bytes().len() > SECRET_LIMIT => {
                return Err(fail);
            }
            Some(value) => Some(Zeroizing::new(data_encoding::BASE64.encode(value.bytes()))),
            None => None,
        };
        let keychain = match &self.keychain {
            Some(path) => Some(path.to_str().ok_or(AcquisitionError::Credentials)?),
            None => None,
        };
        let request = Request {
            op,
            keychain,
            interactive: self.interactive,
            reference,
            secret: encoded_secret.as_deref().map(String::as_str),
            challenge: &encoded_challenge,
        };
        let capacity = encoded_secret.as_ref().map_or(0, |s| s.len())
            + reference.len()
            + keychain.map_or(0, |k| k.len() * 6)
            + 512;
        let mut input = Zeroizing::new(Vec::with_capacity(capacity));
        serde_json::to_writer(&mut *input, &request).map_err(|_| AcquisitionError::Credentials)?;
        let output = self
            .channel
            .exchange(&input)
            .map_err(|_| AcquisitionError::Credentials)?;
        let reply: Reply =
            serde_json::from_slice(&output).map_err(|_| AcquisitionError::Credentials)?;
        let value = match reply.secret {
            Some(text) => Some(Zeroizing::new(
                data_encoding::BASE64
                    .decode(text.as_bytes())
                    .map_err(|_| AcquisitionError::Credentials)?,
            )),
            None => None,
        };
        let expected = tag(
            &challenge,
            op,
            reference,
            reply.status,
            reply.protection,
            value.as_deref().map_or(&[][..], Vec::as_slice),
        );
        if !bool::from(expected.as_bytes().ct_eq(reply.authentication.as_bytes()))
            || (value.is_some() != (op == Operation::Get && reply.status == Status::Ok))
        {
            return Err(fail);
        }
        Ok(Answer {
            status: reply.status,
            secret: value.map(|v| SecretValue::new(v.to_vec())),
            protection: reply.protection,
        })
    }

    #[cfg(test)]
    pub(crate) fn protection(
        &self,
        reference: &str,
    ) -> Result<Option<Protection>, AcquisitionError> {
        let answer = self.call(Operation::Status, reference, None)?;
        match answer.status {
            Status::Ok => Ok(answer.protection),
            Status::Missing => Err(AcquisitionError::CredentialMissing),
            _ => Err(AcquisitionError::Credentials),
        }
    }
}

impl<C: Channel> CredentialBackend for Keychain<C> {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        let answer = self.call(Operation::Get, reference, None)?;
        match (answer.status, answer.secret) {
            (Status::Ok, Some(secret)) => Ok(secret),
            (Status::Missing, _) => Err(AcquisitionError::CredentialMissing),
            _ => Err(AcquisitionError::Credentials),
        }
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        match self.call(Operation::Put, reference, Some(value))?.status {
            Status::Ok => Ok(()),
            _ => Err(AcquisitionError::Credentials),
        }
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        match self.call(Operation::Delete, reference, None)?.status {
            Status::Ok | Status::Missing => Ok(()),
            _ => Err(AcquisitionError::Credentials),
        }
    }
}

#[cfg(test)]
mod tests;
