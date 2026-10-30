use super::*;
use crate::confirmation::{Backend, SyntheticBackend};
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::Value;
use std::path::PathBuf;
use tower::ServiceExt;

const AS_OF: i64 = 1792886400;

#[derive(Default)]
struct Seen {
    offered: usize,
    shown: Vec<Value>,
}
struct Prompt {
    seen: Arc<Mutex<Seen>>,
    unavailable: bool,
    refuse: bool,
    replace: Option<(PathBuf, Vec<u8>)>,
}
impl Backend for Prompt {
    fn available(&self) -> Result<(), String> {
        self.seen.lock().unwrap().offered += 1;
        if self.unavailable {
            Err("Unverified test helper".into())
        } else {
            Ok(())
        }
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        self.seen
            .lock()
            .unwrap()
            .shown
            .push(serde_json::from_slice(request).unwrap());
        if self.refuse {
            return Err(crate::confirmation::CANCELLED.into());
        }
        if let Some((path, bytes)) = self.replace.take() {
            std::fs::write(path, bytes).unwrap();
        }
        SyntheticBackend.exchange(request, challenge)
    }
}

fn fixture() -> (AppState, Account) {
    let state = AppState::new(true);
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

async fn post(state: &Arc<AppState>, action: &str, body: Value) -> (StatusCode, Value) {
    let response = crate::server::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/accounts/{action}"))
                .header("Host", crate::server::ADDRESS)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", state.token.as_str())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap(),
    )
}

