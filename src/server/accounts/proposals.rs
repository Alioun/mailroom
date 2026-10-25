use super::*;
use crate::rules::proposals::{self, Disposition, Selection};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {},
    Preview {
        selection: Selection,
        disposition: Disposition,
    },
    Confirm {
        selection: Selection,
        disposition: Disposition,
        receipt: String,
        confirmed: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    command: Action,
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let _admission = state.accounts.admission.lock().expect("account admission");
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) {
            return Err("Wait for census completion".into());
        }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let guard = state.accounts.ledger.as_ref().map(|l| l.lock()).transpose()?;
        sync_ledger(&mut saved, &store, account.provider, guard.as_ref())?;
        match request.command {
            Action::Open {} => Ok(json!(proposals::generate(&saved, account.provider, store.keys())?)),
            Action::Preview { selection, disposition } => {
                let review = proposals::review(&saved, account.provider, store.keys(), &selection, disposition)?;
                ledger_fresh(&saved, guard.as_ref())?;
                Ok(json!(review))
            }
            Action::Confirm { selection, disposition, receipt, confirmed } => {
                if store.recovery_pending() { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
                if !confirmed { return Err("Confirm the exact displayed proposal review first".into()); }
                proposals::confirm(&mut saved, account.provider, store.keys(), &selection, disposition, &receipt)?;
                ledger_fresh(&saved, guard.as_ref())?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
                Ok(json!(proposals::generate(&saved, account.provider, store.keys())?))
            }
        }
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":error}))).into_response(),
        Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Proposal action interrupted. Reload saved state before reviewing again."}))).into_response(),
    }
}
