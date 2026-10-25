use super::*;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {},
    Start {},
    Cancel {},
    Status {},
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    command: Action,
}
fn installed(controller: &AccountController) -> bool {
    matches!(&controller.source, AccountSource::Synthetic { accounts, .. } if accounts.sieve_installed())
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    if !installed(&state.accounts) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Production ManageSieve installation is unavailable. Access denied before repository, credentials or network.", "installed":false}))).into_response();
    }
    let request = match input(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if matches!(request.command, Action::Status {}) {
        let job = state.accounts.job.lock().expect("account job");
        let job = job
            .as_ref()
            .filter(|j| j.sieve_account.as_ref() == Some(&request.account));
        return Json(json!({"installed":true,"running":job.is_some_and(|j| j.progress.state == ScanState::Running),"error":job.and_then(|j| j.error)})).into_response();
    }
    if matches!(request.command, Action::Cancel {}) {
        let job = state.accounts.job.lock().expect("account job");
        let Some(job) = job
            .as_ref()
            .filter(|j| j.sieve_account.as_ref() == Some(&request.account))
        else {
            return failure(AcquisitionError::InvalidInput);
        };
        job.cancel.cancel();
        return Json(json!({"cancel_requested":true})).into_response();
    }
    if matches!(request.command, Action::Start {}) {
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
            .expect("account job")
            .as_mut()
            .unwrap()
            .sieve_account = Some(request.account.clone());
        drop(operation);
        let worker = state.clone();
        let handle = std::thread::spawn(move || {
            let _operation = worker.accounts.operation.lock().expect("account operation");
            let result = (|| {
                let repo = worker.accounts.repository()?;
                repo.discover_sieve(&request.account, cancel)
            })();
            worker.accounts.finish(result);
        });
        *state.accounts.worker.lock().expect("account worker") = Some(handle);
        drop(admission);
        return Json(json!({"started":true})).into_response();
    }
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, AcquisitionError> {
        let _admission = state.accounts.admission.lock().expect("account admission");
        let _operation = state.accounts.operation.try_lock().map_err(|_| AcquisitionError::LifecyclePending)?;
        let repo = state.accounts.repository()?;
        let account = repo.load(&request.account)?;
        let store = repo.store(&account)?;
        let saved = store.load_state()?;
        let context = repo.sieve_context(&account, &store)?;
        Ok(json!({"installed":true,"current":saved.sieve.current(Some(&context), store.recovery_pending()),"recovery_hold":store.recovery_pending(),"facts":saved.sieve,"limits":{"script_bytes":1048576,"attempt_bytes":4194304,"names":256},"limitations":"Matching observations are not a server transaction and cannot exclude change-and-revert. Include dependencies remain uncaptured. Production access and deployment remain unavailable. Compiler trust is unchanged."}))
    }).await { Ok(Ok(v)) => Json(v).into_response(), Ok(Err(e)) => failure(e), Err(_) => failure(AcquisitionError::Storage) }
}

#[cfg(test)]
#[tokio::test]
async fn no_installed_source_denies_all_actions_before_repository_access() {
    use tower::ServiceExt;
    for synthetic in [false, true] {
        let state = Arc::new(AppState::new(synthetic));
        for action in ["open", "start", "cancel", "status"] {
            let response = super::super::router(state.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/accounts/managesieve")
                        .header("Host", super::super::ADDRESS)
                        .header("Origin", super::super::ORIGIN)
                        .header("Content-Type", "application/json")
                        .header("X-Mailroom-Token", state.token.as_str())
                        .body(axum::body::Body::from(
                            json!({"account":"invented","command":{"action":action}}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}
