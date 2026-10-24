use crate::{
    engine::{self, PreviewInput},
    policy::{self, Operation},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tower_http::services::ServeDir;

pub const ADDRESS: &str = "127.0.0.1:4173";
pub const ORIGIN: &str = "http://127.0.0.1:4173";
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";

async fn boundary(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host_ok = headers.get_all(header::HOST).iter().count() == 1
        && headers.get(header::HOST).is_some_and(|v| v == ADDRESS);
    let origin_ok = headers.get_all(header::ORIGIN).iter().count() <= 1
        && headers.get(header::ORIGIN).is_none_or(|v| v == ORIGIN);
    let fetch_ok = headers
        .get("sec-fetch-site")
        .is_none_or(|v| v == "same-origin" || v == "none");
    let method = request.method();
    let post_ok = method != axum::http::Method::POST
        || (headers.get_all(header::CONTENT_TYPE).iter().count() == 1
            && headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.eq_ignore_ascii_case("application/json")
                        || v.eq_ignore_ascii_case("application/json; charset=utf-8")
                }));
    let mut response = if !host_ok || !origin_ok || !fetch_ok {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Host, Origin or fetch-site rejected"})),
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
    if !availability.allowed {
        return (StatusCode::FORBIDDEN, Json(availability)).into_response();
    }
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"Use /api/preview with synthetic fixtures"})),
    )
        .into_response()
}

pub fn app() -> Router {
    Router::new()
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
        .route("/api/preview", post(preview))
        .route("/api/operations", post(operation))
        .fallback_service(ServeDir::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/ui/dist"
        )))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn(boundary))
}

pub async fn run() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDRESS).await?;
    println!("mailroom synthetic preview: {ORIGIN}");
    axum::serve(listener, app()).await
}