async fn reviewed(state: &Arc<AppState>, account: &Account) -> (Value, Value, Value) {
    let (status, report) = post(state, "report", json!({"account":account.id,"as_of":AS_OF})).await;
    assert_eq!(status, StatusCode::OK);
    let approved: Vec<_> = report["clear_candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].clone())
        .collect();
    let review =
        json!({"account":account.id,"as_of":AS_OF,"version":report["version"],"approved":approved});
    let (status, preview) = post(state, "summary/review", review.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let mut export = review;
    export["receipt"] = preview["receipt"].clone();
    export["confirmed"] = true.into();
    (report, preview, export)
}

fn install_prompt(
    state: &mut AppState,
    unavailable: bool,
    refuse: bool,
    replace: Option<(PathBuf, Vec<u8>)>,
) -> Arc<Mutex<Seen>> {
    let seen = Arc::new(Mutex::new(Seen::default()));
    state.accounts.confirmations = Mutex::new(crate::confirmation::Boundary::with_backend(
        Box::new(Prompt {
            seen: seen.clone(),
            unavailable,
            refuse,
            replace,
        }),
    ));
    seen
}

#[tokio::test]
async fn exact_summary_download_asks_once_and_preview_never_prompts() {
    let (mut state, account) = fixture();
    let seen = install_prompt(&mut state, false, false, None);
    let state = Arc::new(state);
    let (report, preview, export) = reviewed(&state, &account).await;
    assert_eq!(seen.lock().unwrap().offered, 0);
    let (status, downloaded) = post(&state, "summary/export", export.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded["summary"], preview["summary"]);
    assert_eq!(downloaded["filename"], "census-summary.md");
    let shown = &seen.lock().unwrap().shown;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["synthetic"], false);
    let action = &shown[0]["actions"][0];
    assert_eq!(action["purpose"], "agent_summary");
    assert_eq!(action["account"], account.id.as_str());
    assert_eq!(action["report_version"], report["version"]);
    assert_eq!(
        action["clear_domain_count"],
        export["approved"].as_array().unwrap().len()
    );
    assert_eq!(
        action["content_digest"],
        data_encoding::HEXLOWER.encode(&Sha256::digest(
            preview["summary"].as_str().unwrap().as_bytes()
        ))
    );
    assert!(
        shown[0]["display"]
            .as_str()
            .unwrap()
            .contains("Export this census summary")
    );
    assert!(!shown[0].to_string().contains("owner@example.test"));
    println!(
        "MAILROOM_SUMMARY_DISCLOSURE_FIXTURE {}",
        json!({"report":report,"preview":preview,"export":export,"downloaded":downloaded})
    );
}

#[tokio::test]
async fn invalid_or_cross_account_exports_never_offer_a_native_request() {
    let (mut state, account) = fixture();
    let repo = state.accounts.repository().unwrap();
    let other = repo
        .create(
            Provider::Gmail,
            "another@example.test".into(),
            &SecretValue::new(b"invented-password".to_vec()),
        )
        .unwrap();
    let seen = install_prompt(&mut state, false, false, None);
    let state = Arc::new(state);
    let (_, _, export) = reviewed(&state, &account).await;
    let mut altered = Vec::new();
    for (field, value) in [
        ("receipt", json!("0".repeat(64))),
        ("version", json!("0".repeat(64))),
        ("confirmed", json!(false)),
        ("account", json!(other.id)),
        ("filename", json!("private.txt")),
        ("summary", json!("DUMMY_UNREVIEWED_CONTENT")),
    ] {
        let mut changed = export.clone();
        changed[field] = value;
        altered.push(changed);
    }
    let mut changed = export.clone();
    changed["approved"] = json!([]);
    altered.push(changed);
    for input in altered {
        let (status, result) = post(&state, "summary/export", input).await;
        assert_ne!(status, StatusCode::OK);
        assert!(result.get("summary").is_none());
        assert!(!result.to_string().contains("DUMMY_UNREVIEWED_CONTENT"));
    }
    assert_eq!(seen.lock().unwrap().offered, 0);
    assert!(seen.lock().unwrap().shown.is_empty());
}

#[tokio::test]
async fn unavailable_or_refused_native_confirmation_returns_no_summary() {
    for unavailable in [true, false] {
        let (mut state, account) = fixture();
        let seen = install_prompt(&mut state, unavailable, !unavailable, None);
        let state = Arc::new(state);
        let (_, _, export) = reviewed(&state, &account).await;
        let (status, refused) = post(&state, "summary/export", export).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(refused.get("summary").is_none());
        assert_eq!(seen.lock().unwrap().offered, 1);
        assert_eq!(seen.lock().unwrap().shown.len(), usize::from(!unavailable));
    }
}

#[tokio::test]
async fn changed_encrypted_census_after_confirmation_refuses_the_download() {
    let (mut state, account) = fixture();
    let repo = state.accounts.repository().unwrap();
    let store = repo.store(&account).unwrap();
    let mut census = store.load().unwrap();
    let path = store.path();
    let original = std::fs::read(&path).unwrap();
    census
        .confirmed_own_addresses
        .insert("new-owner@example.test".into());
    store.save(&census).unwrap();
    let changed = std::fs::read(&path).unwrap();
    std::fs::write(&path, original).unwrap();
    drop(store);
    let seen = install_prompt(&mut state, false, false, Some((path, changed)));
    let state = Arc::new(state);
    let (_, _, export) = reviewed(&state, &account).await;
    let (status, refused) = post(&state, "summary/export", export).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.get("summary").is_none());
    assert_eq!(seen.lock().unwrap().shown.len(), 1);
    assert!(
        repo.store(&account)
            .unwrap()
            .load()
            .unwrap()
            .confirmed_own_addresses
            .contains("new-owner@example.test")
    );
}

#[test]
fn summary_consent_uses_a_distinct_account_bound_purpose() {
    let (state, account) = fixture();
    let repo = state.accounts.repository().unwrap();
    let store = repo.store(&account).unwrap();
    let report = store
        .load()
        .unwrap()
        .report(store.keys(), AS_OF, false)
        .unwrap();
    let approved = report
        .clear_candidates
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let preview = report.summary_preview(store.keys(), &approved).unwrap();
    let offered = plan(&account.id, store.keys(), &report, &preview, &approved).unwrap();
    assert_ne!(offered.exact_receipt, preview.receipt);
    let other = plan(
        &AccountId::new("another-account").unwrap(),
        store.keys(),
        &report,
        &preview,
        &approved,
    )
    .unwrap();
    assert_ne!(other.exact_receipt, offered.exact_receipt);
    assert_eq!(offered.actions.len(), 1);
}
