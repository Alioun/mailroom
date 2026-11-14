use super::*;
use crate::{
    agent::{Proposal, files::Exchange},
    rules::agent_edits::{REJECTION_LIMIT, Rejection},
};
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

const AS_OF: i64 = 1792886400;

fn fixture() -> (Arc<AppState>, Account) {
    let state = Arc::new(AppState::new(true));
    let repo = state.accounts.repository().unwrap();
    let account = repo
        .create(
            Provider::MailboxOrg,
            "owner@example.test".into(),
            &SecretValue::new(b"invented-password".to_vec()),
        )
        .unwrap();
    acquisition::synthetic::scan(&repo, &account.id, Cancellation::default(), |_| {}).unwrap();
    (state, account)
}

async fn post(state: &Arc<AppState>, route: &str, value: Value) -> (StatusCode, Value) {
    let response = crate::server::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/accounts/{route}"))
                .header("Host", crate::server::ADDRESS)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", state.token.as_str())
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn summary(state: &Arc<AppState>, account: &Account) -> (Value, String) {
    let (status, report) = post(state, "report", json!({"account":account.id,"as_of":AS_OF})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, preview) = post(
        state,
        "summary/review",
        json!({"account":account.id,"as_of":AS_OF,"version":report["version"],"approved":[]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (
        json!({"as_of":AS_OF,"version":report["version"],"approved":[],"receipt":preview["receipt"]}),
        preview["summary"].as_str().unwrap().into(),
    )
}

async fn assistant(
    state: &Arc<AppState>,
    account: &Account,
    command: Value,
) -> (StatusCode, Value) {
    post(
        state,
        "assistant",
        json!({"account":account.id,"command":command}),
    )
    .await
}

#[tokio::test]
async fn browser_share_revalidates_the_exact_preview_and_retries_keep_one_private_request() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (state, account) = fixture();
    let (summary, text) = summary(&state, &account).await;
    let mut changed = summary.clone();
    changed["receipt"] = json!("0".repeat(64));
    for command in [
        json!({"action":"prepare","summary":summary,"confirmed":false}),
        json!({"action":"prepare","summary":changed,"confirmed":true}),
    ] {
        assert_eq!(
            assistant(&state, &account, command).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let command = json!({"action":"prepare","summary":summary,"confirmed":true});
    let (status, first) = assistant(&state, &account, command.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["requests"][0]["phase"], "prepared");
    let (_, again) = assistant(&state, &account, command).await;
    assert_eq!(again["requests"].as_array().unwrap().len(), 1);
    let id = first["requests"][0]["id"].as_str().unwrap();
    let repo = state.accounts.repository().unwrap();
    let inbox = Inbox::open(&repo.paths.data, account.id.as_str()).unwrap();
    let path = inbox.exchange_path(id);
    assert_eq!(Exchange::open(&path).unwrap().summary().unwrap(), text);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path.join("reviewed/census-summary.md"))
            .unwrap()
            .mode()
            & 0o7777,
        0o600
    );
    assert!(!path.join("request.json").exists());
    let config = std::fs::read_to_string(path.join(".mcp.json")).unwrap();
    assert!(!config.contains(state.token.as_str()));
    assert!(!config.contains("invented-password"));
    assert_eq!(first["requests"][0]["summary"]["approved"], json!([]));
}

#[tokio::test]
async fn proposals_load_automatically_and_dismissal_survives_restart_and_full_rejection_history() {
    let (state, account) = fixture();
    let (summary, _) = summary(&state, &account).await;
    let (status, prepared) = assistant(
        &state,
        &account,
        json!({"action":"prepare","summary":summary,"confirmed":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let id = prepared["requests"][0]["id"].as_str().unwrap();
    let repo = state.accounts.repository().unwrap();
    let inbox = Inbox::open(&repo.paths.data, account.id.as_str()).unwrap();
    let exchange = Exchange::open(&inbox.exchange_path(id)).unwrap();
    let mut launched = inbox.record(id).unwrap();
    launched.launch = Launch::Launched {
        thread_id: "thread:fixture".into(),
        thread_link: None,
    };
    inbox.save(&launched).unwrap();
    let (_, status) = assistant(&state, &account, json!({"action":"status"})).await;
    assert_eq!(
        status["requests"][0]["phase"], "launched",
        "launch alone does not count as connected"
    );
    exchange.record_read().unwrap();
    let (_, status) = assistant(&state, &account, json!({"action":"status"})).await;
    assert_eq!(status["requests"][0]["phase"], "connected");
    let proposal: Proposal = serde_json::from_value(json!({"version":1,"summary_sha256":prepared["requests"][0]["summary_sha256"],"recommendations":[{"title":"Review filing","reason":"Invented census counts support review","suggested_change":"Review a local rule"}]})).unwrap();
    let proposal_id = exchange.submit(proposal).unwrap();
    let (_, ready) = assistant(&state, &account, json!({"action":"status"})).await;
    assert_eq!(ready["requests"][0]["phase"], "proposal_ready");
    assert_eq!(ready["requests"][0]["proposals"][0]["id"], proposal_id);
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved.proposals.agent_rejections = (0..REJECTION_LIMIT)
        .map(|i| Rejection {
            edit: format!("{i:064x}"),
            target: "topic.money".into(),
            summary: "0".repeat(64),
            as_of: AS_OF,
        })
        .collect();
    store.save_state(&mut saved).unwrap();
    let command = json!({"action":"dismiss","request_id":id,"proposal_id":proposal_id});
    let busy = state.accounts.operation.lock().unwrap();
    let (status, dismissed) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        assistant(&state, &account, command.clone()),
    )
    .await
    .expect("dismiss must not wait for a census operation");
    drop(busy);
    assert_eq!(status, StatusCode::OK, "{dismissed}");
    assert_eq!(dismissed["requests"][0]["proposals"][0]["dismissed"], true);
    assert_eq!(assistant(&state, &account, command).await.0, StatusCode::OK);
    let reopened = Inbox::open(&repo.paths.data, account.id.as_str()).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.views().unwrap()).unwrap()[0]["proposals"][0]["dismissed"],
        true
    );
    assert_eq!(
        store.load_state().unwrap().proposals.agent_rejections.len(),
        REJECTION_LIMIT
    );
}

#[tokio::test]
async fn browser_share_avoids_the_native_dialog_and_native_source_keeps_it() {
    use crate::confirmation::{Backend, Boundary};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Refuse(Arc<AtomicUsize>);
    impl Backend for Refuse {
        fn available(&self) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("Native dialog unavailable".into())
        }
        fn synthetic(&self) -> bool {
            false
        }
        fn exchange(&mut self, _: &[u8], _: &[u8; 32]) -> Result<Vec<u8>, String> {
            panic!("unavailable dialog must not run")
        }
    }
    let (mut state, account) = fixture();
    let prompts = Arc::new(AtomicUsize::new(0));
    Arc::get_mut(&mut state).unwrap().accounts.confirmations =
        Mutex::new(Boundary::with_backend(Box::new(Refuse(prompts.clone()))));
    let (summary, _) = summary(&state, &account).await;
    let command = json!({"action":"prepare","summary":summary,"confirmed":true});
    let (status, body) = assistant(&state, &account, command.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prompts.load(Ordering::SeqCst), 0);
    let app = Arc::get_mut(&mut state).unwrap();
    let old = std::mem::replace(&mut app.accounts.source, AccountSource::Unavailable);
    let AccountSource::Synthetic {
        accounts,
        _directory,
        ..
    } = old
    else {
        panic!("synthetic fixture")
    };
    app.accounts.source = AccountSource::Installed { accounts };
    assert_eq!(
        assistant(&state, &account, command).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn launch_returns_before_the_cli_finishes_and_reconciles_saved_launches_without_duplicates() {
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
    let (mut state, account) = fixture();
    let fixture_root = tempfile::tempdir().unwrap();
    let cli = fixture_root.path().join("t3-fixture");
    fs::write(&cli, r##"#!/bin/sh
set -eu
case "$1 $2" in
  'thread list') if test -f title; then printf '[{"title":"%s","threadId":"thread:fixture"}]' "$(/bin/cat title)"; else printf '[]'; fi;;
  'thread start') printf '%s' "$4" > title; printf 'start\n' >> starts; /bin/cat > prompt; while ! test -f release; do /bin/sleep 0.02; done; printf '%s' '{"threadId":"thread:fixture"}';;
  'thread show') printf '%s' '{"thread":{"link":"t3-thread://v2/fixture/thread"},"messages":[]}';;
  *) exit 1;;
esac
"##).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    Arc::get_mut(&mut state).unwrap().accounts.assistant_cli = Some(t3::Client::for_tests(
        cli,
        fixture_root.path().as_os_str().to_owned(),
    ));
    let (summary, _) = summary(&state, &account).await;
    let (_, prepared) = assistant(
        &state,
        &account,
        json!({"action":"prepare","summary":summary,"confirmed":true}),
    )
    .await;
    let id = prepared["requests"][0]["id"].as_str().unwrap();
    let command = json!({"action":"launch","request_id":id});
    let (status, launching) = tokio::time::timeout(
        Duration::from_secs(2),
        assistant(&state, &account, command.clone()),
    )
    .await
    .expect("launch response must not wait for the CLI");
    assert_eq!(status, StatusCode::OK, "{launching}");
    assert_eq!(launching["requests"][0]["phase"], "launching");
    let (_, again) = assistant(&state, &account, command.clone()).await;
    assert_eq!(again["requests"][0]["phase"], "launching");
    let repo = state.accounts.repository().unwrap();
    let inbox = Inbox::open(&repo.paths.data, account.id.as_str()).unwrap();
    let exchange = inbox.exchange_path(id);
    fs::write(exchange.join("release"), b"").unwrap();
    let wait = || async {
        loop {
            if matches!(inbox.record(id).unwrap().launch, Launch::Launched { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(3), wait())
        .await
        .unwrap();
    let (_, launched) = assistant(&state, &account, command).await;
    assert_eq!(launched["requests"][0]["phase"], "launched");
    assert_eq!(
        launched["requests"][0]["thread_link"],
        "t3-thread://v2/fixture/thread"
    );
    assert_eq!(
        fs::read_to_string(exchange.join("starts")).unwrap(),
        "start\n"
    );
    let mut interrupted = inbox.record(id).unwrap();
    interrupted.launch = Launch::Launching;
    inbox.save(&interrupted).unwrap();
    assistant(&state, &account, json!({"action":"status"})).await;
    tokio::time::timeout(Duration::from_secs(3), wait())
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(exchange.join("starts")).unwrap(),
        "start\n"
    );
}

#[tokio::test]
async fn assistant_inbox_is_scoped_to_its_account_and_rejects_unknown_authority_fields() {
    let (state, account) = fixture();
    let (summary, _) = summary(&state, &account).await;
    let (_, prepared) = assistant(
        &state,
        &account,
        json!({"action":"prepare","summary":summary,"confirmed":true}),
    )
    .await;
    let repo = state.accounts.repository().unwrap();
    let other = repo
        .create(
            Provider::MailboxOrg,
            "other@example.test".into(),
            &SecretValue::new(b"other-invented-password".to_vec()),
        )
        .unwrap();
    let (_, status) = assistant(&state, &other, json!({"action":"status"})).await;
    assert_eq!(status["requests"], json!([]));
    let (status, _) = assistant(&state, &other, json!({"action":"dismiss","request_id":prepared["requests"][0]["id"],"proposal_id":"0".repeat(64)})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = assistant(&state, &account, json!({"action":"status","deploy":true})).await;
    assert!(!status.is_success());
}
