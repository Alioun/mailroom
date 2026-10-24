mod candidates;
mod mute;
mod safety;
mod synthetic;
#[cfg(test)]
mod tests;
use crate::{
    acquisition::Provider,
    census::{AccountState, CensusKeys},
};
pub(crate) use candidates::{report, source};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;
pub(crate) use synthetic::{InstalledSynthetic, boundary};
pub const SEMANTICS: &str = "exact-subscription-unsubscribe-v1";
pub(crate) fn digest(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub history: Vec<Entry>,
    pub mutes: BTreeMap<String, OwnedMute>,
    pub synthetic_acknowledgments: BTreeMap<String, Acknowledgment>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    OneClick,
    BrowserPreparation,
    MailAppPreparation,
    LocalMail,
    RetirementPreparation,
    Mute,
    Keep,
    RemoveMute,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Intent,
    Uncertain,
    HttpObserved,
    MailHandoff,
    Prepared,
    Kept,
    Muted,
    MuteRemoved,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub subscription: String,
    pub sender_key: String,
    pub method: Method,
    pub time: i64,
    pub status: Status,
    pub host: Option<String>,
    pub payload_digest: String,
    pub source_ids: Vec<String>,
    pub http_status: Option<u16>,
    pub success: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgment {
    pub payload_digest: String,
    pub host: Option<String>,
    pub http_status: Option<u16>,
    pub mail_handoff: bool,
    pub verb: String,
    pub form: Option<String>,
    pub headers: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedMute {
    pub rule: crate::rules::config::Rule,
    pub subscription: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Folders {
    pub personal: String,
    pub business: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub subscription: String,
    pub method: Method,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub as_of: i64,
    pub selections: Vec<Selection>,
    pub folders: Folders,
}
#[derive(Serialize)]
pub struct ActionPlan {
    pub subscription: String,
    pub account: String,
    pub sender: String,
    pub recipient: String,
    pub list_id: Option<String>,
    pub source: String,
    pub header_digest: String,
    pub method: Method,
    pub host: Option<String>,
    pub payload_digest: String,
    pub role: String,
    pub destination: Option<String>,
    pub effect: String,
    pub copy_card: Option<CopyCard>,
    pub mute_rule: Option<crate::rules::config::Rule>,
    pub source_ids: Vec<String>,
}
#[derive(Serialize)]
pub struct CopyCard {
    pub uri: Option<String>,
    pub to: Option<String>,
    pub from: Option<String>,
    pub subject: Option<String>,
    pub body: Option<String>,
}
#[derive(Serialize)]
pub struct Preview {
    pub account: String,
    pub state_revision: u64,
    pub workspace_revision: Option<u64>,
    pub semantics: &'static str,
    pub actions: Vec<ActionPlan>,
    pub receipt: String,
    pub confirmation_action: &'static str,
    pub local_mail_confirmation: Option<&'static str>,
    pub proof: Option<crate::rules::proof::Proof>,
    pub scope: &'static str,
}
const CONFIRM: &str = "Confirm exactly these subscription actions (synthetic only)";
const MAIL_CONFIRM: &str = "Send exactly these unsubscribe messages from the displayed receiving addresses (synthetic adapter, no OS dialog)";
const PURPOSE: &[u8] = b"mailroom/unsubscribe-exact-review/v1";

pub(crate) fn preview(
    state: &AccountState,
    provider: Provider,
    keys: &CensusKeys,
    input: &ReviewInput,
    installed: Option<&InstalledSynthetic>,
) -> Result<Preview, String> {
    if installed.is_none() {
        return Err("Production unsubscribe actions are denied".into());
    }
    if input.selections.is_empty()
        || input.selections.len() > 64
        || state.unsubscribe.history.len() + input.selections.len() > 256
    {
        return Err("Choose 1..64 exact subscriptions; history limit 256".into());
    }
    crate::rules::config::segment(&input.folders.personal)?;
    crate::rules::config::segment(&input.folders.business)?;
    let report = report(state, provider, keys, input.as_of, installed)?;
    if !report.complete {
        return Err("Complete the census before confirming actions".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    let mut actions = Vec::new();
    let mut proposed = state.clone();
    for selection in &input.selections {
        if !unique.insert(&selection.subscription) {
            return Err("Duplicate subscription action".into());
        }
        if selection.method == Method::RemoveMute {
            actions.push(mute::remove_plan(&mut proposed, &selection.subscription)?);
            continue;
        }
        let row = report
            .rows
            .iter()
            .find(|r| r.id == selection.subscription)
            .ok_or("Unknown exact subscription")?;
        if !row.methods.contains(&selection.method) {
            return Err("Selected method is unavailable".into());
        }
        let source = candidates::source(state, &row.source).ok_or("Source changed")?;
        let uris = candidates::uris(&source.headers);
        let https = uris.iter().find(|s| s.starts_with("https://"));
        let mailto = uris.iter().find(|s| s.starts_with("mailto:"));
        let mut plan = ActionPlan {
            subscription: row.id.clone(),
            account: state.census.account.as_str().into(),
            sender: row.sender.clone(),
            recipient: row.recipient.clone(),
            list_id: row.list_id.clone(),
            source: row.source.clone(),
            header_digest: row.header_digest.clone(),
            method: selection.method.clone(),
            host: None,
            payload_digest: String::new(),
            role: row.role.clone(),
            destination: None,
            effect: String::new(),
            copy_card: None,
            mute_rule: None,
            source_ids: row.source_ids.clone(),
        };
        match selection.method {
            Method::OneClick => {
                let post = installed
                    .ok_or("No synthetic adapter")?
                    .post(https.ok_or("HTTPS unavailable")?)?;
                plan.host = Some(post.host().into());
                plan.payload_digest = post.request_digest();
                plan.effect = "One synthetic POST of List-Unsubscribe=One-Click, Content-Type application/x-www-form-urlencoded, no cookies/referrer or redirects; HTTP status is not unsubscribe success. No Undo.".into();
            }
            Method::LocalMail | Method::MailAppPreparation => {
                let mail =
                    safety::Mail::parse(mailto.ok_or("Mailto unavailable")?, &row.recipient)?;
                plan.payload_digest = mail.digest();
                plan.host = mail.to.split_once('@').map(|(_, h)| h.into());
                plan.effect = if selection.method == Method::LocalMail { "One synthetic mail handoff after additional action-naming confirmation. Unsubscribe success unknown. No Undo." } else { "Preparation only. Nothing opens or sends." }.into();
                plan.copy_card = Some(CopyCard {
                    uri: None,
                    to: Some(mail.to),
                    from: Some(mail.from),
                    subject: Some(mail.subject),
                    body: Some(mail.body.to_string()),
                });
            }
            Method::BrowserPreparation => {
                let endpoint = safety::Endpoint::parse(https.ok_or("HTTPS unavailable")?)?;
                plan.host = Some(endpoint.host);
                plan.payload_digest = digest(https.unwrap().as_bytes());
                plan.effect = "Preparation only. Nothing opens or requests this URL.".into();
                plan.copy_card = Some(CopyCard {
                    uri: https.cloned(),
                    to: None,
                    from: None,
                    subject: None,
                    body: None,
                });
            }
            Method::RetirementPreparation => {
                plan.effect = "Preparation only. Address and alias retirement are unavailable; nothing is disabled.".into();
            }
            Method::Mute => {
                let (rule, destination) = mute::add(&mut proposed, row, &input.folders)?;
                plan.payload_digest =
                    digest(&serde_json::to_vec(&rule).map_err(|_| "Rule encoding")?);
                plan.destination = Some(destination);
                plan.mute_rule = Some(rule);
                plan.effect = "Save an exact local override for future messages, marked read. No deployment or retroactive mail changes. Invoice copies remain independent.".into();
            }
            Method::Keep => {
                plan.effect = "Keep this exact subscription. No external effect.".into();
            }
            Method::RemoveMute => unreachable!(),
        }
        actions.push(plan);
    }
    let changed = actions
        .iter()
        .any(|a| matches!(a.method, Method::Mute | Method::RemoveMute));
    let proof = if changed {
        Some(mute::proof(
            &proposed,
            provider,
            input.as_of,
            installed.is_some(),
        )?)
    } else {
        None
    };
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(SEMANTICS, state, provider, input, &actions, &proof))
            .map_err(|_| "Review encoding")?,
    );
    let receipt = keys.binding_for(PURPOSE, &payload);
    let local_mail_confirmation = actions
        .iter()
        .any(|a| a.method == Method::LocalMail)
        .then_some(MAIL_CONFIRM);
    Ok(Preview {
        account: state.census.account.as_str().into(),
        state_revision: state.revision,
        workspace_revision: state.workspace.as_ref().map(|w| w.revision),
        semantics: SEMANTICS,
        actions,
        receipt,
        confirmation_action: CONFIRM,
        local_mail_confirmation,
        proof,
        scope: "Subscriptions are approved separately for each account, receiving address and mailing list. Sender grouping grants no authority.",
    })
}

pub(crate) fn confirm(
    state: &mut AccountState,
    preview: &Preview,
    receipt: &str,
    action: &str,
    mail_action: Option<&str>,
    now: i64,
) -> Result<Vec<String>, String> {
    if preview.proof.as_ref().is_some_and(|p| p.gate == "blocked") {
        return Err("Mute proof conflicts with pinned expectations".into());
    }
    if receipt.len() != 64
        || preview
            .receipt
            .as_bytes()
            .ct_eq(receipt.as_bytes())
            .unwrap_u8()
            != 1
        || action != CONFIRM
        || preview
            .local_mail_confirmation
            .is_some_and(|s| mail_action != Some(s))
        || state
            .unsubscribe
            .history
            .iter()
            .any(|e| matches!(e.status, Status::Intent | Status::Uncertain))
    {
        return Err("Exact list or additional synthetic mail confirmation rejected; reconcile pending intent".into());
    }
    let mut ids = Vec::new();
    for (i, a) in preview.actions.iter().enumerate() {
        let id = digest(format!("{receipt}/{i}").as_bytes());
        if state.unsubscribe.history.iter().any(|e| e.id == id) {
            return Err("Replayed review".into());
        }
        let status = match a.method {
            Method::OneClick | Method::LocalMail => Status::Intent,
            Method::Mute => {
                mute::install(state, a)?;
                Status::Muted
            }
            Method::RemoveMute => {
                mute::remove(state, &a.subscription)?;
                Status::MuteRemoved
            }
            Method::Keep => Status::Kept,
            _ => Status::Prepared,
        };
        state.unsubscribe.history.push(Entry {
            id: id.clone(),
            subscription: a.subscription.clone(),
            sender_key: a.sender.clone(),
            method: a.method.clone(),
            time: now,
            status,
            host: a.host.clone(),
            payload_digest: a.payload_digest.clone(),
            source_ids: a.source_ids.clone(),
            http_status: None,
            success: "unknown".into(),
        });
        ids.push(id);
    }
    Ok(ids)
}
pub(crate) fn observe(
    state: &mut AccountState,
    installed: &InstalledSynthetic,
    id: &str,
) -> Result<(), String> {
    let entry = state
        .unsubscribe
        .history
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or("Unknown unsubscribe intent")?;
    if !matches!(entry.method, Method::OneClick | Method::LocalMail) {
        return Err("No external intent".into());
    }
    match installed.received().get(id) {
        Some(ack)
            if ack.payload_digest == entry.payload_digest
                && ack.host == entry.host
                && ((entry.method == Method::OneClick
                    && ack.verb == "POST"
                    && ack.form.as_deref() == Some("List-Unsubscribe=One-Click")
                    && ack.http_status.is_some()
                    && !ack.mail_handoff)
                    || (entry.method == Method::LocalMail
                        && ack.verb == "MAIL"
                        && ack.mail_handoff
                        && ack.http_status.is_none())) =>
        {
            entry.http_status = ack.http_status;
            entry.status = if ack.mail_handoff {
                Status::MailHandoff
            } else {
                Status::HttpObserved
            };
        }
        _ => entry.status = Status::Uncertain,
    }
    Ok(())
}
