pub(super) mod backlog;
pub(super) mod folder_provisioning;
pub(super) mod invoices;
pub(super) mod ledger;
pub(super) mod lifecycle;
pub(super) mod managesieve;
pub(super) mod proposals;
pub(super) mod recovery;
pub(super) mod sieve_transactions;
pub(super) mod subscription_review;
pub(super) mod unsubscribe;
pub(super) mod winddown;
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
    admission: Mutex<()>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    ledger: Option<crate::ledger::InstalledLedger>,
    confirmations: Mutex<crate::confirmation::Boundary>,
    #[cfg(test)]
    repository_accesses: std::sync::atomic::AtomicUsize,
}
enum AccountSource {
    Os,
    Synthetic {
        accounts: Arc<Accounts>,
        _directory: tempfile::TempDir,
        mailboxes: std::path::PathBuf,
    },
}
#[derive(Clone, Serialize)]
struct Job {
    #[serde(skip)]
    folder_account: Option<AccountId>,
    #[serde(skip)]
    sieve_account: Option<AccountId>,
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
                mailboxes: directory.path().join("mailboxes"),
                _directory: directory,
            }
        } else {
            AccountSource::Os
        };
        let ledger = match &source {
            AccountSource::Synthetic { mailboxes, .. } => {
                std::fs::create_dir_all(mailboxes).expect("synthetic mailbox directory");
                Some(
                    crate::ledger::InstalledLedger::install(mailboxes.join("ledger"))
                        .expect("synthetic private ledger"),
                )
            }
            _ => None,
        };
        Self {
            confirmations: Mutex::new(if synthetic {
                crate::confirmation::Boundary::synthetic()
            } else {
                crate::confirmation::Boundary::default()
            }),
            #[cfg(test)]
            repository_accesses: std::sync::atomic::AtomicUsize::new(0),
            ledger,
            source,
            job: Mutex::new(None),
            operation: Mutex::new(()),
            admission: Mutex::new(()),
            worker: Mutex::new(None),
        }
    }
    fn repository(&self) -> Result<Arc<Accounts>, AcquisitionError> {
        #[cfg(test)]
        self.repository_accesses
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            folder_account: None,
            sieve_account: None,
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
    match value {
        Ok(Json(value)) => Ok(value),
        Err(error) if error.status() == StatusCode::PAYLOAD_TOO_LARGE => Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error":"Account request exceeds the body limit"})),
        )
            .into_response()),
        Err(_) => Err(failure(AcquisitionError::InvalidInput)),
    }
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
    _body: Result<Json<ConnectRequest>, JsonRejection>,
) -> Response {
    if state.accounts.synthetic() {
        return failure(AcquisitionError::InvalidInput);
    }
    (StatusCode::FORBIDDEN, Json(json!({"error":"Protected account installation is unavailable. Production account creation is denied before credential or provider access."}))).into_response()
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
    let admission = match state.accounts.admission.try_lock() {
        Ok(guard) => guard,
        Err(_) => return failure(AcquisitionError::LifecyclePending),
    };
    let cancel = match state.accounts.reserve() {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let worker = state.clone();
    let handle = std::thread::spawn(move || {
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
    let cancel = match state.accounts.reserve() {
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
    evidence: crate::census::acquisition::EvidenceView,
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
        if state.accounts.synthetic() && !repo.installed_test_transport() {
            let mut saved = store.load_state()?;
            if crate::invoice::initialize_synthetic(&mut saved) {
                store.save_state(&mut saved)?;
            }
        }
        let mut census = store.load()?;
        if let Some((address, confirmed)) = confirm {
            census.confirm_address(&address, confirmed)?;
            store.save(&census)?;
        }
        let mut evidence = census.acquisition.view(crate::census::acquisition::now());
        if store.recovery_pending() {
            evidence.historical();
        }
        Ok::<_, AcquisitionError>(CensusView {
            evidence,
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
        let mut report = census.report(
            store.keys(),
            request.as_of,
            account.provider == Provider::Gmail,
        )?;
        if store.recovery_pending() {
            report.acquisition.historical();
        }
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
        let ledger_guard = state
            .accounts
            .ledger
            .as_ref()
            .map(|l| l.lock())
            .transpose()
            .map_err(|_| AcquisitionError::Storage)?;
        sync_ledger(&mut saved, &store, account.provider, ledger_guard.as_ref())
            .map_err(|_| AcquisitionError::InvalidInput)?;
        match action {
            WorkspaceAction::Open => Ok(json!(workspace::view(&saved, account.provider))),
            WorkspaceAction::Save { revision, input } => {
                if revision != saved.revision {
                    return Err(AcquisitionError::InvalidInput);
                }
                crate::ledger::validate_workspace_edit(&saved, &input)
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                workspace::validate(&input, &saved, account.provider)
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                let revision = saved.workspace.as_ref().map_or(Ok(1), |w| {
                    w.revision.checked_add(1).ok_or(AcquisitionError::Storage)
                })?;
                saved.workspace = Some(Workspace { revision, input });
                if let Some(guard) = &ledger_guard {
                    let snapshot = guard.read().map_err(|_| AcquisitionError::Storage)?;
                    crate::ledger::reconcile(&mut saved, &snapshot, account.provider)
                        .map_err(|_| AcquisitionError::InvalidInput)?;
                }
                store.save_state(&mut saved)?;
                Ok(json!(workspace::view(&saved, account.provider)))
            }
            WorkspaceAction::Preview => {
                let proof = workspace::preview(&saved, account.provider, store.keys())
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                ledger_fresh(&saved, ledger_guard.as_ref())
                    .map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!(proof))
            }
            WorkspaceAction::Download { receipt } => {
                let preview =
                    workspace::confirm_export(&mut saved, account.provider, store.keys(), &receipt)
                        .map_err(|_| AcquisitionError::InvalidInput)?;
                ledger_fresh(&saved, ledger_guard.as_ref())
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

fn ledger_fresh(
    saved: &crate::census::AccountState,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<(), String> {
    if let Some(guard) = guard {
        if guard.read()?.content_version != saved.ledger.source_version {
            return Err("Ledger changed while proving. Review fresh proof".into());
        }
    }
    Ok(())
}

fn sync_ledger(
    saved: &mut crate::census::AccountState,
    store: &crate::census::EncryptedStore,
    provider: Provider,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<(), String> {
    if store.recovery_pending() {
        return Ok(());
    }
    if let Some(guard) = guard {
        let snapshot = guard.read()?;
        if saved.workspace.is_some() && crate::ledger::reconcile(saved, &snapshot, provider)? {
            store.save_state(saved).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn provider_evidence_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_PROVIDER_EVIDENCE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let mut repo = acquisition::persistent_test_accounts(root);
    acquisition::install_provider_fixture(&mut repo, root);
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn durable_browser_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") else {
        return;
    };
    let installation =
        std::env::var_os("MAILROOM_RECOVERY_INSTALLATION").unwrap_or_else(|| root.clone());
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(acquisition::persistent_test_accounts(std::path::Path::new(
            &installation,
        ))),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: std::path::Path::new(&root).join("mailboxes"),
    };
    state.accounts.ledger = Some(
        crate::ledger::InstalledLedger::install(
            std::env::var_os("MAILROOM_RECOVERY_LEDGER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::Path::new(&root).join("ledger")),
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn production_consequential_routes_deny_before_repository_or_confirmation_access() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(false));
    let router = super::router(state.clone());
    let input = json!({"as_of":1792886400_i64,"selections":[{"subscription":"synthetic-subscription","method":"local_mail"}],"folders":{"personal":"Muted","business":"Quiet"}});
    let list_input = json!({"as_of":1792886400_i64,"selections":[{"account":"synthetic-account","subscription":"synthetic-subscription","method":"local_mail"}],"folders":{"personal":"Muted","business":"Quiet"}});
    let cases = [
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"preview","selection":"INBOX/1/1","target":"accounting"}}),
        ),
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"send","selection":"INBOX/1/1","target":"accounting","receipt":"a".repeat(64),"confirmation_action":"yes"}}),
        ),
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"prepare_mail_app","selection":"INBOX/1/1","target":"accounting"}}),
        ),
        (
            "unsubscribe",
            json!({"account":"synthetic-account","command":{"action":"preview","input":input}}),
        ),
        (
            "unsubscribe",
            json!({"account":"synthetic-account","command":{"action":"confirm","input":input,"receipt":"a".repeat(64),"confirmation_action":"yes","mail_confirmation_action":"yes"}}),
        ),
        (
            "subscription-review",
            json!({"command":{"action":"preview","input":list_input}}),
        ),
        (
            "subscription-review",
            json!({"command":{"action":"confirm","input":list_input,"receipt":"a".repeat(64),"confirmation_action":"yes","mail_confirmation_action":"yes"}}),
        ),
    ];
    for (path, body) in cases {
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/accounts/{path}"))
                    .header("Host", super::ADDRESS)
                    .header("Origin", super::ORIGIN)
                    .header("Content-Type", "application/json")
                    .header("X-Mailroom-Token", state.token.as_str())
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
    assert!(
        state
            .accounts
            .confirmations
            .lock()
            .unwrap()
            .offer(crate::confirmation::Plan {
                actions: vec![],
                exact_receipt: "a".repeat(64)
            })
            .is_err()
    );
}

#[cfg(test)]
#[tokio::test]
async fn managesieve_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let fake = acquisition::SieveFake::start("normal", Some(root.join("sieve-mode")));
    let mut repo = acquisition::persistent_test_accounts(root);
    repo.install_sieve_fixture(&fake);
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn managesieve_transactions_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_TRANSACTIONS_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (repo, _account, _fake) = acquisition::sieve_transaction::tests::install(root);
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn folder_provisioning_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (repo, _account, _fake) = acquisition::folder_provisioning::tests::install(root);
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::ADDRESS).await.unwrap();
    axum::serve(listener, super::router(Arc::new(state)))
        .await
        .unwrap();
}
