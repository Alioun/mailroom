use super::*;
use crate::server::{ADDRESS, ORIGIN};
use std::{path::Path, sync::atomic::Ordering};
use tower::ServiceExt;

const MASTER: &str = "invented master password 41c2";
const CANARY: &str = "WEB-VAULT-CANARY-app-password-9b1e";
const CORRECTED: &str = "WEB-VAULT-CANARY-corrected-5d70";
const ACCOUNT_ROUTES: &[&str] = &[
    "backlog",
    "backlog/gmail",
    "backlog/protocol",
    "invoices",
    "ledger",
    "winddown",
    "unsubscribe",
    "subscription-review",
    "list",
    "overview",
    "lifecycle",
    "recovery",
    "recovery/import",
    "connect",
    "reconnect",
    "interrupted",
    "synthetic",
    "synthetic-gmail",
    "scan",
    "cancel",
    "open",
    "confirm",
    "report",
    "workspace/open",
    "workspace/save",
    "proposals",
    "agent-edits",
    "managesieve",
    "folders",
    "sieve-transactions",
    "proof/preview",
    "proof/download",
    "summary/review",
    "summary/export",
    "write-mode",
    "safety-settings",
    "pick-file",
    "takeout",
];

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        Self { root }
    }
    fn base(&self) -> std::path::PathBuf {
        self.root.path().canonicalize().unwrap()
    }
    fn paths(&self) -> AccountPaths {
        AccountPaths {
            config: self.base().join("config"),
            data: self.base().join("data"),
        }
    }
    /// A fresh server process over the same roots, with the dummy provider installed.
    fn start(&self) -> Arc<AppState> {
        let paths = self.paths();
        let vault = Arc::new(Vault::open_for_tests(&paths.config).unwrap());
        let mut accounts = Accounts::new(paths, vault.clone());
        crate::acquisition::install_provider_fixture(&mut accounts, &self.base());
        let mut state = AppState::new(false);
        state.accounts = AccountController::with_vault(accounts, vault).unwrap();
        Arc::new(state)
    }
    fn provider(&self, mode: &str) {
        std::fs::write(self.base().join("provider-mode"), mode).unwrap();
    }
    fn assert_no_canary_on_disk(&self) {
        for directory in [self.base().join("config"), self.base().join("data")] {
            for entry in walk(&directory) {
                let bytes = std::fs::read(&entry).unwrap();
                for canary in [CANARY, CORRECTED, MASTER] {
                    assert!(
                        !contains(&bytes, canary.as_bytes()),
                        "{canary} in {entry:?}"
                    );
                }
            }
        }
    }
}
fn walk(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = vec![];
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

async fn call(
    app: &Arc<AppState>,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let mut request = axum::http::Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let response = crate::server::router(app.clone())
        .oneshot(request.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .unwrap();
    for canary in [CANARY, CORRECTED, MASTER] {
        assert!(
            !contains(&bytes, canary.as_bytes()),
            "{canary} in a response to {path}"
        );
    }
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}
async fn post(
    app: &Arc<AppState>,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let token = app.token.to_string();
    call(
        app,
        "POST",
        path,
        Some(body),
        &[
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", &token),
        ],
    )
    .await
}
async fn status_of(app: &Arc<AppState>) -> serde_json::Value {
    let token = app.token.to_string();
    let (status, body) = call(
        app,
        "GET",
        "/api/accounts/status",
        None,
        &[("Host", ADDRESS), ("X-Mailroom-Token", &token)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body
}
/// Waits for the account worker the last request started, then returns the status.
async fn settled(app: &Arc<AppState>) -> serde_json::Value {
    let worker = app.accounts.worker.lock().unwrap().take();
    if let Some(worker) = worker {
        tokio::task::spawn_blocking(move || worker.join().unwrap())
            .await
            .unwrap();
    }
    status_of(app).await
}
fn accesses(app: &Arc<AppState>) -> usize {
    app.accounts.repository_accesses.load(Ordering::SeqCst)
}

#[tokio::test]
async fn locked_vault_refuses_every_account_and_ledger_route_before_the_repository() {
    let fixture = Fixture::new();
    {
        let app = fixture.start();
        post(
            &app,
            "/api/vault/create",
            json!({"password":MASTER,"confirmation":MASTER}),
        )
        .await;
        post(
            &app,
            "/api/accounts/connect",
            json!({"provider":"mailbox_org","login":"owner@example.test","password":CANARY}),
        )
        .await;
        assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    }
    for unlocked in [false, true] {
        let app = fixture.start();
        if unlocked {
            // A vault that locks itself again, after another writer replaced its file.
            post(&app, "/api/vault/unlock", json!({"password":MASTER})).await;
            let file = fixture.paths().config.join("vault.mrv");
            let copy = fixture.paths().config.join("copy");
            std::fs::copy(&file, &copy).unwrap();
            std::fs::rename(&copy, &file).unwrap();
            let id = app.accounts.repository().unwrap().list().unwrap()[0]
                .id
                .clone();
            let (status, _) = post(
                &app,
                "/api/accounts/reconnect",
                json!({"account":id,"password":CORRECTED}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let after = settled(&app).await;
            assert_eq!(after["job"]["error"], "vault_locked");
            assert_eq!(after["vault"], "locked");
        }
        let before = accesses(&app);
        let status = status_of(&app).await;
        assert_eq!(status["source"], "vault");
        assert_eq!(status["vault"], "locked");
        assert_eq!(status["interrupted"], json!([]));
        for route in ACCOUNT_ROUTES {
            let (code, body) = post(&app, &format!("/api/accounts/{route}"), json!({})).await;
            assert_eq!(code, StatusCode::LOCKED, "{route}");
            assert_eq!(body["code"], "vault_locked", "{route}");
        }
        assert_eq!(
            accesses(&app),
            before,
            "a locked route reached the repository"
        );
        assert_eq!(
            app.accounts.repository().err(),
            Some(AcquisitionError::VaultLocked)
        );
        assert_eq!(
            app.accounts.ledger().err(),
            Some(AcquisitionError::VaultLocked)
        );
        let (code, _) = call(
            &app,
            "POST",
            "/api/accounts/subscription-review",
            Some(json!({})),
            &[
                ("Host", ADDRESS),
                ("Origin", ORIGIN),
                ("Content-Type", "application/json"),
            ],
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "authorization runs before the vault gate"
        );
    }
    fixture.assert_no_canary_on_disk();
}

#[tokio::test]
async fn vault_routes_refuse_other_sites_hosts_and_tokenless_requests() {
    let fixture = Fixture::new();
    let app = fixture.start();
    let token = app.token.to_string();
    let body = json!({"password":MASTER,"confirmation":MASTER});
    for headers in [
        vec![
            ("Host", ADDRESS),
            ("Origin", "https://outside.example"),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", token.as_str()),
        ],
        vec![
            ("Host", "localhost:4173"),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", token.as_str()),
        ],
        vec![
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
        ],
        vec![
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", token.as_str()),
            ("Sec-Fetch-Site", "cross-site"),
        ],
    ] {
        for path in ["/api/vault/create", "/api/vault/unlock"] {
            let (code, _) = call(&app, "POST", path, Some(body.clone()), &headers).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{path} {headers:?}");
        }
    }
    let (code, _) = call(
        &app,
        "POST",
        "/api/vault/create",
        Some(body.clone()),
        &[
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "text/plain"),
            ("X-Mailroom-Token", &token),
        ],
    )
    .await;
    assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let oversized = json!({"password":"x".repeat(20 * 1024),"confirmation":"x"});
    assert_eq!(
        post(&app, "/api/vault/create", oversized).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(status_of(&app).await["vault"], "absent");
    assert!(!fixture.paths().config.join("vault.mrv").exists());
}

#[tokio::test]
async fn browser_creates_the_vault_and_connects_corrects_and_reopens_dummy_accounts() {
    let fixture = Fixture::new();
    let app = fixture.start();
    let status = status_of(&app).await;
    assert_eq!(
        (status["source"].as_str(), status["vault"].as_str()),
        (Some("vault"), Some("absent"))
    );
    assert_eq!(status["synthetic"], false);

    let (code, body) = post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":"invented other password"}),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("mismatch"))
    );
    let (code, body) = post(
        &app,
        "/api/vault/create",
        json!({"password":"short","confirmation":"short"}),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("weak_password"))
    );
    let (code, body) = post(&app, "/api/vault/unlock", json!({"password":MASTER})).await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("absent"))
    );
    let (code, body) = post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":MASTER}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["vault"], "unlocked");
    let (code, body) = post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":MASTER}),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("exists"))
    );

    fixture.provider("normal");
    let (code, body) = post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"mailbox_org","login":"owner@example.test","password":CANARY}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["job"]["progress"]["state"], "running");
    let done = settled(&app).await;
    assert_eq!(done["job"]["progress"]["state"], "complete", "{done}");
    assert!(done["job"]["progress"]["folders"].as_u64().unwrap() > 0);
    let (_, accounts) = post(&app, "/api/accounts/list", json!({})).await;
    assert_eq!(accounts[0]["connection"], "observed");
    let first = accounts[0]["id"].as_str().unwrap().to_owned();

    let (code, _) = post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"mailbox_org","login":"OWNER@example.test","password":CANARY}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        settled(&app).await["job"]["error"],
        "invalid_input",
        "duplicate account"
    );

    fixture.provider("connect_failure");
    post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"gmail","login":"second@example.test","password":CANARY}),
    )
    .await;
    let failed = settled(&app).await;
    assert_eq!(failed["job"]["progress"]["state"], "failed");
    assert_eq!(failed["job"]["error"], "tls");
    assert_eq!(failed["job"]["account"]["login"], "second@example.test");
    let second = failed["job"]["account"]["id"].as_str().unwrap().to_owned();
    let (_, accounts) = post(&app, "/api/accounts/list", json!({})).await;
    let saved = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == second.as_str())
        .unwrap();
    assert_eq!(saved["connection"], "failed");

    fixture.provider("normal");
    post(&app, "/api/accounts/reconnect", json!({"account":second})).await;
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    let repo = app.accounts.repository().unwrap();
    let before = repo.load(&AccountId::new(&second).unwrap()).unwrap();
    post(
        &app,
        "/api/accounts/reconnect",
        json!({"account":second,"password":CORRECTED}),
    )
    .await;
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    let after = repo.load(&AccountId::new(&second).unwrap()).unwrap();
    assert_eq!(repo.password(&after).unwrap().bytes(), CORRECTED.as_bytes());
    assert_eq!(
        repo.password(&before).err(),
        Some(AcquisitionError::CredentialMissing)
    );
    assert_eq!(
        serde_json::to_value(&after).unwrap()["connection"],
        "observed"
    );

    post(&app, "/api/accounts/scan", json!({"account":first})).await;
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    let (code, view) = post(&app, "/api/accounts/open", json!({"account":first})).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!view["census"]["folders"].as_object().unwrap().is_empty());
    let (_code, body) = post(
        &app,
        "/api/accounts/ledger",
        json!({"account":first,"command":{"action":"open"}}),
    )
    .await;
    assert_eq!(
        body["error"],
        "Save a rule workspace before loading the ledger"
    );
    let (code, body) = post(
        &app,
        "/api/accounts/workspace/open",
        json!({"account":first}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");

    let (code, body) = post(&app, "/api/accounts/lifecycle", json!({"action":"status"})).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["production_available"], true);
    let (code, _) = post(&app, "/api/operations", json!({"operation":"send_message"})).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, body) = post(
        &app,
        "/api/accounts/write-mode",
        json!({"live_writes":true,"confirmed":false}),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(status_of(&app).await["live_writes"], false);
    let (code, body) = post(
        &app,
        "/api/accounts/write-mode",
        json!({"live_writes":true,"confirmed":true}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["live_writes"], true);
    let (code, body) = post(
        &app,
        "/api/accounts/write-mode",
        json!({"live_writes":false}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["live_writes"], false);
    for operation in [
        Operation::SendMessage,
        Operation::MoveMessages,
        Operation::Unsubscribe,
        Operation::UploadRules,
    ] {
        assert!(policy::write_permit(operation).is_err());
    }
    drop(repo);
    drop(app);
    fixture.assert_no_canary_on_disk();

    let app = fixture.start();
    assert_eq!(status_of(&app).await["vault"], "locked");
    let (code, body) = post(
        &app,
        "/api/vault/unlock",
        json!({"password":"invented wrong password"}),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("unlock_failed"))
    );
    assert_eq!(status_of(&app).await["vault"], "locked");
    let (code, body) = post(&app, "/api/vault/unlock", json!({"password":MASTER})).await;
    assert_eq!(
        (code, body["vault"].as_str()),
        (StatusCode::OK, Some("unlocked"))
    );
    let (_, accounts) = post(&app, "/api/accounts/list", json!({})).await;
    assert_eq!(accounts.as_array().unwrap().len(), 2);
    let (code, _) = post(&app, "/api/accounts/open", json!({"account":first})).await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn an_interrupted_creation_is_listed_and_cleaned_up_from_the_browser() {
    let fixture = Fixture::new();
    let app = fixture.start();
    post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":MASTER}),
    )
    .await;
    let repo = app.accounts.repository().unwrap();
    crate::acquisition::lifecycle::inject_failure(Some("create_password"));
    assert!(
        repo.create_reviewed(
            Provider::Gmail,
            "owner@example.test".into(),
            &SecretValue::new(CANARY.as_bytes().to_vec()),
            |_| Ok(())
        )
        .is_err()
    );
    crate::acquisition::lifecycle::inject_failure(None);
    let status = status_of(&app).await;
    let pending = status["interrupted"].as_array().unwrap().clone();
    assert_eq!(pending.len(), 1);
    let (code, body) = post(
        &app,
        "/api/accounts/interrupted",
        json!({"account":pending[0]}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["interrupted"], json!([]));
    assert!(repo.list().unwrap().is_empty());
    let (code, _) = post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"gmail","login":"owner@example.test","password":CANARY}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    fixture.provider("normal");
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
}

#[tokio::test]
async fn native_and_synthetic_servers_keep_their_routes_closed() {
    for synthetic in [false, true] {
        let app = Arc::new(AppState::new(synthetic));
        let status = status_of(&app).await;
        assert_eq!(status["vault"], serde_json::Value::Null);
        assert!(!app.accounts.vault_locked());
        for (path, body) in [
            (
                "/api/vault/create",
                json!({"password":MASTER,"confirmation":MASTER}),
            ),
            ("/api/vault/unlock", json!({"password":MASTER})),
            (
                "/api/accounts/reconnect",
                json!({"account":"invented-account","password":CANARY}),
            ),
            (
                "/api/accounts/interrupted",
                json!({"account":"invented-account"}),
            ),
        ] {
            let (code, response) = post(&app, path, body).await;
            assert_eq!(
                (code, response["code"].as_str()),
                (StatusCode::FORBIDDEN, Some("not_local_web")),
                "{path}"
            );
        }
        assert_eq!(app.accounts.repository_accesses.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn vault_lifecycle_lock_password_change_takeout_recovery_and_assistant_config() {
    let fixture = Fixture::new();
    let app = fixture.start();
    let (code, _) = post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":MASTER}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    fixture.provider("normal");
    let (code, _) = post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"gmail","login":"owner@example.test","password":CANARY}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    let (_, accounts) = post(&app, "/api/accounts/list", json!({})).await;
    let id = accounts[0]["id"].as_str().unwrap().to_owned();

    // Assistant provider/model defaults and updates
    let (code, cfg) = post(
        &app,
        "/api/accounts/assistant",
        json!({"account":id,"command":{"action":"config_get"}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(cfg["config"]["provider"], "codex_home_v2");
    assert_eq!(cfg["config"]["model"], "gpt-6.1-sol");
    let (code, cfg) = post(
        &app,
        "/api/accounts/assistant",
        json!({"account":id,"command":{"action":"config_set","provider":"anthropic","model":"claude-opus"}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(cfg["config"]["provider"], "anthropic");
    assert_eq!(cfg["config"]["model"], "claude-opus");
    let (code, _) = post(
        &app,
        "/api/accounts/assistant",
        json!({"account":id,"command":{"action":"config_set","provider":"--bad","model":"x"}}),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

    // Takeout .mbox import
    let mbox = fixture.base().join("export.mbox");
    std::fs::write(
        &mbox,
        concat!(
            "From 1@xxx Sat Nov 21 00:00:00 +0000 2026\n",
            "X-Gmail-Labels: Inbox,Unread\n",
            "From: Bank <info@dkb.de>\n",
            "To: owner@example.test\n",
            "Subject: Statement\n",
            "Date: Sat, 21 Nov 2026 00:00:00 +0000\n",
            "\n",
            "PRIVATE-TAKEOUT-BODY-CANARY\n",
        ),
    )
    .unwrap();
    let (code, body) = post(
        &app,
        "/api/accounts/takeout",
        json!({"account":id,"path":mbox.to_str().unwrap()}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["summary"]["imported"], 1);

    // Encrypted recovery backup with user passphrase
    let rec_pass = "recovery backup passphrase 9876";
    let (code, preview) = post(
        &app,
        "/api/accounts/recovery",
        json!({"action":"selected_capture_preview","accounts":[id],"include_ledger":true,"passphrase":rec_pass}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{preview}");
    let (code, captured) = post(
        &app,
        "/api/accounts/recovery",
        json!({"action":"selected_capture","preview":preview["preview"],"receipt":preview["receipt"],"passphrase":rec_pass}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{captured}");
    assert_eq!(captured["captured"], true);

    // Live writes arm, then vault lock disarms live writes and locks routes
    let (code, body) = post(
        &app,
        "/api/accounts/write-mode",
        json!({"live_writes":true,"confirmed":true}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["live_writes"], true);
    let (code, body) = post(&app, "/api/vault/lock", json!({})).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["vault"], "locked");
    assert_eq!(body["live_writes"], false);
    let (code, _) = post(&app, "/api/accounts/list", json!({})).await;
    assert_eq!(code, StatusCode::LOCKED);

    // Unlock and change master password
    let (code, _) = post(&app, "/api/vault/unlock", json!({"password":MASTER})).await;
    assert_eq!(code, StatusCode::OK);
    let new_master = "brand new master password 8742";
    let (code, body) = post(
        &app,
        "/api/vault/change-password",
        json!({"current":"wrong current password","password":new_master,"confirmation":new_master}),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (code, body) = post(
        &app,
        "/api/vault/change-password",
        json!({"current":MASTER,"password":new_master,"confirmation":new_master}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, _) = post(&app, "/api/vault/lock", json!({})).await;
    assert_eq!(code, StatusCode::OK);
    let (code, _) = post(&app, "/api/vault/unlock", json!({"password":MASTER})).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    let (code, body) = post(&app, "/api/vault/unlock", json!({"password":new_master})).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["vault"], "unlocked");
    assert_eq!(body["live_writes"], false);
}

#[tokio::test]
async fn council_track1_zero_account_config_bundle_info_takeout_errors_and_readonly_preview() {
    let fixture = Fixture::new();
    let app = fixture.start();
    let (code, _) = post(
        &app,
        "/api/vault/create",
        json!({"password":MASTER,"confirmation":MASTER}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);

    // 0-account assistant config_get & config_set work without an account id
    let (code, cfg) = post(
        &app,
        "/api/accounts/assistant",
        json!({"command":{"action":"config_get"}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{cfg}");
    assert_eq!(cfg["config"]["provider"], "codex_home_v2");
    let (code, cfg) = post(
        &app,
        "/api/accounts/assistant",
        json!({"command":{"action":"config_set","provider":"anthropic","model":"claude-opus"}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{cfg}");
    assert_eq!(cfg["config"]["model"], "claude-opus");

    // 0-account recovery status includes bundles_dir and bundle_info
    let (code, rec) = post(&app, "/api/accounts/recovery", json!({"action":"status"})).await;
    assert_eq!(code, StatusCode::OK, "{rec}");
    assert!(rec["bundles_dir"].as_str().unwrap().ends_with("/bundles"));
    assert_eq!(rec["bundle_info"], json!([]));

    // Connect a dummy account & verify takeout error classification + quoted paths
    fixture.provider("normal");
    let (code, _) = post(
        &app,
        "/api/accounts/connect",
        json!({"provider":"gmail","login":"owner@example.test","password":CANARY}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(settled(&app).await["job"]["progress"]["state"], "complete");
    let (_, accounts) = post(&app, "/api/accounts/list", json!({})).await;
    let id = accounts[0]["id"].as_str().unwrap().to_owned();

    let (code, body) = post(
        &app,
        "/api/accounts/takeout",
        json!({"account":id,"path":"/nonexistent/path/takeout.zip"}),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "archive_file");

    let (code, body) = post(
        &app,
        "/api/accounts/takeout",
        json!({"account":id,"path":"/nonexistent/path/missing.mbox"}),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "not_found");

    // Quoted .mbox path succeeds and returns truncated: false
    let mbox = fixture.base().join("quoted.mbox");
    std::fs::write(
        &mbox,
        concat!(
            "From 1@xxx Sat Nov 21 00:00:00 +0000 2026\n",
            "X-Gmail-Labels: Inbox,Unread\n",
            "From: News <news@shop.example>\n",
            "To: owner@example.test\n",
            "Delivered-To: owner@example.test\n",
            "List-Unsubscribe: <https://shop.example/unsub>, <mailto:unsub@shop.example?subject=unsubscribe>\n",
            "Subject: Weekly digest\n",
            "Date: Sat, 21 Nov 2026 00:00:00 +0000\n",
            "\n",
            "body\n",
        ),
    )
    .unwrap();
    let quoted = format!("\"{}\"", mbox.display());
    let (code, body) = post(
        &app,
        "/api/accounts/takeout",
        json!({"account":id,"path":quoted}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["summary"]["imported"], 1);
    assert_eq!(body["summary"]["truncated"], false);

    // Subscriptions review is available in vault read-only mode & trusts Delivered-To
    let (code, sub_view) = post(
        &app,
        "/api/accounts/subscription-review",
        json!({"command":{"action":"open","as_of":1795219200}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{sub_view}");
    assert_eq!(sub_view["review_available"], true);
    let rows = sub_view["accounts"][0]["report"]["rows"]
        .as_array()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["recipient_evidence"], "provider_delivered_to");
    let methods: Vec<&str> = rows[0]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(methods.contains(&"local_mail"), "{methods:?}");
    assert!(methods.contains(&"browser_preparation"), "{methods:?}");
    let sub_id = rows[0]["id"].as_str().unwrap().to_owned();

    // Preview works in read-only mode
    let (code, prev) = post(
        &app,
        "/api/accounts/subscription-review",
        json!({"command":{"action":"preview","input":{"as_of":1795219200,"selections":[{"account":id,"subscription":sub_id,"method":"browser_preparation"}],"folders":{"personal":"Muted","business":"Quiet"}}}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{prev}");

    // Local-only Confirm (browser_preparation) works in read-only mode
    let (code, confirmed) = post(
        &app,
        "/api/accounts/subscription-review",
        json!({"command":{"action":"confirm","input":{"as_of":1795219200,"selections":[{"account":id,"subscription":sub_id,"method":"browser_preparation"}],"folders":{"personal":"Muted","business":"Quiet"}},"receipt":prev["receipt"],"confirmation_action":prev["confirmation_action"],"mail_confirmation_action":null}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{confirmed}");

    // External Confirm (local_mail) is STILL forbidden in read-only mode!
    let (code, prev_mail) = post(
        &app,
        "/api/accounts/subscription-review",
        json!({"command":{"action":"preview","input":{"as_of":1795219200,"selections":[{"account":id,"subscription":sub_id,"method":"local_mail"}],"folders":{"personal":"Muted","business":"Quiet"}}}}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{prev_mail}");
    let (code, _) = post(
        &app,
        "/api/accounts/subscription-review",
        json!({"command":{"action":"confirm","input":{"as_of":1795219200,"selections":[{"account":id,"subscription":sub_id,"method":"local_mail"}],"folders":{"personal":"Muted","business":"Quiet"}},"receipt":prev_mail["receipt"],"confirmation_action":prev_mail["confirmation_action"],"mail_confirmation_action":prev_mail["local_mail_confirmation"]}}),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unlocked_vault_defaults_to_30m_idle_lock_and_auto_locks_when_elapsed() {
    let fixture = Fixture::new();
    let app = fixture.start();
    let (code, created) = post(
        &app,
        "/api/vault/create",
        json!({"password": MASTER, "confirmation": MASTER}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{created}");
    assert_eq!(created["vault"], "unlocked");
    assert_eq!(created["live_writes_timeout_secs"], 900);
    assert_eq!(created["vault_idle_timeout_secs"], 1800);

    // Arm live writes, then simulate 31 minutes of inactivity.
    let (code, armed) = post(
        &app,
        "/api/accounts/write-mode",
        json!({"live_writes": true, "confirmed": true}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{armed}");
    assert_eq!(armed["live_writes"], true);

    app.accounts.rewind_safety_timers_for_test(25 * 60);
    // Passive background polls (like Assistant status) must NOT reset the idle timer.
    let token = app.token.to_string();
    let (code, _) = call(
        &app,
        "POST",
        "/api/accounts/list",
        Some(json!({})),
        &[
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", &token),
            ("X-Mailroom-Passive", "1"),
        ],
    )
    .await;
    assert_eq!(code, StatusCode::OK);

    app.accounts.rewind_safety_timers_for_test(6 * 60);
    let after = status_of(&app).await;
    assert_eq!(after["vault"], "locked");
    assert_eq!(after["live_writes"], false);

    // Even passive requests remain blocked with 423 once locked.
    let (code, body) = call(
        &app,
        "POST",
        "/api/accounts/list",
        Some(json!({})),
        &[
            ("Host", ADDRESS),
            ("Origin", ORIGIN),
            ("Content-Type", "application/json"),
            ("X-Mailroom-Token", &token),
            ("X-Mailroom-Passive", "1"),
        ],
    )
    .await;
    assert_eq!(code, StatusCode::LOCKED, "{body}");
}
