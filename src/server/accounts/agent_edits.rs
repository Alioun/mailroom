use super::*;
use crate::{agent::Proposal, rules::agent_edits::Disposition};
use std::collections::BTreeSet;

/// The reviewed summary the proposal answers. It is regenerated from the saved
/// census and must reproduce the same receipt.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    as_of: i64,
    version: String,
    approved: BTreeSet<String>,
    receipt: String,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {},
    Confirm {
        disposition: Disposition,
        receipt: String,
        confirmed: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    summary: Summary,
    proposal: Proposal,
    command: Action,
}

const CHANGED: &str = "Summary or census changed. Review a fresh summary first";

pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(request) => request,
        Err(error) => return error,
    };
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        use subtle::ConstantTimeEq;
        let _admission = state.accounts.admission.lock().expect("account admission");
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job_running() {
            return Err("Wait for census completion".into());
        }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let context = repo.workspace_context(&account, &store).map_err(|e| e.to_string())?;
        let guard = state.accounts.ledger().map_err(|e| e.to_string())?.map(|l| l.lock()).transpose()?;
        sync_ledger(&mut saved, &store, &context, guard.as_ref())?;
        let mut report = saved
            .census
            .report(store.keys(), request.summary.as_of, account.provider == Provider::Gmail)
            .map_err(|_| CHANGED)?;
        if store.recovery_pending() {
            report.acquisition.historical();
        }
        if report.version != request.summary.version {
            return Err(CHANGED.into());
        }
        let summary = report.summary_preview(store.keys(), &request.summary.approved).map_err(|_| CHANGED)?;
        if !bool::from(summary.receipt.as_bytes().ct_eq(request.summary.receipt.as_bytes())) {
            return Err(CHANGED.into());
        }
        match request.command {
            Action::Preview {} => {
                let review = crate::rules::agent_edits::review(&saved, &context, store.keys(), &request.proposal, &summary.summary)?;
                ledger_fresh(&saved, guard.as_ref())?;
                Ok(json!(review))
            }
            Action::Confirm { disposition, receipt, confirmed } => {
                if store.recovery_pending() { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
                if !confirmed { return Err("Confirm the exact displayed rule edit review first".into()); }
                crate::rules::agent_edits::confirm(&mut saved, &context, store.keys(), &request.proposal, &summary.summary, disposition, &receipt)?;
                ledger_fresh(&saved, guard.as_ref())?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
                let status = match disposition { Disposition::Accept => "local_rules_saved", Disposition::Reject => "edits_rejected" };
                Ok(json!({"status": status, "workspace": workspace::view(&saved, &context)}))
            }
        }
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": error}))).into_response(),
        Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": "Agent edit action interrupted. Reload saved state before reviewing again."}))).into_response(),
    }
}

#[cfg(test)]
mod tests;
