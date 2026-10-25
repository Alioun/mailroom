use crate::{
    census::{AccountId, Census, Direction, MessageRecord, ScanState, parse_headers},
    rules::{
        self, EngineContext, config,
        proof::{self, Baseline, Pin, Proof},
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Documents {
    pub preset_yaml: String,
    pub override_yaml: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub documents: Documents,
    pub context: EngineContext,
    pub messages: Vec<Fixture>,
    pub as_of: i64,
    pub pins: Vec<Pin>,
    pub baseline: Baseline,
    pub previous: Option<Documents>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub headers: String,
    pub current_folder: String,
    #[serde(default)]
    pub delivery: FixtureDelivery,
    #[serde(default)]
    pub observed_flags: Option<BTreeSet<String>>,
}

#[derive(Default, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum FixtureDelivery {
    #[default]
    Unavailable,
    Unsupported,
    Ambiguous,
    Known {
        recipient: crate::census::delivery::Recipient,
    },
}

pub fn artifact(
    documents: &Documents,
    context: &EngineContext,
) -> Result<rules::CompiledArtifact, String> {
    let (preset, overrides) = config::documents(&documents.preset_yaml, &documents.override_yaml)?;
    rules::compile(&preset, &overrides, context)
}
pub fn synthetic_census(input: &PreviewInput) -> Result<Census, String> {
    if input.messages.is_empty() || input.messages.len() > 200 {
        return Err("Supply 1..200 synthetic header fixtures".into());
    }
    let mut census =
        Census::new(AccountId::new(&input.context.account).map_err(|_| "Invalid account")?);
    let mut ids = BTreeSet::new();
    for (i, m) in input.messages.iter().enumerate() {
        config::identifier(&m.id)?;
        if !ids.insert(&m.id) {
            return Err("Duplicate synthetic fixture ID".into());
        }
        let headers = parse_headers(m.headers.as_bytes(), Direction::Received)
            .map_err(|_| "Invalid header-only fixture")?;
        let uid = (i + 1) as u32;
        let folder = census
            .folders
            .entry(m.current_folder.clone())
            .or_insert_with(|| crate::census::FolderCensus {
                uidvalidity: 1,
                state: ScanState::Complete,
                live_uids: BTreeSet::new(),
                messages: Default::default(),
            });
        folder.live_uids.insert(uid);
        folder.messages.insert(
            uid,
            MessageRecord {
                delivery: match &m.delivery {
                    FixtureDelivery::Unavailable => crate::census::delivery::DeliveryEvidence::Unavailable,
                    FixtureDelivery::Unsupported => crate::census::delivery::DeliveryEvidence::Unsupported,
                    FixtureDelivery::Ambiguous => crate::census::delivery::DeliveryEvidence::Ambiguous,
                    FixtureDelivery::Known { recipient } => match &input.context.recipient_evidence {
                        rules::EvidenceSource::SyntheticEnvelope { source } => crate::census::delivery::DeliveryEvidence::Known { recipient: recipient.clone(), source: source.clone(), message: crate::census::delivery::DeliveryEvidence::message_binding(&format!("{}/1/{uid}", m.current_folder), headers.message_id_hash.as_deref()) },
                        _ => return Err("Known fixture envelope requires a separately explicit synthetic envelope context".into()),
                    }
                },
                uid,
                size: m.headers.len() as u64,
                internal_date: "24-Oct-2026 00:00:00 +0000".into(),
                flags_observed: m.observed_flags.is_some(),
                flags: m.observed_flags.clone().unwrap_or_default(),
                gmail_message_id: Some(m.id.clone()),
                gmail_thread_id: None,
                gmail_labels: BTreeSet::new(),
                headers,
            },
        );
    }
    census.acquisition_state = ScanState::Complete;
    Ok(census)
}
pub fn preview(input: &PreviewInput) -> Result<Proof, String> {
    let compiled = artifact(&input.documents, &input.context)?;
    let previous = input
        .previous
        .as_ref()
        .map(|d| artifact(d, &input.context))
        .transpose()?;
    proof::prove(
        compiled,
        &synthetic_census(input)?,
        input.as_of,
        &input.pins,
        input.baseline,
        previous.as_ref(),
    )
}
