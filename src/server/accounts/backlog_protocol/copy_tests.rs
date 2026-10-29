use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(
    state: &Arc<AppState>,
    account: &AccountId,
    command: serde_json::Value,
    token: bool,
    expected: StatusCode,
) -> serde_json::Value {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/accounts/backlog/protocol")
        .header("Host", crate::server::ADDRESS)
        .header("Origin", crate::server::ORIGIN)
        .header("Content-Type", "application/json");
    if token {
        request = request.header("X-Mailroom-Token", state.token.as_str());
    }
    let response = crate::server::router(state.clone())
        .oneshot(
            request
                .body(Body::from(
                    json!({"account": account, "command": command}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()["cache-control"], "no-store");
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn protected_copy_history_status_preserves_phases_and_denies_uninstalled_actions() {
    use crate::backlog::protocol::tests::{Stage, run};
    let state = Arc::new(AppState::new(true));
    let repo = state.accounts.repository().unwrap();
    let account = repo
        .create(
            Provider::MailboxOrg,
            "copy-router@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    let mut fixtures = vec![];
    for stage in [
        Stage::Prepared,
        Stage::CopyUncertain,
        Stage::Copied,
        Stage::StoreUncertain,
        Stage::Marked,
        Stage::ExpungeUncertain,
        Stage::Filed,
        Stage::Undone,
    ] {
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.protocol_backlog.runs = vec![run(&account.id, stage)];
        store.save_state(&mut saved).unwrap();
        let ciphertext = std::fs::read(store.path()).unwrap();
        let id = saved.protocol_backlog.runs[0].id.clone();
        assert_eq!(
            repo.backlog_run(
                &account,
                &store,
                &mut saved,
                &id,
                false,
                Cancellation::default(),
                |_| Ok(())
            ),
            Err(AcquisitionError::InvalidInput)
        );
        assert_eq!(
            repo.backlog_reconcile(&account, &store, &mut saved, &id, Cancellation::default()),
            Err(AcquisitionError::InvalidInput)
        );
        assert!(matches!(
            repo.backlog_undo_preview(&account, &store, &saved, &id, Cancellation::default()),
            Err(AcquisitionError::InvalidInput)
        ));
        drop(store);
        let status = call(
            &state,
            &account.id,
            json!({"action": "status"}),
            true,
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["journal"], json!(saved.protocol_backlog));
        assert_eq!(status["installed"], false);
        for action in [
            "reconcile",
            "resume",
            "undo_preview",
            "resume_undo",
            "close_preview",
        ] {
            call(
                &state,
                &account.id,
                json!({"action": action, "run": "a".repeat(64)}),
                true,
                StatusCode::FORBIDDEN,
            )
            .await;
        }
        call(
            &state,
            &account.id,
            json!({"action": "status"}),
            false,
            StatusCode::FORBIDDEN,
        )
        .await;
        let store = repo.store(&account).unwrap();
        assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
        fixtures.push(json!({"stage": format!("{stage:?}"), "view": status}));
    }
    println!(
        "MAILROOM_COPY_ROUTER_FIXTURE {}",
        json!({"account": account.id, "fixtures": fixtures})
    );
}
