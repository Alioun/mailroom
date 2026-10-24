use super::*;
use crate::unsubscribe::{self, InstalledSynthetic, ReviewInput};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {
        as_of: i64,
    },
    Preview {
        input: ReviewInput,
    },
    Confirm {
        input: ReviewInput,
        receipt: String,
        confirmation_action: String,
        mail_confirmation_action: Option<String>,
    },
    Recover {
        entry: String,
        as_of: i64,
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
    if !state.accounts.synthetic() && !matches!(request.command, Action::Open { .. }) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Production unsubscribe, native confirmation, send, rule deployment and alias mutations are denied"}))).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for census completion".to_owned()); }
        let repo = state.accounts.repository().map_err(|_| "Account unavailable")?;
        let account = repo.load(&request.account).map_err(|_| "Account unavailable")?;
        let store = repo.store(&account).map_err(|_| "Account storage unavailable")?;
        let mut saved = store.load_state().map_err(|_| "Account state unavailable")?;
        let mut installed = match &state.accounts.source { AccountSource::Synthetic { mailboxes, .. } => Some(InstalledSynthetic::open(mailboxes, &store, &saved)?), _ => None };
        let as_of = match request.command {
            Action::Open { as_of } => as_of,
            Action::Preview { input } => return unsubscribe::preview(&saved, account.provider, store.keys(), &input, installed.as_ref()).map(|p| json!(p)),
            Action::Confirm { input, receipt, confirmation_action, mail_confirmation_action } => {
                if saved.invoices.sends.iter().any(|s| s.status != crate::invoice::Status::Completed) || saved.backlog.iter().any(|r| !matches!(r.status, crate::backlog::Status::Completed | crate::backlog::Status::Failed)) { return Err("Reconcile pending invoice/backlog work first".into()); }
                let preview = unsubscribe::preview(&saved, account.provider, store.keys(), &input, installed.as_ref())?;
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "Invalid clock")?.as_secs() as i64;
                let ids = unsubscribe::confirm(&mut saved, &preview, &receipt, &confirmation_action, mail_confirmation_action.as_deref(), now)?;
                store.save_state(&mut saved).map_err(|_| "Persist unsubscribe intent")?;
                unsubscribe::boundary("before");
                for (id, action) in ids.iter().zip(preview.actions.iter()) {
                    if matches!(action.method, unsubscribe::Method::OneClick | unsubscribe::Method::LocalMail) {
                        let entry = saved.unsubscribe.history.iter().find(|e| &e.id == id).ok_or("Missing intent")?.clone();
                        let source = unsubscribe::source(&saved, &action.source).ok_or("Source changed")?;
                        let adapter = installed.as_mut().ok_or("No installed fake transport")?;
                        adapter.submit(id, &entry, source, &action.recipient)?;
                        unsubscribe::boundary("effect");
                        unsubscribe::observe(&mut saved, adapter, id)?;
                        store.save_state(&mut saved).map_err(|_| "Persist unsubscribe observation")?;
                    }
                }
                input.as_of
            },
            Action::Recover { entry, as_of } => {
                unsubscribe::observe(&mut saved, installed.as_ref().ok_or("No installed fake transport")?, &entry)?;
                store.save_state(&mut saved).map_err(|_| "Persist recovery observation")?;
                as_of
            },
        };
        let report = unsubscribe::report(&saved, account.provider, store.keys(), as_of, installed.as_ref())?;
        Ok::<_, String>(json!({"report":report,"history":saved.unsubscribe.history,"mutes":saved.unsubscribe.mutes,"synthetic":installed.is_some(),"received":installed.as_ref().map(|a| a.received()),"production_available":false,"q20_scope":"separate_subscriptions"}))
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Unsubscribe review rejected. Reload the exact preview. Reconcile uncertain effects without retrying. User-edited mutes are preserved."}))).into_response(),
    }
}
