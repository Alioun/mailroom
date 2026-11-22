use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct WorkspaceSaveRequest {
    account: AccountId,
    revision: u64,
    input: WorkspaceInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct DownloadRequest {
    account: AccountId,
    receipt: String,
    confirmed: bool,
}

enum WorkspaceAction {
    Open,
    Save {
        revision: u64,
        input: WorkspaceInput,
    },
    Preview,
    Download {
        receipt: String,
    },
}

pub(in crate::server) async fn workspace_open(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    workspace_action(state, request.account, WorkspaceAction::Open).await
}

pub(in crate::server) async fn workspace_save(
    State(state): State<Arc<AppState>>,
    body: Result<Json<WorkspaceSaveRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    workspace_action(
        state,
        request.account,
        WorkspaceAction::Save {
            revision: request.revision,
            input: request.input,
        },
    )
    .await
}

pub(in crate::server) async fn proof_preview(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    workspace_action(state, request.account, WorkspaceAction::Preview).await
}

pub(in crate::server) async fn proof_download(
    State(state): State<Arc<AppState>>,
    body: Result<Json<DownloadRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !request.confirmed {
        return failure(AcquisitionError::InvalidInput);
    }
    workspace_action(
        state,
        request.account,
        WorkspaceAction::Download {
            receipt: request.receipt,
        },
    )
    .await
}

async fn workspace_action(
    state: Arc<AppState>,
    id: AccountId,
    action: WorkspaceAction,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job_running() {
            return Err(AcquisitionError::InvalidInput);
        }
        let repo = state.accounts.repository()?;
        let account = repo.load(&id)?;
        let store = repo.store(&account)?;
        let mut saved = store.load_state()?;
        let context = repo.workspace_context(&account, &store)?;
        let ledger_guard = state
            .accounts
            .ledger()?
            .map(|l| l.lock())
            .transpose()
            .map_err(|_| AcquisitionError::Storage)?;
        sync_ledger(&mut saved, &store, &context, ledger_guard.as_ref())
            .map_err(|_| AcquisitionError::InvalidInput)?;
        match action {
            WorkspaceAction::Open => Ok(json!(workspace::view(&saved, &context))),
            WorkspaceAction::Save { revision, input } => {
                if revision != saved.revision {
                    return Err(AcquisitionError::InvalidInput);
                }
                crate::ledger::validate_workspace_edit(&saved, &input)
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                workspace::validate(&input, &saved, &context)
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                let revision = saved.workspace.as_ref().map_or(Ok(1), |w| {
                    w.revision.checked_add(1).ok_or(AcquisitionError::Storage)
                })?;
                saved.workspace = Some(Workspace { revision, input });
                if let Some(guard) = &ledger_guard {
                    let snapshot = guard.read().map_err(|_| AcquisitionError::Storage)?;
                    crate::ledger::reconcile(&mut saved, &snapshot, &context)
                        .map_err(|_| AcquisitionError::InvalidInput)?;
                }
                store.save_state(&mut saved)?;
                Ok(json!(workspace::view(&saved, &context)))
            }
            WorkspaceAction::Preview => {
                let proof = workspace::preview(&saved, &context, store.keys())
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                ledger_fresh(&saved, ledger_guard.as_ref())
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!(proof))
            }
            WorkspaceAction::Download { receipt } => {
                let preview =
                    workspace::confirm_export(&mut saved, &context, store.keys(), &receipt)
                        .map_err(|_| AcquisitionError::InvalidInput)?;
                ledger_fresh(&saved, ledger_guard.as_ref())
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                store.save_state(&mut saved)?;
                Ok(json!({
                    "filename": preview.filename,
                    "sieve": preview.proof.artifact.executable(),
                    "identity": preview.proof.artifact.identity(),
                    "workspace": workspace::view(&saved, &context)
                }))
            }
        }
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

pub(super) fn ledger_fresh(
    saved: &crate::census::AccountState,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<(), String> {
    if let Some(guard) = guard {
        if saved.workspace.is_some() && guard.read()?.content_version != saved.ledger.source_version
        {
            return Err("Ledger changed while proving. Review fresh proof".into());
        }
    }
    Ok(())
}

pub(super) fn sync_ledger(
    saved: &mut crate::census::AccountState,
    store: &crate::census::EncryptedStore,
    context: &workspace::WorkspaceContext,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<(), String> {
    if store.recovery_pending() {
        return Ok(());
    }
    if let Some(guard) = guard {
        let snapshot = guard.read()?;
        if saved.workspace.is_some() && crate::ledger::reconcile(saved, &snapshot, context)? {
            store.save_state(saved).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
