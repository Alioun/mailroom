use super::*;
use crate::winddown::{self, Change, Disclosure, Source};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::server) enum Action {
    Open,
    Preview {
        version: String,
        change: Change,
    },
    Confirm {
        version: String,
        change: Change,
        receipt: String,
        confirmed: bool,
    },
    ExportPreview {
        version: String,
        disclosure: Disclosure,
    },
    Export {
        version: String,
        disclosure: Disclosure,
        receipt: String,
        confirmed: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    as_of: i64,
    command: Action,
}
fn same(received: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    bool::from(received.as_bytes().ct_eq(expected.as_bytes()))
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !state.accounts.local_bookkeeping() {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Protected saved-census report installation is unavailable. Reports grant no provider authority."}))).into_response();
    }
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let _operation = state.accounts.operation.lock().expect("account operation");
        let data = sender_data(request.as_of)?;
        if state.accounts.job_running() {
            return Err("Wait for census completion".into());
        }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        if store.recovery_pending() { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let guard = state.accounts.ledger().map_err(|e| e.to_string())?.ok_or("No installed ledger")?.lock()?;
        sync_ledger(&mut saved, &store, &repo.workspace_context(&account, &store).map_err(|e| e.to_string())?, Some(&guard))?;
        let ledger = guard.read()?;
        let proposed = match &request.command {
            Action::Preview { change: Change::Settings { settings }, .. } | Action::Confirm { change: Change::Settings { settings }, .. } => Some(settings),
            _ => None,
        };
        let mut loaded = Vec::new();
        let mut available = Vec::new();
        for a in repo.list().map_err(|e| e.to_string())? {
            if a.id == request.account { continue; }
            let s = repo.store(&a).map_err(|e| e.to_string())?;
            if s.recovery_pending() { continue; }
            let v = s.load_state().map_err(|e| e.to_string())?;
            available.push(json!({"account":a.id,"login":a.login,"owned_addresses":v.census.confirmed_own_addresses}));
            if saved.winddown.settings.comparisons.contains_key(a.id.as_str()) || proposed.is_some_and(|p| p.comparisons.contains_key(a.id.as_str())) { loaded.push((a, s, v)); }
        }
        let comparisons_for = |settings: &winddown::Settings| loaded.iter().filter(|(a, _, _)| settings.comparisons.contains_key(a.id.as_str())).map(|(a, s, v)| Source { state: v, provider: a.provider, keys: s.keys() }).collect::<Vec<_>>();
        if matches!(request.command, Action::Open) {
            let report = winddown::derive(&Source { state: &saved, provider: account.provider, keys: store.keys() }, &comparisons_for(&saved.winddown.settings), &ledger, &data, request.as_of)?;
            let mut known = saved.winddown.known.clone();
            for row in report.rows { known.insert(row.id.clone(), row); }
            if serde_json::to_vec(&known).map_err(|e| e.to_string())? != serde_json::to_vec(&saved.winddown.known).map_err(|e| e.to_string())? {
                saved.winddown.known = known;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            }
        }
        let source = Source { state: &saved, provider: account.provider, keys: store.keys() };
        let comparisons = comparisons_for(&saved.winddown.settings);
        let report = winddown::derive(&source, &comparisons, &ledger, &data, request.as_of)?;
        let response = match &request.command {
            Action::Open => json!({"report": report, "terminal":report.terminal(), "available":available}),
            Action::Preview { version, change } | Action::Confirm { version, change, receipt: _, confirmed: _ } => {
                let next_settings = match &change { Change::Settings { settings } => settings, _ => &saved.winddown.settings };
                let next_comparisons = comparisons_for(next_settings);
                let review = winddown::review(&source, &comparisons, &next_comparisons, &ledger, &data, request.as_of, &version, &change)?;
                match &request.command {
                    Action::Confirm { receipt, confirmed, .. } => {
                        if !confirmed || !same(receipt, &review.receipt) { return Err("Exact local decision requires a current purpose-specific confirmation".into()); }
                        if guard.read()?.content_version != ledger.content_version { return Err("Ledger changed. Reload".into()); }
                        winddown::apply(&mut saved, &review);
                        store.save_state(&mut saved).map_err(|e| e.to_string())?;
                        json!({"saved":true})
                    }
                    _ => json!(review),
                }
            }
            Action::ExportPreview { version, disclosure } | Action::Export { version, disclosure, .. } => {
                if &report.version != version { return Err("Report changed. Reload and review the exact output again".into()); }
                let preview = report.export_preview(store.keys(), *disclosure);
                if let Action::Export { receipt, confirmed, .. } = &request.command {
                    if !confirmed || !same(receipt, &preview.receipt) { return Err("Exact disclosure requires its own current confirmation".into()); }
                }
                json!(preview)
            }
        };
        if guard.read()?.content_version != ledger.content_version { return Err("Ledger changed during report action. Reload".into()); }
        Ok(response)
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":error}))).into_response(),
        Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Wind-down action interrupted. Reopen saved state. Preserve files and restore the original key if local state cannot be read."}))).into_response(),
    }
}
fn sender_data(as_of: i64) -> Result<winddown::SenderData, String> {
    let data = crate::sender_data::bundled()
        .and_then(|data| data.winddown(as_of))
        .map_err(|e| e.to_string())?;
    #[cfg(test)]
    let data = if std::env::var("MAILROOM_WINDDOWN_FIXTURE").ok().as_deref() == Some("1") {
        winddown::SenderData {
            version: "synthetic-reviewed-sender-data/v1".into(),
            reviewed: std::collections::BTreeMap::from([(
                "platform.example".into(),
                winddown::SenderEntry {
                    transactional: true,
                    settings_link: Some("https://platform.example/settings".into()),
                },
            )]),
            catalog: None,
        }
    } else {
        data
    };
    Ok(data)
}
