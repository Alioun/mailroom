use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub preset: Preset,
    pub overrides: Overrides,
    pub messages: Vec<Fixture>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub id: String,
    pub major: u32,
    pub rules: Vec<Rule>,
    pub copies: Vec<CopyRule>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    pub preset: String,
    pub major: u32,
    pub patches: Vec<Patch>,
    pub additions: Vec<Rule>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub id: String,
    pub enabled: Option<bool>,
    pub destination: Option<Destination>,
    pub conditions: Option<Vec<Condition>>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub stage: Stage,
    pub conditions: Vec<Condition>,
    pub destination: Destination,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Guard,
    Bulk,
    From,
    Subject,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRule {
    pub id: String,
    pub conditions: Vec<Condition>,
    pub recipient: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Destination {
    Inbox,
    Folder { name: String },
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    FromAddress { value: String },
    FromDomain { value: String },
    SubjectContains { value: String },
    ListIdExists,
    OneClick,
    SpamFlag,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub headers: String,
}

struct Message {
    id: String,
    headers: BTreeMap<String, String>,
}

struct FilingRule {
    rule: Rule,
    priority: u8,
    reason: String,
}

pub struct Program {
    filing: Vec<FilingRule>,
    copies: Vec<CopyRule>,
    trace: Vec<String>,
}

#[derive(Serialize)]
pub struct Preview {
    pub source: &'static str,
    pub semantics: &'static str,
    pub counts: Counts,
    pub decisions: Vec<Decision>,
    pub trace: Vec<String>,
    pub sieve: String,
    pub omissions: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct Counts {
    pub messages: usize,
    pub filed: usize,
    pub inbox: usize,
    pub copies: usize,
    pub copied_messages: usize,
}

#[derive(Serialize)]
pub struct Decision {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub destination: Destination,
    pub matched_rule: Option<String>,
    pub reason: String,
    pub copies: Vec<CopyDecision>,
}

#[derive(Serialize)]
pub struct CopyDecision {
    pub rule: String,
    pub recipient: String,
}

fn text(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 200 || !value.bytes().all(|b| (32..127).contains(&b)) {
        return Err(format!("{label} must be 1..200 printable ASCII characters"));
    }
    Ok(())
}

fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("IDs must contain 1..64 letters, digits, hyphens or underscores".into());
    }
    Ok(())
}

