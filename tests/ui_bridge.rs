use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
};
use tower::ServiceExt;

#[derive(Deserialize)]
struct BrowserRequest {
    id: u64,
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

#[test]
#[ignore = "Private-pipe synthetic application adapter for browser verification"]
fn synthetic_ui_bridge() {
    assert!(mailroom::disable_core_dumps());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let app = mailroom::server::synthetic_account_app();
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let input: BrowserRequest = serde_json::from_str(&line.unwrap()).unwrap();
        assert!(input.path.starts_with('/') && !input.path.starts_with("//"));
        let mut request = Request::builder()
            .method(input.method.as_str())
            .uri(&input.path);
        for (name, value) in input.headers {
            request = request.header(name, value);
        }
        let response = runtime
            .block_on(
                app.clone()
                    .oneshot(request.body(Body::from(input.body)).unwrap()),
            )
            .unwrap();
        let status = response.status().as_u16();
        let headers: BTreeMap<_, _> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap().to_owned()))
            .collect();
        let bytes = runtime
            .block_on(response.into_body().collect())
            .unwrap()
            .to_bytes();
        let record = serde_json::json!({ "id": input.id, "status": status, "headers": headers, "body": data_encoding::BASE64.encode(&bytes) });
        writeln!(stdout, "MAILROOM_UI_RESPONSE {record}").unwrap();
        stdout.flush().unwrap();
    }
}
