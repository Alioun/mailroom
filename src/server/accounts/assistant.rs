use super::*;
use crate::agent::{
    inbox::{Inbox, Launch, Summary},
    t3,
};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Status {},
    ConfigGet {},
    ConfigSet {
        provider: String,
        model: String,
    },
    Prepare {
        summary: Summary,
        confirmed: bool,
    },
    Launch {
        request_id: String,
        #[serde(default)]
        force_new: bool,
    },
    Dismiss {
        request_id: String,
        proposal_id: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    #[serde(default)]
    account: Option<AccountId>,
    command: Action,
}

const CHANGED: &str = "Summary or census changed. Review a fresh summary first";

fn reviewed(
    state: &AppState,
    repo: &Accounts,
    account: &Account,
    summary: &Summary,
) -> Result<String, String> {
    use subtle::ConstantTimeEq;
    if state
        .accounts
        .job
        .lock()
        .expect("account job")
        .as_ref()
        .is_some_and(|j| j.progress.state == ScanState::Running)
    {
        return Err("Wait for the account operation to finish".into());
    }
    let store = repo.store(account).map_err(|e| e.to_string())?;
    let census = store.load().map_err(|e| e.to_string())?;
    let mut report = census
        .report(
            store.keys(),
            summary.as_of,
            account.provider == Provider::Gmail,
        )
        .map_err(|_| CHANGED)?;
    if store.recovery_pending() {
        report.acquisition.historical();
    }
    if report.version != summary.version {
        return Err(CHANGED.into());
    }
    let preview = report
        .summary_preview(store.keys(), &summary.approved)
        .map_err(|_| CHANGED)?;
    if !bool::from(preview.receipt.as_bytes().ct_eq(summary.receipt.as_bytes())) {
        return Err(CHANGED.into());
    }
    if !matches!(
        state.accounts.source,
        AccountSource::Vault { .. } | AccountSource::Synthetic { .. }
    ) {
        disclosure::confirm(state, account, &store, &report, &preview, &summary.approved)
            .map_err(|e| e.to_string())?;
    }
    Ok(preview.summary)
}

fn spawn_launch(
    state: &AppState,
    inbox: Inbox,
    id: String,
    lock: crate::file_lock::ExclusiveFileLock,
    allow_create: bool,
    config: t3::Config,
) {
    #[cfg(test)]
    let client = state
        .accounts
        .assistant_cli
        .clone()
        .map(Ok)
        .unwrap_or_else(t3::Client::installed);
    #[cfg(not(test))]
    let client = {
        let _ = state;
        t3::Client::installed()
    };
    std::thread::spawn(move || {
        let _lock = lock;
        let Ok(mut record) = inbox.record(&id) else {
            return;
        };
        let folder = inbox.exchange_path(&id);
        let result = (|| {
            let executable = std::env::current_exe().map_err(|_| t3::Failure {
                message: "Assistant executable unavailable".into(),
                uncertain: !allow_create,
            })?;
            let prompt = t3::prompt(&executable, &folder).map_err(|message| t3::Failure {
                message: message.into(),
                uncertain: !allow_create,
            })?;
            let client = client.map_err(|message| t3::Failure {
                message: message.into(),
                uncertain: !allow_create,
            })?;
            client.launch_with_config(
                &folder,
                &format!("Mailroom review {id}"),
                &prompt,
                allow_create,
                &config,
            )
        })();
        record.launch = match result {
            Ok(thread) => Launch::Launched {
                thread_id: thread.id,
                thread_link: thread.link,
            },
            Err(failure) => Launch::Failed {
                error: failure.message,
                uncertain: failure.uncertain,
            },
        };
        let _ = inbox.save(&record);
    });
}

pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(request) => request,
        Err(error) => return error,
    };
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let needs_census = matches!(request.command, Action::Prepare { .. } | Action::Launch { .. });
        let _admission = needs_census.then(|| state.accounts.admission.lock().expect("account admission"));
        let _operation = needs_census.then(|| state.accounts.operation.lock().expect("account operation"));
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        if matches!(request.command, Action::ConfigGet {}) {
            let config = t3::load_config(&repo.paths.data);
            return Ok(json!({"config": config}));
        }
        if let Action::ConfigSet { provider, model } = &request.command {
            let saved = t3::save_config(
                &repo.paths.data,
                &t3::Config {
                    provider: provider.trim().to_owned(),
                    model: model.trim().to_owned(),
                },
            )?;
            return Ok(json!({"config": saved}));
        }
        let config = t3::load_config(&repo.paths.data);
        let account_id = request.account.ok_or_else(|| "Account is required".to_owned())?;
        let account = repo.load(&account_id).map_err(|e| e.to_string())?;
        let inbox = Inbox::open(&repo.paths.data, account.id.as_str())?;
        match request.command {
            Action::ConfigGet {} | Action::ConfigSet { .. } => unreachable!(),
            Action::Status {} => {
                for record in inbox.records()? {
                    if matches!(record.launch, Launch::Launching) {
                        if let Ok(lock) = inbox.launch_lock(&record.id) {
                            spawn_launch(&state, Inbox::open(&repo.paths.data, account.id.as_str())?, record.id, lock, false, config.clone());
                        }
                    }
                }
            }
            Action::Prepare { summary, confirmed } => {
                if !confirmed { return Err("Confirm sharing the exact displayed summary first".into()); }
                let text = reviewed(&state, &repo, &account, &summary)?;
                let executable = std::env::current_exe().map_err(|_| "Assistant executable unavailable")?;
                inbox.prepare(summary, &text, &executable)?;
            }
            Action::Launch { request_id, force_new } => {
                let mut record = inbox.record(&request_id)?;
                let allow_create = match record.launch {
                    Launch::Prepared | Launch::Failed { uncertain: false, .. } => true,
                    Launch::Failed { uncertain: true, .. } if force_new => true,
                    Launch::Launching | Launch::Failed { uncertain: true, .. } => false,
                    Launch::Launched { .. } => return Ok(json!({"requests": inbox.views()?, "config": config})),
                };
                if allow_create { reviewed(&state, &repo, &account, &record.summary)?; }
                if let Ok(lock) = inbox.launch_lock(&request_id) {
                    record.launch = Launch::Launching;
                    inbox.save(&record)?;
                    spawn_launch(&state, Inbox::open(&repo.paths.data, account.id.as_str())?, request_id, lock, allow_create, config.clone());
                }
            }
            Action::Dismiss { request_id, proposal_id } => inbox.dismiss(&request_id, &proposal_id)?,
        }
        Ok(json!({"requests": inbox.views()?, "config": config}))
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":error}))).into_response(),
        Err(_) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Assistant action interrupted. Reload saved requests before trying again"}))).into_response(),
    }
}

#[cfg(test)]
mod tests;
