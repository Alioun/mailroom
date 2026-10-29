mod adapters;
mod report;
mod synthetic;
#[cfg(test)]
mod tests;
use crate::{
    acquisition::Provider,
    census::{AccountState, CensusKeys},
    rules,
};
pub(crate) use adapters::{record_original, submit_smtp};
pub(crate) use report::report;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;
pub(crate) use synthetic::{SyntheticTransport, boundary, initialize_synthetic};

pub const SEMANTICS: &str = "invoice-evidence-exact-send-v1";
/// The sending identity of the bundled synthetic fixture.
pub(crate) const SYNTHETIC_SENDER: &str = "synthetic-approved-sender@example.test";
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub sources: BTreeMap<String, Source>,
    pub sends: Vec<Send>,
    pub synthetic_received: BTreeMap<String, Plan>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentEvidence {
    Unknown,
    Known {
        body: Body,
        attachments: Vec<Attachment>,
    },
    FilenameHint {
        names: Vec<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Body {
    Receipt,
    LinkOnly,
    Other,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub folder: String,
    pub uidvalidity: u32,
    pub uid: u32,
    pub message_id_hash: Option<String>,
    pub original: String,
    pub evidence: ContentEvidence,
    /// Read from the provider by original-message acquisition, not a bundled fixture.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acquired: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub account: String,
    pub state_revision: u64,
    pub report_version: String,
    pub selection: String,
    pub source: Source,
    pub content_digest: String,
    pub target_name: String,
    pub target: String,
    pub sending_address: String,
    pub semantics: String,
    pub confirmation_action: String,
    pub mode: SendMode,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    SyntheticTransport,
    NativeForwardPreparationOnly,
    /// SMTP submission of the exact original through the account's server.
    Smtp,
    /// Kept so schema-22 records still load; no new send uses it, because Mail
    /// cannot be bound to the confirmed original and sender.
    MailAppHandoff,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Intent,
    Uncertain,
    Completed,
    /// Certainly not submitted; a new send needs a new preview and confirmation.
    NotSent,
    /// Opened in the mail app. Whether the person sent it is unknown.
    HandedOff,
}
impl Status {
    /// Whether this send is finished and no longer blocks other work.
    pub fn settled(&self) -> bool {
        matches!(self, Self::Completed | Self::NotSent | Self::HandedOff)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub id: String,
    pub plan: Plan,
    pub status: Status,
    /// The SMTP server's answer, reply codes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp: Option<crate::send::smtp::Outcome>,
    /// What the owner recorded after an uncertain outcome. Absent for every other send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<crate::send::Resolution>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Diagnostic {
    ContentUnknown,
    FilenameHint { names: Vec<String> },
    DocumentContentTypes,
    UnsupportedFormat,
    LinkOnly,
    ReceiptBody,
    InvoiceSubject,
    RecipientHeld,
    BulkExcludesKeywordCopy,
    HandoffUnknown,
    IndependentEvidenceRequired,
    KeywordMimeRequired,
    WorkspaceRequired,
    RuleOmitted { rule: String, detail: String },
}

#[derive(Serialize)]
pub struct Row {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub role: String,
    pub reasons: Vec<Diagnostic>,
    pub evidence: ContentEvidence,
    pub predicted_targets: Vec<String>,
    pub matching_rules: Vec<String>,
    pub copy_eligibility: Vec<rules::proof::CopyEligibility>,
    pub handoff: String,
    pub accounting_acceptance: String,
    pub sender_proposal: Option<String>,
    pub send_available: bool,
}
#[derive(Serialize)]
pub struct Report {
    pub version: String,
    pub semantics: String,
    pub revision: u64,
    pub artifact: Option<String>,
    pub complete: bool,
    pub count: usize,
    pub eligible_business: usize,
    pub unresolved: usize,
    pub rows: Vec<Row>,
    pub warnings: Vec<Diagnostic>,
}
#[derive(Serialize)]
pub struct Preview {
    pub plan: Plan,
    pub receipt: String,
    pub native_available: bool,
    pub(crate) confirmation: Option<crate::confirmation::Request>,
}
pub(crate) fn confirmation_plan(preview: &Preview) -> crate::confirmation::Plan {
    use crate::confirmation::ExactAction;
    let p = &preview.plan;
    // The real adapters name the exact bytes they hand over; the receipt binds the rest.
    let message = || digest(p.source.original.as_bytes());
    let action = match p.mode {
        SendMode::Smtp => ExactAction::SmtpSend {
            account: p.account.clone(),
            operation: format!("Send invoice {} to {}", p.selection, p.target_name),
            target: p.target.clone(),
            sending_identity: p.sending_address.clone(),
            content_digest: message(),
        },
        SendMode::SyntheticTransport
        | SendMode::NativeForwardPreparationOnly
        | SendMode::MailAppHandoff => ExactAction::InvoiceSend {
            account: p.account.clone(),
            revision: p.state_revision,
            report_version: p.report_version.clone(),
            target_name: p.target_name.clone(),
            target: p.target.clone(),
            sending_identity: p.sending_address.clone(),
            source: p.selection.clone(),
            content_digest: p.content_digest.clone(),
        },
    };
    crate::confirmation::Plan {
        exact_receipt: preview.receipt.clone(),
        actions: vec![action],
    }
}
fn digest(value: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(value))
}
pub(crate) fn preview(
    state: &AccountState,
    provider: Provider,
    keys: &CensusKeys,
    selection: &str,
    target_name: &str,
    synthetic: bool,
    mode: SendMode,
    sending_address: &str,
) -> Result<Preview, String> {
    let fixture_mode = matches!(
        mode,
        SendMode::SyntheticTransport | SendMode::NativeForwardPreparationOnly
    );
    if fixture_mode && !synthetic {
        return Err("No original-message/native/SMTP adapter installed".into());
    }
    // Mail finds a message only by Message-ID across every account and picks the
    // sender itself, so it cannot be bound to the confirmed bytes and identity.
    if mode == SendMode::MailAppHandoff {
        return Err("Mail cannot be bound to the confirmed original and sender. Send by SMTP or forward it yourself.".into());
    }
    if !rules::config::mailbox(sending_address) {
        return Err("Invalid sending address".into());
    }
    let report = report(state, provider, synthetic, keys)?;
    if !report.complete
        || !report
            .rows
            .iter()
            .any(|r| r.id == selection && r.send_available)
    {
        return Err("Selection is not an eligible current report row".into());
    }
    let (folder, uidvalidity, uid) = census_location(state, provider, selection)?;
    let source_id = format!("{folder}/{uidvalidity}/{uid}");
    let source = state
        .invoices
        .sources
        .get(&source_id)
        .filter(|s| s.acquired || (fixture_mode && synthetic))
        .ok_or("Original content unavailable")?
        .clone();
    let folder = state
        .census
        .folders
        .get(&source.folder)
        .ok_or("Source folder missing")?;
    let message = folder.messages.get(&source.uid).ok_or("Source missing")?;
    if folder.uidvalidity != source.uidvalidity
        || message.headers.message_id_hash != source.message_id_hash
    {
        return Err("Source changed".into());
    }
    let w = state.workspace.as_ref().ok_or("Workspace missing")?;
    let (_, config) = rules::config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )?;
    let target = config
        .targets
        .get(target_name)
        .ok_or("Named target missing")?;
    let content_digest = digest(&serde_json::to_vec(&source).map_err(|_| "Source encoding")?);
    let plan = Plan {
        account: state.census.account.as_str().into(),
        state_revision: state.revision,
        report_version: report.version,
        selection: selection.into(),
        source,
        content_digest,
        target_name: target_name.into(),
        target: target.address.clone(),
        sending_address: sending_address.into(),
        semantics: SEMANTICS.into(),
        confirmation_action: match mode {
            SendMode::Smtp => "Send exactly this invoice to this target from this sending address",
            _ => "Send exactly this invoice to this target from this sending address (synthetic only)",
        }
        .into(),
        mode,
    };
    let receipt = keys.binding_for(
        b"mailroom/invoice-send-confirm/v1",
        &serde_json::to_vec(&(state, &plan)).map_err(|_| "Plan encoding")?,
    );
    Ok(Preview {
        plan,
        receipt,
        native_available: false,
        confirmation: None,
    })
}

/// The census folder, UIDVALIDITY and UID of a report row. Gmail rows name the
/// message by its Gmail ID, so any folder holding it serves.
pub(crate) fn census_location(
    state: &AccountState,
    provider: Provider,
    selection: &str,
) -> Result<(String, u32, u32), String> {
    state
        .census
        .folders
        .iter()
        .find_map(|(name, f)| {
            f.messages
                .values()
                .find(|m| {
                    if provider == Provider::Gmail {
                        m.gmail_message_id
                            .as_ref()
                            .is_some_and(|id| format!("gmail/{id}") == selection)
                    } else {
                        format!("{name}/{}/{}", f.uidvalidity, m.uid) == selection
                    }
                })
                .map(|m| (name.clone(), f.uidvalidity, m.uid))
        })
        .ok_or_else(|| "Source missing".into())
}

/// Records send intent for a plan the confirmation boundary approved and revalidated.
pub(crate) fn record_intent(
    state: &mut AccountState,
    preview: Preview,
    consent: &crate::confirmation::Confirmed,
    action: &str,
) -> Result<String, String> {
    {
        let receipt = consent.receipt();
        if !matches!(
            preview.plan.mode,
            SendMode::SyntheticTransport | SendMode::Smtp
        ) || action != preview.plan.confirmation_action
            || receipt.len() != 64
            || preview
                .receipt
                .as_bytes()
                .ct_eq(receipt.as_bytes())
                .unwrap_u8()
                != 1
        {
            return Err("Exact synthetic action confirmation rejected".into());
        }
        push_intent(state, preview)
    }
}

pub(super) fn push_intent(state: &mut AccountState, preview: Preview) -> Result<String, String> {
    if state.invoices.sends.iter().any(|s| !s.status.settled())
        || state.invoices.sends.len() >= 64
        || state.invoices.sends.iter().any(|s| s.id == preview.receipt)
    {
        return Err("Recover pending invoice operation first".into());
    }
    let id = preview.receipt;
    state.invoices.sends.push(Send {
        id: id.clone(),
        plan: preview.plan,
        status: Status::Intent,
        smtp: None,
        resolved: None,
    });
    Ok(id)
}

/// Records the owner's finding for an uncertain SMTP send.
/// Nothing is sent again: a send recorded as not sent needs a fresh preview and
/// confirmation, whose receipt differs because this state changed.
pub(crate) fn resolve(
    state: &mut AccountState,
    id: &str,
    resolution: crate::send::Resolution,
) -> Result<(), String> {
    let send = state
        .invoices
        .sends
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or("Unknown invoice operation")?;
    if send.status != Status::Uncertain || send.plan.mode != SendMode::Smtp {
        return Err("Only an uncertain send can be resolved".into());
    }
    send.status = match resolution {
        crate::send::Resolution::Sent => Status::Completed,
        crate::send::Resolution::NotSent => Status::NotSent,
    };
    send.resolved = Some(resolution);
    Ok(())
}

/// Census messages in the account's Sent mail with the same Message-ID as each
/// uncertain send's original, as `folder/uidvalidity/uid`. Read from the saved
/// census only; a fresh census shows a copy saved after the send.
pub(crate) fn sent_evidence(state: &AccountState) -> BTreeMap<String, Vec<String>> {
    state
        .invoices
        .sends
        .iter()
        .filter(|s| s.status == Status::Uncertain)
        .map(|s| {
            let matches = state
                .census
                .folders
                .iter()
                .flat_map(|(name, f)| {
                    f.messages
                        .values()
                        .filter(|m| {
                            m.headers.direction == crate::census::Direction::Sent
                                && m.headers.message_id_hash.is_some()
                                && m.headers.message_id_hash == s.plan.source.message_id_hash
                        })
                        .map(move |m| format!("{name}/{}/{}", f.uidvalidity, m.uid))
                })
                .collect();
            (s.id.clone(), matches)
        })
        .collect()
}

pub(crate) fn observe(
    state: &mut AccountState,
    transport: &SyntheticTransport,
    id: &str,
) -> Result<(), String> {
    let send = state
        .invoices
        .sends
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or("Unknown invoice operation")?;
    match transport.received().get(id) {
        Some(actual) if actual == &send.plan => {
            send.status = Status::Completed;
            Ok(())
        }
        _ => {
            send.status = Status::Uncertain;
            Ok(())
        }
    }
}
