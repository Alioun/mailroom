use super::*;
use crate::ledger::{self, Batch};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::server) enum Action {
    Open,
    Preview {
        batch: Batch,
    },
    Confirm {
        batch: Batch,
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
    // Deny before repository construction or credential access.
    if !state.accounts.synthetic() || state.accounts.ledger.is_none() {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"No production ledger adapter is installed. Local bookkeeping grants no account-write authority."}))).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for census completion".to_string()); }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let installed = state.accounts.ledger.as_ref().ok_or("No installed ledger")?;
        let guard = installed.lock()?;
        let snapshot = guard.read()?;
        let changed = ledger::reconcile(&mut saved, &snapshot, account.provider)?;
        if changed { store.save_state(&mut saved).map_err(|e| e.to_string())?; }
        match request.command {
            Action::Open => Ok(json!(ledger::view(&saved, snapshot, changed))),
            Action::Preview { batch } => ledger::review(&saved, &snapshot, account.provider, store.keys(), &batch).map(|v| json!(v)),
            Action::Confirm { batch, receipt, confirmed } => {
                use subtle::ConstantTimeEq;
                if !confirmed { return Err("Exact ledger review requires confirmation".into()); }
                let review = ledger::review(&saved, &snapshot, account.provider, store.keys(), &batch)?;
                if !bool::from(review.receipt.as_bytes().ct_eq(receipt.as_bytes())) { return Err("Ledger review changed. Reload and review again".into()); }
                let new_source = guard.replace(&snapshot.content_version, &review.resulting_document)?;
                boundary("file");
                ledger::reconcile(&mut saved, &new_source, account.provider)?;
                // This second replacement is not atomic with the authoritative file save.
                // Recovery always reconstructs from the file and never retries its old write.
                store.save_state(&mut saved).map_err(|_| "Ledger file saved. Projection save unconfirmed. Reload to reconcile from the file; do not repeat the old confirmation".to_string())?;
                Ok(json!(ledger::view(&saved, new_source, true)))
            }
        }
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":error}))).into_response(),
        Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Ledger action interrupted. Reload and reconcile from the authoritative file before reviewing again."}))).into_response(),
    }
}
fn boundary(name: &str) {
    #[cfg(test)]
    if std::env::var("MAILROOM_LEDGER_BOUNDARY").ok().as_deref() == Some(name) {
        if let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") {
            let _ = std::fs::write(
                std::path::Path::new(&root).join(format!("ledger-reached-{name}")),
                b"synthetic boundary",
            );
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    let _ = name;
}
