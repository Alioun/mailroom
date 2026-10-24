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
use std::sync::{Arc, Mutex};

pub(super) struct AccountController {
    source: AccountSource,
    job: Mutex<Option<Job>>,
    operation: Mutex<()>,
}
enum AccountSource {
    Os,
    Synthetic {
        accounts: Arc<Accounts>,
        _directory: tempfile::TempDir,
    },
}
#[derive(Clone, Serialize)]
struct Job {
    account: Option<Account>,
    progress: ScanProgress,
    error: Option<AcquisitionError>,
    #[serde(skip)]
    cancel: Cancellation,
}
impl AccountController {
    pub fn new(synthetic: bool) -> Self {
        let source = if synthetic {
            let directory = tempfile::tempdir().expect("synthetic account directory");
            AccountSource::Synthetic {
                accounts: Arc::new(acquisition::synthetic::accounts(directory.path())),
                _directory: directory,
            }
        } else {
            AccountSource::Os
        };
        Self {
            source,
            job: Mutex::new(None),
            operation: Mutex::new(()),
        }
    }
    fn repository(&self) -> Result<Arc<Accounts>, AcquisitionError> {
        match &self.source {
            AccountSource::Os => Ok(Arc::new(Accounts::new(
                AccountPaths::per_user()?,
                Arc::new(OsCredentials),
            ))),
            AccountSource::Synthetic { accounts, .. } => Ok(accounts.clone()),
        }
    }
    fn synthetic(&self) -> bool {
        matches!(self.source, AccountSource::Synthetic { .. })
    }
    fn reserve(&self) -> Result<Cancellation, AcquisitionError> {
        let mut job = self.job.lock().expect("account job");
        if job
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let cancel = Cancellation::default();
        *job = Some(Job {
            account: None,
            progress: ScanProgress {
                state: ScanState::Running,
                saved: 0,
                folders: 0,
            },
            error: None,
            cancel: cancel.clone(),
        });
        Ok(cancel)
    }
    fn finish(&self, result: Result<(), AcquisitionError>) {
        let mut current = self.job.lock().expect("account job");
        let job = current.as_mut().expect("reserved account job");
        match result {
            Ok(()) => job.progress.state = ScanState::Complete,
            Err(error) => {
                let error = if job.cancel.is_cancelled()
                    && matches!(
                        error,
                        AcquisitionError::Protocol
                            | AcquisitionError::Tls
                            | AcquisitionError::Authentication
                    ) {
                    AcquisitionError::Cancelled
                } else {
                    error
                };
                job.progress.state = if error == AcquisitionError::Cancelled {
                    ScanState::Cancelled
                } else {
                    ScanState::Failed
                };
                job.error = Some(error);
            }
        }
    }
}
fn failure(error: AcquisitionError) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"error": error.to_string()})),
    )
        .into_response()
}
fn input<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    value
        .map(|Json(v)| v)
        .map_err(|_| failure(AcquisitionError::InvalidInput))
}
pub(super) async fn status(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({"synthetic": state.accounts.synthetic(), "job": state.accounts.job.lock().expect("account job").clone()})).into_response()
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
    let input = match input(body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if state.accounts.synthetic() || !policy::authorize(Operation::Connect).allowed {
        return failure(AcquisitionError::InvalidInput);
    }
    launch_connection(state, input).await
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
    launch_connection(
        state,
        ConnectRequest {
            provider,
            login: "owner@example.test".into(),
            password: SecretValue::new(b"synthetic-account-password".to_vec()),
        },
    )
    .await
}
async fn launch_connection(state: Arc<AppState>, input: ConnectRequest) -> Response {
    let cancel = match state.accounts.reserve() {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let worker = state.clone();
    tokio::task::spawn_blocking(move || {
        let _operation = worker.accounts.operation.lock().expect("account operation");
        let result = (|| {
            let repo = worker.accounts.repository()?;
            let account = repo.create(input.provider, input.login, &input.password)?;
            worker
                .accounts
                .job
                .lock()
                .expect("account job")
                .as_mut()
                .expect("reserved account job")
                .account = Some(account.clone());
            let folders = if worker.accounts.synthetic() {
                1
            } else {
                repo.probe(&account.id, cancel)?
            };
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
    let cancel = match state.accounts.reserve() {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let worker = state.clone();
    tokio::task::spawn_blocking(move || {
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
            let progress = |mut progress: ScanProgress| {
                progress.state = ScanState::Running;
                worker
                    .accounts
                    .job
                    .lock()
                    .expect("account job")
                    .as_mut()
                    .expect("reserved account job")
                    .progress = progress
            };
            if worker.accounts.synthetic() {
                acquisition::synthetic::scan(&repo, &account.id, cancel, progress)?;
            } else {
                repo.scan(&account.id, cancel, progress)?;
            }
            Ok(())
        })();
        worker.accounts.finish(result);
    });
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmRequest {
    account: AccountId,
    address: String,
    confirmed: bool,
}
pub(super) async fn open(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    census_action(state, request.account, None).await
}
pub(super) async fn confirm(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    census_action(
        state,
        request.account,
        Some((request.address, request.confirmed)),
    )
    .await
}
#[derive(Serialize)]
struct CensusView {
    census: Census,
    addresses: Vec<crate::census::AddressCandidate>,
}
async fn census_action(
    state: Arc<AppState>,
    id: AccountId,
    confirm: Option<(String, bool)>,
) -> Response {
    if state
        .accounts
        .job
        .lock()
        .expect("account job")
        .as_ref()
        .is_some_and(|j| j.progress.state == ScanState::Running)
    {
        return failure(AcquisitionError::InvalidInput);
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        let repo = state.accounts.repository()?;
        let account = repo.load(&id)?;
        let store = repo.store(&account)?;
        let mut census = store.load()?;
        if let Some((address, confirmed)) = confirm {
            census.confirm_address(&address, confirmed)?;
            store.save(&census)?;
        }
        Ok::<_, AcquisitionError>(CensusView {
            addresses: census.own_address_candidates(),
            census,
        })
    })
    .await
    {
        Ok(Ok(view)) => Json(view).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportRequest {
    account: AccountId,
    as_of: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewRequest {
    account: AccountId,
    as_of: i64,
    version: String,
    approved: std::collections::BTreeSet<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExportRequest {
    account: AccountId,
    as_of: i64,
    version: String,
    approved: std::collections::BTreeSet<String>,
    receipt: String,
    confirmed: bool,
}
pub(super) async fn report(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ReportRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    report_action(state, request, None, None).await
}
pub(super) async fn review_summary(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ReviewRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    report_action(
        state,
        ReportRequest {
            account: request.account.clone(),
            as_of: request.as_of,
        },
        Some(request),
        None,
    )
    .await
}
pub(super) async fn export_summary(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ExportRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !request.confirmed {
        return failure(AcquisitionError::InvalidInput);
    }
    report_action(
        state,
        ReportRequest {
            account: request.account.clone(),
            as_of: request.as_of,
        },
        Some(ReviewRequest {
            account: request.account,
            as_of: request.as_of,
            version: request.version,
            approved: request.approved,
        }),
        Some(request.receipt),
    )
    .await
}
async fn report_action(
    state: Arc<AppState>,
    request: ReportRequest,
    review: Option<ReviewRequest>,
    receipt: Option<String>,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state
            .accounts
            .job
            .lock()
            .expect("account job")
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let repo = state.accounts.repository()?;
        let account = repo.load(&request.account)?;
        let store = repo.store(&account)?;
        let census = store.load()?;
        let report = census.report(
            store.keys(),
            request.as_of,
            account.provider == Provider::Gmail,
        )?;
        if let Some(review) = review {
            if report.version != review.version {
                return Err(AcquisitionError::InvalidInput);
            }
            let preview = report.summary_preview(store.keys(), &review.approved)?;
            if let Some(receipt) = receipt {
                use subtle::ConstantTimeEq;
                if !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
                    return Err(AcquisitionError::InvalidInput);
                }
                return Ok(json!({"summary": preview.summary, "filename": "census-summary.md"}));
            }
            return Ok(json!(preview));
        }
        Ok::<_, AcquisitionError>(json!(report))
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProofRequest {
    account: AccountId,
    documents: crate::engine::Documents,
    as_of: i64,
    pins: Vec<crate::rules::proof::Pin>,
    baseline: crate::rules::proof::Baseline,
    previous: Option<crate::engine::Documents>,
}
pub(super) async fn proof(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ProofRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state
            .accounts
            .job
            .lock()
            .expect("account job")
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let repo = state.accounts.repository()?;
        let account = repo.load(&request.account)?;
        let census = repo.store(&account)?.load()?;
        let engine = if account.provider == Provider::Gmail {
            crate::rules::Engine::GmailBacklog
        } else {
            crate::rules::Engine::MailboxSieve
        };
        let context = crate::rules::EngineContext::local(request.account.as_str().into(), engine);
        let artifact = crate::engine::artifact(&request.documents, &context)
            .map_err(|_| AcquisitionError::InvalidInput)?;
        let previous = request
            .previous
            .as_ref()
            .map(|d| crate::engine::artifact(d, &context))
            .transpose()
            .map_err(|_| AcquisitionError::InvalidInput)?;
        crate::rules::proof::prove(
            artifact,
            &census,
            request.as_of,
            &request.pins,
            request.baseline,
            previous.as_ref(),
        )
        .map_err(|_| AcquisitionError::InvalidInput)
    })
    .await
    {
        Ok(Ok(proof)) => Json(proof).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkspaceSaveRequest {
    account: AccountId,
    revision: u64,
    input: WorkspaceInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DownloadRequest {
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
pub(super) async fn workspace_open(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    workspace_action(state, request.account, WorkspaceAction::Open).await
}
pub(super) async fn workspace_save(
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
pub(super) async fn proof_preview(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    workspace_action(state, request.account, WorkspaceAction::Preview).await
}
pub(super) async fn proof_download(
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
        if state
            .accounts
            .job
            .lock()
            .expect("account job")
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let repo = state.accounts.repository()?;
        let account = repo.load(&id)?;
        let store = repo.store(&account)?;
        let mut saved = store.load_state()?;
        match action {
            WorkspaceAction::Open => Ok(json!(workspace::view(&saved, account.provider))),
            WorkspaceAction::Save { revision, input } => {
                if revision != saved.revision {
                    return Err(AcquisitionError::InvalidInput);
                }
                workspace::validate(&input, &saved, account.provider)
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                let revision = saved.workspace.as_ref().map_or(Ok(1), |w| {
                    w.revision.checked_add(1).ok_or(AcquisitionError::Storage)
                })?;
                saved.workspace = Some(Workspace { revision, input });
                store.save_state(&mut saved)?;
                Ok(json!(workspace::view(&saved, account.provider)))
            }
            WorkspaceAction::Preview => workspace::preview(&saved, account.provider, store.keys())
                .map(|v| json!(v))
                .map_err(|_| AcquisitionError::InvalidInput),
            WorkspaceAction::Download { receipt } => {
                let preview =
                    workspace::confirm_export(&mut saved, account.provider, store.keys(), &receipt)
                        .map_err(|_| AcquisitionError::InvalidInput)?;
                store.save_state(&mut saved)?;
                Ok(json!({
                    "filename": preview.filename,
                    "sieve": preview.proof.artifact.executable(),
                    "identity": preview.proof.artifact.identity(),
                    "workspace": workspace::view(&saved, account.provider)
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

#[cfg(test)]
#[tokio::test]
async fn durable_browser_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") else {
        return;
    };
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(acquisition::persistent_test_accounts(std::path::Path::new(
            &root,
        ))),
        _directory: tempfile::tempdir().unwrap(),
    };
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}
