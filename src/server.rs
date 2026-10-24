mod accounts;
use crate::{
    census::{
        CensusError,
        demo::{DemoCensus, DemoMode},
    },
    engine::{self, PreviewInput},
    policy::{self, Operation},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::{RngCore, rngs::OsRng};
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};
use subtle::ConstantTimeEq;
use tower_http::services::ServeDir;
use zeroize::Zeroizing;

pub const ADDRESS: &str = "127.0.0.1:4173";
pub const ORIGIN: &str = "http://127.0.0.1:4173";
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";

struct AppState {
    token: Zeroizing<String>,
    demo: Mutex<DemoCensus>,
    accounts: accounts::AccountController,
}

impl AppState {
    fn new(synthetic_accounts: bool) -> Self {
        let mut random = Zeroizing::new([0; 32]);
        OsRng.fill_bytes(random.as_mut());
        Self {
            accounts: accounts::AccountController::new(synthetic_accounts),
            token: Zeroizing::new(data_encoding::HEXLOWER.encode(random.as_ref())),
            demo: Mutex::new(DemoCensus::new().expect("synthetic sandbox must be writable")),
        }
    }
}

async fn boundary(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host_ok = headers.get_all(header::HOST).iter().count() == 1
        && headers.get(header::HOST).is_some_and(|v| v == ADDRESS);
    let origin_ok = headers.get_all(header::ORIGIN).iter().count() <= 1
        && headers.get(header::ORIGIN).is_none_or(|v| v == ORIGIN);
    let fetch_ok = headers
        .get("sec-fetch-site")
        .is_none_or(|v| v == "same-origin" || v == "none");
    let method = request.method();
    let bootstrap = request.uri().path() == "/api/session" && method == axum::http::Method::GET;
    let token_ok = !request.uri().path().starts_with("/api/")
        || bootstrap
        || (headers.get_all("x-mailroom-token").iter().count() == 1
            && headers
                .get("x-mailroom-token")
                .is_some_and(|value| bool::from(value.as_bytes().ct_eq(state.token.as_bytes()))));
    let post_ok = method != axum::http::Method::POST
        || (headers.get_all(header::CONTENT_TYPE).iter().count() == 1
            && headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.eq_ignore_ascii_case("application/json")
                        || v.eq_ignore_ascii_case("application/json; charset=utf-8")
                }));
    let mut response = if !host_ok || !origin_ok || !fetch_ok || !token_ok {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Local session authorization rejected"})),
        )
            .into_response()
    } else if !post_ok {
        (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({"error":"Use application/json"})),
        )
            .into_response()
    } else {
        next.run(request).await
    };
    for (name, value) in [
        ("content-security-policy", CSP),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        ("cross-origin-resource-policy", "same-origin"),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}

async fn preview(Json(input): Json<PreviewInput>) -> Response {
    let authorization = policy::authorize(Operation::Preview);
    if !authorization.allowed {
        return (StatusCode::FORBIDDEN, Json(authorization)).into_response();
    }
    match engine::preview(&input) {
        Ok(result) => Json(result).into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":error})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    operation: Operation,
}

async fn operation(Json(input): Json<OperationRequest>) -> Response {
    let availability = policy::authorize(input.operation);
    if !availability.allowed || matches!(availability.effect, policy::Effect::AccountRead) {
        return (StatusCode::FORBIDDEN, Json(availability)).into_response();
    }
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"Use the dedicated synthetic preview or census route"})),
    )
        .into_response()
}

fn census_error(error: CensusError) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"error": error.to_string()})),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DemoStart {
    mode: DemoMode,
}

async fn demo_start(State(state): State<Arc<AppState>>, Json(input): Json<DemoStart>) -> Response {
    let authorization = policy::authorize(Operation::SyntheticCensus);
    if !authorization.allowed {
        return (StatusCode::FORBIDDEN, Json(authorization)).into_response();
    }
    let generation = match state.demo.lock().expect("demo lock").start(input.mode) {
        Ok(generation) => generation,
        Err(error) => return census_error(error),
    };
    let worker_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(450)).await;
            let mut demo = worker_state.demo.lock().expect("demo lock");
            match demo.advance(generation) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => {
                    demo.fail();
                    break;
                }
            }
        }
    });
    Json(state.demo.lock().expect("demo lock").status()).into_response()
}

async fn demo_status(State(state): State<Arc<AppState>>) -> Response {
    Json(state.demo.lock().expect("demo lock").status()).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

async fn demo_cancel(
    State(state): State<Arc<AppState>>,
    Json(_input): Json<EmptyRequest>,
) -> Response {
    let mut demo = state.demo.lock().expect("demo lock");
    match demo.cancel() {
        Ok(()) => Json(demo.status()).into_response(),
        Err(error) => census_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmAddress {
    address: String,
    confirmed: bool,
}

async fn demo_confirm(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfirmAddress>,
) -> Response {
    let mut demo = state.demo.lock().expect("demo lock");
    match demo.confirm(&input.address, input.confirmed) {
        Ok(()) => Json(demo.status()).into_response(),
        Err(error) => census_error(error),
    }
}

pub fn app() -> Router {
    build_app(false)
}

pub fn synthetic_account_app() -> Router {
    build_app(true)
}

fn build_app(synthetic_accounts: bool) -> Router {
    let state = Arc::new(AppState::new(synthetic_accounts));
    Router::new()
        .route(
            "/api/session",
            get({
                let state = state.clone();
                move || async move { Json(json!({"token": state.token.as_str()})) }
            }),
        )
        .route(
            "/api/sample",
            get(|| async {
                Json(
                    serde_json::from_str::<serde_json::Value>(include_str!(
                        "../examples/sample.json"
                    ))
                    .expect("valid bundled fixture"),
                )
            }),
        )
        .route(
            "/api/availability",
            get(|| async { Json(policy::OPERATIONS.map(policy::authorize)) }),
        )
        .route("/api/accounts/status", get(accounts::status))
        .route("/api/accounts/list", post(accounts::list))
        .route("/api/accounts/connect", post(accounts::connect))
        .route("/api/accounts/synthetic", post(accounts::synthetic))
        .route("/api/accounts/scan", post(accounts::scan))
        .route("/api/accounts/cancel", post(accounts::cancel))
        .route("/api/accounts/open", post(accounts::open))
        .route("/api/accounts/confirm", post(accounts::confirm))
        .route("/api/preview", post(preview))
        .route("/api/operations", post(operation))
        .route("/api/census/demo", get(demo_status))
        .route("/api/census/demo/start", post(demo_start))
        .route("/api/census/demo/cancel", post(demo_cancel))
        .route("/api/census/demo/confirm", post(demo_confirm))
        .fallback_service(ServeDir::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/ui/dist"
        )))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state)
}

pub async fn run() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDRESS).await?;
    let synthetic = std::env::args()
        .skip(1)
        .any(|arg| arg == "--synthetic-accounts");
    println!("mailroom local interface: {ORIGIN}");
    axum::serve(listener, build_app(synthetic)).await
}
