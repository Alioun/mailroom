use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use mailroom::{
    policy::{self, Effect, Operation},
    server,
};
use tower::ServiceExt;

async fn request(
    path: &str,
    body: &str,
    origin: Option<&str>,
    host: &str,
    content_type: &str,
) -> axum::response::Response {
    let app = server::app();
    let session = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("host", server::ADDRESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let session: serde_json::Value =
        serde_json::from_slice(&session.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("x-mailroom-token", session["token"].as_str().unwrap())
        .header("content-type", content_type);
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    app.oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn all_account_mutations_are_denied_through_http() {
    for operation in policy::OPERATIONS {
        let authorization = policy::authorize(operation);
        if matches!(authorization.effect, Effect::SyntheticRead) {
            continue;
        }
        if matches!(authorization.effect, Effect::AccountWrite) {
            assert!(!authorization.allowed);
        }
        let body = serde_json::json!({"operation":operation}).to_string();
        let response = request(
            "/api/operations",
            &body,
            Some(server::ORIGIN),
            server::ADDRESS,
            "application/json",
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{operation:?}");
    }
}

#[tokio::test]
async fn caller_cannot_supply_write_authority_or_source() {
    for body in [
        r#"{"operation":"send_message","synthetic":true}"#,
        r#"{"operation":"upload_rules","source":"http://127.0.0.1","allowed":true}"#,
        r#"{"operation":"unknown"}"#,
    ] {
        let response = request(
            "/api/operations",
            body,
            None,
            server::ADDRESS,
            "application/json",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(!policy::authorize(Operation::SendMessage).allowed);
}

#[tokio::test]
async fn boundaries_reject_foreign_hosts_origins_content_types_and_large_bodies() {
    for (origin, host, content_type, expected) in [
        (
            Some("https://evil.example"),
            server::ADDRESS,
            "application/json",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("null"),
            server::ADDRESS,
            "application/json",
            StatusCode::FORBIDDEN,
        ),
        (
            None,
            "evil.example:4173",
            "application/json",
            StatusCode::FORBIDDEN,
        ),
        (
            None,
            server::ADDRESS,
            "text/plain",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ] {
        assert_eq!(
            request("/api/preview", "{}", origin, host, content_type)
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        request(
            "/api/preview",
            &"x".repeat(262_145),
            None,
            server::ADDRESS,
            "application/json"
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn actual_preview_has_counts_csp_and_no_cors() {
    let response = request(
        "/api/preview",
        include_str!("../examples/sample.json"),
        Some(server::ORIGIN),
        server::ADDRESS,
        "application/json",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-security-policy"], server::CSP);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["counts"]["filed"], 10);
    assert_eq!(value["counts"]["inbox"], 4);
    assert_eq!(value["counts"]["copies"], 3);
}

#[tokio::test]
async fn api_requires_a_token_from_this_launch_and_rejects_duplicate_tokens() {
    let app = server::app();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("host", server::ADDRESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let session: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = session["token"].as_str().unwrap();
    for path in ["/api/sample", "/api/availability", "/api/census/demo"] {
        for supplied in [None, Some("invalid")] {
            let mut request = Request::builder().uri(path).header("host", server::ADDRESS);
            if let Some(supplied) = supplied {
                request = request.header("x-mailroom-token", supplied);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", server::ADDRESS)
                    .header("x-mailroom-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let other_launch = server::app();
    let response = other_launch
        .oneshot(
            Request::builder()
                .uri("/api/census/demo")
                .header("host", server::ADDRESS)
                .header("x-mailroom-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/census/demo")
                .header("host", server::ADDRESS)
                .header("x-mailroom-token", token)
                .header("x-mailroom-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = server::app()
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("host", server::ADDRESS)
                .header("origin", "https://foreign.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn census_http_cannot_accept_account_source_or_authority_injection() {
    for input in [
        r#"{"mode":"normal","account":"real-account"}"#,
        r#"{"mode":"normal","source":"https://example.test"}"#,
        r#"{"mode":"normal","allowed":true}"#,
        r#"{"mode":"normal","password":"synthetic"}"#,
    ] {
        assert_eq!(
            request(
                "/api/census/demo/start",
                input,
                None,
                server::ADDRESS,
                "application/json"
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

#[tokio::test]
async fn synthetic_account_flow_uses_fake_credentials_and_preserves_http_authority_boundaries() {
    let app = server::synthetic_account_app();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("host", server::ADDRESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let session: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = session["token"].as_str().unwrap();
    async fn post(
        app: &axum::Router,
        token: &str,
        action: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/accounts/{action}"))
                    .header("host", server::ADDRESS)
                    .header("x-mailroom-token", token)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        (status, value)
    }
    async fn status(app: &axum::Router, token: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/accounts/status")
                    .header("host", server::ADDRESS)
                    .header("x-mailroom-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
    async fn terminal(app: &axum::Router, token: &str) -> serde_json::Value {
        let started = std::time::Instant::now();
        let mut saved = None;
        let mut folders = None;
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let result = status(app, token).await;
                let progress = &result["job"]["progress"];
                match progress["state"].as_str() {
                    Some("complete" | "cancelled" | "failed") => return result,
                    Some("running") => {
                        saved = progress["saved"].as_u64();
                        folders = progress["folders"].as_u64();
                    }
                    _ => panic!("Synthetic account has a missing or invalid progress state"),
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Synthetic account did not finish after {:?}; last running progress: saved={saved:?}, folders={folders:?}",
                started.elapsed()
            )
        })
    }
    for action in [
        "connect",
        "scan",
        "open",
        "confirm",
        "cancel",
        "list",
        "synthetic",
        "synthetic-gmail",
        "report",
        "proof",
        "workspace/open",
        "workspace/save",
        "proof/preview",
        "proof/download",
        "summary/review",
        "summary/export",
    ] {
        let (code, value) = post(&app, token, action, serde_json::json!({"host":"fake.invalid", "password": {"synthetic-sensitive-value": true}, "allowed":true})).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!value.to_string().contains("synthetic-sensitive-value"));
    }
    assert_eq!(
        post(&app, token, "synthetic", serde_json::json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let connected = terminal(&app, token).await;
    assert_eq!(connected["job"]["progress"]["state"], "complete");
    let id = connected["job"]["account"]["id"].as_str().unwrap();
    let account = serde_json::json!({"account":id});
    post(&app, token, "scan", account.clone()).await;
    assert_eq!(
        post(&app, token, "scan", account.clone()).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    post(&app, token, "cancel", serde_json::json!({})).await;
    assert_eq!(
        terminal(&app, token).await["job"]["progress"]["state"],
        "cancelled"
    );
    post(&app, token, "scan", account.clone()).await;
    let scanned = terminal(&app, token).await;
    assert_eq!(scanned["job"]["progress"]["state"], "complete");
    assert_eq!(scanned["job"]["progress"]["saved"], 100);
    let (code, view) = post(&app, token, "open", account.clone()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(view["addresses"][0]["confirmed"], false);
    let (_, confirmed) = post(
        &app,
        token,
        "confirm",
        serde_json::json!({"account":id,"address":"owner@example.test","confirmed":true}),
    )
    .await;
    assert!(
        confirmed["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["address"] == "owner@example.test" && a["confirmed"] == true)
    );
    let as_of = 1792886400_i64;
    let (_, report) = post(
        &app,
        token,
        "report",
        serde_json::json!({"account":id,"as_of":as_of}),
    )
    .await;
    assert_eq!(report["complete"], true);
    assert_eq!(report["received"]["messages"], 99);
    assert_eq!(report["sent"], 1);
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../examples/sample.json")).unwrap();
    let proof_request = serde_json::json!({"account":id,"documents":sample["documents"],"as_of":as_of,"pins":[],"baseline":"current","previous":null});
    let (code, proof) = post(&app, token, "proof", proof_request.clone()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(proof["counts"]["messages"], 99);
    assert_eq!(proof["counts"]["review"], 99);
    assert_eq!(proof["counts"]["copies"], 0);
    assert_eq!(
        proof["artifact"]["context"]["recipient_evidence"],
        "unknown"
    );
    let mut injected = proof_request;
    injected["context"] = sample["context"].clone();
    assert_eq!(
        post(&app, token, "proof", injected).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let approved = serde_json::json!([]);
    let review = serde_json::json!({"account":id,"as_of":as_of,"version":report["version"],"approved":approved});
    let (code, preview) = post(&app, token, "summary/review", review.clone()).await;
    assert_eq!(code, StatusCode::OK);
    let summary = preview["summary"].as_str().unwrap();
    for private in [
        "shop.example",
        "owner@example.test",
        "Synthetic label",
        "synthetic-private-token",
        "Synthetic account header",
    ] {
        assert!(!summary.contains(private), "{private}");
    }
    let mut export = review.clone();
    export["receipt"] = preview["receipt"].clone();
    export["confirmed"] = true.into();
    let (code, shared) = post(&app, token, "summary/export", export.clone()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(shared["summary"], preview["summary"]);
    assert_eq!(shared["filename"], "census-summary.md");
    let mut invalid = export.clone();
    invalid["confirmed"] = false.into();
    assert_eq!(
        post(&app, token, "summary/export", invalid).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let candidate = report["clear_candidates"][0]["id"].as_str().unwrap();
    let mut altered = export.clone();
    altered["approved"] = serde_json::json!([candidate]);
    assert_eq!(
        post(&app, token, "summary/export", altered).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = review.clone();
    invalid["approved"] = serde_json::json!(["caller-injected-private-token"]);
    let (code, failure) = post(&app, token, "summary/review", invalid).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        !failure
            .to_string()
            .contains("caller-injected-private-token")
    );
    post(
        &app,
        token,
        "confirm",
        serde_json::json!({"account":id,"address":"owner@example.test","confirmed":false}),
    )
    .await;
    assert_eq!(
        post(&app, token, "summary/review", review).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post(&app, token, "summary/export", export.clone()).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    post(&app, token, "synthetic-gmail", serde_json::json!({})).await;
    let other = terminal(&app, token).await;
    let other_id = other["job"]["account"]["id"].as_str().unwrap();
    let (_, partial) = post(
        &app,
        token,
        "report",
        serde_json::json!({"account":other_id,"as_of":as_of}),
    )
    .await;
    assert_eq!(partial["complete"], false);
    export["account"] = other_id.into();
    assert_eq!(
        post(&app, token, "summary/export", export).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    post(&app, token, "scan", serde_json::json!({"account":other_id})).await;
    terminal(&app, token).await;
    let (_, gmail) = post(
        &app,
        token,
        "report",
        serde_json::json!({"account":other_id,"as_of":as_of}),
    )
    .await;
    assert_eq!(gmail["gmail_arrivals"]["2026-10"], 99);
    assert!(policy::authorize(Operation::Census).allowed);
    assert!(!policy::authorize(Operation::ChangeFlags).allowed);
}

#[tokio::test]
async fn backlog_routes_install_no_production_write_authority() {
    for command in [
        serde_json::json!({"action":"start", "options":{"scope":"inbox", "protect_flagged":true, "selected":null}, "receipt":"fake", "confirmed":true}),
        serde_json::json!({"action":"advance", "run":"fake"}),
        serde_json::json!({"action":"stop", "run":"fake"}),
        serde_json::json!({"action":"resume", "run":"fake"}),
        serde_json::json!({"action":"undo_preview", "run":"fake"}),
        serde_json::json!({"action":"undo", "run":"fake", "receipt":"fake", "confirmed":true}),
        serde_json::json!({"action":"resume_undo", "run":"fake"}),
    ] {
        let response = request(
            "/api/accounts/backlog",
            &serde_json::json!({"account":"nonexistent-fake-account", "command":command})
                .to_string(),
            None,
            server::ADDRESS,
            "application/json",
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for body in [
        serde_json::json!({"account":"fake", "command":{"action":"open"}, "synthetic":true}),
        serde_json::json!({"account":"fake", "command":{"action":"open", "synthetic":true}}),
        serde_json::json!({"account":"fake", "command":{"action":"preview", "options":{"scope":"inbox", "protect_flagged":true, "selected":null}, "context":{"allowed":true}}}),
    ] {
        assert_eq!(
            request(
                "/api/accounts/backlog",
                &body.to_string(),
                None,
                server::ADDRESS,
                "application/json"
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
