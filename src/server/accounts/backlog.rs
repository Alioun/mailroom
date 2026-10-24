use super::*;
use crate::backlog::{self, Options, SyntheticMailbox};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Open,
    Preview {
        options: Options,
    },
    Start {
        options: Options,
        receipt: String,
        confirmed: bool,
    },
    Advance {
        run: String,
    },
    Resume {
        run: String,
    },
    Stop {
        run: String,
    },
    UndoPreview {
        run: String,
    },
    Undo {
        run: String,
        receipt: String,
        confirmed: bool,
    },
    ResumeUndo {
        run: String,
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
    if !state.accounts.synthetic()
        && !matches!(request.command, Action::Open | Action::Preview { .. })
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"No production backlog write capability is installed"})),
        )
            .into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for the account action".into()); }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let mut mailbox = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } if saved.census.acquisition_state == ScanState::Complete => Some(SyntheticMailbox::open(mailboxes, &store, &saved.census).map_err(|e| e.to_string())?),
            _ => None,
        };
        match request.command {
            Action::Open => {},
            Action::Preview { options } => return backlog::preview(&saved, account.provider, store.keys(), options, mailbox.as_ref()).map(|p| json!(p)),
            Action::Start { options, receipt, confirmed } => {
                if saved.invoices.sends.iter().any(|s| s.status != crate::invoice::Status::Completed) { return Err("Recover the pending invoice submission first".into()); }
                if !confirmed { return Err("Explicit synthetic confirmation required".into()); }
                let preview = backlog::preview(&saved, account.provider, store.keys(), options, mailbox.as_ref())?;
                backlog::start(&mut saved, preview, &receipt)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Advance { run } => backlog::advance(&store, &mut saved, mailbox.as_mut().ok_or("No installed synthetic mailbox")?, &run)?,
            Action::Resume { run } => {
                backlog::resume(&mut saved, &run)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Stop { run } => { backlog::stop(&mut saved, &run)?; store.save_state(&mut saved).map_err(|e| e.to_string())?; },
            Action::UndoPreview { run } => return backlog::undo_preview(&saved, mailbox.as_ref().ok_or("No installed synthetic mailbox")?, store.keys(), &run).map(|p| json!(p)),
            Action::Undo { run, receipt, confirmed } => {
                if !confirmed { return Err("Explicit conditional undo confirmation required".into()); }
                backlog::undo(&store, &mut saved, mailbox.as_mut().ok_or("No installed synthetic mailbox")?, &run, Some(&receipt))?;
            },
            Action::ResumeUndo { run } => {
                if !saved.backlog.iter().any(|r| r.id == run && r.undo_requested) { return Err("No previously confirmed undo".into()); }
                backlog::undo(&store, &mut saved, mailbox.as_mut().ok_or("No installed synthetic mailbox")?, &run, None)?;
            },
        }
        Ok::<_, String>(json!({"runs": saved.backlog, "synthetic": mailbox.is_some(), "mailbox": mailbox.as_ref().map(|m| &m.snapshot().census)}))
    }).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(_)) | Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Backlog action rejected. Refresh the preview or recover the saved run. Existing files are retained."}))).into_response(),
    }
}
