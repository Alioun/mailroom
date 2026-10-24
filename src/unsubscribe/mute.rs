use super::*;
use crate::{
    engine,
    rules::{
        self,
        config::*,
        proof::{self, Baseline},
    },
};
fn documents(state: &AccountState) -> Result<(Preset, Overrides), String> {
    let w = state
        .workspace
        .as_ref()
        .ok_or("Save a rule workspace first")?;
    rules::config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
}
fn save(state: &mut AccountState, config: &Overrides) -> Result<(), String> {
    let w = state.workspace.as_mut().ok_or("Workspace missing")?;
    w.input.documents.override_yaml =
        serde_yaml_ng::to_string(config).map_err(|_| "Rule encoding")?;
    w.revision = w.revision.checked_add(1).ok_or("Revision limit")?;
    Ok(())
}
pub(super) fn add(
    state: &mut AccountState,
    row: &candidates::Candidate,
    folders: &Folders,
) -> Result<(Rule, String), String> {
    let (_, mut config) = documents(state)?;
    let folder = match row.role.as_str() {
        "personal" => &folders.personal,
        "business" => &folders.business,
        _ => return Err("Unknown or mixed role retains Inbox".into()),
    };
    let source = candidates::source(state, &row.source).ok_or("Source changed")?;
    if source.headers.from.len() != 1 {
        return Err("Exact From unavailable".into());
    }
    let condition = match &row.list_id {
        Some(value) => Condition::ListIdExact {
            value: value.clone(),
        },
        None => Condition::FromAddress {
            value: source.headers.from[0].address.clone(),
        },
    };
    let mut when = vec![
        condition,
        Condition::ToAddress {
            value: row.recipient.clone(),
        },
    ];
    if row.list_id.is_none() {
        when.push(Condition::ListIdAbsent);
    }
    let rule = Rule {
        id: format!("user.mute.{}", row.id),
        category: Category::Newsletters,
        stage: Stage::From,
        when,
        folder: Some(folder.clone()),
        enabled: true,
        accounts: std::collections::BTreeSet::from([state.census.account.as_str().into()]),
        retain: false,
        mark_read: true,
    };
    validate_rule(&rule)?;
    if config.additions.iter().any(|r| r.id == rule.id)
        || state.unsubscribe.mutes.contains_key(&row.id)
    {
        return Err("Existing override must be explicitly removed first".into());
    }
    config.additions.insert(0, rule.clone());
    let root = if row.role == "personal" {
        &config.layout.personal_root
    } else {
        &config.layout.business_root
    };
    let destination = format!("{root}/{folder}");
    save(state, &config)?;
    state.unsubscribe.mutes.insert(
        row.id.clone(),
        OwnedMute {
            rule: rule.clone(),
            subscription: row.id.clone(),
        },
    );
    Ok((rule, destination))
}
pub(super) fn install(state: &mut AccountState, plan: &ActionPlan) -> Result<(), String> {
    let (_, mut config) = documents(state)?;
    let rule = plan.mute_rule.as_ref().ok_or("Missing mute rule")?;
    if config.additions.iter().any(|r| r.id == rule.id) {
        return Err("Mute override already exists".into());
    }
    config.additions.insert(0, rule.clone());
    save(state, &config)?;
    state.unsubscribe.mutes.insert(
        plan.subscription.clone(),
        OwnedMute {
            rule: rule.clone(),
            subscription: plan.subscription.clone(),
        },
    );
    Ok(())
}
pub(super) fn remove(state: &mut AccountState, subscription: &str) -> Result<Rule, String> {
    let (_, mut config) = documents(state)?;
    let owned = state
        .unsubscribe
        .mutes
        .get(subscription)
        .ok_or("Unknown owned mute")?;
    let matching: Vec<_> = config
        .additions
        .iter()
        .filter(|r| r.id == owned.rule.id)
        .collect();
    if matching.len() != 1
        || config.patches.iter().any(|p| p.id == owned.rule.id)
        || serde_json::to_vec(matching[0]).map_err(|_| "Rule encoding")?
            != serde_json::to_vec(&owned.rule).map_err(|_| "Rule encoding")?
    {
        return Err("Mute changed since creation; preserve user edits".into());
    }
    let rule = owned.rule.clone();
    config.additions.retain(|r| r.id != rule.id);
    save(state, &config)?;
    state.unsubscribe.mutes.remove(subscription);
    Ok(rule)
}
pub(super) fn remove_plan(
    state: &mut AccountState,
    subscription: &str,
) -> Result<ActionPlan, String> {
    let sender = state
        .unsubscribe
        .history
        .iter()
        .rev()
        .find(|e| e.subscription == subscription && e.method == Method::Mute)
        .map(|e| e.sender_key.clone())
        .ok_or("Owned mute has no creation record")?;
    let rule = remove(state, subscription)?;
    let recipient = rule
        .when
        .iter()
        .find_map(|c| match c {
            Condition::ToAddress { value } => Some(value.clone()),
            _ => None,
        })
        .ok_or("Owned mute has no exact receiving target")?;
    let list_id = rule.when.iter().find_map(|c| match c {
        Condition::ListIdExact { value } => Some(value.clone()),
        _ => None,
    });
    Ok(ActionPlan { subscription: subscription.into(), account: state.census.account.as_str().into(), sender, recipient, list_id, source: String::new(), header_digest: digest(&serde_json::to_vec(&rule).map_err(|_| "Rule encoding")?), method: Method::RemoveMute, host: None, payload_digest: digest(&serde_json::to_vec(&rule).map_err(|_| "Rule encoding")?), role: "future_routing_only".into(), destination: None, effect: "Remove only the unchanged owned override. No external unsubscribe Undo or restoration of earlier mail/flags.".into(), copy_card: None, mute_rule: Some(rule), source_ids: Vec::new() })
}
pub(super) fn proof(
    state: &AccountState,
    provider: Provider,
    as_of: i64,
    synthetic: bool,
) -> Result<proof::Proof, String> {
    let context = if synthetic {
        let sample: serde_json::Value =
            serde_json::from_str(include_str!("../../examples/sample.json"))
                .map_err(|_| "Fixture context")?;
        let mut context: rules::EngineContext =
            serde_json::from_value(sample["context"].clone()).map_err(|_| "Fixture context")?;
        context.account = state.census.account.as_str().into();
        context.engine = if provider == Provider::Gmail {
            rules::Engine::GmailBacklog
        } else {
            rules::Engine::MailboxSieve
        };
        context
    } else {
        rules::EngineContext::local(
            state.census.account.as_str().into(),
            rules::Engine::MailboxSieve,
        )
    };
    let w = state.workspace.as_ref().ok_or("Workspace missing")?;
    let artifact = engine::artifact(&w.input.documents, &context)?;
    if state
        .unsubscribe
        .mutes
        .values()
        .any(|m| artifact.omissions().iter().any(|o| o.rule == m.rule.id))
    {
        return Err("Mute cannot be represented safely by installed engine capabilities".into());
    }
    proof::prove(
        artifact,
        &state.census,
        as_of,
        &w.input.pins,
        Baseline::Current,
        None,
    )
}
