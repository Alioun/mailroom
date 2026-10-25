use super::*;
use crate::acquisition::lifecycle::ForgetPreview;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::server) enum Request {
    Status {},
    ReplacePassword {
        account: AccountId,
        password: SecretValue,
    },
    ForgetPreview {
        account: AccountId,
    },
    Forget {
        account: AccountId,
        preview: ForgetPreview,
        confirmed: bool,
    },
    Rotate {
        account: AccountId,
        confirmed: bool,
    },
    Recover {
        account: AccountId,
    },
}
impl Request {
    fn account(&self) -> Option<&AccountId> {
        match self {
            Self::Status { .. } => None,
            Self::ReplacePassword { account, .. }
            | Self::ForgetPreview { account }
            | Self::Forget { account, .. }
            | Self::Rotate { account, .. }
            | Self::Recover { account } => Some(account),
        }
    }
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    if !state.accounts.synthetic() {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Protected lifecycle installation is unavailable. Production lifecycle actions are denied before credential access."}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if matches!(
        &request,
        Request::Forget {
            confirmed: false,
            ..
        } | Request::Rotate {
            confirmed: false,
            ..
        }
    ) {
        return failure(AcquisitionError::InvalidInput);
    }
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, AcquisitionError> {
        // Admission remains closed through cancellation, join and the durable transaction.
        let _admission = if matches!(request, Request::Status { .. }) {
            None
        } else {
            Some(state.accounts.admission.lock().expect("account admission"))
        };
        if !matches!(request, Request::Status { .. }) {
            if let Some(job) = state.accounts.job.lock().expect("account job").as_ref() {
                if job
                    .sieve_account
                    .as_ref()
                    .is_some_and(|id| Some(id) == request.account())
                    || job
                        .account
                        .as_ref()
                        .is_some_and(|a| Some(&a.id) == request.account())
                {
                    job.cancel.cancel();
                }
            }
            if let Some(worker) = state.accounts.worker.lock().expect("account worker").take() {
                worker.join().map_err(|_| AcquisitionError::Storage)?;
            }
        }
        let _operation = state.accounts.operation.lock().expect("account operation");
        let repo = state.accounts.repository()?;
        let AccountSource::Synthetic { mailboxes, .. } = &state.accounts.source else {
            return Err(AcquisitionError::InvalidInput);
        };
        match &request {
            Request::Status { .. } => {
                return Ok(
                    json!({"recovery":repo.recovery_status()?,"production_available":false}),
                );
            }
            Request::ReplacePassword { account, password } => {
                repo.replace_password(account, password)?
            }
            Request::ForgetPreview { account } => {
                return Ok(json!(repo.forget_preview(account, mailboxes)?));
            }
            Request::Forget {
                account,
                preview,
                confirmed,
            } => repo.forget(account, mailboxes, preview, *confirmed)?,
            Request::Rotate { account, confirmed } => {
                if !confirmed {
                    return Err(AcquisitionError::InvalidInput);
                }
                repo.rotate(account, mailboxes)?;
            }
            Request::Recover { account } => repo.recover(account, mailboxes)?,
        }
        let mut job = state.accounts.job.lock().expect("account job");
        if job.as_ref().is_some_and(|j| {
            j.sieve_account
                .as_ref()
                .is_some_and(|id| Some(id) == request.account())
                || j.account
                    .as_ref()
                    .is_some_and(|a| Some(&a.id) == request.account())
        }) {
            *job = None;
        }
        Ok(json!({"saved":true,"recovery":repo.recovery_status()?}))
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[cfg(test)]
#[tokio::test]
async fn recovery_status_remains_available_while_work_admission_is_closed() {
    let state = Arc::new(AppState::new(true));
    let admission = state.accounts.admission.lock().unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        handle(State(state.clone()), Ok(Json(Request::Status {}))),
    )
    .await;
    drop(admission);
    assert_eq!(response.unwrap().status(), StatusCode::OK);
}

#[cfg(test)]
#[tokio::test]
async fn oversized_lifecycle_request_preserves_payload_too_large_response() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(true));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/accounts/lifecycle")
        .header("host", super::super::ADDRESS)
        .header("content-type", "application/json")
        .header("x-mailroom-token", state.token.as_str())
        .body(axum::body::Body::from(
            " ".repeat(262_145) + r#"{"action":"status"}"#,
        ))
        .unwrap();
    let response = super::super::router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()["cache-control"], "no-store");
}
