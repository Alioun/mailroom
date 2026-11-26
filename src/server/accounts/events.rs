use super::*;
use axum::extract::Query;
use std::time::Duration;

#[derive(Deserialize)]
pub(in crate::server) struct EventsQuery {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub(super) fn status_json(state: &AppState) -> serde_json::Value {
    let accounts = &state.accounts;
    let safety = accounts.safety_snapshot();
    let source = match &accounts.source {
        AccountSource::Os => "native",
        AccountSource::Unavailable => "unavailable",
        AccountSource::Installed { .. } => "installed",
        AccountSource::Synthetic { .. } => "synthetic",
        AccountSource::Vault { .. } => "vault",
    };
    json!({
        "synthetic": accounts.synthetic(),
        "source": source,
        "live_writes": safety.live_writes,
        "live_writes_expires_in_secs": safety.live_writes_expires_in_secs,
        "live_writes_timeout_secs": safety.live_writes_timeout_secs,
        "vault_idle_timeout_secs": safety.vault_idle_timeout_secs,
        "vault_locks_in_secs": safety.vault_locks_in_secs,
        "vault": accounts.vault().map(|vault| vault.status()),
        "vault_problem": accounts.vault().and_then(|vault| vault.problem()),
        "interrupted": web::interrupted(accounts),
        "job": accounts.job_snapshot(),
    })
}

/// Long-poll status event endpoint: returns immediately when `seq != since`,
/// otherwise waits for the next state/progress/safety change or timeout.
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
) -> Response {
    state.accounts.enforce_timeouts();
    let mut rx = state.accounts.subscribe_events();
    let current = *rx.borrow_and_update();
    if query.since == Some(current) {
        let default_wait_ms = if state
            .accounts
            .safety_snapshot()
            .live_writes_expires_in_secs
            .is_some()
            || state
                .accounts
                .safety_snapshot()
                .vault_locks_in_secs
                .is_some()
        {
            5_000
        } else {
            20_000
        };
        let wait_ms = query.timeout_ms.unwrap_or(default_wait_ms).min(25_000);
        let _ = tokio::time::timeout(Duration::from_millis(wait_ms), rx.changed()).await;
        state.accounts.enforce_timeouts();
    }
    let seq = state.accounts.event_seq();
    Json(json!({
        "seq": seq,
        "status": status_json(&state),
    }))
    .into_response()
}
