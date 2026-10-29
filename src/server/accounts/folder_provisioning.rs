use super::*;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {
        #[serde(default)]
        intent: crate::backlog::Intent,
    },
    Confirm {
        receipt: String,
        confirmed: bool,
        #[serde(default)]
        intent: crate::backlog::Intent,
    },
    Status {},
    Cancel {},
    Reconcile {
        operation: String,
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
    if !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if accounts.folders_installed())
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Folder creation requires a separately installed synthetic create source. Production and read-only installations deny before repository, credential, helper or network access."}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if accounts.folders_account(&request.account))
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
            .filter(|j| j.folder_account.as_ref() == Some(&request.account))
        {
            job.cancel.cancel();
        }
        return Json(json!({"cancel_requested":true})).into_response();
    }
    if matches!(request.command, Action::Confirm { .. }) {
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
            .folder_account = Some(request.account.clone());
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
            json!({"installed":true,"recovery_hold":store.recovery_pending(),"journal":saved.folder_provisioning,"limitations":acquisition::folder_provisioning::LIMITATIONS}),
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
    sync_ledger(
        &mut saved,
        &store,
        &repo.workspace_context(&account, &store)?,
        guard.as_ref(),
    )
    .map_err(|_| AcquisitionError::InvalidInput)?;
    ledger_fresh(&saved, guard.as_ref()).map_err(|_| AcquisitionError::InvalidInput)?;
    match &request.command {
        Action::Preview { intent } => Ok(json!(
            repo.folder_preview_for(&account, &store, &saved, *intent, cancel)?
        )),
        Action::Confirm {
            receipt,
            confirmed,
            intent,
        } => {
            if !confirmed {
                return Err(AcquisitionError::InvalidInput);
            }
            mailbox_consent(
                &state,
                &account.id,
                if *intent == crate::backlog::Intent::ArchiveUnmatched {
                    "Create the previewed archive folders"
                } else {
                    "Create the previewed folders"
                },
                receipt,
            )?;
            repo.folder_confirm_for(
                &account,
                &store,
                &mut saved,
                receipt,
                *intent,
                cancel,
                |s| ledger_fresh(s, guard.as_ref()),
            )?;
            Ok(json!({"journal":saved.folder_provisioning}))
        }
        Action::Reconcile { operation } => {
            repo.folder_reconcile(&account, &store, &mut saved, operation, cancel)?;
            Ok(
                json!({"installed":true,"recovery_hold":false,"journal":saved.folder_provisioning,"limitations":acquisition::folder_provisioning::LIMITATIONS}),
            )
        }
        Action::Status {} | Action::Cancel {} => unreachable!(),
    }
}
#[cfg(test)]
#[tokio::test]
async fn absent_read_only_and_wrong_account_deny_without_repository_credentials_helper_or_network()
{
    use tower::ServiceExt;
    for installation in 0..4 {
        let mut app = AppState::new(installation != 0);
        let root = tempfile::tempdir().unwrap();
        let mut folder_fake = None;
        let mut read_fake = None;
        if installation == 2 {
            let fake = acquisition::SieveFake::start("normal", None);
            if let AccountSource::Synthetic { accounts, .. } = &mut app.accounts.source {
                Arc::get_mut(accounts).unwrap().install_sieve_fixture(&fake);
            }
            read_fake = Some(fake);
        }
        if installation == 3 {
            let (repo, _, fake) = acquisition::folder_provisioning::tests::install(root.path());
            app.accounts.source = AccountSource::Synthetic {
                accounts: Arc::new(repo),
                _directory: tempfile::tempdir().unwrap(),
                mailboxes: root.path().join("mailboxes"),
            };
            folder_fake = Some(fake);
        }
        let credential_accesses = match &mut app.accounts.source {
            AccountSource::Synthetic { accounts, .. } => {
                Some(Arc::get_mut(accounts).unwrap().track_credential_accesses())
            }
            AccountSource::Os | AccountSource::Installed { .. } | AccountSource::Unavailable => {
                None
            }
        };
        let state = Arc::new(app);
        for action in ["preview", "confirm", "status", "cancel", "reconcile"] {
            let command = match action {
                "confirm" => json!({"action":action,"receipt":"a".repeat(64),"confirmed":true}),
                "reconcile" => json!({"action":action,"operation":"a".repeat(64)}),
                _ => json!({"action":action}),
            };
            let response = super::super::router(state.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/accounts/folders")
                        .header("Host", super::super::ADDRESS)
                        .header("Origin", super::super::ORIGIN)
                        .header("Content-Type", "application/json")
                        .header("X-Mailroom-Token", state.token.as_str())
                        .body(axum::body::Body::from(
                            json!({"account":"invented","command":command}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if installation == 3 {
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
        if let Some(accesses) = credential_accesses {
            assert_eq!(accesses.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
        if let Some(fake) = &folder_fake {
            assert_eq!(
                fake.connections.load(std::sync::atomic::Ordering::SeqCst),
                0
            );
        }
        if let Some(fake) = &read_fake {
            assert!(fake.commands.lock().unwrap().is_empty());
        }
        drop(folder_fake);
    }
}
