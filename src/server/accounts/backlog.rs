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
        if state.accounts.job_running() {
            return Err("Wait for the account action".into());
        }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        if store.recovery_pending() && !matches!(request.command, Action::Open) { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let ledger_guard = if matches!(request.command, Action::Stop { .. }) { None } else { state.accounts.ledger().map_err(|e| e.to_string())?.map(|l| l.lock()).transpose()? };
        // Stop must persist even when the authoritative ledger needs repair.
        if !matches!(request.command, Action::Stop { .. }) {
            sync_ledger(&mut saved, &store, &repo.workspace_context(&account, &store).map_err(|e| e.to_string())?, ledger_guard.as_ref())?;
        }
        let mut mailbox = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } if saved.census.acquisition_state == ScanState::Complete => Some(SyntheticMailbox::open(mailboxes, &store, &saved.census).map_err(|e| e.to_string())?),
            _ => None,
        };
        match request.command {
            Action::Open => {}
            Action::Preview { options } => {
                if saved.workspace.is_none() {
                    return Err("no_local_rules".into());
                }
                if saved.census.acquisition_state != ScanState::Complete {
                    return Err("scan_incomplete".into());
                }
                return backlog::preview(
                    &saved,
                    account.provider,
                    store.keys(),
                    options,
                    mailbox.as_ref(),
                )
                .map(|p| json!(p));
            }
            Action::Start { options, receipt, confirmed } => {
                if saved.unsubscribe.history.iter().any(|e| matches!(e.status, crate::unsubscribe::Status::Intent | crate::unsubscribe::Status::Uncertain)) { return Err("pending_unsubscribe".into()); }
                if saved.invoices.sends.iter().any(|s| !s.status.settled()) { return Err("pending_invoice".into()); }
                if !confirmed { return Err("confirmation_required".into()); }
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
                if !confirmed { return Err("confirmation_required".into()); }
                backlog::undo(&store, &mut saved, mailbox.as_mut().ok_or("No installed synthetic mailbox")?, &run, Some(&receipt))?;
            },
            Action::ResumeUndo { run } => {
                if !saved.backlog.iter().any(|r| r.id == run && r.undo_requested) { return Err("no_confirmed_undo".into()); }
                backlog::undo(&store, &mut saved, mailbox.as_mut().ok_or("No installed synthetic mailbox")?, &run, None)?;
            },
        }
        Ok::<_, String>(json!({"protocol_installed": repo.backlog_installed(Some(&account.id)), "protocol_history": !saved.protocol_backlog.runs.is_empty(), "gmail_installed": repo.labels_installed(Some(&account.id)), "gmail_history": !saved.gmail_backlog.runs.is_empty(), "has_local_rules": saved.workspace.is_some(), "runs": saved.backlog, "synthetic": mailbox.is_some(), "mailbox": mailbox.as_ref().map(|m| &m.snapshot().census)}))
    }).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(reason)) => {
            let (code, error) = match reason.as_str() {
                "no_local_rules" => (
                    "no_local_rules",
                    "Create local rules first before previewing a backlog sort.",
                ),
                "scan_incomplete" => (
                    "scan_incomplete",
                    "Complete a mailbox scan before previewing a backlog sort.",
                ),
                "pending_unsubscribe" => (
                    "pending_unsubscribe",
                    "Reconcile pending unsubscribe actions before starting a backlog sort.",
                ),
                "pending_invoice" => (
                    "pending_invoice",
                    "Resolve pending invoice sends before starting a backlog sort.",
                ),
                "Wait for the account action" => (
                    "job_running",
                    "Wait for the active account operation to finish first.",
                ),
                "Fresh provider evidence and reconciliation required after recovery" => (
                    "recovery_pending",
                    "Run a fresh scan to reconcile this account after recovery.",
                ),
                _ => (
                    "backlog_rejected",
                    "Backlog preview or action could not run. Check that local rules are saved and try again.",
                ),
            };
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"code": code, "error": error})),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"code": "backlog_rejected", "error": "Backlog preview or action could not run. Try again."})),
        )
            .into_response(),
    }
}

#[cfg(test)]
#[path = "backlog_archive_tests.rs"]
mod archive_tests;
