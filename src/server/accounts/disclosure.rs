use super::*;
use crate::{
    census::{
        CensusKeys, EncryptedStore,
        report::{Report, SummaryPreview},
    },
    confirmation::{ExactAction, Plan},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

const PURPOSE: &[u8] = b"mailroom/agent-summary-disclosure/v1";

fn plan(
    account: &AccountId,
    keys: &CensusKeys,
    report: &Report,
    preview: &SummaryPreview,
    approved: &BTreeSet<String>,
) -> Result<Plan, AcquisitionError> {
    let content_digest =
        data_encoding::HEXLOWER.encode(&Sha256::digest(preview.summary.as_bytes()));
    let payload = Zeroizing::new(
        serde_json::to_vec(&(
            account,
            &report.version,
            approved,
            &preview.receipt,
            &content_digest,
        ))
        .map_err(|_| AcquisitionError::InvalidInput)?,
    );
    Ok(Plan {
        actions: vec![ExactAction::AgentSummary {
            account: account.as_str().into(),
            report_version: report.version.clone(),
            content_digest,
            clear_domain_count: approved.len(),
        }],
        exact_receipt: keys.binding_for(PURPOSE, &payload),
    })
}

pub(super) fn confirm(
    state: &AppState,
    account: &Account,
    store: &EncryptedStore,
    report: &Report,
    preview: &SummaryPreview,
    approved: &BTreeSet<String>,
) -> Result<(), AcquisitionError> {
    let initial = plan(&account.id, store.keys(), report, preview, approved)?;
    let mut boundary = state.accounts.confirmations.lock().expect("confirmations");
    let (handle, _) = boundary
        .offer(initial.clone())
        .map_err(|_| AcquisitionError::InvalidInput)?;
    boundary
        .confirm(&handle, &initial, || {
            let census = store.load().map_err(|_| "Disclosure state unavailable")?;
            let mut current = census
                .report(
                    store.keys(),
                    report.as_of,
                    account.provider == Provider::Gmail,
                )
                .map_err(|_| "Disclosure state unavailable")?;
            if store.recovery_pending() {
                current.acquisition.historical();
            }
            let current_preview = current
                .summary_preview(store.keys(), approved)
                .map_err(|_| "Disclosure selection changed")?;
            plan(
                &account.id,
                store.keys(),
                &current,
                &current_preview,
                approved,
            )
            .map_err(|_| "Disclosure plan changed".into())
        })
        .map(|_| ())
        .map_err(|_| AcquisitionError::InvalidInput)
}

#[cfg(test)]
mod tests;