fn domain(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let reserved = [
        "example.com",
        "example.net",
        "example.org",
        "test",
        "invalid",
    ];
    value.len() <= 253
        && value.split('.').all(|p| {
            !p.is_empty()
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && reserved
            .iter()
            .any(|d| lower == *d || lower.ends_with(&format!(".{d}")))
}

fn mailbox(value: &str) -> bool {
    let Some((local, host)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        && domain(host)
}

fn destination(value: &Destination) -> Result<(), String> {
    if let Destination::Folder { name } = value {
        text(name, "Folder")?;
        if name.eq_ignore_ascii_case("inbox")
            || name
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(
                "Use the Inbox destination for retention; folder paths need nonempty segments"
                    .into(),
            );
        }
    }
    Ok(())
}

fn conditions(values: &[Condition]) -> Result<(), String> {
    if values.is_empty() || values.len() > 16 {
        return Err("Each rule needs 1..16 AND conditions".into());
    }
    for condition in values {
        match condition {
            Condition::FromAddress { value } if !mailbox(value) => {
                return Err(
                    "From address must be a bare synthetic mailbox in a reserved example domain"
                        .into(),
                );
            }
            Condition::FromDomain { value } if !domain(value) => {
                return Err("From domain must be a reserved example domain".into());
            }
            Condition::SubjectContains { value } => {
                text(value, "Subject condition")?;
                if value.contains("=?") {
                    return Err("Encoded words are unsupported".into());
                }
            }
            _ => (),
        }
    }
    Ok(())
}

fn validate_rule(rule: &Rule) -> Result<(), String> {
    identifier(&rule.id)?;
    destination(&rule.destination)?;
    conditions(&rule.conditions)?;
    let suitable = rule.conditions.iter().any(|c| {
        matches!(
            (rule.stage, c),
            (Stage::Guard, Condition::SpamFlag)
                | (Stage::Bulk, Condition::ListIdExists | Condition::OneClick)
                | (
                    Stage::From,
                    Condition::FromAddress { .. } | Condition::FromDomain { .. }
                )
                | (Stage::Subject, Condition::SubjectContains { .. })
        )
    });
    if !suitable {
        return Err(format!("Rule {} has no condition for its stage", rule.id));
    }
    Ok(())
}

pub fn compile(input: &PreviewInput) -> Result<Program, String> {
    identifier(&input.preset.id)?;
    if input.preset.major == 0
        || input.overrides.preset != input.preset.id
        || input.overrides.major != input.preset.major
    {
        return Err("Overrides must pin the supplied preset ID and nonzero major version".into());
    }
    if input.preset.rules.len() + input.overrides.additions.len() + input.preset.copies.len() > 100
    {
        return Err("This unit supports at most 100 rules".into());
    }
    let mut ids = HashSet::new();
    let mut filing = Vec::new();
    let mut trace = vec![format!("Preset {}@{}", input.preset.id, input.preset.major)];
    for rule in &input.preset.rules {
        validate_rule(rule)?;
        if !ids.insert(rule.id.clone()) {
            return Err(format!("Duplicate ID {}", rule.id));
        }
        let priority = match rule.stage {
            Stage::Guard => 0,
            Stage::Bulk => 2,
            Stage::From => 3,
            Stage::Subject => 4,
        };
        filing.push(FilingRule {
            rule: rule.clone(),
            priority,
            reason: "preset".into(),
        });
    }
    let mut patched = HashSet::new();
    for patch in &input.overrides.patches {
        if !patched.insert(&patch.id) {
            return Err(format!("Duplicate patch {}", patch.id));
        }
        let Some(target) = filing.iter_mut().find(|r| r.rule.id == patch.id) else {
            return Err(format!("Unknown override ID {}", patch.id));
        };
        if patch.enabled.is_none() && patch.destination.is_none() && patch.conditions.is_none() {
            return Err("An override patch must change something".into());
        }
        if let Some(dest) = &patch.destination {
            target.rule.destination = dest.clone();
        }
        if let Some(when) = &patch.conditions {
            target.rule.conditions = when.clone();
        }
        validate_rule(&target.rule)?;
        target.reason = "preset patched by override".into();
        target.priority = if target.rule.stage == Stage::Guard {
            0
        } else {
            1
        };
        trace.push(format!(
            "Override {}: {}",
            patch.id,
            if patch.enabled == Some(false) {
                "disabled"
            } else {
                "patched; before bulk, From and Subject"
            }
        ));
    }
    filing.retain(|r| {
        !input
            .overrides
            .patches
            .iter()
            .any(|p| p.id == r.rule.id && p.enabled == Some(false))
    });
    for rule in &input.overrides.additions {
        validate_rule(rule)?;
        if rule.stage == Stage::Guard {
            return Err("New guard rules belong in the preset, not overrides".into());
        }
        if !ids.insert(rule.id.clone()) {
            return Err(format!("Duplicate ID {}", rule.id));
        }
        trace.push(format!("Override {}: added", rule.id));
        filing.push(FilingRule {
            rule: rule.clone(),
            priority: 1,
            reason: "added by override".into(),
        });
    }
    for rule in &input.preset.copies {
        identifier(&rule.id)?;
        if !ids.insert(rule.id.clone()) {
            return Err(format!("Duplicate ID {}", rule.id));
        }
        conditions(&rule.conditions)?;
        if !mailbox(&rule.recipient) {
            return Err(
                "Copy recipient must be a synthetic mailbox in a reserved example domain".into(),
            );
        }
    }
    filing.sort_by_key(|r| r.priority);
    trace.extend(
        filing
            .iter()
            .map(|r| format!("{}: {}", r.rule.id, r.reason)),
    );
    Ok(Program {
        filing,
        copies: input.preset.copies.clone(),
        trace,
    })
}

fn parse(fixture: &Fixture) -> Result<Message, String> {
    identifier(&fixture.id)?;
    if fixture.headers.len() > 16_384
        || !fixture.headers.is_ascii()
        || fixture.headers.contains("=?")
    {
        return Err(
            "Fixtures require at most 16 KiB of ASCII headers without encoded words".into(),
        );
    }
    let normalized = fixture.headers.replace("\r\n", "\n");
    let mut headers = BTreeMap::new();
    let mut last: Option<String> = None;
    let mut ended = false;
    for line in normalized.lines() {
        if line.is_empty() {
            ended = true;
            continue;
        }
        if ended {
            return Err("Message bodies are unsupported; supply headers only".into());
        }
        if line.bytes().any(|b| b < 32 && b != b'\t' || b == 127) {
            return Err("Invalid control character in headers".into());
        }
        if line.starts_with([' ', '\t']) {
            let Some(key) = &last else {
                return Err("Header continuation without a header".into());
            };
            let value: &mut String = headers.get_mut(key).unwrap();
            value.push(' ');
            value.push_str(line.trim());
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err("Malformed header line".into());
        };
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("Malformed header name".into());
        }
        let key = key.to_ascii_lowercase();
        if headers
            .insert(key.clone(), value.trim().to_owned())
            .is_some()
        {
            return Err(format!("Duplicate header {key} is unsupported"));
        }
        last = Some(key);
    }
    let Some(from) = headers.get("from") else {
        return Err("A From header is required".into());
    };
    if !mailbox(from) {
        return Err("From must be one bare synthetic mailbox; display names and address lists are unsupported".into());
    }
    for field in ["to", "cc"] {
        if let Some(value) = headers.get(field) {
            if !value.split(',').all(|v| mailbox(v.trim())) {
                return Err("Recipients must be bare synthetic mailboxes".into());
            }
        }
    }
    Ok(Message {
        id: fixture.id.clone(),
        headers,
    })
}

fn matches(conditions: &[Condition], message: &Message) -> bool {
    let get = |key: &str| message.headers.get(key).map(String::as_str).unwrap_or("");
    conditions.iter().all(|c| match c {
        Condition::FromAddress { value } => get("from").eq_ignore_ascii_case(value),
        Condition::FromDomain { value } => {
            let host = get("from").split_once('@').unwrap().1.to_ascii_lowercase();
            let value = value.to_ascii_lowercase();
            host == value || host.ends_with(&format!(".{value}"))
        }
        Condition::SubjectContains { value } => get("subject")
            .to_ascii_lowercase()
            .contains(&value.to_ascii_lowercase()),
        Condition::ListIdExists => message.headers.contains_key("list-id"),
        Condition::OneClick => get("list-unsubscribe-post")
            .to_ascii_lowercase()
            .contains("list-unsubscribe=one-click"),
        Condition::SpamFlag => get("x-spam-flag").eq_ignore_ascii_case("yes"),
    })
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn sieve_condition(condition: &Condition) -> String {
    match condition {
        Condition::FromAddress { value } => format!(
            "address :all :is :comparator \"i;ascii-casemap\" \"from\" {}",
            quote(value)
        ),
        Condition::FromDomain { value } => format!(
            "anyof (address :domain :is \"from\" {}, address :domain :matches \"from\" {})",
            quote(value),
            quote(&format!("*.{value}"))
        ),
        Condition::SubjectContains { value } => {
            format!("header :contains \"subject\" {}", quote(value))
        }
        Condition::ListIdExists => "exists \"list-id\"".into(),
        Condition::OneClick => {
            "header :contains \"list-unsubscribe-post\" \"List-Unsubscribe=One-Click\"".into()
        }
        Condition::SpamFlag => "header :is \"x-spam-flag\" \"yes\"".into(),
    }
}

fn sieve_test(conditions: &[Condition]) -> String {
    format!(
        "allof ({})",
        conditions
            .iter()
            .map(sieve_condition)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

impl Program {
    pub fn sieve(&self) -> String {
        let mut result = "# Synthetic preview only. Not validated against a provider.\nrequire [\"fileinto\", \"copy\"];\n\n".to_owned();
        for copy in &self.copies {
            result.push_str(&format!(
                "# {}\nif {} {{ redirect :copy {}; }}\n",
                copy.id,
                sieve_test(&copy.conditions),
                quote(&copy.recipient)
            ));
        }
        for filing in &self.filing {
            let action = match &filing.rule.destination {
                Destination::Inbox => "keep;".into(),
                Destination::Folder { name } => format!("fileinto {};", quote(name)),
            };
            result.push_str(&format!(
                "\n# {} ({})\nif {} {{ {}; stop; }}\n",
                filing.rule.id,
                filing.reason,
                sieve_test(&filing.rule.conditions),
                action.trim_end_matches(';')
            ));
        }
        result.push_str("\nkeep;\n");
        result
    }

    pub fn preview(&self, fixtures: &[Fixture]) -> Result<Preview, String> {
        if fixtures.is_empty() || fixtures.len() > 200 {
            return Err("Supply 1..200 synthetic messages".into());
        }
        let mut ids = HashSet::new();
        let mut decisions = Vec::new();
        for fixture in fixtures {
            if !ids.insert(&fixture.id) {
                return Err(format!("Duplicate message ID {}", fixture.id));
            }
            let message = parse(fixture)?;
            let copies = self
                .copies
                .iter()
                .filter(|r| matches(&r.conditions, &message))
                .map(|r| CopyDecision {
                    rule: r.id.clone(),
                    recipient: r.recipient.clone(),
                })
                .collect();
            let filing = self
                .filing
                .iter()
                .find(|r| matches(&r.rule.conditions, &message));
            decisions.push(Decision {
                id: message.id,
                subject: message.headers.get("subject").cloned().unwrap_or_default(),
                from: message.headers["from"].clone(),
                destination: filing
                    .map(|r| r.rule.destination.clone())
                    .unwrap_or(Destination::Inbox),
                matched_rule: filing.map(|r| r.rule.id.clone()),
                reason: filing
                    .map(|r| r.reason.clone())
                    .unwrap_or("unmatched; retain in Inbox".into()),
                copies,
            });
        }
        let filed = decisions
            .iter()
            .filter(|d| matches!(d.destination, Destination::Folder { .. }))
            .count();
        let counts = Counts {
            messages: decisions.len(),
            filed,
            inbox: decisions.len() - filed,
            copies: decisions.iter().map(|d| d.copies.len()).sum(),
            copied_messages: decisions.iter().filter(|d| !d.copies.is_empty()).count(),
        };
        Ok(Preview {
            source: "synthetic",
            semantics: "mailroom-ascii-headers-v1",
            counts,
            decisions,
            trace: self.trace.clone(),
            sieve: self.sieve(),
            omissions: vec![
                "Compiled subset evaluation, not execution of Sieve or provider verification.",
                "ASCII headers only. No MIME bodies, encoded words, display names, envelope tests, regex, or arbitrary Sieve.",
                "Conditions are ANDed. Stable input order breaks ties within each precedence stage. From-domain matches include subdomains.",
                "Copies run before filing, including guard retention. Folders must already exist; this output never creates them.",
                "No connection, census, pinned-message proof, deployment, journal, undo, unsubscribe, or daily-use readiness.",
            ],
        })
    }
}

pub fn preview(input: &PreviewInput) -> Result<Preview, String> {
    compile(input)?.preview(&input.messages)
}
