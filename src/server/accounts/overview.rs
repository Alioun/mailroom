use super::*;
use crate::backlog::{protocol::Status as BacklogStatus, retention};

const MAX_RECENT_ACTIONS: usize = 40;

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
    timestamp: Option<u64>,
    undo_open: bool,
    undo_until: Option<u64>,
    page: &'static str,
    #[serde(skip)]
    needs_attention: bool,
    #[serde(skip)]
    history_order: usize,
}

struct BacklogRunProjection<'a> {
    id_prefix: &'static str,
    label: &'static str,
    run_id: &'a str,
    status: BacklogStatus,
    settled: bool,
    closed: bool,
    completed_at: Option<u64>,
    message_count: usize,
}

fn enum_label<T: Serialize>(value: &T, fallback: &str) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| fallback.into())
}

fn record_backlog_run(
    account: &Account,
    run: BacklogRunProjection<'_>,
    now: u64,
    row: &mut AccountOverviewRow,
    actions: &mut Vec<RecentAction>,
) {
    let status_str = enum_label(&run.status, "unknown");
    let undo_open = matches!(
        run.status,
        BacklogStatus::Complete | BacklogStatus::Stopped | BacklogStatus::Conflict
    ) && retention::undo_open(run.completed_at, now)
        && !run.closed;
    let undo_until = run.completed_at.map(retention::deadline);
    if undo_open {
        row.active_undo_windows += 1;
        if let Some(until) = undo_until {
            row.nearest_undo_until =
                Some(row.nearest_undo_until.map_or(until, |cur| cur.min(until)));
        }
    }
    let uncertain = matches!(run.status, BacklogStatus::Uncertain);
    if uncertain {
        row.uncertain_actions += 1;
    }
    if !run.settled {
        row.pending_backlog_runs += 1;
    }
    actions.push(RecentAction {
        id: format!("{}-{}", run.id_prefix, run.run_id),
        account: account.id.as_str().to_owned(),
        account_login: account.login.clone(),
        kind: "backlog",
        status: status_str,
        summary: format!("{} backlog · {} messages", run.label, run.message_count),
        timestamp: run.completed_at.or(Some(now)),
        undo_open,
        undo_until,
        page: "backlog",
        needs_attention: uncertain || !run.settled,
        history_order: 0,
    });
}

fn record_backlog_summary(
    account: &Account,
    id_prefix: &'static str,
    label: &'static str,
    summary_id: &str,
    status: BacklogStatus,
    applied: usize,
    messages: usize,
    completed_at: u64,
    actions: &mut Vec<RecentAction>,
) {
    actions.push(RecentAction {
        id: format!("{id_prefix}-{summary_id}"),
        account: account.id.as_str().to_owned(),
        account_login: account.login.clone(),
        kind: "backlog",
        status: enum_label(&status, "complete"),
        summary: format!("{label} backlog summary · {applied}/{messages} applied"),
        timestamp: Some(completed_at),
        undo_open: false,
        undo_until: None,
        page: "backlog",
        needs_attention: false,
        history_order: 0,
    });
}

