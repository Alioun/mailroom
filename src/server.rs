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
        Self::with_accounts(accounts::AccountController::new(synthetic_accounts))
    }
    fn with_accounts(accounts: accounts::AccountController) -> Self {
        let mut random = Zeroizing::new([0; 32]);
        OsRng.fill_bytes(random.as_mut());
        Self {
            accounts,
            token: Zeroizing::new(data_encoding::HEXLOWER.encode(random.as_ref())),
            demo: Mutex::new(DemoCensus::new().expect("synthetic sandbox must be writable")),
        }
    }
}

/// While the vault is locked, every account route but status answers 423 before its handler.
async fn vault_gate(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path.starts_with("/api/accounts/")
        && path != "/api/accounts/status"
        && state.accounts.vault_locked()
    {
        return (
            StatusCode::LOCKED,
            Json(json!({
                "error": crate::acquisition::AcquisitionError::VaultLocked.to_string(),
                "code": "vault_locked",
            })),
        )
            .into_response();
    }
    next.run(request).await
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

async fn operation(
    State(state): State<Arc<AppState>>,
    Json(input): Json<OperationRequest>,
) -> Response {
    let availability = policy::authorize_with_mode(input.operation, state.accounts.live_writes());
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
    router(Arc::new(AppState::new(synthetic_accounts)))
}

fn router(state: Arc<AppState>) -> Router {
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
            get({
                let state = state.clone();
                move || async move {
                    let live = state.accounts.live_writes();
                    Json(policy::OPERATIONS.map(|op| policy::authorize_with_mode(op, live)))
                }
            }),
        )
        .route("/api/accounts/backlog", post(accounts::backlog::handle))
        .route(
            "/api/accounts/backlog/gmail",
            post(accounts::backlog_protocol::handle_gmail),
        )
        .route(
            "/api/accounts/backlog/protocol",
            post(accounts::backlog_protocol::handle),
        )
        .route("/api/accounts/invoices", post(accounts::invoices::handle))
        .route("/api/accounts/ledger", post(accounts::ledger::handle))
        .route("/api/accounts/winddown", post(accounts::winddown::handle))
        .route(
            "/api/accounts/unsubscribe",
            post(accounts::unsubscribe::handle),
        )
        .route(
            "/api/accounts/subscription-review",
            post(accounts::subscription_review::handle),
        )
        .route("/api/accounts/status", get(accounts::status))
        .route("/api/accounts/list", post(accounts::list))
        .route("/api/accounts/lifecycle", post(accounts::lifecycle::handle))
        .route("/api/accounts/recovery", post(accounts::recovery::handle))
        .route("/api/accounts/connect", post(accounts::connect))
        .route("/api/accounts/reconnect", post(accounts::web::reconnect))
        .route(
            "/api/accounts/interrupted",
            post(accounts::web::finish_interrupted),
        )
        .route(
            "/api/vault/create",
            post(accounts::web::create).layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT)),
        )
        .route(
            "/api/vault/unlock",
            post(accounts::web::unlock).layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT)),
        )
        .route(
            "/api/vault/lock",
            post(accounts::web::lock).layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT)),
        )
        .route(
            "/api/vault/change-password",
            post(accounts::web::change_password).layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT)),
        )
        .route("/api/accounts/write-mode", post(accounts::web::write_mode))
        .route("/api/accounts/takeout", post(accounts::web::takeout))
        .route("/api/accounts/synthetic", post(accounts::synthetic))
        .route(
            "/api/accounts/synthetic-gmail",
            post(accounts::synthetic_gmail),
        )
        .route("/api/accounts/scan", post(accounts::scan))
        .route("/api/accounts/cancel", post(accounts::cancel))
        .route("/api/accounts/open", post(accounts::open))
        .route("/api/accounts/confirm", post(accounts::confirm))
        .route("/api/accounts/report", post(accounts::report))
        .route(
            "/api/accounts/workspace/open",
            post(accounts::workspace_open),
        )
        .route(
            "/api/accounts/workspace/save",
            post(accounts::workspace_save),
        )
        .route("/api/accounts/proposals", post(accounts::proposals::handle))
        .route("/api/accounts/assistant", post(accounts::assistant::handle))
        .route(
            "/api/accounts/agent-edits",
            post(accounts::agent_edits::handle),
        )
        .route(
            "/api/accounts/managesieve",
            post(accounts::managesieve::handle),
        )
        .route(
            "/api/accounts/folders",
            post(accounts::folder_provisioning::handle),
        )
        .route(
            "/api/accounts/sieve-transactions",
            post(accounts::sieve_transactions::handle),
        )
        .route("/api/accounts/proof/preview", post(accounts::proof_preview))
        .route(
            "/api/accounts/proof/download",
            post(accounts::proof_download),
        )
        .route(
            "/api/accounts/summary/review",
            post(accounts::review_summary),
        )
        .route(
            "/api/accounts/summary/export",
            post(accounts::export_summary),
        )
        .route("/api/preview", post(preview))
        .route("/api/operations", post(operation))
        .route("/api/census/demo", get(demo_status))
        .route("/api/census/demo/start", post(demo_start))
        .route("/api/census/demo/cancel", post(demo_cancel))
        .route("/api/census/demo/confirm", post(demo_confirm))
        // A setup build serves the UI copy the wizard published next to it.
        .fallback_service(ServeDir::new(
            option_env!("MAILROOM_UI_DIR")
                .unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/dist")),
        ))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), vault_gate))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state)
}

const VAULT_BODY_LIMIT: usize = 16 * 1024;

pub async fn run() -> std::io::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let synthetic = arguments.iter().any(|arg| arg == "--synthetic-accounts");
    let local_vault = arguments.iter().any(|arg| arg == "--local-vault");
    let state = if local_vault {
        if synthetic {
            println!("status=conflicting_modes");
            return Err(std::io::Error::other("conflicting modes"));
        }
        // The vault takes its lease before the port is bound, so a second server stops here.
        match accounts::AccountController::local_vault() {
            Ok(accounts) => AppState::with_accounts(accounts),
            Err(error) => {
                println!("status=vault_{}", error.code());
                return Err(std::io::Error::other("vault unavailable"));
            }
        }
    } else {
        AppState::new(synthetic)
    };
    let listener = tokio::net::TcpListener::bind(ADDRESS).await?;
    println!("mailroom local interface: {ORIGIN}");
    axum::serve(listener, router(Arc::new(state))).await
}
