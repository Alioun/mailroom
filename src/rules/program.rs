use super::{
    config::*,
    sieve::{self, Action, Statement, Test},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    MailboxSieve,
    GmailBacklog,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Unknown,
    SyntheticDeliveredTo,
    SyntheticOriginalTo,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineContext {
    pub account: String,
    pub engine: Engine,
    pub recipient_evidence: EvidenceSource,
    pub capabilities: BTreeSet<String>,
}
impl EngineContext {
    pub fn local(account: String, engine: Engine) -> Self {
        Self {
            account,
            engine,
            recipient_evidence: EvidenceSource::Unknown,
            capabilities: BTreeSet::new(),
        }
    }
    fn recipient_field(&self) -> Option<&'static str> {
        match self.recipient_evidence {
            EvidenceSource::Unknown => None,
            EvidenceSource::SyntheticDeliveredTo => Some("delivered-to"),
            EvidenceSource::SyntheticOriginalTo => Some("x-original-to"),
        }
    }
}
#[derive(Clone, Serialize)]
pub struct Omission {
    pub rule: String,
    pub reason: String,
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectiveDefinition {
    Filing { rule: Rule },
    Forward { rule: Forward },
}
#[derive(Clone, Serialize)]
pub struct EffectiveRule {
    pub id: String,
    pub origin: String,
    pub enabled: bool,
    pub accounts: BTreeSet<String>,
    pub definition: EffectiveDefinition,
}
#[derive(Clone, Serialize)]
pub struct CompiledArtifact {
    identity: String,
    context: EngineContext,
    executable: String,
    effective: Vec<EffectiveRule>,
    trace: Vec<String>,
    omissions: Vec<Omission>,
    folders: BTreeSet<String>,
}
impl CompiledArtifact {
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn context(&self) -> &EngineContext {
        &self.context
    }
    pub fn executable(&self) -> &str {
        &self.executable
    }
    pub fn effective(&self) -> &[EffectiveRule] {
        &self.effective
    }
    pub fn trace(&self) -> &[String] {
        &self.trace
    }
    pub fn omissions(&self) -> &[Omission] {
        &self.omissions
    }
    pub fn folders(&self) -> &BTreeSet<String> {
        &self.folders
    }
    pub(super) fn statements(&self) -> Result<Vec<Statement>, String> {
        sieve::parse(&self.executable)
    }
}
fn all(t: Vec<Test>) -> Test {
    if t.is_empty() {
        Test::True
    } else {
        Test::All(t)
    }
}
fn any(t: Vec<Test>) -> Test {
    if t.is_empty() {
        Test::False
    } else {
        Test::Any(t)
    }
}
fn not(t: Test) -> Test {
    Test::Not(Box::new(t))
}
fn address(field: &str, part: &str, mode: &str, value: &str) -> Test {
    Test::Address {
        field: field.into(),
        part: part.into(),
        mode: mode.into(),
        value: value.into(),
    }
}
fn header(field: &str, mode: &str, value: &str) -> Test {
    Test::Header {
        field: field.into(),
        mode: mode.into(),
        value: value.into(),
    }
}
fn one_click() -> Test {
    all(vec![
        header("list-unsubscribe-post", "is", "List-Unsubscribe=One-Click"),
        header("list-unsubscribe", "matches", "*<https://?*>*"),
    ])
}
fn bulk() -> Test {
    any(vec![Test::Exists("list-id".into()), one_click()])
}
fn role_test(roles: &Roles, role: Role, field: &str) -> Test {
    any(vec![
        any(roles
            .addresses
            .iter()
            .filter(|(_, r)| **r == role)
            .map(|(a, _)| address(field, "all", "is", a))
            .collect()),
        all(vec![
            not(any(roles
                .addresses
                .keys()
                .map(|a| address(field, "all", "is", a))
                .collect())),
            any(roles
                .domains
                .iter()
                .filter(|(_, r)| **r == role)
                .map(|(d, _)| address(field, "domain", "is", d))
                .collect()),
        ]),
    ])
}
fn condition(c: &Condition, recipient: &str) -> Test {
    match c {
        Condition::FromAddress { value } => address("from", "all", "is", value),
        Condition::FromDomain { value } => any(vec![
            address("from", "domain", "is", value),
            address("from", "domain", "matches", &format!("*.{value}")),
        ]),
        Condition::ToAddress { value } => address(recipient, "all", "is", value),
        Condition::SubjectContains { value } => header("subject", "contains", value),
        Condition::ListIdExists => Test::Exists("list-id".into()),
        Condition::OneClick => one_click(),
        Condition::NonBulk => not(bulk()),
        Condition::Attachment => unreachable!("unsupported rules omitted in full"),
    }
}
fn scoped(accounts: &BTreeSet<String>, context: &EngineContext) -> bool {
    accounts.is_empty() || accounts.contains(&context.account)
}
fn path(layout: &Layout, role: Role, category: Category, folder: Option<&str>) -> String {
    let root = if role == Role::Business {
        &layout.business_root
    } else {
        &layout.personal_root
    };
    format!(
        "{root}/{}",
        folder.unwrap_or_else(|| category.label(layout.language))
    )
}
fn append_filing(
    statements: &mut Vec<Statement>,
    id: &str,
    test: Test,
    folder: Option<String>,
    mark_read: bool,
) {
    let mut actions = Vec::new();
    if mark_read {
        actions.push(Action::Seen);
    }
    actions.push(folder.map(Action::File).unwrap_or(Action::Keep));
    actions.push(Action::Stop);
    statements.push(Statement {
        id: id.into(),
        test,
        actions,
    });
}

pub fn compile(
    preset: &Preset,
    overrides: &Overrides,
    context: &EngineContext,
) -> Result<CompiledArtifact, String> {
    identifier(&preset.id)?;
    identifier(&context.account)?;
    if preset.major == 0 || overrides.preset != preset.id || overrides.major != preset.major {
        return Err("Override must pin the supplied preset and nonzero major version".into());
    }
    if preset.rules.len()
        + overrides.additions.len()
        + overrides.forwards.len()
        + overrides.ledger.len()
        > 200
    {
        return Err("At most 200 authored rules and ledger entries".into());
    }
    let layout = &overrides.layout;
    for f in [
        &layout.personal_root,
        &layout.business_root,
        &layout.new_addresses,
        &layout.retired_addresses,
    ] {
        segment(f)?;
    }
    if layout.personal_root == layout.business_root {
        return Err("Separate role roots must differ".into());
    }
    for (a, _) in &overrides.roles.addresses {
        if !mailbox(a) || a != &a.to_ascii_lowercase() {
            return Err("Role addresses need canonical lowercase mailboxes".into());
        }
    }
    for d in overrides
        .roles
        .domains
        .keys()
        .chain(overrides.roles.signup_domains.iter())
    {
        if !domain(d) {
            return Err("Invalid approved role domain".into());
        }
    }
    for d in &overrides.roles.signup_domains {
        if !overrides
            .roles
            .domains
            .get(d)
            .is_some_and(|r| matches!(r, Role::Business | Role::Personal))
        {
            return Err(
                "Sign-up domains need an explicitly approved personal or business role".into(),
            );
        }
    }
    let mut ids = BTreeSet::new();
    let mut effective = Vec::new();
    let mut rules = Vec::new();
    let mut trace = vec![format!("Preset {}@{}", preset.id, preset.major)];
    let mut insert = |rule: Rule, origin: &str, priority: u8| -> Result<(), String> {
        validate_rule(&rule)?;
        if !ids.insert(rule.id.clone()) {
            return Err("Duplicate stable rule ID".into());
        }
        effective.push(EffectiveRule {
            id: rule.id.clone(),
            origin: origin.into(),
            enabled: rule.enabled,
            accounts: rule.accounts.clone(),
            definition: EffectiveDefinition::Filing { rule: rule.clone() },
        });
        rules.push((priority, origin.to_owned(), rule));
        Ok(())
    };
    for r in &preset.rules {
        insert(
            r.clone(),
            "preset",
            match r.stage {
                Stage::Guard => 0,
                Stage::From => 4,
                Stage::Subject => 5,
            },
        )?;
    }
    for r in &overrides.additions {
        if r.stage == Stage::Guard || !r.id.starts_with("user.") {
            return Err("New rules need a user.* ID and cannot add guards".into());
        }
        insert(r.clone(), "override addition", 1)?;
    }
    let mut patched = BTreeSet::new();
    for p in &overrides.patches {
        let mut id = p.id.clone();
        let mut visited = BTreeSet::new();
        while let Some(next) = preset.renamed.get(&id) {
            if !visited.insert(id.clone()) {
                return Err("Cyclic renamed ID map".into());
            }
            trace.push(format!("Renamed {id} to {next}"));
            id = next.clone();
        }
        if !patched.insert(id.clone()) {
            return Err("Duplicate patch after ID migration".into());
        }
        let (priority, origin, rule) = rules
            .iter_mut()
            .find(|(_, _, r)| r.id == id)
            .ok_or("Unknown override ID; additions must explicitly use user.*")?;
        if p.enabled.is_none()
            && p.folder.is_none()
            && p.when.is_none()
            && p.extend.is_empty()
            && p.accounts.is_none()
            && p.mark_read.is_none()
        {
            return Err("An override patch must change a value".into());
        }
        if let Some(v) = p.enabled {
            rule.enabled = v;
        }
        if let Some(v) = &p.folder {
            rule.folder = Some(v.clone());
        }
        if let Some(v) = &p.when {
            rule.when = v.clone();
        }
        rule.when.extend(p.extend.clone());
        if let Some(v) = &p.accounts {
            rule.accounts = v.clone();
        }
        if let Some(v) = p.mark_read {
            rule.mark_read = v;
        }
        validate_rule(rule)?;
        *priority = if rule.stage == Stage::Guard { 0 } else { 1 };
        *origin = "preset patched by override".into();
        let e = effective.iter_mut().find(|r| r.id == id).unwrap();
        e.origin = origin.clone();
        e.enabled = rule.enabled;
        e.accounts = rule.accounts.clone();
        e.definition = EffectiveDefinition::Filing { rule: rule.clone() };
        trace.push(format!(
            "Override {id}: {}",
            if rule.enabled {
                "patched before bulk"
            } else {
                "disabled"
            }
        ));
    }
    for (old, new) in &preset.renamed {
        identifier(old)?;
        identifier(new)?;
        if ids.contains(old) {
            return Err("Renamed ID is still active".into());
        }
        let mut id = new;
        let mut visited = BTreeSet::from([old]);
        while let Some(next) = preset.renamed.get(id) {
            if !visited.insert(id) {
                return Err("Cyclic renamed ID map".into());
            }
            id = next;
        }
        if !ids.contains(id) {
            return Err("Renamed ID has no current rule".into());
        }
    }
    let mut omissions = Vec::new();
    let mut statements = Vec::new();
    let needed = ["fileinto", "relational", "comparator-i;ascii-numeric"];
    let safety_capable = needed.iter().all(|c| context.capabilities.contains(*c));
    let recipient = context.recipient_field().unwrap_or("delivered-to");
    let personal = role_test(&overrides.roles, Role::Personal, recipient);
    let business = role_test(&overrides.roles, Role::Business, recipient);
    let safe = if context.recipient_field().is_some() && safety_capable {
        all(vec![
            Test::Count(recipient.into()),
            any(vec![personal.clone(), business.clone()]),
        ])
    } else {
        omissions.push(Omission { rule: "system.role_safety".into(), reason: "Recipient provenance or required capabilities are unknown. All messages retain Inbox for review; no copies.".into() });
        Test::False
    };
    append_filing(
        &mut statements,
        "system.role_safety",
        not(safe),
        None,
        false,
    );
    let mut ledger_addresses = BTreeSet::new();
    for entry in &overrides.ledger {
        if !mailbox(&entry.address) || !ledger_addresses.insert(entry.address.clone()) {
            return Err("Invalid or duplicate ledger address".into());
        }
        if let Some(f) = &entry.folder {
            segment(f)?;
        }
    }
    for (name, target) in &overrides.targets {
        identifier(name)?;
        if !mailbox(&target.address) {
            return Err("Invalid private forwarding target".into());
        }
    }
    for f in &overrides.forwards {
        identifier(&f.id)?;
        identifier(&f.target)?;
        if f.id.starts_with("system.") || !ids.insert(f.id.clone()) {
            return Err("Duplicate or reserved forward ID".into());
        }
        let target = overrides
            .targets
            .get(&f.target)
            .ok_or("Forward references an undefined private target")?;
        effective.push(EffectiveRule {
            id: f.id.clone(),
            origin: "override forward".into(),
            enabled: f.enabled,
            accounts: f.accounts.clone(),
            definition: EffectiveDefinition::Forward { rule: f.clone() },
        });
        let trigger = match &f.trigger {
            ForwardTrigger::InvoiceAddress => any(overrides
                .ledger
                .iter()
                .filter(|e| e.invoice)
                .map(|e| address(recipient, "all", "is", &e.address))
                .collect()),
            ForwardTrigger::ApprovedSenders { addresses } => {
                if addresses.is_empty() || addresses.iter().any(|a| !mailbox(a)) {
                    return Err("Approved invoice senders require explicit valid mailboxes".into());
                }
                any(addresses
                    .iter()
                    .map(|a| address("from", "all", "is", a))
                    .collect())
            }
            ForwardTrigger::Keywords { words } => {
                if words.is_empty()
                    || words
                        .iter()
                        .any(|w| w.is_empty() || w.len() > 200 || w.chars().any(char::is_control))
                {
                    return Err("Invalid invoice keywords".into());
                }
                Test::False
            }
        };
        if !f.enabled || !scoped(&f.accounts, context) {
            continue;
        }
        let reason = if context.engine == Engine::GmailBacklog {
            Some("Gmail backlog has no invoice forward")
        } else if matches!(f.trigger, ForwardTrigger::Keywords { .. }) {
            Some(
                "Keyword forward requires nonbulk mail and PDF/XML attachment evidence. Header census has no attachment evidence; whole rule omitted",
            )
        } else if !["copy", "duplicate"]
            .iter()
            .all(|c| context.capabilities.contains(*c))
        {
            Some("Required copy/duplicate capability is unknown; whole forward omitted")
        } else {
            None
        };
        if let Some(reason) = reason {
            omissions.push(Omission {
                rule: f.id.clone(),
                reason: reason.into(),
            });
            continue;
        }
        let handle = format!("mailroom.{}", f.id);
        statements.push(Statement {
            id: f.id.clone(),
            test: all(vec![
                business.clone(),
                trigger,
                Test::Exists("message-id".into()),
                not(Test::Duplicate(handle)),
            ]),
            actions: vec![Action::Copy(target.address.clone())],
        });
    }
    rules.sort_by_key(|(priority, _, _)| *priority);
    for (priority, _, rule) in &rules {
        if *priority <= 1 {
            add_rule(
                &mut statements,
                &mut omissions,
                rule,
                overrides,
                context,
                recipient,
            )?;
        }
    }
    for entry in &overrides.ledger {
        for role in [Role::Personal, Role::Business] {
            let folder = match entry.status {
                LedgerStatus::Retired => Some(layout.retired_addresses.as_str()),
                LedgerStatus::Active => entry.folder.as_deref(),
            };
            append_filing(
                &mut statements,
                &format!("system.ledger.{}", ledger_digest(&entry.address)),
                all(vec![
                    role_test(&overrides.roles, role, recipient),
                    address(recipient, "all", "is", &entry.address),
                ]),
                Some(path(layout, role, entry.category, folder)),
                false,
            );
        }
    }
    for role in [Role::Personal, Role::Business] {
        let signup = any(overrides
            .roles
            .signup_domains
            .iter()
            .map(|d| address(recipient, "domain", "is", d))
            .collect());
        let known = any(ledger_addresses
            .iter()
            .map(|a| address(recipient, "all", "is", a))
            .collect());
        append_filing(
            &mut statements,
            "system.new_addresses",
            all(vec![
                role_test(&overrides.roles, role, recipient),
                signup,
                not(known),
            ]),
            Some(path(
                layout,
                role,
                Category::Orders,
                Some(&layout.new_addresses),
            )),
            false,
        );
        append_filing(
            &mut statements,
            "system.bulk",
            all(vec![role_test(&overrides.roles, role, recipient), bulk()]),
            Some(path(layout, role, Category::Newsletters, None)),
            false,
        );
    }
    for (priority, _, rule) in &rules {
        if *priority > 1 {
            add_rule(
                &mut statements,
                &mut omissions,
                rule,
                overrides,
                context,
                recipient,
            )?;
        }
    }
    append_filing(&mut statements, "system.unmatched", Test::True, None, false);
    let mut extensions = BTreeSet::from([
        "fileinto".into(),
        "relational".into(),
        "comparator-i;ascii-numeric".into(),
    ]);
    if statements
        .iter()
        .any(|s| s.actions.iter().any(|a| matches!(a, Action::Copy(_))))
    {
        extensions.extend(["copy".into(), "duplicate".into()]);
    }
    if statements
        .iter()
        .any(|s| s.actions.iter().any(|a| matches!(a, Action::Seen)))
    {
        extensions.insert("imap4flags".into());
    }
    if !safety_capable || context.recipient_field().is_none() {
        statements.truncate(1);
        extensions.clear();
        for rule in effective
            .iter()
            .filter(|r| r.enabled && scoped(&r.accounts, context))
        {
            if !omissions.iter().any(|o| o.rule == rule.id) {
                omissions.push(Omission { rule: rule.id.clone(), reason: "Recipient provenance or role-safety capability is unavailable; entire rule omitted".into() });
            }
        }
        omissions.push(Omission {
            rule: "system.bulk".into(),
            reason:
                "Recipient provenance or role-safety capability is unavailable; bulk filing omitted"
                    .into(),
        });
    }
    let executable = sieve::render(&statements, &extensions);
    sieve::parse(&executable)?;
    let folders = statements
        .iter()
        .flat_map(|s| &s.actions)
        .filter_map(|a| {
            if let Action::File(f) = a {
                Some(f.clone())
            } else {
                None
            }
        })
        .collect();
    trace.extend(effective.iter().map(|e| {
        format!(
            "{}: {}{}",
            e.id,
            e.origin,
            if e.enabled { "" } else { "; disabled" }
        )
    }));
    let binding = serde_json::to_vec(&(
        "mailroom-sieve-subset-v2",
        preset,
        overrides,
        context,
        &effective,
        &executable,
    ))
    .map_err(|_| "Artifact identity serialization failed")?;
    Ok(CompiledArtifact {
        identity: data_encoding::HEXLOWER.encode(&Sha256::digest(binding)),
        context: context.clone(),
        executable,
        effective,
        trace,
        omissions,
        folders,
    })
}
fn ledger_digest(value: &str) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(value.as_bytes()))[..16].into()
}
fn add_rule(
    statements: &mut Vec<Statement>,
    omissions: &mut Vec<Omission>,
    rule: &Rule,
    overrides: &Overrides,
    context: &EngineContext,
    recipient: &str,
) -> Result<(), String> {
    if !rule.enabled || !scoped(&rule.accounts, context) {
        return Ok(());
    }
    let reason = if rule.when.iter().any(|c| matches!(c, Condition::Attachment)) {
        Some("Required attachment evidence is unavailable; entire rule omitted")
    } else if rule.mark_read && !context.capabilities.contains("imap4flags") {
        Some("Required imap4flags capability is unknown; entire rule omitted")
    } else {
        None
    };
    if let Some(reason) = reason {
        omissions.push(Omission {
            rule: rule.id.clone(),
            reason: reason.into(),
        });
        return Ok(());
    }
    for role in [Role::Personal, Role::Business] {
        let mut tests = vec![role_test(&overrides.roles, role, recipient)];
        tests.extend(rule.when.iter().map(|c| condition(c, recipient)));
        append_filing(
            statements,
            &rule.id,
            all(tests),
            if rule.retain {
                None
            } else {
                Some(path(
                    &overrides.layout,
                    role,
                    rule.category,
                    rule.folder.as_deref(),
                ))
            },
            rule.mark_read,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        engine,
        rules::proof::{self, Baseline, Destination},
    };
    #[test]
    fn proof_executes_changed_output_instead_of_source_rules() {
        let input: engine::PreviewInput =
            serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
        let mut compiled = engine::artifact(&input.documents, &input.context).unwrap();
        compiled.executable = compiled
            .executable
            .replace("Business/Studio", "Business/Changed");
        let proof = proof::prove(
            compiled.clone(),
            &engine::synthetic_census(&input).unwrap(),
            input.as_of,
            &[],
            Baseline::Current,
            None,
        )
        .unwrap();
        assert_eq!(
            proof.decisions[0].destination,
            Destination::Folder {
                name: "Business/Changed".into()
            }
        );
        compiled.executable = compiled.executable.replace("fileinto", "unsupported");
        assert!(
            proof::prove(
                compiled,
                &engine::synthetic_census(&input).unwrap(),
                input.as_of,
                &[],
                Baseline::Current,
                None
            )
            .is_err()
        );
    }
}
