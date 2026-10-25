use super::*;
use crate::backlog::{
    Options,
    protocol::{self, Status},
};

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
    let status_only = matches!(
        &body,
        Ok(Json(Request {
            command: Action::Status {},
            ..
        }))
    );
    if !status_only
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if accounts.backlog_installed(None))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Protocol MOVE requires a separately installed loopback message source. Production and ordinary read/create installations deny before repository or credential access."}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !status_only
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if accounts.backlog_installed(Some(&request.account)))
    {
        return failure(AcquisitionError::InvalidInput);
    }
    if matches!(request.command, Action::Cancel {}) {
        return match tokio::task::spawn_blocking(move || {
            let _admission = state.accounts.admission.lock().expect("admission");
            let matching = {
                let job = state.accounts.job.lock().expect("job");
                if let Some(job) = job
                    .as_ref()
                    .filter(|j| j.backlog_account.as_ref() == Some(&request.account))
                {
                    job.cancel.cancel();
                    true
                } else {
                    false
                }
            };
            if matching {
                if let Some(handle) = state.accounts.worker.lock().expect("worker").take() {
                    handle.join().map_err(|_| AcquisitionError::Storage)?;
                }
            }
            let _operation = state.accounts.operation.lock().expect("operation");
            crate::acquisition::backlog::boundary("move_cancel_before_save");
            execute(&state, &request, Cancellation::default())
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
            | Action::Resume { .. }
            | Action::Undo { .. }
            | Action::ResumeUndo { .. }
    ) {
        let admission = match state.accounts.admission.try_lock() {
            Ok(g) => g,
            Err(_) => return failure(AcquisitionError::LifecyclePending),
        };
        let operation = match state.accounts.operation.try_lock() {
            Ok(g) => g,
            Err(_) => return failure(AcquisitionError::LifecyclePending),
        };
        let cancel = match state.accounts.reserve() {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        state
            .accounts
            .job
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .backlog_account = Some(request.account.clone());
        drop(operation);
        let worker = state.clone();
        let handle = std::thread::spawn(move || {
            let _operation = worker.accounts.operation.lock().expect("operation");
            let result = execute(&worker, &request, cancel).map(|_| ());
            worker.accounts.finish(result);
        });
        *state.accounts.worker.lock().unwrap() = Some(handle);
        drop(admission);
        return Json(json!({"started":true})).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _admission = state.accounts.admission.lock().expect("admission");
        let _operation = if status_only {
            if state
                .accounts
                .job
                .lock()
                .expect("job")
                .as_ref()
                .is_some_and(|j| j.progress.state == ScanState::Running)
            {
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
        execute(&state, &request, Cancellation::default())
    })
    .await
    {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}
fn execute(
    state: &Arc<AppState>,
    request: &Request,
    cancel: Cancellation,
) -> Result<serde_json::Value, AcquisitionError> {
    let repo = state.accounts.repository()?;
    let account = repo.load(&request.account)?;
    let store = repo.store(&account)?;
    let mut saved = store.load_state()?;
    if matches!(request.command, Action::Status {}) {
        return Ok(
            json!({"installed":repo.backlog_installed(Some(&account.id)),"recovery_hold":store.recovery_pending(),"journal":saved.protocol_backlog,"limitations":protocol::LIMITATIONS}),
        );
    }
    if store.recovery_pending() {
        return Err(AcquisitionError::LifecyclePending);
    }
    if matches!(request.command, Action::Cancel {}) {
        for r in &mut saved.protocol_backlog.runs {
            if !matches!(r.status, Status::Complete | Status::Undone) {
                r.stop_requested = true;
                if r.status == Status::Prepared {
                    r.status = Status::Stopped;
                }
            }
        }
        store.save_state(&mut saved)?;
        return Ok(json!({"cancel_requested":true,"journal":saved.protocol_backlog}));
    }
    let guard = state
        .accounts
        .ledger
        .as_ref()
        .map(|l| l.lock())
        .transpose()
        .map_err(|_| AcquisitionError::Storage)?;
    sync_ledger(
        &mut saved,
        &store,
        &repo.workspace_context(&account, &store)?,
        guard.as_ref(),
    )
    .map_err(|_| AcquisitionError::InvalidInput)?;
    ledger_fresh(&saved, guard.as_ref()).map_err(|_| AcquisitionError::InvalidInput)?;
    let fresh = |s: &crate::census::AccountState| {
        ledger_fresh(s, guard.as_ref())?;
        if let Some(job) = state
            .accounts
            .job
            .lock()
            .expect("job")
            .as_mut()
            .filter(|j| j.backlog_account.as_ref() == Some(&request.account))
        {
            if let Some(run) = s.protocol_backlog.runs.last() {
                let entries: Vec<_> = if run.undo_receipt.is_some() {
                    run.reverse.iter().map(|r| &r.evidence).collect()
                } else {
                    run.batches.iter().collect()
                };
                job.progress.saved = entries
                    .iter()
                    .filter(|e| protocol::verified(e))
                    .map(|e| e.command().messages().len())
                    .sum();
                job.progress.folders = entries.len();
            }
        }
        Ok(())
    };
    match &request.command {
        Action::Preview { options } => {
            return Ok(json!(repo.backlog_preview(
                &account,
                &store,
                &saved,
                options.clone(),
                cancel
            )?));
        }
        Action::Confirm {
            options,
            receipt,
            confirmed,
        } => {
            if !confirmed
                || saved.unsubscribe.history.iter().any(|e| {
                    matches!(
                        e.status,
                        crate::unsubscribe::Status::Intent | crate::unsubscribe::Status::Uncertain
                    )
                })
                || saved
                    .invoices
                    .sends
                    .iter()
                    .any(|s| s.status != crate::invoice::Status::Completed)
            {
                return Err(AcquisitionError::InvalidInput);
            }
            repo.backlog_confirm(
                &account,
                &store,
                &mut saved,
                options.clone(),
                receipt,
                cancel,
                fresh,
            )?;
        }
        Action::Reconcile { run } => {
            repo.backlog_reconcile(&account, &store, &mut saved, run, cancel)?
        }
        Action::Resume { run } => {
            repo.backlog_run(&account, &store, &mut saved, run, false, cancel, fresh)?
        }
        Action::UndoPreview { run } => {
            return Ok(json!(
                repo.backlog_undo_preview(&account, &store, &saved, run, cancel)?
            ));
        }
        Action::Undo {
            run,
            receipt,
            confirmed,
        } => {
            if !confirmed {
                return Err(AcquisitionError::InvalidInput);
            }
            repo.backlog_undo(&account, &store, &mut saved, run, receipt, cancel, fresh)?;
        }
        Action::ResumeUndo { run } => {
            repo.backlog_run(&account, &store, &mut saved, run, true, cancel, fresh)?
        }
        Action::Status {} | Action::Cancel {} => unreachable!(),
    }
    Ok(json!({"journal":saved.protocol_backlog}))
}

#[cfg(test)]
#[tokio::test]
async fn denied_sources_and_malformed_requests_never_open_repository_credentials_or_helpers() {
    use tower::ServiceExt;
    for synthetic in [false, true] {
        let mut app = AppState::new(synthetic);
        let accesses = match &mut app.accounts.source {
            AccountSource::Synthetic { accounts, .. } => {
                Some(Arc::get_mut(accounts).unwrap().track_credential_accesses())
            }
            _ => None,
        };
        let state = Arc::new(app);
        for body in ["{".to_string(), json!({"account":"invented","command":{"action":"confirm","confirmed":true,"receipt":"a".repeat(64),"endpoint":"127.0.0.1"}}).to_string(), json!({"account":"invented","command":{"action":"cancel"}}).to_string()] {
            let response = super::super::router(state.clone()).oneshot(axum::http::Request::builder().method("POST").uri("/api/accounts/backlog/protocol").header("Host", super::super::ADDRESS).header("Origin", super::super::ORIGIN).header("Content-Type", "application/json").header("X-Mailroom-Token", state.token.as_str()).body(axum::body::Body::from(body)).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(state.accounts.confirmations.lock().unwrap().accesses, 0);
        if let Some(accesses) = accesses {
            assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn read_create_and_wrong_account_sources_deny_before_repository_or_credentials() {
    use tower::ServiceExt;
    for source in ["read_only", "create_only", "wrong_account"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("move-source"), source).unwrap();
        let (mut repo, account, _fake) = acquisition::backlog::fixture_install(root.path());
        let credentials = repo.track_credential_accesses();
        let mut app = AppState::new(true);
        app.accounts.source = AccountSource::Synthetic {
            accounts: Arc::new(repo),
            _directory: tempfile::tempdir().unwrap(),
            mailboxes: root.path().join("mailboxes"),
        };
        let state = Arc::new(app);
        let id = if source == "wrong_account" {
            "uninstalled-account"
        } else {
            account.id.as_str()
        };
        let bodies = [
            "{".into(),
            json!({"account":id,"command":{"action":"cancel"}}).to_string(),
            json!({"account":id,"command":{"action":"confirm","receipt":"a".repeat(64),"confirmed":true,"endpoint":"invented"}}).to_string(),
        ];
        for body in bodies {
            let response = super::super::router(state.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/accounts/backlog/protocol")
                        .header("Host", super::super::ADDRESS)
                        .header("Origin", super::super::ORIGIN)
                        .header("Content-Type", "application/json")
                        .header("X-Mailroom-Token", state.token.as_str())
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if source == "wrong_account" {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(state.accounts.confirmations.lock().unwrap().accesses, 0);
        assert_eq!(credentials.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
#[tokio::test]
async fn status_waits_for_a_reader_and_keeps_the_protocol_panel_available() {
    use tower::ServiceExt;
    let app = AppState::new(true);
    let AccountSource::Synthetic { accounts, .. } = &app.accounts.source else {
        unreachable!()
    };
    let account = accounts
        .create(
            acquisition::Provider::MailboxOrg,
            "reader@example.test".into(),
            &acquisition::SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    let state = Arc::new(app);
    let held = state.accounts.operation.lock().unwrap();
    let next = state.clone();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/accounts/backlog/protocol")
        .header("Host", super::super::ADDRESS)
        .header("Origin", super::super::ORIGIN)
        .header("Content-Type", "application/json")
        .header("X-Mailroom-Token", state.token.as_str())
        .body(axum::body::Body::from(
            json!({"account":account.id,"command":{"action":"status"}}).to_string(),
        ))
        .unwrap();
    let pending =
        tokio::spawn(async move { super::super::router(next).oneshot(request).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!pending.is_finished());
    drop(held);
    assert_eq!(pending.await.unwrap().status(), StatusCode::OK);
}
