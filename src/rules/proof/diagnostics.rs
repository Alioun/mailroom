use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    DeliveryTagLimits,
    SyntheticRecipientLimits,
    AttachmentAndDuplicateLimits,
    FolderTopologyUnverified,
    PartialCensus,
    InvalidDates,
    RulesWithoutSelections,
    ShadowedRules,
    StalePins,
}

impl Notice {
    pub fn message(self) -> &'static str {
        match self {
            Self::DeliveryTagLimits => {
                "Delivery tags label a matched ingress address, not authenticated Gmail origin. Sieve starts with empty accumulated flags. Observed IMAP flags are separate; permanent keyword storage is unverified. A verified matching envelope route can add its keyword while a recipient-role hold keeps the message in Inbox without read changes or accounting copies."
            }
            Self::SyntheticRecipientLimits => {
                "Recipient safety accepts exactly one verified synthetic delivery recipient. Multiple recipients retain Inbox for review, including same-role recipients. Real provider provenance remains unknown."
            }
            Self::AttachmentAndDuplicateLimits => {
                "Header census proves no attachment presence or accounting acceptance. Duplicate state starts empty for each proof and has no historical provider cache."
            }
            Self::FolderTopologyUnverified => {
                "Folder paths use a logical slash separator. Actual provider separator, existing folders and rule coexistence remain unverified."
            }
            Self::PartialCensus => "Census is partial; counts cover saved records only",
            Self::InvalidDates => {
                "Missing, invalid or future Date values are excluded from 13-month counts"
            }
            Self::RulesWithoutSelections => {
                "Some enabled rules match no selected messages in 13 months"
            }
            Self::ShadowedRules => "Some rules are fully shadowed by earlier compiled rules",
            Self::StalePins => "Some census pins are stale",
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCheck {
    NotRun,
}

impl RuntimeCheck {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotRun => {
                "Not run by this proof. Run scripts/verify-sieve.sh on synthetic artifacts. This does not establish provider behavior."
            }
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Diagnostics {
    pub notices: Vec<Notice>,
    pub runtime_cross_check: RuntimeCheck,
}

#[cfg(test)]
mod tests;