/// Orders actions so open undo windows and items needing attention always surface first,
/// and undated history (Sieve, folders, invoices) is never starved out by >40 older dated rows.
fn sort_and_cap_actions(actions: Vec<RecentAction>) -> Vec<RecentAction> {
    let mut priority = Vec::new();
    let mut dated = Vec::new();
    let mut undated = Vec::new();
    for action in actions {
        if action.undo_open || action.needs_attention {
            priority.push(action);
        } else if action.timestamp.is_some() {
            dated.push(action);
        } else {
            undated.push(action);
        }
    }
    priority.sort_by(|a, b| {
        b.undo_open
            .cmp(&a.undo_open)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(|| b.history_order.cmp(&a.history_order))
            .then_with(|| a.id.cmp(&b.id))
    });
    dated.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
    undated.sort_by(|a, b| {
        b.history_order
            .cmp(&a.history_order)
            .then_with(|| a.id.cmp(&b.id))
    });

    if priority.len() >= MAX_RECENT_ACTIONS {
        priority.truncate(MAX_RECENT_ACTIONS);
        return priority;
    }
    let remaining = MAX_RECENT_ACTIONS - priority.len();
    // Reserve up to a third of remaining slots for recent undated actions when both pools overflow.
    let undated_budget = if dated.len() + undated.len() > remaining {
        undated.len().min((remaining / 3).max(1))
    } else {
        undated.len()
    };
    let dated_budget = remaining.saturating_sub(undated_budget);
    dated.truncate(dated_budget);
    undated.truncate(remaining.saturating_sub(dated.len()));

    let mut result = priority;
    result.extend(dated);
    result.extend(undated);
    result
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

                    for run in &saved.protocol_backlog.runs {
                        record_backlog_run(
                            &account,
                            BacklogRunProjection {
                                id_prefix: "imap-run",
                                label: "IMAP",
                                run_id: &run.id,
                                status: run.status,
                                settled: run.settled(),
                                closed: run.closed.is_some(),
                                completed_at: run.completed_at,
                                message_count: run.plan.filing.changes.len(),
                            },
                            now,
                            &mut row,
                            &mut actions,
                        );
                    }
                    for summary in &saved.protocol_backlog.summaries {
                        record_backlog_summary(
                            &account,
                            "imap-sum",
                            "IMAP",
                            &summary.id,
                            summary.status,
                            summary.applied,
                            summary.messages,
                            summary.completed_at,
                            &mut actions,
                        );
                    }

                    for run in &saved.gmail_backlog.runs {
                        record_backlog_run(
                            &account,
                            BacklogRunProjection {
                                id_prefix: "gmail-run",
                                label: "Gmail",
                                run_id: &run.id,
                                status: run.status,
                                settled: run.settled(),
                                closed: run.closed.is_some(),
                                completed_at: run.completed_at,
                                message_count: run.plan.filing.changes.len(),
                            },
                            now,
                            &mut row,
                            &mut actions,
                        );
                    }
                    for summary in &saved.gmail_backlog.summaries {
                        record_backlog_summary(
                            &account,
                            "gmail-sum",
                            "Gmail",
                            &summary.id,
                            summary.status,
                            summary.applied,
                            summary.messages,
                            summary.completed_at,
                            &mut actions,
                        );
                    }

                    // Sieve transactions (undated domain records — preserve insertion order)
                    for (idx, tx) in saved.sieve_transactions.history.iter().enumerate() {
                        let status_str = enum_label(&tx.status, "unknown");
                        if status_str == "complete" || status_str == "completed" {
                            row.sieve_deployed = true;
                        }
                        let uncertain = status_str == "uncertain";
                        if uncertain {
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
                            timestamp: None,
                            undo_open: false,
                            undo_until: None,
                            page: "rules",
                            needs_attention: uncertain,
                            history_order: idx,
                        });
                    }

                    // Folder / label provisioning (undated domain records)
                    for (idx, op) in saved.folder_provisioning.history.iter().enumerate() {
                        let status_str = enum_label(&op.status, "unknown");
                        let uncertain = status_str == "uncertain";
                        if uncertain {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("folder-{}", op.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "folders",
                            status: status_str,
                            summary: format!("Folder provisioning · {} steps", op.steps.len()),
                            timestamp: None,
                            undo_open: false,
                            undo_until: None,
                            page: "rules",
                            needs_attention: uncertain,
                            history_order: idx,
                        });
                    }

                    // Invoice sends (undated domain records)
                    for (idx, send) in saved.invoices.sends.iter().enumerate() {
                        let status_str = enum_label(&send.status, "unknown");
                        let unsettled = !send.status.settled();
                        if unsettled {
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
                            timestamp: None,
                            undo_open: false,
                            undo_until: None,
                            page: "invoices",
                            needs_attention: unsettled,
                            history_order: idx,
                        });
                    }

                    // Unsubscribe history (dated)
                    for entry in &saved.unsubscribe.history {
                        let status_str = enum_label(&entry.status, "unknown");
                        let method_str = enum_label(&entry.method, "action");
                        let attention = matches!(
                            entry.status,
                            crate::unsubscribe::Status::Intent
                                | crate::unsubscribe::Status::Uncertain
                        );
                        if attention {
                            row.uncertain_actions += 1;
                        }
                        actions.push(RecentAction {
                            id: format!("unsub-{}", entry.id),
                            account: account.id.as_str().to_owned(),
                            account_login: account.login.clone(),
                            kind: "unsubscribe",
                            status: status_str,
                            summary: format!("{} · {}", method_str, entry.sender_key),
                            timestamp: (entry.time > 0).then_some(entry.time as u64),
                            undo_open: false,
                            undo_until: None,
                            page: "subscriptions",
                            needs_attention: attention,
                            history_order: 0,
                        });
                    }
                }
            }
            rows.push(row);
        }

        let actions = sort_and_cap_actions(actions);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn action(
        id: &str,
        kind: &'static str,
        timestamp: Option<u64>,
        needs_attention: bool,
        history_order: usize,
    ) -> RecentAction {
        RecentAction {
            id: id.into(),
            account: "acc".into(),
            account_login: "owner@example.test".into(),
            kind,
            status: "complete".into(),
            summary: id.into(),
            timestamp,
            undo_open: false,
            undo_until: None,
            page: "rules",
            needs_attention,
            history_order,
        }
    }

    #[test]
    fn sort_and_cap_preserves_undated_and_attention_actions_past_forty_dated_entries() {
        let mut items: Vec<RecentAction> = (0..50)
            .map(|i| {
                action(
                    &format!("dated-{i}"),
                    "backlog",
                    Some(1_700_000_000 + i),
                    false,
                    0,
                )
            })
            .collect();
        items.push(action("sieve-new", "sieve", None, false, 5));
        items.push(action("invoice-unsettled", "invoice", None, true, 1));

        let result = sort_and_cap_actions(items);
        assert_eq!(result.len(), MAX_RECENT_ACTIONS);
        assert_eq!(result[0].id, "invoice-unsettled");
        assert!(result[0].timestamp.is_none());
        assert!(
            result
                .iter()
                .any(|item| item.id == "sieve-new" && item.timestamp.is_none()),
            "recent undated sieve action must not be evicted by >40 dated items"
        );
    }
}
