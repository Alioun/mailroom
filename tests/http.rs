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
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("content-type", content_type);
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    server::app()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
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
        assert!(!authorization.allowed);
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
    assert_eq!(value["counts"]["filed"], 4);
    assert_eq!(value["counts"]["inbox"], 2);
    assert_eq!(value["counts"]["copies"], 4);
}
