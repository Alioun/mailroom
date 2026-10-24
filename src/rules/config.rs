use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub id: String,
    pub major: u32,
    #[serde(default)]
    pub renamed: BTreeMap<String, String>,
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    pub preset: String,
    pub major: u32,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub roles: Roles,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub additions: Vec<Rule>,
    #[serde(default)]
    pub ledger: Vec<LedgerEntry>,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
    #[serde(default)]
    pub forwards: Vec<Forward>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub language: Language,
    pub personal_root: String,
    pub business_root: String,
    pub new_addresses: String,
    pub retired_addresses: String,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            language: Language::En,
            personal_root: "Personal".into(),
            business_root: "Business".into(),
            new_addresses: "New addresses".into(),
            retired_addresses: "Retired addresses".into(),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    En,
    De,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    #[serde(default)]
    pub addresses: BTreeMap<String, Role>,
    #[serde(default)]
    pub domains: BTreeMap<String, Role>,
    #[serde(default)]
    pub signup_domains: BTreeSet<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Personal,
    Business,
    Unfiltered,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub category: Category,
    pub stage: Stage,
    pub when: Vec<Condition>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub accounts: BTreeSet<String>,
    #[serde(default)]
    pub retain: bool,
    #[serde(default)]
    pub mark_read: bool,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Money,
    Contracts,
    Orders,
    Travel,
    Social,
    Newsletters,
}
impl Category {
    pub fn label(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Money, Language::En) => "Money",
            (Self::Money, Language::De) => "Finanzen",
            (Self::Contracts, Language::En) => "Contracts",
            (Self::Contracts, Language::De) => "Verträge",
            (Self::Orders, Language::En) => "Orders",
            (Self::Orders, Language::De) => "Bestellungen",
            (Self::Travel, Language::En) => "Travel",
            (Self::Travel, Language::De) => "Reisen",
            (Self::Social, Language::En) => "Social",
            (Self::Social, Language::De) => "Soziale Netzwerke",
            (Self::Newsletters, Language::En) => "Newsletters",
            (Self::Newsletters, Language::De) => "Newsletter",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Guard,
    From,
    Subject,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    FromAddress { value: String },
    FromDomain { value: String },
    ToAddress { value: String },
    SubjectContains { value: String },
    ListIdExists,
    OneClick,
    NonBulk,
    Attachment,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub id: String,
    pub enabled: Option<bool>,
    pub folder: Option<String>,
    pub when: Option<Vec<Condition>>,
    #[serde(default)]
    pub extend: Vec<Condition>,
    pub accounts: Option<BTreeSet<String>>,
    pub mark_read: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntry {
    pub address: String,
    pub status: LedgerStatus,
    pub category: Category,
    pub folder: Option<String>,
    #[serde(default)]
    pub invoice: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LedgerStatus {
    Active,
    Retired,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub address: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub id: String,
    pub target: String,
    pub trigger: ForwardTrigger,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub accounts: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForwardTrigger {
    InvoiceAddress,
    ApprovedSenders { addresses: BTreeSet<String> },
    Keywords { words: Vec<String> },
}

pub fn documents(preset: &str, overrides: &str) -> Result<(Preset, Overrides), String> {
    fn parse<T: serde::de::DeserializeOwned>(source: &str) -> Result<T, String> {
        if source.len() > 64 * 1024 {
            return Err("Rule document exceeds 64 KiB".into());
        }
        let json: serde_json::Value =
            serde_yaml_ng::from_str(source).map_err(|_| "Invalid YAML rule document")?;
        serde_json::from_value(json)
            .map_err(|_| "Rule document does not match the common JSON model".into())
    }
    Ok((parse(preset)?, parse(overrides)?))
}

pub fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 96
        || value.split('.').any(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    {
        return Err(
            "Stable IDs need nonempty dot-separated letters, digits, hyphens or underscores".into(),
        );
    }
    Ok(())
}
pub fn segment(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || value.trim() != value
        || value.chars().any(|c| c.is_control() || c == '/')
        || matches!(value, "." | "..")
        || value.eq_ignore_ascii_case("inbox")
    {
        return Err(
            "Folder names need one nonempty Unicode segment; paths have at most two levels".into(),
        );
    }
    Ok(())
}
pub fn domain(value: &str) -> bool {
    value.len() <= 253
        && value.contains('.')
        && value.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
pub fn mailbox(value: &str) -> bool {
    value.split_once('@').is_some_and(|(local, host)| {
        !local.is_empty()
            && local.len() <= 64
            && !local.starts_with('.')
            && !local.ends_with('.')
            && !local.contains("..")
            && local
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            && domain(host)
    })
}
pub fn validate_rule(rule: &Rule) -> Result<(), String> {
    identifier(&rule.id)?;
    if rule.id.starts_with("system.") {
        return Err("The system ID namespace is reserved".into());
    }
    if let Some(folder) = &rule.folder {
        segment(folder)?;
    }
    if rule.when.is_empty() || rule.when.len() > 16 {
        return Err("Each rule needs 1..16 AND conditions".into());
    }
    for account in &rule.accounts {
        identifier(account)?;
    }
    for condition in &rule.when {
        match condition {
            Condition::FromAddress { value } | Condition::ToAddress { value }
                if !mailbox(value) =>
            {
                return Err("Invalid rule mailbox".into());
            }
            Condition::FromDomain { value } if !domain(value) => {
                return Err("Invalid rule domain; use lowercase ASCII IDNA".into());
            }
            Condition::SubjectContains { value }
                if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) =>
            {
                return Err("Invalid Subject condition".into());
            }
            _ => (),
        }
    }
    if rule.stage == Stage::Guard && (!rule.retain || rule.mark_read) {
        return Err("Guards must retain unread state in Inbox".into());
    }
    Ok(())
}
