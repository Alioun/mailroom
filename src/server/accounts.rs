pub(super) mod agent_edits;
pub(super) mod assistant;
pub(super) mod backlog;
pub(super) mod backlog_protocol;
pub(super) mod census_routes;
mod controller;
mod disclosure;
pub(super) mod events;
#[cfg(test)]
mod fixtures_tests;
pub(super) mod folder_provisioning;
pub(super) mod invoices;
pub(super) mod ledger;
pub(super) mod lifecycle;
mod local;
pub(super) mod managesieve;
pub(super) mod overview;
pub(super) mod proposals;
pub(super) mod recovery;
pub(super) mod sieve_transactions;
pub(super) mod subscription_review;
pub(super) mod unsubscribe;
pub(super) mod web;
pub(super) mod winddown;
#[cfg(test)]
mod wiring_tests;
pub(super) mod workspace_routes;

pub(super) use census_routes::{confirm, export_summary, open, report, review_summary};
pub(in crate::server) use controller::AccountController;
use controller::{AccountSource, failure, input, mailbox_consent, valid_timeout_secs};
use workspace_routes::{ledger_fresh, sync_ledger};
pub(super) use workspace_routes::{proof_download, proof_preview, workspace_open, workspace_save};

use super::{AppState, EmptyRequest};
use crate::{
    acquisition::{
        self, Account, AccountPaths, Accounts, AcquisitionError, Cancellation, OsCredentials,
        Provider, ScanProgress, SecretValue,
    },
    census::{AccountId, Census, ScanState},
    policy::{self, Operation},
    rules::workspace::{self, Workspace, WorkspaceInput},
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

pub(super) async fn status(State(state): State<Arc<AppState>>) -> Response {
    Json(events::status_json(&state)).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectRequest {
    provider: Provider,
    login: String,
    password: SecretValue,
}

pub(super) async fn connect(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ConnectRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        if state.accounts.synthetic() {
            return failure(AcquisitionError::InvalidInput);
        }
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Protected account installation is unavailable. Production account creation is denied before credential or provider access."}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !policy::authorize(Operation::Connect).allowed {
        return failure(AcquisitionError::InvalidInput);
    }
    launch_connection(state, move |repo| {
        repo.create_reviewed(request.provider, request.login, &request.password, |_| {
            web::connect_allowed()
        })
    })
    .await
}

pub(super) async fn synthetic(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = input(body) {
        return error;
    }
    if !state.accounts.synthetic() {
        return failure(AcquisitionError::InvalidInput);
    }
    synthetic_connection(state, Provider::MailboxOrg).await
}

pub(super) async fn synthetic_gmail(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = input(body) {
        return error;
    }
    if !state.accounts.synthetic() {
        return failure(AcquisitionError::InvalidInput);
    }
    synthetic_connection(state, Provider::Gmail).await
}

async fn synthetic_connection(state: Arc<AppState>, provider: Provider) -> Response {
    launch_connection(state, move |repo| {
        repo.create(
            provider,
            "owner@example.test".into(),
            &SecretValue::new(b"synthetic-account-password".to_vec()),
        )
    })
    .await
}

/// Runs `establish` in the account worker, then probes the account it returns read-only and
/// records whether the provider accepted the connection.
async fn launch_connection(
    state: Arc<AppState>,
    establish: impl FnOnce(&Accounts) -> Result<Account, AcquisitionError> + Send + 'static,
) -> Response {
    let admission = match state.accounts.admission.try_lock() {
        Ok(guard) => guard,
        Err(_) => return failure(AcquisitionError::LifecyclePending),
    };
    let cancel = match state.accounts.reserve("connect") {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let worker = state.clone();
    let handle = std::thread::spawn(move || {
        let _operation = worker.accounts.operation.lock().expect("account operation");
        let result = (|| {
            let repo = worker.accounts.repository()?;
            let account = establish(&repo)?;
            worker
                .accounts
                .job
                .lock()
                .expect("account job")
                .as_mut()
                .expect("reserved account job")
                .account = Some(account.clone());
            worker.accounts.notify_change();
            let probe = if worker.accounts.synthetic() {
                Ok(1)
            } else {
                repo.probe(&account.id, cancel)
            };
            repo.connection_observed(&account.id, probe.is_ok())?;
            let folders = probe?;
            worker
                .accounts
                .job
                .lock()
                .expect("account job")
                .as_mut()
                .expect("reserved account job")
                .progress
                .folders = folders;
            Ok(())
        })();
        worker.accounts.finish(result);
    });
    *state.accounts.worker.lock().expect("account worker") = Some(handle);
    drop(admission);
    status(State(state)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccountRequest {
    account: AccountId,
}

pub(super) async fn scan(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !policy::authorize(Operation::Census).allowed {
        return failure(AcquisitionError::InvalidInput);
    }
    let admission = match state.accounts.admission.try_lock() {
        Ok(guard) => guard,
        Err(_) => return failure(AcquisitionError::LifecyclePending),
    };
    let cancel = match state.accounts.reserve("scan") {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let worker = state.clone();
    let handle = std::thread::spawn(move || {
        let _operation = worker.accounts.operation.lock().expect("account operation");
        let result = (|| {
            let repo = worker.accounts.repository()?;
            let account = repo.load(&request.account)?;
            worker
                .accounts
                .job
                .lock()
                .expect("account job")
                .as_mut()
                .expect("reserved account job")
                .account = Some(account.clone());
            worker.accounts.notify_change();
            let progress = |mut progress: ScanProgress| {
                progress.state = ScanState::Running;
                worker
                    .accounts
                    .job
                    .lock()
                    .expect("account job")
                    .as_mut()
                    .expect("reserved account job")
                    .progress = progress;
                worker.accounts.notify_change();
            };
            if worker.accounts.synthetic() && !repo.installed_test_transport() {
                acquisition::synthetic::scan(&repo, &account.id, cancel, progress)?;
            } else {
                repo.scan(&account.id, cancel, progress)?;
            }
            Ok(())
        })();
        worker.accounts.finish(result);
    });
    *state.accounts.worker.lock().expect("account worker") = Some(handle);
    drop(admission);
    status(State(state)).await
}

pub(super) async fn cancel(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = input(body) {
        return error;
    }
    if let Some(job) = state.accounts.job.lock().expect("account job").as_ref() {
        job.cancel.cancel();
    }
    state.accounts.notify_change();
    status(State(state)).await
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = input(body) {
        return error;
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        state.accounts.repository()?.list()
    })
    .await
    {
        Ok(Ok(accounts)) => Json(accounts).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}
