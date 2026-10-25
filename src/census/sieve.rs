use super::{AccountId, CensusError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Cancelled,
    Deadline,
    Connect,
    Tls,
    Unsupported,
    Protocol,
    Refused,
    CredentialUnavailable,
    Quota,
    Missing,
    Referral,
    Bye,
    Eof,
    Limit,
    Unstable,
}

pub type Capabilities = BTreeMap<String, String>;
pub const ADAPTER: &str = "managesieve-read-v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptName {
    pub name: String,
    pub active: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub name: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}
impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Body([PRIVATE])")
    }
}
impl Body {
    pub(crate) fn new(name: String, bytes: Vec<u8>) -> Self {
        let sha256 = data_encoding::HEXLOWER.encode(&Sha256::digest(&bytes));
        Self {
            name,
            bytes,
            sha256,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub account: AccountId,
    pub provider: crate::acquisition::Provider,
    pub profile: String,
    pub source_epoch: String,
    pub tls_identity: String,
    pub authority: String,
    pub adapter: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Running,
    Complete,
    Failed,
    Cancelled,
    Interrupted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub context: Context,
    pub outcome: Outcome,
    pub failure: Option<Failure>,
    pub capabilities: Option<Capabilities>,
    pub names: Option<Vec<ScriptName>>,
    pub bodies: Vec<Body>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub attempt: String,
    pub observed_at: u64,
    pub context: Context,
    pub capabilities: Capabilities,
    pub names: Vec<ScriptName>,
    pub bodies: Vec<Body>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub version: u32,
    pub latest: Option<Attempt>,
    pub last_successful: Option<Baseline>,
}
impl Facts {
    pub(crate) fn interrupted(&mut self) {
        if let Some(a) = &mut self.latest {
            if a.outcome == Outcome::Running {
                a.outcome = Outcome::Interrupted;
            }
        }
    }
    pub fn current(&self, context: Option<&Context>, recovery_hold: bool) -> bool {
        if recovery_hold {
            return false;
        }
        matches!((&self.latest, &self.last_successful, context), (Some(a), Some(b), Some(c)) if a.outcome == Outcome::Complete && a.id == b.attempt && a.context == *c && b.context == *c)
    }
    pub(crate) fn validate(&self, account: &AccountId) -> Result<(), CensusError> {
        let fail = CensusError::Storage;
        if self == &Self::default() {
            return Ok(());
        }
        if self.version != 1 || self.latest.is_none() {
            return Err(fail);
        }
        let a = self.latest.as_ref().unwrap();
        validate_context(&a.context, account)?;
        if a.id.is_empty() || a.id.len() > 64 || a.started_at == 0 {
            return Err(fail);
        }
        let shape_valid = match a.outcome {
            Outcome::Running | Outcome::Interrupted => {
                a.finished_at.is_none() && a.failure.is_none()
            }
            Outcome::Complete => a.finished_at.is_some() && a.failure.is_none(),
            Outcome::Failed => {
                a.finished_at.is_some() && a.failure.is_some_and(|e| e != Failure::Cancelled)
            }
            Outcome::Cancelled => a.finished_at.is_some() && a.failure == Some(Failure::Cancelled),
        };
        if !shape_valid || a.finished_at.is_some_and(|end| end < a.started_at) {
            return Err(CensusError::Storage);
        }
        validate_observation(a.capabilities.as_ref(), a.names.as_deref(), &a.bodies)?;
        if let Some(b) = &self.last_successful {
            validate_context(&b.context, account)?;
            if b.attempt.is_empty() || b.attempt.len() > 64 || b.observed_at == 0 {
                return Err(fail);
            }
            validate_observation(Some(&b.capabilities), Some(&b.names), &b.bodies)?;
            let required: BTreeSet<_> = b
                .names
                .iter()
                .filter(|n| n.active || n.name == "Open-Xchange")
                .map(|n| n.name.as_str())
                .collect();
            if required != b.bodies.iter().map(|b| b.name.as_str()).collect() {
                return Err(fail);
            }
        }
        if a.outcome == Outcome::Complete {
            let b = self.last_successful.as_ref().ok_or(CensusError::Storage)?;
            if a.id != b.attempt
                || a.context != b.context
                || a.capabilities.as_ref() != Some(&b.capabilities)
                || a.names.as_ref() != Some(&b.names)
                || a.bodies != b.bodies
            {
                return Err(fail);
            }
        }
        Ok(())
    }
}
fn validate_context(c: &Context, account: &AccountId) -> Result<(), CensusError> {
    if c.account != *account
        || c.adapter != ADAPTER
        || c.profile != "installed-loopback-starttls-plain"
        || [&c.source_epoch, &c.tls_identity, &c.authority]
            .iter()
            .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err(CensusError::Storage);
    }
    Ok(())
}
fn validate_observation(
    caps: Option<&Capabilities>,
    names: Option<&[ScriptName]>,
    bodies: &[Body],
) -> Result<(), CensusError> {
    if let Some(caps) = caps {
        if !caps.contains_key("IMPLEMENTATION")
            || !caps.contains_key("SIEVE")
            || caps.get("VERSION").map(String::as_str) != Some("1.0")
        {
            return Err(CensusError::Storage);
        }
        if caps.len() > 256
            || caps.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || !k.bytes().all(|b| b.is_ascii_graphic())
                    || k != &k.to_ascii_uppercase()
                    || v.len() > 8192
            })
            || serde_json::to_vec(caps)
                .map_err(|_| CensusError::Storage)?
                .len()
                > 128 * 1024
        {
            return Err(CensusError::Storage);
        }
    }
    if let Some(names) = names {
        let unique: BTreeSet<_> = names.iter().map(|s| &s.name).collect();
        if caps.is_none()
            || names.len() > 256
            || unique.len() != names.len()
            || names.iter().filter(|s| s.active).count() > 1
            || names.iter().any(|s| !valid_script_name(&s.name))
        {
            return Err(CensusError::Storage);
        }
    }
    let unique: BTreeSet<_> = bodies.iter().map(|b| &b.name).collect();
    if bodies.len() > 2 || unique.len() != bodies.len() {
        return Err(CensusError::Storage);
    }
    for b in bodies {
        if b.bytes.len() > 1024 * 1024
            || b.sha256 != data_encoding::HEXLOWER.encode(&Sha256::digest(&b.bytes))
            || !names.is_some_and(|ns| {
                ns.iter()
                    .any(|n| n.name == b.name && (n.active || n.name == "Open-Xchange"))
            })
        {
            return Err(CensusError::Storage);
        }
    }
    Ok(())
}

pub(crate) fn valid_script_name(value: &str) -> bool {
    use icu_properties::{CodePointMapData, props::GeneralCategory};
    let categories = CodePointMapData::<GeneralCategory>::new();
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('\u{feff}')
        && !value.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{2028}' | '\u{2029}')
                || categories.get(c) == GeneralCategory::Unassigned
        })
}
