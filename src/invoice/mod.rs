mod report;
mod synthetic;
#[cfg(test)]
mod tests;
use crate::{
    acquisition::Provider,
    census::{AccountState, CensusKeys},
    rules,
};
pub(crate) use report::report;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;
pub(crate) use synthetic::{SyntheticTransport, boundary, initialize_synthetic};

pub const SEMANTICS: &str = "invoice-evidence-exact-send-v1";
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Intent,
    Uncertain,
    Completed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub id: String,
    pub plan: Plan,
    pub status: Status,
}
#[derive(Serialize)]
pub struct Row {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub role: String,
    pub reasons: Vec<String>,
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
    pub warnings: Vec<String>,
}
#[derive(Serialize)]
pub struct Preview {
    pub plan: Plan,
    pub receipt: String,
    pub native_available: bool,
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
    fallback: bool,
) -> Result<Preview, String> {
    if !synthetic {
        return Err("No original-message/native/SMTP adapter installed".into());
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
    let source_id = if provider == Provider::Gmail {
        state
            .census
            .folders
            .iter()
            .find_map(|(name, f)| {
                f.messages
                    .values()
                    .find(|m| {
                        m.gmail_message_id
                            .as_ref()
                            .is_some_and(|id| format!("gmail/{id}") == selection)
                    })
                    .map(|m| format!("{name}/{}/{}", f.uidvalidity, m.uid))
            })
            .ok_or("Source missing")?
    } else {
        selection.into()
    };
    let source = state
        .invoices
        .sources
        .get(&source_id)
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
        sending_address: "synthetic-approved-sender@example.test".into(),
        semantics: SEMANTICS.into(),
        confirmation_action:
            "Send exactly this invoice to this target from this sending address (synthetic only)"
                .into(),
        mode: if fallback {
            SendMode::NativeForwardPreparationOnly
        } else {
            SendMode::SyntheticTransport
        },
    };
    let receipt = keys.binding_for(
        b"mailroom/invoice-send-confirm/v1",
        &serde_json::to_vec(&(state, &plan)).map_err(|_| "Plan encoding")?,
    );
    Ok(Preview {
        plan,
        receipt,
        native_available: false,
    })
}

pub(crate) struct SyntheticConfirmation;

impl SyntheticConfirmation {
    pub(crate) fn confirm(
        &self,
        state: &mut AccountState,
        preview: Preview,
        receipt: &str,
        action: &str,
    ) -> Result<String, String> {
        if preview.plan.mode != SendMode::SyntheticTransport
            || action != preview.plan.confirmation_action
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
        if state
            .invoices
            .sends
            .iter()
            .any(|s| s.status != Status::Completed)
            || state.invoices.sends.len() >= 64
        {
            return Err("Recover pending invoice operation first".into());
        }
        let id = preview.receipt;
        state.invoices.sends.push(Send {
            id: id.clone(),
            plan: preview.plan,
            status: Status::Intent,
        });
        Ok(id)
    }
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
