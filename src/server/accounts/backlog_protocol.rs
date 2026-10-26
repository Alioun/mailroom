use super::*;
use crate::backlog::{
    Options, gmail_journal,
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
    let status_only = matches!(
        &body,
        Ok(Json(Request {
            command: Action::Status {},
            ..
        }))
    );
    if !status_only
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if kind.installed(accounts, None))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":kind.denial()}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !status_only
        && !matches!(&state.accounts.source, AccountSource::Synthetic { accounts, .. } if kind.installed(accounts, Some(&request.account)))
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
    }) || saved
        .invoices
        .sends
        .iter()
        .any(|s| s.status != crate::invoice::Status::Completed)
}
fn execute(
    state: &Arc<AppState>,
    request: &Request,
    cancel: Cancellation,
    kind: Kind,
) -> Result<serde_json::Value, AcquisitionError> {
    let repo = state.accounts.repository()?;
    let account = repo.load(&request.account)?;
    let store = repo.store(&account)?;
    let mut saved = store.load_state()?;
    let journal = |saved: &crate::census::AccountState| match kind {
        Kind::Move => json!(saved.protocol_backlog),
        Kind::Gmail => json!(saved.gmail_backlog),
    };
    let report = |saved: &crate::census::AccountState, mut value: serde_json::Value| {
        if kind == Kind::Gmail {
            value["outcomes"] = gmail_journal::outcomes(&saved.gmail_backlog);
        }
        value
    };
    if matches!(request.command, Action::Status {}) {
        return Ok(report(
            &saved,
            json!({"installed":kind.installed(&repo, Some(&account.id)),"recovery_hold":store.recovery_pending(),"journal":journal(&saved),"limitations":match kind { Kind::Move => protocol::LIMITATIONS, Kind::Gmail => gmail_journal::LIMITATIONS }}),
        ));
    }
    if store.recovery_pending() {
        return Err(AcquisitionError::LifecyclePending);
    }
    if matches!(request.command, Action::Cancel {}) {
        let runs: Vec<(&mut Status, &mut bool)> = match kind {
            Kind::Move => saved
                .protocol_backlog
                .runs
                .iter_mut()
                .map(|r| (&mut r.status, &mut r.stop_requested))
                .collect(),
            Kind::Gmail => saved
                .gmail_backlog
                .runs
                .iter_mut()
                .map(|r| (&mut r.status, &mut r.stop_requested))
                .collect(),
        };
        for (status, stop_requested) in runs {
            if !matches!(status, Status::Complete | Status::Undone) {
                *stop_requested = true;
                if *status == Status::Prepared {
                    *status = Status::Stopped;
                }
            }
        }
        store.save_state(&mut saved)?;
        return Ok(report(
            &saved,
            json!({"cancel_requested":true,"journal":journal(&saved)}),
        ));
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
            if kind == Kind::Gmail {
                if let Some(run) = s.gmail_backlog.runs.last() {
                    let entries = run.entries(run.undo_receipt.is_some());
                    job.progress.saved = entries
                        .iter()
                        .filter(|b| b.verified())
                        .map(|b| b.intent().messages().len())
                        .sum();
                    job.progress.folders = entries.len();
                }
            } else if let Some(run) = s.protocol_backlog.runs.last() {
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
    if kind == Kind::Gmail {
        match &request.command {
            Action::Preview { options } => {
                return Ok(json!(repo.gmail_preview(
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
                if !confirmed || other_effects_pending(&saved) {
                    return Err(AcquisitionError::InvalidInput);
                }
                repo.gmail_confirm(
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
                repo.gmail_reconcile(&account, &store, &mut saved, run, cancel)?
            }
            Action::Resume { run } => {
                repo.gmail_run(&account, &store, &mut saved, run, false, cancel, fresh)?
            }
            Action::UndoPreview { run } => {
                return Ok(json!(
                    repo.gmail_undo_preview(&account, &store, &saved, run, cancel)?
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
                repo.gmail_undo(&account, &store, &mut saved, run, receipt, cancel, fresh)?;
            }
            Action::ResumeUndo { run } => {
                repo.gmail_run(&account, &store, &mut saved, run, true, cancel, fresh)?
            }
            Action::Status {} | Action::Cancel {} => unreachable!(),
        }
        return Ok(
            json!({"journal":saved.gmail_backlog,"outcomes":gmail_journal::outcomes(&saved.gmail_backlog)}),
        );
    }
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
            if !confirmed || other_effects_pending(&saved) {
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
async fn gmail_route_denies_every_uninstalled_or_wrong_source_before_repository_or_credentials() {
    use tower::ServiceExt;
    let post = |state: Arc<AppState>, uri: &'static str, body: String| async move {
        super::super::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Host", super::super::ADDRESS)
                    .header("Origin", super::super::ORIGIN)
                    .header("Content-Type", "application/json")
                    .header("X-Mailroom-Token", state.token.as_str())
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    };
    let bodies = |id: &str| {
        [
            "{".to_string(),
            json!({"account":id,"command":{"action":"cancel"}}).to_string(),
            json!({"account":id,"command":{"action":"preview","options":crate::backlog::Options::default()}}).to_string(),
            json!({"account":id,"command":{"action":"confirm","options":crate::backlog::Options::default(),"receipt":"a".repeat(64),"confirmed":true,"label":"Privat/Studio"}}).to_string(),
        ]
    };
    for synthetic in [false, true] {
        let state = Arc::new(AppState::new(synthetic));
        for body in bodies("invented") {
            assert_eq!(
                post(state.clone(), "/api/accounts/backlog/gmail", body).await,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
    for case in [
        "move_only",
        "gmail_absent",
        "gmail_wrong_account",
        "gmail_on_move_route",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, account, _move, _gmail) = if case == "move_only" {
            let (repo, account, fake) = acquisition::backlog::fixture_install(root.path());
            (repo, account, Some(fake), None)
        } else {
            if case == "gmail_absent" {
                std::fs::write(root.path().join("gmail-source"), "absent").unwrap();
            }
            let (repo, account, fake) = acquisition::gmail_backlog::fixture_install(root.path());
            (repo, account, None, Some(fake))
        };
        let credentials = repo.track_credential_accesses();
        let mut app = AppState::new(true);
        app.accounts.source = AccountSource::Synthetic {
            accounts: Arc::new(repo),
            _directory: tempfile::tempdir().unwrap(),
            mailboxes: root.path().join("mailboxes"),
        };
        let state = Arc::new(app);
        let id = if case == "gmail_wrong_account" {
            "uninstalled-account"
        } else {
            account.id.as_str()
        };
        let uri = if case == "gmail_on_move_route" {
            "/api/accounts/backlog/protocol"
        } else {
            "/api/accounts/backlog/gmail"
        };
        for body in bodies(id).into_iter().skip(1) {
            assert_eq!(
                post(state.clone(), uri, body).await,
                if case == "gmail_wrong_account" {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::FORBIDDEN
                },
                "{case}"
            );
        }
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{case}"
        );
        assert_eq!(
            credentials.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{case}"
        );
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

#[cfg(test)]
mod authority_matrix {
    use super::*;
    use crate::acquisition::backlog::{fixture_install, fixture_mutated};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    const LEAVES: [&str; 11] = [
        "account",
        "login",
        "address_remote",
        "address_port",
        "tls_name",
        "root",
        "epoch",
        "authority",
        "workspace_epoch",
        "workspace_absent",
        "none",
    ];

    struct Installed {
        state: Arc<AppState>,
        credentials: Arc<AtomicUsize>,
    }

    fn install(root: &std::path::Path, original: &acquisition::Accounts, leaf: &str) -> Installed {
        let mut repo = fixture_mutated(root, original, leaf);
        let account = original.list().unwrap().remove(0);
        let credentials = repo.track_password_accesses(&account);
        let mut app = AppState::new(true);
        app.accounts.source = AccountSource::Synthetic {
            accounts: Arc::new(repo),
            _directory: tempfile::tempdir().unwrap(),
            mailboxes: root.join("mailboxes"),
        };
        app.accounts.ledger =
            Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
        Installed {
            state: Arc::new(app),
            credentials,
        }
    }

    /// Posts one protocol command. A started worker is joined so its saved
    /// effects and failure are observable before the next comparison.
    async fn post(
        state: &Arc<AppState>,
        account: &AccountId,
        command: serde_json::Value,
    ) -> (StatusCode, bool, serde_json::Value) {
        let body = json!({"account": account, "command": command}).to_string();
        let response = super::super::super::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/accounts/backlog/protocol")
                    .header("Host", super::super::super::ADDRESS)
                    .header("Origin", super::super::super::ORIGIN)
                    .header("Content-Type", "application/json")
                    .header("X-Mailroom-Token", state.token.as_str())
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if value["started"] != true {
            return (status, false, value);
        }
        let worker = state.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(handle) = worker.accounts.worker.lock().unwrap().take() {
                handle.join().unwrap();
            }
        })
        .await
        .unwrap();
        let job = state.accounts.job.lock().unwrap().clone().unwrap();
        (status, job.progress.state == ScanState::Complete, value)
    }

    struct Observed {
        wires: usize,
        logins: usize,
        journal: crate::backlog::protocol::Journal,
    }

    fn observe(
        root: &std::path::Path,
        repo: &acquisition::Accounts,
        account: &acquisition::Account,
    ) -> Observed {
        let provider = repo
            .fixture_sieve_store(&account.id, root)
            .unwrap()
            .load_state()
            .unwrap();
        Observed {
            wires: std::fs::read_to_string(root.join("move-wire-exact.jsonl"))
                .unwrap_or_default()
                .lines()
                .count(),
            logins: provider
                .synthetic_folders
                .unwrap()
                .commands
                .iter()
                .filter(|c| *c == "LOGIN")
                .count(),
            journal: repo
                .store(account)
                .unwrap()
                .load_state()
                .unwrap()
                .protocol_backlog,
        }
    }

    fn restore(
        repo: &acquisition::Accounts,
        account: &acquisition::Account,
        journal: &crate::backlog::protocol::Journal,
    ) {
        let store = repo.store(account).unwrap();
        let mut saved = store.load_state().unwrap();
        if serde_json::to_vec(&saved.protocol_backlog).unwrap()
            != serde_json::to_vec(journal).unwrap()
        {
            saved.protocol_backlog = journal.clone();
            store.save_state(&mut saved).unwrap();
        }
    }

    /// Evidence of every run without its status, which a binding conflict may set.
    fn evidence(journal: &crate::backlog::protocol::Journal) -> serde_json::Value {
        let mut value = serde_json::to_value(journal).unwrap();
        for run in value["runs"].as_array_mut().unwrap() {
            run.as_object_mut().unwrap().remove("status");
        }
        value
    }

    /// What one mutated leaf may do for one action. `observes` marks a fresh
    /// preview under a different but self-consistent installation, which reads
    /// the mailbox and never writes.
    fn expected(leaf: &str, action: &str) -> (StatusCode, bool, bool, bool) {
        const DENIED: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;
        let worker = matches!(action, "confirm" | "resume" | "undo" | "resume_undo");
        let fresh = matches!(action, "preview" | "confirm");
        let status = if worker { StatusCode::OK } else { DENIED };
        match leaf {
            "authority" | "address_remote" => (StatusCode::FORBIDDEN, false, false, false),
            "account" => (DENIED, false, false, false),
            "login" if action == "cancel" => (StatusCode::OK, false, false, false),
            "login" => (status, false, false, false),
            _ if action == "cancel" => (StatusCode::OK, false, false, false),
            "address_port" | "tls_name" | "root" => (status, fresh, false, !fresh),
            "workspace_absent" => (status, fresh, fresh, !fresh),
            "epoch" | "workspace_epoch" => (
                if action == "preview" {
                    StatusCode::OK
                } else {
                    status
                },
                fresh,
                fresh,
                !fresh,
            ),
            _ => unreachable!(),
        }
    }

    /// A receipt the unmutated installation issues for the current saved state,
    /// so a denial below can only come from the mutated leaf.
    async fn receipt(
        control: &Installed,
        account: &AccountId,
        command: &serde_json::Value,
    ) -> String {
        let preview = if command["action"] == "undo" {
            json!({"action":"undo_preview","run":command["run"]})
        } else {
            json!({"action":"preview","options":command["options"]})
        };
        let (status, _, value) = post(&control.state, account, preview).await;
        assert_eq!(status, StatusCode::OK);
        value["receipt"].as_str().unwrap().to_owned()
    }

    async fn matrix(
        root: &std::path::Path,
        original: &acquisition::Accounts,
        account: &acquisition::Account,
        control: &Installed,
        actions: &[(&str, serde_json::Value)],
        results: &mut Vec<serde_json::Value>,
    ) {
        let snapshot = observe(root, original, account).journal;
        for leaf in LEAVES.iter().filter(|l| **l != "none") {
            let installed = install(root, original, leaf);
            for (action, command) in actions {
                restore(original, account, &snapshot);
                let mut command = command.clone();
                if command.get("receipt").is_some() {
                    command["receipt"] = receipt(control, &account.id, &command).await.into();
                }
                let before = observe(root, original, account);
                let repository = installed
                    .state
                    .accounts
                    .repository_accesses
                    .load(Ordering::SeqCst);
                let credentials = installed.credentials.load(Ordering::SeqCst);
                let (status, completed, _) = post(&installed.state, &account.id, command).await;
                let after = observe(root, original, account);
                let (want, password, login, conflict) = expected(leaf, action);
                let read_repository = installed
                    .state
                    .accounts
                    .repository_accesses
                    .load(Ordering::SeqCst)
                    > repository;
                let read_password = installed.credentials.load(Ordering::SeqCst) > credentials;
                let label = format!("{leaf} {action}");
                assert_eq!(status, want, "{label}");
                assert!(!completed, "{label}");
                assert_eq!(read_password, password, "{label}");
                assert_eq!(after.logins > before.logins, login, "{label}");
                assert_eq!(after.wires, before.wires, "{label}");
                if matches!(*leaf, "authority" | "address_remote" | "account") {
                    assert!(!read_repository, "{label}");
                }
                assert_eq!(
                    evidence(&after.journal),
                    evidence(&before.journal),
                    "{label}"
                );
                for (a, b) in after.journal.runs.iter().zip(&before.journal.runs) {
                    assert!(
                        a.status == b.status || (conflict && a.status == Status::Conflict),
                        "{label}"
                    );
                }
                assert_eq!(
                    after.journal.runs.len(),
                    before.journal.runs.len(),
                    "{label}"
                );
                results.push(json!({"leaf":leaf,"action":action,"status":status.as_u16(),"repository":read_repository,"password":read_password,"login":after.logins > before.logins,"moves":after.wires - before.wires}));
            }
        }
        restore(original, account, &snapshot);
    }

    fn stop_before_first_batch(
        cancel: &Cancellation,
    ) -> impl Fn(&crate::census::AccountState) -> Result<(), String> {
        let calls = std::cell::Cell::new(0);
        let cancel = cancel.clone();
        move |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                cancel.cancel();
            }
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_installed_authority_leaf_mutation_denies_each_protocol_action_before_effects() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::write(root.join("workspace-authority"), "matrix-fixture-v1").unwrap();
        let (original, account, _fake) = fixture_install(root);
        let options = serde_json::to_value(crate::backlog::Options::default()).unwrap();
        let mut results = vec![];
        let control = install(root, &original, "none");
        let (status, _, fresh) = post(
            &control.state,
            &account.id,
            json!({"action":"preview","options":options}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let issued = fresh["receipt"].as_str().unwrap().to_owned();
        matrix(
            root,
            &original,
            &account,
            &control,
            &[
                ("preview", json!({"action":"preview","options":options})),
                ("confirm", json!({"action":"confirm","options":options,"receipt":"fresh","confirmed":true})),
            ],
            &mut results,
        )
        .await;
        let cancel = Cancellation::default();
        let store = original.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        assert!(
            original
                .backlog_confirm(
                    &account,
                    &store,
                    &mut saved,
                    crate::backlog::Options::default(),
                    &issued,
                    cancel.clone(),
                    stop_before_first_batch(&cancel)
                )
                .is_err()
        );
        drop(store);
        let run = observe(root, &original, &account).journal.runs[0].clone();
        assert_eq!(run.status, Status::Stopped);
        assert!(!run.batches[0].attempted());
        let id = run.id.clone();
        matrix(
            root,
            &original,
            &account,
            &control,
            &[
                ("resume", json!({"action":"resume","run":id})),
                ("cancel", json!({"action":"cancel"})),
            ],
            &mut results,
        )
        .await;
        let (status, completed, _) = post(
            &control.state,
            &account.id,
            json!({"action":"resume","run":id}),
        )
        .await;
        assert_eq!((status, completed), (StatusCode::OK, true));
        assert_eq!(
            observe(root, &original, &account).journal.runs[0].status,
            Status::Complete
        );
        let (status, _, _) = post(
            &control.state,
            &account.id,
            json!({"action":"reconcile","run":id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        matrix(
            root,
            &original,
            &account,
            &control,
            &[
                ("reconcile", json!({"action":"reconcile","run":id})),
                ("undo_preview", json!({"action":"undo_preview","run":id})),
                (
                    "undo",
                    json!({"action":"undo","run":id,"receipt":"fresh","confirmed":true}),
                ),
            ],
            &mut results,
        )
        .await;
        let undo = receipt(&control, &account.id, &json!({"action":"undo","run":id})).await;
        let cancel = Cancellation::default();
        let first = std::cell::Cell::new(true);
        let stopping = cancel.clone();
        let store = original.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        assert!(
            original
                .backlog_undo(
                    &account,
                    &store,
                    &mut saved,
                    &id,
                    &undo,
                    cancel,
                    move |_| {
                        if first.replace(false) {
                            stopping.cancel();
                        }
                        Ok(())
                    }
                )
                .is_err()
        );
        drop(store);
        let run = observe(root, &original, &account).journal.runs[0].clone();
        assert_eq!(run.status, Status::Stopped);
        assert!(run.undo_receipt.is_some() && !run.reverse[0].evidence.attempted());
        matrix(
            root,
            &original,
            &account,
            &control,
            &[("resume_undo", json!({"action":"resume_undo","run":id}))],
            &mut results,
        )
        .await;
        let (status, completed, _) = post(
            &control.state,
            &account.id,
            json!({"action":"resume_undo","run":id}),
        )
        .await;
        assert_eq!((status, completed), (StatusCode::OK, true));
        let done = observe(root, &original, &account);
        assert_eq!(done.journal.runs[0].status, Status::Undone);
        assert_eq!(done.wires, 2);
        assert_eq!(results.len(), 10 * 8);
        if let Some(path) = std::env::var_os("MAILROOM_AUTHORITY_MATRIX") {
            std::fs::write(path, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
        }
    }
}
