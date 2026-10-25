use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Recipient(String);
impl TryFrom<String> for Recipient {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if !crate::rules::config::mailbox(&value) {
            return Err("Unsupported delivery mailbox".into());
        }
        Ok(Self(value))
    }
}
impl From<Recipient> for String {
    fn from(value: Recipient) -> Self {
        value.0
    }
}
impl Recipient {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub account: String,
    pub provider: String,
    pub identity: String,
    pub profile: String,
    pub epoch: String,
    pub authority: String,
}
impl Source {
    pub fn validate(&self) -> bool {
        [
            &self.account,
            &self.provider,
            &self.identity,
            &self.profile,
            &self.epoch,
            &self.authority,
        ]
        .iter()
        .all(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryEvidence {
    #[default]
    Unavailable,
    Unsupported,
    Ambiguous,
    Known {
        recipient: Recipient,
        source: Source,
        message: String,
    },
}
impl DeliveryEvidence {
    pub fn validate(&self) -> bool {
        match self {
            Self::Known {
                source, message, ..
            } => {
                source.validate()
                    && !message.is_empty()
                    && message.len() <= 1024
                    && !message.chars().any(char::is_control)
            }
            _ => true,
        }
    }
    pub fn resolve<'a>(
        &'a self,
        installed: Option<&Source>,
        message: &str,
    ) -> (Option<&'a str>, &'static str) {
        match self {
            Self::Known {
                recipient,
                source,
                message: bound,
            } if source.validate() && installed == Some(source) && bound == message => {
                (Some(recipient.as_str()), "known")
            }
            Self::Known { .. } => (None, "incompatible"),
            Self::Unavailable => (None, "unavailable"),
            Self::Unsupported => (None, "unsupported"),
            Self::Ambiguous => (None, "ambiguous"),
        }
    }
    pub fn message_binding(id: &str, hash: Option<&str>) -> String {
        format!("{id}/{}", hash.unwrap_or("no-message-id"))
    }
}
