mod execute;
use execute::execute;
#[cfg(test)]
mod copy_tests;
#[cfg(test)]
mod tests;

use super::*;
use crate::backlog::{
    Options, gmail_journal,
    protocol::{self, Status},
};

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CopyUndoSelection {
    Initial {
        run: String,
    },
    AfterClear {
        run: String,
        forward: usize,
        clear: usize,
    },
}

impl CopyUndoSelection {
    fn target(&self) -> acquisition::CopyUndoTarget<'_> {
        match self {
            Self::Initial { run } => acquisition::CopyUndoTarget::Initial { run },
            Self::AfterClear {
                run,
                forward,
                clear,
            } => acquisition::CopyUndoTarget::AfterClear {
                run,
                forward: *forward,
                clear: *clear,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {
        options: Options,
    },
    Confirm {
        options: Options,
        receipt: String,
        confirmed: bool,
    },
    Status {},
    Cancel {},
    Reconcile {
        run: String,
    },
    Resume {
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
    CopyConfirm {
        options: Options,
        receipt: String,
        confirmed: bool,
    },
    CopyContinue {
        run: String,
        work: acquisition::CopyContinuationWork,
    },
    CopyUndoPreview {
        target: CopyUndoSelection,
    },
    CopyUndoConfirm {
        target: CopyUndoSelection,
        receipt: String,
        confirmed: bool,
    },
    ClosePreview {
        run: String,
    },
    Close {
        run: String,
        receipt: String,
        confirmed: bool,
    },
    LabelPreview {
        options: Options,
    },
    LabelConfirm {
        options: Options,
        receipt: String,
        confirmed: bool,
    },
    LabelReconcile {
        operation: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    command: Action,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Move,
    Gmail,
}
impl Kind {
    fn installed(self, accounts: &acquisition::Accounts, id: Option<&AccountId>) -> bool {
        match self {
            Self::Move => accounts.backlog_installed(id),
            Self::Gmail => accounts.labels_installed(id),
        }
    }
    fn denial(self) -> &'static str {
        match self {
            Self::Move => {
                "Protocol MOVE requires a separately installed loopback message source. Production and ordinary read/create installations deny before repository or credential access."
            }
            Self::Gmail => {
                "Gmail label sorting requires a separately installed loopback X-GM-EXT-1 source. Production and ordinary read/create installations deny before repository or credential access."
            }
        }
    }
}
pub(in crate::server) async fn handle(
    state: State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    serve(state, body, Kind::Move).await
}
pub(in crate::server) async fn handle_gmail(
    state: State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    serve(state, body, Kind::Gmail).await
}
async fn serve(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
    kind: Kind,
) -> Response {
    if matches!(
        &body,
        Ok(Json(Request {
            command: Action::CopyConfirm { .. }
                | Action::CopyContinue { .. }
                | Action::CopyUndoPreview { .. }
                | Action::CopyUndoConfirm { .. },
            ..
        }))
    ) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"COPY workflow installation is held pending connected-runtime and interface acceptance."}))).into_response();
    }
    let is_vault = state.accounts.vault_live();
    let status_only = matches!(
        &body,
        Ok(Json(Request {
            command: Action::Status {},
            ..
        }))
    );
    if !status_only
        && !is_vault
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if kind.installed(accounts, None))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":kind.denial()}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !status_only
        && !is_vault
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if kind.installed(accounts, Some(&request.account)))
    {
        return failure(AcquisitionError::InvalidInput);
    }
    if is_vault
        && !state.accounts.live_writes()
        && matches!(
            request.command,
            Action::Confirm { .. }
                | Action::Resume { .. }
                | Action::Undo { .. }
                | Action::ResumeUndo { .. }
                | Action::Close { .. }
                | Action::LabelConfirm { .. }
        )
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Live provider writes are locked in Read-only mode. Enable Live writes in Settings to move messages or update Gmail labels."}))).into_response();
    }
    if matches!(request.command, Action::Cancel {}) {
        return match tokio::task::spawn_blocking(move || {
            let _admission = state.accounts.admission.lock().expect("admission");
            let matching = state
                .accounts
                .cancel_job_if(|j| j.backlog_account.as_ref() == Some(&request.account));
            if matching && kind == Kind::Gmail {
                crate::acquisition::gmail_backlog::boundary("gmail_cancel_signalled");
            }
            if matching {
                if let Some(handle) = state.accounts.worker.lock().expect("worker").take() {
                    handle.join().map_err(|_| AcquisitionError::Storage)?;
                }
            }
            let _operation = state.accounts.operation.lock().expect("operation");
            crate::acquisition::backlog::boundary("move_cancel_before_save");
            execute(&state, &request, Cancellation::default(), kind)
        })
        .await
        {
            Ok(Ok(value)) => Json(value).into_response(),
            Ok(Err(error)) => failure(error),
            Err(_) => failure(AcquisitionError::Storage),
        };
    }
    if matches!(
        request.command,
        Action::Confirm { .. }
            | Action::CopyConfirm { .. }
            | Action::CopyContinue {
                work: acquisition::CopyContinuationWork::Resume
                    | acquisition::CopyContinuationWork::ResumeUndo,
                ..
            }
            | Action::Resume { .. }
            | Action::Undo { .. }
            | Action::CopyUndoConfirm { .. }
            | Action::ResumeUndo { .. }
            | Action::LabelConfirm { .. }
    ) {
        let admission = match state.accounts.admission.try_lock() {
            Ok(g) => g,
            Err(_) => return failure(AcquisitionError::LifecyclePending),
        };
        let operation = match state.accounts.operation.try_lock() {
            Ok(g) => g,
            Err(_) => return failure(AcquisitionError::LifecyclePending),
        };
        let cancel = match state.accounts.reserve_for("backlog", |job| {
            job.backlog_account = Some(request.account.clone());
        }) {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        drop(operation);
        let worker = state.clone();
        let handle = std::thread::spawn(move || {
            let _operation = worker.accounts.operation.lock().expect("operation");
            let result = execute(&worker, &request, cancel, kind).map(|_| ());
            worker.accounts.finish(result);
        });
        *state.accounts.worker.lock().unwrap() = Some(handle);
        drop(admission);
        return Json(json!({"started":true})).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _admission = state.accounts.admission.lock().expect("admission");
        let _operation = if status_only {
            if state.accounts.job_running() {
                return Err(AcquisitionError::LifecyclePending);
            }
            state.accounts.operation.lock().expect("operation")
        } else {
            state
                .accounts
                .operation
                .try_lock()
                .map_err(|_| AcquisitionError::LifecyclePending)?
        };
        execute(&state, &request, Cancellation::default(), kind)
    })
    .await
    {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}
/// Unresolved unsubscribe or invoice sends must finish before sorting starts.
fn other_effects_pending(saved: &crate::census::AccountState) -> bool {
    saved.unsubscribe.history.iter().any(|e| {
        matches!(
            e.status,
            crate::unsubscribe::Status::Intent | crate::unsubscribe::Status::Uncertain
        )
    }) || saved.invoices.sends.iter().any(|s| !s.status.settled())
}
