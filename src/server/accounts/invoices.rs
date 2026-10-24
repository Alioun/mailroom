use super::*;
use crate::invoice::{self, SyntheticConfirmation, SyntheticTransport};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Open,
    Preview {
        selection: String,
        target: String,
    },
    PrepareMailApp {
        selection: String,
        target: String,
    },
    Send {
        selection: String,
        target: String,
        receipt: String,
        confirmation_action: String,
    },
    Recover {
        send: String,
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
    if !state.accounts.synthetic() && !matches!(request.command, Action::Open) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"No production invoice, native confirmation or SMTP adapter is installed"}))).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for census completion".to_string()); }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let synthetic = state.accounts.synthetic();
        if synthetic && invoice::initialize_synthetic(&mut saved) { store.save_state(&mut saved).map_err(|e| e.to_string())?; }
        let mut transport = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } => Some(SyntheticTransport::open(mailboxes, &store, &saved)?),
            _ => None,
        };
        match request.command {
            Action::Open => {},
            Action::Preview { selection, target } => return invoice::preview(&saved, account.provider, store.keys(), &selection, &target, synthetic, false).map(|p| json!(p)),
            Action::PrepareMailApp { selection, target } => return invoice::preview(&saved, account.provider, store.keys(), &selection, &target, synthetic, true).map(|p| json!({"preview":p,"status":"preparation_only", "limitation":"No native forward-original integration or OS dialog is installed. Nothing launched. Original MIME and target are retained in this protected preview; no blank mailto compose is substituted."})),
            Action::Send { selection, target, receipt, confirmation_action } => {
                if saved.backlog.iter().any(|r| !matches!(r.status, crate::backlog::Status::Completed | crate::backlog::Status::Failed)) { return Err("Finish the backlog operation first".into()); }
                let preview = invoice::preview(&saved, account.provider, store.keys(), &selection, &target, synthetic, false)?;
                let confirmation = match state.accounts.source { AccountSource::Synthetic { .. } => SyntheticConfirmation, _ => return Err("No installed confirmation adapter".into()) };
                let id = confirmation.confirm(&mut saved, preview, &receipt, &confirmation_action)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
                invoice::boundary("before");
                let send = saved.invoices.sends.last().ok_or("Missing intent")?;
                let installed = transport.as_mut().ok_or("No installed synthetic transport")?;
                installed.submit(&id, &send.plan)?;
                invoice::boundary("effect");
                invoice::observe(&mut saved, installed, &id)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Recover { send } => {
                invoice::observe(&mut saved, transport.as_ref().ok_or("No installed synthetic transport")?, &send)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
        }
        let report = invoice::report(&saved, account.provider, synthetic, store.keys())?;
        Ok::<_, String>(json!({"report":report,"sends":saved.invoices.sends,"synthetic":synthetic,"received":transport.as_ref().map(|t| t.received()),"targets":saved.workspace.as_ref().and_then(|w| crate::rules::config::documents(&w.input.documents.preset_yaml, &w.input.documents.override_yaml).ok()).map(|(_,c)| c.targets.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()}))
    }).await {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Invoice action rejected. Reload the report and exact preview. Uncertain submissions require reconciliation, never blind retry."}))).into_response(),
    }
}
