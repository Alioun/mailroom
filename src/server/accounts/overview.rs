use super::*;
use crate::backlog::retention;

#[derive(Serialize)]
struct AccountOverviewRow {
    id: String,
    provider: Provider,
    login: String,
    connection: String,
    scan_state: ScanState,
    saved_messages: usize,
    folders: usize,
    recovery_pending: bool,
    active_undo_windows: usize,
    nearest_undo_until: Option<u64>,
    uncertain_actions: usize,
    sieve_deployed: bool,
    pending_backlog_runs: usize,
}

#[derive(Serialize)]
struct RecentAction {
    id: String,
    account: String,
    account_login: String,
    kind: &'static str,
    status: String,
    summary: String,
    timestamp: u64,
    undo_open: bool,
    undo_until: Option<u64>,
    page: &'static str,
}

pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = input(body) {
        return error;
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        let repo = state.accounts.repository()?;
        let accounts = repo.list()?;
        let now = retention::now();
        let mut rows = Vec::with_capacity(accounts.len());
        let mut actions: Vec<RecentAction> = Vec::new();

        for account in accounts {
            let connection = serde_json::to_value(&account)
                .ok()
                .and_then(|v| {
                    v.get("connection")
                        .and_then(|c| c.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "unverified".into());
            let mut row = AccountOverviewRow {
                id: account.id.as_str().to_owned(),
                provider: account.provider,
                login: account.login.clone(),
                connection,
                scan_state: ScanState::Idle,
                saved_messages: 0,
                folders: 0,
                recovery_pending: false,
                active_undo_windows: 0,
                nearest_undo_until: None,
                uncertain_actions: 0,
                sieve_deployed: false,
                pending_backlog_runs: 0,
            };

            if let Ok(store) = repo.store(&account) {
                row.recovery_pending = store.recovery_pending();
                if let Ok(saved) = store.load_state() {
                    let census = &*saved.census;
                    row.scan_state = census.acquisition_state;
                    row.folders = census.folders.len();
                    row.saved_messages = census.folders.values().map(|f| f.messages.len()).sum();

                    // Protocol (IMAP) backlog runs
                    for run in &saved.protocol_backlog.runs {
                        let status_str = serde_json::to_value(run.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        let undo_open = matches!(
                            run.status,
                            crate::backlog::protocol::Status::Complete
                                | crate::backlog::protocol::Status::Stopped
                                | crate::backlog::protocol::Status::Conflict
                        ) && retention::undo_open(run.completed_at, now)
                            && run.closed.is_none();
                        let undo_until = run.completed_at.map(retention::deadline);
                        if undo_open {
                            row.active_undo_windows += 1;
                            if let Some(until) = undo_until {
                                row.nearest_undo_until = Some(
                                    row.nearest_undo_until.map_or(until, |cur| cur.min(until)),
                                );
                            }
                        }
                        if matches!(run.status, crate::backlog::protocol::Status::Uncertain) {
                            row.uncertain_actions += 1;
                        }
                        if matches!(
                            run.status,
                            crate::backlog::protocol::Status::Running
                                | crate::backlog::protocol::Status::Stopped
                                | crate::backlog::protocol::Status::Conflict
                                | crate::backlog::protocol::Status::Uncertain
                        ) {
                            row.pending_backlog_runs += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("imap-run-{}", run.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "backlog",
                            status: status_str,
                            summary: format!(
                                "IMAP backlog · {} messages",
                                run.plan.filing.changes.len()
                            ),
                            timestamp: run.completed_at.unwrap_or(now),
                            undo_open,
                            undo_until,
                            page: "backlog",
                        });
                    }
                    for summary in &saved.protocol_backlog.summaries {
                        let status_str = serde_json::to_value(summary.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "complete".into());
                        actions.push(RecentAction {
                            id: format!("imap-sum-{}", summary.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "backlog",
                            status: status_str,
                            summary: format!(
                                "IMAP backlog summary · {}/{} applied",
                                summary.applied, summary.messages
                            ),
                            timestamp: summary.completed_at,
                            undo_open: false,
                            undo_until: None,
                            page: "backlog",
                        });
                    }

                    // Gmail backlog runs
                    for run in &saved.gmail_backlog.runs {
                        let status_str = serde_json::to_value(run.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        let undo_open = matches!(
                            run.status,
                            crate::backlog::protocol::Status::Complete
                                | crate::backlog::protocol::Status::Stopped
                                | crate::backlog::protocol::Status::Conflict
                        ) && retention::undo_open(run.completed_at, now)
                            && run.closed.is_none();
                        let undo_until = run.completed_at.map(retention::deadline);
                        if undo_open {
                            row.active_undo_windows += 1;
                            if let Some(until) = undo_until {
                                row.nearest_undo_until = Some(
                                    row.nearest_undo_until.map_or(until, |cur| cur.min(until)),
                                );
                            }
                        }
                        if matches!(run.status, crate::backlog::protocol::Status::Uncertain) {
                            row.uncertain_actions += 1;
                        }
                        if matches!(
                            run.status,
                            crate::backlog::protocol::Status::Running
                                | crate::backlog::protocol::Status::Stopped
                                | crate::backlog::protocol::Status::Conflict
                                | crate::backlog::protocol::Status::Uncertain
                        ) {
                            row.pending_backlog_runs += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("gmail-run-{}", run.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "backlog",
                            status: status_str,
                            summary: format!(
                                "Gmail backlog · {} messages",
                                run.plan.filing.changes.len()
                            ),
                            timestamp: run.completed_at.unwrap_or(now),
                            undo_open,
                            undo_until,
                            page: "backlog",
                        });
                    }
                    for summary in &saved.gmail_backlog.summaries {
                        let status_str = serde_json::to_value(summary.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "complete".into());
                        actions.push(RecentAction {
                            id: format!("gmail-sum-{}", summary.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "backlog",
                            status: status_str,
                            summary: format!(
                                "Gmail backlog summary · {}/{} applied",
                                summary.applied, summary.messages
                            ),
                            timestamp: summary.completed_at,
                            undo_open: false,
                            undo_until: None,
                            page: "backlog",
                        });
                    }

                    // Sieve transactions
                    for (idx, tx) in saved.sieve_transactions.history.iter().enumerate() {
                        let status_str = serde_json::to_value(tx.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        if status_str == "complete" || status_str == "completed" {
                            row.sieve_deployed = true;
                        }
                        if status_str == "uncertain" {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("sieve-{}", tx.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "sieve",
                            status: status_str,
                            summary: format!(
                                "Sieve {} · {}",
                                if tx.compensates.is_some() {
                                    "rollback"
                                } else {
                                    "deploy"
                                },
                                tx.target
                            ),
                            timestamp: idx as u64,
                            undo_open: false,
                            undo_until: None,
                            page: "rules",
                        });
                    }

                    // Folder / label provisioning
                    for (idx, op) in saved.folder_provisioning.history.iter().enumerate() {
                        let status_str = serde_json::to_value(op.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        if status_str == "uncertain" {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("folder-{}", op.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "folders",
                            status: status_str,
                            summary: format!("Folder provisioning · {} steps", op.steps.len()),
                            timestamp: idx as u64,
                            undo_open: false,
                            undo_until: None,
                            page: "rules",
                        });
                    }

                    // Invoice sends
                    for (idx, send) in saved.invoices.sends.iter().enumerate() {
                        let status_str = serde_json::to_value(&send.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        if !send.status.settled() {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("invoice-{}", send.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "invoice",
                            status: status_str,
                            summary: format!(
                                "Invoice to {} ({})",
                                send.plan.target_name, send.plan.target
                            ),
                            timestamp: idx as u64,
                            undo_open: false,
                            undo_until: None,
                            page: "invoices",
                        });
                    }

                    // Unsubscribe history
                    for entry in &saved.unsubscribe.history {
                        let status_str = serde_json::to_value(&entry.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        let method_str = serde_json::to_value(&entry.method)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "action".into());
                        if matches!(
                            entry.status,
                            crate::unsubscribe::Status::Intent
                                | crate::unsubscribe::Status::Uncertain
                        ) {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("unsub-{}", entry.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "unsubscribe",
                            status: status_str,
                            summary: format!("{} · {}", method_str, entry.sender_key),
                            timestamp: entry.time.max(0) as u64,
                            undo_open: false,
                            undo_until: None,
                            page: "subscriptions",
                        });
                    }
                }
            }
            rows.push(row);
        }

        actions.sort_by(|a, b| {
            b.undo_open
                .cmp(&a.undo_open)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
                .then_with(|| a.id.cmp(&b.id))
        });
        actions.truncate(40);

        Ok::<_, AcquisitionError>(json!({
            "accounts": rows,
            "recent_actions": actions,
        }))
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}
