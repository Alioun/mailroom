use super::*;

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
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

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
    state.accounts.installed_ledger = Some(
        crate::ledger::InstalledLedger::install(
            std::env::var_os("MAILROOM_RECOVERY_LEDGER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::Path::new(&root).join("ledger")),
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn production_consequential_routes_deny_before_repository_or_confirmation_access() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(false));
    let router = super::super::router(state.clone());
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
            json!({"account":"synthetic-account","command":{"action":"preview","selection":"INBOX/1/1","target":"accounting","mode":"smtp"}}),
        ),
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"send","selection":"INBOX/1/1","target":"accounting","receipt":"a".repeat(64),"confirmation_action":"yes","mode":"smtp"}}),
        ),
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"resolve","send":"a".repeat(64),"resolution":"not_sent"}}),
        ),
        (
            "invoices",
            json!({"account":"synthetic-account","command":{"action":"recover","send":"a".repeat(64)}}),
        ),
        (
            "unsubscribe",
            json!({"account":"synthetic-account","command":{"action":"resolve","entry":"a".repeat(64),"resolution":"sent","as_of":1792886400_i64}}),
        ),
        (
            "unsubscribe",
            json!({"account":"synthetic-account","command":{"action":"recover","entry":"a".repeat(64),"as_of":1792886400_i64}}),
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
                    .header("Host", super::super::ADDRESS)
                    .header("Origin", super::super::ORIGIN)
                    .header("Content-Type", "application/json")
                    .header("X-Mailroom-Token", state.token.as_str())
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path} {body}");
        let wired = body["command"]["mode"].is_string()
            || body["command"]["action"] == "resolve"
            || path == "subscription-review"
            || (path == "unsubscribe" && body["command"]["action"] != "recover");
        let answer: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        if wired {
            assert_eq!(
                answer["error"],
                policy::authorize(Operation::SendMessage).reason,
                "{path} {body}: the route reached the write policy, which denied it"
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
    let refused = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/accounts/invoices")
                .header("Host", super::super::ADDRESS)
                .header("Origin", super::super::ORIGIN)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", state.token.as_str())
                .body(axum::body::Body::from(
                    json!({"account":"synthetic-account","command":{"action":"preview","selection":"INBOX/1/1","target":"accounting","mode":"mail_app"}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "no invoice Mail handoff exists"
    );
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
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn managesieve_transactions_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_TRANSACTIONS_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (mut repo, account, _fake) = acquisition::sieve_transaction::tests::install(root);
    if let Ok(epoch) = std::fs::read_to_string(root.join("workspace-authority")) {
        if !epoch.is_empty() {
            repo.install_synthetic_workspace(&account, &epoch);
        }
    }
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn folder_provisioning_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (mut repo, account, _fake) = acquisition::folder_provisioning::tests::install(root);
    if let Ok(epoch) = std::fs::read_to_string(root.join("workspace-authority")) {
        if !epoch.is_empty() {
            repo.install_synthetic_workspace(&account, &epoch);
        }
    }
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn envelope_keyword_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_ENVELOPE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (repo, _account, _folder, _sieve) =
        acquisition::workspace_authority::envelope_tests::install(root);
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn gmail_backlog_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_GMAIL_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (mut repo, _account, _fake) = acquisition::gmail_backlog::fixture_install(root);
    if let Some(installation) = std::env::var_os("MAILROOM_GMAIL_INSTALLATION") {
        repo = acquisition::persistent_test_accounts(std::path::Path::new(&installation));
    }
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn backlog_protocol_server_fixture() {
    let Some(root) = std::env::var_os("MAILROOM_MOVE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let (mut repo, _account, _fake) = acquisition::backlog::fixture_install(root);
    if let Some(installation) = std::env::var_os("MAILROOM_MOVE_INSTALLATION") {
        repo = acquisition::persistent_test_accounts(std::path::Path::new(&installation));
    }
    let mut state = AppState::new(true);
    state.accounts.source = AccountSource::Synthetic {
        accounts: Arc::new(repo),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: root.join("mailboxes"),
    };
    state.accounts.installed_ledger =
        Some(crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap());
    let listener = tokio::net::TcpListener::bind(super::super::ADDRESS)
        .await
        .unwrap();
    axum::serve(listener, super::super::router(Arc::new(state)))
        .await
        .unwrap();
}

#[tokio::test]
async fn safety_timers_auto_disarm_and_respect_running_jobs() {
    let state = Arc::new(AppState::new(true));
    state.accounts.arm_live_writes(true, Some(120));
    assert!(state.accounts.live_writes());
    assert!(
        state
            .accounts
            .safety_snapshot()
            .live_writes_expires_in_secs
            .is_some_and(|s| s <= 120)
    );

    // While a job is actively running, timers genuinely pause and preserve remaining time.
    let before_job = state
        .accounts
        .safety_snapshot()
        .live_writes_expires_in_secs
        .unwrap();
    let _cancel = state.accounts.reserve("scan").unwrap();
    state.accounts.rewind_safety_timers_for_test(300);
    assert!(state.accounts.live_writes());
    let during_job = state
        .accounts
        .safety_snapshot()
        .live_writes_expires_in_secs
        .unwrap();
    assert!(
        during_job + 2 >= before_job,
        "paused timer must preserve remaining seconds during a job: before={before_job} during={during_job}"
    );

    // Once the job finishes, remaining time resumes intact rather than immediately disarming.
    state.accounts.finish(Ok(()));
    assert!(state.accounts.live_writes());
    assert!(
        state
            .accounts
            .safety_snapshot()
            .live_writes_expires_in_secs
            .is_some_and(|s| s >= before_job.saturating_sub(2))
    );

    // After resuming, an elapsed live_writes window disarms automatically.
    state.accounts.rewind_safety_timers_for_test(300);
    assert!(!state.accounts.live_writes());
    assert!(
        state
            .accounts
            .safety_snapshot()
            .live_writes_expires_in_secs
            .is_none()
    );
}

#[tokio::test]
async fn events_and_overview_routes_enforce_session_and_report_changes() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(true));
    let router = super::super::router(state.clone());

    // Initial event fetch returns current seq and status.
    let res = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/accounts/events")
                .header("Host", super::super::ADDRESS)
                .header("Origin", super::super::ORIGIN)
                .header("X-Mailroom-Token", state.token.as_str())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let first: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    let seq1 = first["seq"].as_u64().unwrap();
    assert_eq!(first["status"]["live_writes"], false);

    // Changing write-mode increments seq so ?since=seq1 returns immediately.
    state.accounts.arm_live_writes(true, Some(300));
    let res2 = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("/api/accounts/events?since={seq1}&timeout_ms=50"))
                .header("Host", super::super::ADDRESS)
                .header("Origin", super::super::ORIGIN)
                .header("X-Mailroom-Token", state.token.as_str())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let second: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res2.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(second["seq"].as_u64().unwrap() > seq1);
    assert_eq!(second["status"]["live_writes"], true);

    // Overview returns structured accounts and recent_actions arrays.
    let ov = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/accounts/overview")
                .header("Host", super::super::ADDRESS)
                .header("Origin", super::super::ORIGIN)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", state.token.as_str())
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ov.status(), StatusCode::OK);
    let ov_json: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(ov.into_body(), 1 << 20).await.unwrap())
            .unwrap();
    assert!(ov_json["accounts"].is_array());
    assert!(ov_json["recent_actions"].is_array());
}
