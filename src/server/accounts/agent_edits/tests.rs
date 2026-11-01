use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const AS_OF: i64 = 1792886400;

fn fixture() -> (Arc<AppState>, Account) {
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
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let sample: Value =
        serde_json::from_str(include_str!("../../../../examples/sample.json")).unwrap();
    saved.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: serde_json::from_value(sample["documents"].clone()).unwrap(),
            pins: vec![],
            as_of: AS_OF,
            baseline: crate::rules::proof::Baseline::Current,
        },
    });
    store.save_state(&mut saved).unwrap();
    (Arc::new(state), account)
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Reviews a summary the way the report panel does and returns the request
/// fragment that binds a proposal to it, plus the summary text.
async fn reviewed_summary(state: &Arc<AppState>, account: &Account) -> (Value, String) {
    let (status, report) = post(state, "report", json!({"account":account.id,"as_of":AS_OF})).await;
    assert_eq!(status, StatusCode::OK);
    let approved: Vec<_> = report["clear_candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].clone())
        .collect();
    let (status, preview) = post(
        state,
        "summary/review",
        json!({"account":account.id,"as_of":AS_OF,"version":report["version"],"approved":approved}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (
        json!({"as_of":AS_OF,"version":report["version"],"approved":approved,"receipt":preview["receipt"]}),
        preview["summary"].as_str().unwrap().to_owned(),
    )
}

fn proposal(summary: &str, folder: &str) -> Value {
    json!({
        "version": 1,
        "summary_sha256": data_encoding::HEXLOWER.encode(&Sha256::digest(summary.as_bytes())),
        "recommendations": [{"title":"File travel","reason":"Invented <img src=x> reason.","suggested_change":"Move travel mail."}],
        "rule_edits": {"patches": [{"id":"topic.travel","folder":folder}]},
    })
}

fn request(account: &Account, summary: &Value, proposal: &Value, command: Value) -> Value {
    json!({"account":account.id,"summary":summary,"proposal":proposal,"command":command})
}

fn confirm(disposition: &str, receipt: &Value, confirmed: bool) -> Value {
    json!({"action":"confirm","disposition":disposition,"receipt":receipt,"confirmed":confirmed})
}

#[tokio::test]
async fn agent_edits_need_an_exact_fresh_receipt_and_explicit_confirmation() {
    let (state, account) = fixture();
    let (summary, text) = reviewed_summary(&state, &account).await;
    let trips = proposal(&text, "Trips");
    let (status, review) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["edits"][0]["target"], "topic.travel");
    assert_eq!(review["account"], account.id.as_str());
    assert!(
        review["resulting_documents"]["override_yaml"]
            .as_str()
            .unwrap()
            .contains("Trips")
    );
    assert!(
        !review["original_documents"]["override_yaml"]
            .as_str()
            .unwrap()
            .contains("Trips")
    );
    assert!(review["current"]["proof"].is_object() && review["resulting"]["proof"].is_object());
    let accept = review["accept_receipt"].clone();
    let refusals = [
        request(
            &account,
            &summary,
            &trips,
            confirm("accept", &accept, false),
        ),
        request(
            &account,
            &summary,
            &trips,
            confirm("accept", &json!("0".repeat(64)), true),
        ),
        request(
            &account,
            &summary,
            &trips,
            confirm("accept", &review["reject_receipt"], true),
        ),
        request(&account, &summary, &trips, confirm("reject", &accept, true)),
        request(
            &account,
            &summary,
            &proposal(&text, "Elsewhere"),
            confirm("accept", &accept, true),
        ),
        request(
            &account,
            &json!({"as_of":AS_OF,"version":summary["version"],"approved":summary["approved"],"receipt":"0".repeat(64)}),
            &trips,
            confirm("accept", &accept, true),
        ),
        request(
            &account,
            &json!({"as_of":AS_OF,"version":"changed-report-version","approved":summary["approved"],"receipt":summary["receipt"]}),
            &trips,
            confirm("accept", &accept, true),
        ),
    ];
    for body in refusals {
        let (status, _) = post(&state, "agent-edits", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut unknown = request(&account, &summary, &trips, confirm("accept", &accept, true));
    unknown["deploy"] = true.into();
    assert!(!post(&state, "agent-edits", unknown).await.0.is_success());
    let mut smuggled = trips.clone();
    smuggled["rule_edits"]["forwards"] = json!([]);
    assert!(
        !post(
            &state,
            "agent-edits",
            request(
                &account,
                &summary,
                &smuggled,
                confirm("accept", &accept, true)
            )
        )
        .await
        .0
        .is_success()
    );
    let (_, opened) = post(&state, "workspace/open", json!({"account":account.id})).await;
    assert_eq!(
        opened["workspace"]["revision"].as_u64().unwrap() + 1,
        review["workspace_revision"].as_u64().unwrap(),
        "refusals saved nothing"
    );

    let (status, saved) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, confirm("accept", &accept, true)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["status"], "local_rules_saved");
    assert_eq!(
        saved["workspace"]["workspace"]["revision"],
        review["workspace_revision"]
    );
    assert_eq!(
        saved["workspace"]["workspace"]["input"]["documents"]["override_yaml"],
        review["resulting_documents"]["override_yaml"]
    );
    assert!(
        saved["workspace"]["revision"].as_u64().unwrap() > review["revision"].as_u64().unwrap()
    );
    let (status, _) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, confirm("accept", &accept, true)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "replay");
    println!(
        "MAILROOM_AGENT_RULE_EDITS_FIXTURE {}",
        json!({"account":account.id,"binding":summary,"summary":text,"proposal":trips,"review":review,"saved":saved})
    );
    let (status, proof) = post(&state, "proof/preview", json!({"account":account.id})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(proof["workspace_revision"], review["workspace_revision"]);
}

#[tokio::test]
async fn rejected_agent_edits_stay_rejected_and_owner_edits_make_reviews_stale() {
    let (state, account) = fixture();
    let (summary, text) = reviewed_summary(&state, &account).await;
    let trips = proposal(&text, "Trips");
    let (_, review) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    let (_, opened) = post(&state, "workspace/open", json!({"account":account.id})).await;
    let mut input = opened["workspace"]["input"].clone();
    input["documents"]["override_yaml"] = input["documents"]["override_yaml"]
        .as_str()
        .unwrap()
        .replace("folder: Releases", "folder: Builds")
        .into();
    let (status, _) = post(
        &state,
        "workspace/save",
        json!({"account":account.id,"revision":opened["revision"],"input":input}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post(
        &state,
        "agent-edits",
        request(
            &account,
            &summary,
            &trips,
            confirm("accept", &review["accept_receipt"], true),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "owner edit made the review stale"
    );

    let (_, review) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    let (status, rejected) = post(
        &state,
        "agent-edits",
        request(
            &account,
            &summary,
            &trips,
            confirm("reject", &review["reject_receipt"], true),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["status"], "edits_rejected");
    assert!(
        !rejected["workspace"]["workspace"]["input"]["documents"]["override_yaml"]
            .as_str()
            .unwrap()
            .contains("Trips")
    );
    let (status, again) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        again["error"],
        "Agent proposal repeats a rejected rule edit"
    );
    let (status, _) = post(
        &state,
        "agent-edits",
        request(
            &account,
            &summary,
            &proposal(&text, "Journeys"),
            json!({"action":"preview"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a changed edit is reviewable");
}

#[tokio::test]
async fn a_full_rejection_history_refuses_rejections_and_keeps_refusing_rejected_edits() {
    use crate::rules::agent_edits::{REJECTION_LIMIT, Rejection};
    let (state, account) = fixture();
    let store = state
        .accounts
        .repository()
        .unwrap()
        .store(&account)
        .unwrap();
    let mut saved = store.load_state().unwrap();
    saved.proposals.agent_rejections = (0..REJECTION_LIMIT - 1)
        .map(|i| Rejection {
            edit: format!("{i:064x}"),
            target: "topic.money".into(),
            summary: "0".repeat(64),
            as_of: AS_OF,
        })
        .collect();
    store.save_state(&mut saved).unwrap();
    drop(store);
    let (summary, text) = reviewed_summary(&state, &account).await;
    let trips = proposal(&text, "Trips");
    let (_, review) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    assert_eq!(
        review["rejection_history"],
        json!({"recorded":511,"limit":512})
    );
    let (status, rejected) = post(
        &state,
        "agent-edits",
        request(
            &account,
            &summary,
            &trips,
            confirm("reject", &review["reject_receipt"], true),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");

    let (status, again) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &trips, json!({"action":"preview"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        again["error"],
        "Agent proposal repeats a rejected rule edit"
    );

    let journeys = proposal(&text, "Journeys");
    let (status, full) = post(
        &state,
        "agent-edits",
        request(&account, &summary, &journeys, json!({"action":"preview"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{full}");
    assert_eq!(
        full["rejection_history"],
        json!({"recorded":512,"limit":512})
    );
    assert!(full["reject_receipt"].is_null());
    assert!(full["accept_receipt"].is_string());
    let (status, refused) = post(
        &state,
        "agent-edits",
        request(
            &account,
            &summary,
            &journeys,
            confirm("reject", &full["accept_receipt"], true),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused["error"],
        "The rejection history is full. It holds 512 of 512 rejected edits and has no room for 1 more. Rejections are never removed, so rejected edits are never proposed again. Nothing was changed"
    );
    let store = state
        .accounts
        .repository()
        .unwrap()
        .store(&account)
        .unwrap();
    let kept = store.load_state().unwrap().proposals.agent_rejections;
    assert_eq!(kept.len(), REJECTION_LIMIT);
    assert_eq!(kept[0].edit, format!("{:064x}", 0));
    println!(
        "MAILROOM_AGENT_RULE_EDITS_FULL_FIXTURE {}",
        json!({"account":account.id,"binding":summary,"proposal":journeys,"review":full})
    );
}
