use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn command(
    state: &Arc<AppState>,
    account: &str,
    value: serde_json::Value,
    expected: StatusCode,
) -> serde_json::Value {
    let response = super::super::super::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/accounts/backlog")
                .header("Host", crate::server::ADDRESS)
                .header("Origin", crate::server::ORIGIN)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", state.token.as_str())
                .body(Body::from(
                    json!({"account":account,"command":value}).to_string(),
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
async fn separate_archive_http_preview_confirmation_execution_and_undo() {
    for provider in [Provider::MailboxOrg, Provider::Gmail] {
        let state = Arc::new(AppState::new(true));
        let repo = state.accounts.repository().unwrap();
        let account = repo
            .create(
                provider,
                "archive-owner@example.test".into(),
                &SecretValue::new(b"invented".to_vec()),
            )
            .unwrap();
        {
            let store = repo.store(&account).unwrap();
            let mut saved = store.load_state().unwrap();
            let mut input: crate::engine::PreviewInput =
                serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
            input.context.account = account.id.as_str().into();
            input.messages.truncate(1);
            input.messages[0].current_folder = "INBOX".into();
            input.messages[0].headers = "From: archive-person@unmatched.example.test\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: Personal note\r\nMessage-ID: <archive-http@example.test>\r\n\r\n".into();
            let mut census = crate::engine::synthetic_census(&input).unwrap();
            census.folder_roles.insert(
                if provider == Provider::Gmail {
                    "[Gmail]/All Mail"
                } else {
                    "Archiv"
                }
                .into(),
                [if provider == Provider::Gmail {
                    "all"
                } else {
                    "archive"
                }
                .into()]
                .into(),
            );
            for message in census
                .folders
                .get_mut("INBOX")
                .unwrap()
                .messages
                .values_mut()
            {
                message.internal_date = "01-May-2024 00:00:00 +0000".into();
                message.flags = [r"\Answered".into()].into();
                message.gmail_message_id = Some("1001".into());
                message.gmail_labels = [r"\Inbox".into(), "Other".into()].into();
            }
            saved.census = census.into();
            saved.workspace = Some(crate::rules::workspace::Workspace {
                revision: 1,
                input: crate::rules::workspace::WorkspaceInput {
                    documents: input.documents,
                    pins: vec![],
                    as_of: input.as_of,
                    baseline: crate::rules::proof::Baseline::Current,
                },
            });
            store.save_state(&mut saved).unwrap();
        }
        let id = account.id.as_str();
        let sort = json!({"scope":"inbox","protect_flagged":true,"selected":null});
        let archive = json!({"scope":"inbox","protect_flagged":true,"selected":null,"intent":"archive_unmatched"});
        let ordinary = command(
            &state,
            id,
            json!({"action":"preview","options":sort}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(ordinary["plan"]["changes"].as_array().unwrap().len(), 0);
        let p = command(
            &state,
            id,
            json!({"action":"preview","options":archive}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(p["plan"]["changes"].as_array().unwrap().len(), 1);
        assert_ne!(p["receipt"], ordinary["receipt"]);
        for (receipt, confirmed) in [(&ordinary["receipt"], true), (&p["receipt"], false)] {
            command(
                &state,
                id,
                json!({"action":"start","options":archive,"receipt":receipt,"confirmed":confirmed}),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
        command(
            &state,
            id,
            json!({"action":"start","options":archive,"receipt":p["receipt"],"confirmed":true}),
            StatusCode::OK,
        )
        .await;
        let done = command(
            &state,
            id,
            json!({"action":"advance","run":p["receipt"]}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(done["runs"][0]["status"], "completed");
        assert_eq!(
            done["runs"][0]["plan"]["options"]["intent"],
            "archive_unmatched"
        );
        let undo = command(
            &state,
            id,
            json!({"action":"undo_preview","run":p["receipt"]}),
            StatusCode::OK,
        )
        .await;
        let undone = command(
            &state,
            id,
            json!({"action":"undo","run":p["receipt"],"receipt":undo["receipt"],"confirmed":true}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(undone["runs"][0]["undo"][0]["state"], "restored");
        println!(
            "MAILROOM_ARCHIVE_FIXTURE {}",
            json!({"provider":provider,"ordinary":ordinary,"preview":p,"completed":done,"undone":undone})
        );
    }
}
