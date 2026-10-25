use super::*;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {},
    Confirm {
        receipt: String,
        confirmed: bool,
    },
    Status {},
    Cancel {},
    Reconcile {
        operation: String,
    },
    CompensationPreview {
        operation: String,
    },
    CompensationConfirm {
        operation: String,
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
fn installed(controller: &AccountController) -> bool {
    matches!(&controller.source, AccountSource::Synthetic { accounts, .. } if accounts.sieve_write_installed())
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    if !installed(&state.accounts) {
        return (StatusCode::FORBIDDEN,Json(json!({"error":"ManageSieve writes require a separately installed fixture ownership source. Production and read-only installations deny before repository, credentials, helper or network access."}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if accounts.sieve_write_account(&request.account))
    {
        return failure(AcquisitionError::InvalidInput);
    }
    if matches!(request.command, Action::Cancel {}) {
        if let Some(job) = state
            .accounts
            .job
            .lock()
            .expect("account job")
            .as_ref()
            .filter(|j| j.sieve_account.as_ref() == Some(&request.account))
        {
            job.cancel.cancel();
        }
        return Json(json!({"cancel_requested":true})).into_response();
    }
    if matches!(
        request.command,
        Action::Confirm { .. } | Action::CompensationConfirm { .. }
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
            .expect("job")
            .as_mut()
            .unwrap()
            .sieve_account = Some(request.account.clone());
        drop(operation);
        let worker = state.clone();
        let handle = std::thread::spawn(move || {
            let _operation = worker.accounts.operation.lock().expect("account operation");
            let result = execute(&worker, &request, cancel).map(|_| ());
            worker.accounts.finish(result);
        });
        *state.accounts.worker.lock().expect("worker") = Some(handle);
        drop(admission);
        return Json(json!({"started":true})).into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let _admission = state.accounts.admission.lock().expect("account admission");
        let _operation = state
            .accounts
            .operation
            .try_lock()
            .map_err(|_| AcquisitionError::LifecyclePending)?;
        execute(&state, &request, Cancellation::default())
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
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
            json!({"installed":true,"recovery_hold":store.recovery_pending(),"journal":saved.sieve_transactions,"limitations":acquisition::sieve_transaction::LIMITATIONS}),
        );
    }
    if store.recovery_pending() {
        return Err(AcquisitionError::LifecyclePending);
    }
    let guard = state
        .accounts
        .ledger
        .as_ref()
        .map(|l| l.lock())
        .transpose()
        .map_err(|_| AcquisitionError::Storage)?;
    sync_ledger(&mut saved, &store, account.provider, guard.as_ref())
        .map_err(|_| AcquisitionError::InvalidInput)?;
    ledger_fresh(&saved, guard.as_ref()).map_err(|_| AcquisitionError::InvalidInput)?;
    match &request.command {
        Action::Preview {} => Ok(json!(
            repo.transaction_preview(&account, &store, &saved, None, cancel)?
        )),
        Action::CompensationPreview { operation } => Ok(json!(repo.transaction_preview(
            &account,
            &store,
            &saved,
            Some(operation),
            cancel
        )?)),
        Action::Confirm { receipt, confirmed }
        | Action::CompensationConfirm {
            receipt, confirmed, ..
        } => {
            if !confirmed {
                return Err(AcquisitionError::InvalidInput);
            }
            let original = if let Action::CompensationConfirm { operation, .. } = &request.command {
                Some(operation.as_str())
            } else {
                None
            };
            repo.transaction_confirm(
                &account,
                &store,
                &mut saved,
                original,
                receipt,
                cancel,
                |s| ledger_fresh(s, guard.as_ref()),
            )?;
            Ok(json!({"journal":saved.sieve_transactions}))
        }
        Action::Reconcile { operation } => {
            repo.transaction_reconcile(&account, &store, &mut saved, operation, cancel)?;
            Ok(
                json!({"installed":true,"recovery_hold":false,"journal":saved.sieve_transactions,"limitations":acquisition::sieve_transaction::LIMITATIONS}),
            )
        }
        Action::Status {} | Action::Cancel {} => unreachable!(),
    }
}

#[cfg(test)]
#[tokio::test]
async fn absent_write_installation_denies_every_action_without_repository_access() {
    use tower::ServiceExt;
    for installation in 0..3 {
        let fake = (installation == 2).then(|| acquisition::SieveFake::start("normal", None));
        let mut app = AppState::new(installation != 0);
        if let (Some(fake), AccountSource::Synthetic { accounts, .. }) =
            (&fake, &mut app.accounts.source)
        {
            Arc::get_mut(accounts).unwrap().install_sieve_fixture(fake);
        }
        let state = Arc::new(app);
        for action in [
            "preview",
            "confirm",
            "status",
            "cancel",
            "reconcile",
            "compensation_preview",
            "compensation_confirm",
        ] {
            let response = super::super::router(state.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/accounts/sieve-transactions")
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
        if let Some(fake) = fake {
            assert!(fake.commands.lock().unwrap().is_empty());
        }
    }
}
