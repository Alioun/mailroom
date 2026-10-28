use super::*;
use crate::invoice::{self, SyntheticTransport};

/// The wired real adapters a send can use instead of the synthetic transport.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Wired {
    Smtp,
    MailApp,
}
impl Wired {
    fn mode(self) -> invoice::SendMode {
        match self {
            Self::Smtp => invoice::SendMode::Smtp,
            Self::MailApp => invoice::SendMode::MailAppHandoff,
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Open,
    /// Reads the row's original read-only, so its MIME facts and exact bytes are known.
    Acquire {
        selection: String,
    },
    Preview {
        selection: String,
        target: String,
        #[serde(default)]
        mode: Option<Wired>,
    },
    Send {
        selection: String,
        target: String,
        receipt: String,
        confirmation_action: String,
        #[serde(default)]
        mode: Option<Wired>,
    },
    Recover {
        send: String,
    },
    /// Records the owner's finding for an uncertain send. Sends nothing.
    Resolve {
        send: String,
        resolution: crate::send::Resolution,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    command: Action,
}
fn targets(saved: &crate::census::AccountState) -> Vec<String> {
    saved
        .workspace
        .as_ref()
        .and_then(|w| {
            crate::rules::config::documents(
                &w.input.documents.preset_yaml,
                &w.input.documents.override_yaml,
            )
            .ok()
        })
        .map(|(_, c)| c.targets.keys().cloned().collect())
        .unwrap_or_default()
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Every wired action asks the write policy first, before any repository,
    // credential, confirmation or network access.
    let permit = match &request.command {
        Action::Open | Action::Acquire { .. } => None,
        Action::Preview { mode: Some(_), .. }
        | Action::Send { mode: Some(_), .. }
        | Action::Resolve { .. } => match state.accounts.writes.permit(Operation::SendMessage) {
            Ok(permit) => Some(permit),
            Err(reason) => return (StatusCode::FORBIDDEN, Json(json!({"error":reason, "native_status":crate::confirmation::native_status()}))).into_response(),
        },
        _ if !state.accounts.synthetic() => return (StatusCode::FORBIDDEN, Json(json!({"error":"The synthetic invoice transport exists only in the fixture", "native_status":crate::confirmation::native_status()}))).into_response(),
        _ => None,
    };
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for census completion".to_string()); }
        let repo = state.accounts.repository().map_err(|e| e.to_string())?;
        let account = repo.load(&request.account).map_err(|e| e.to_string())?;
        let store = repo.store(&account).map_err(|e| e.to_string())?;
        if store.recovery_pending() && !matches!(request.command, Action::Open) { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
        let mut saved = store.load_state().map_err(|e| e.to_string())?;
        let ledger_guard = state.accounts.ledger.as_ref().map(|l| l.lock()).transpose()?;
        sync_ledger(&mut saved, &store, &repo.workspace_context(&account, &store).map_err(|e| e.to_string())?, ledger_guard.as_ref())?;
        let synthetic = state.accounts.synthetic();
        if synthetic && invoice::initialize_synthetic(&mut saved) { store.save_state(&mut saved).map_err(|e| e.to_string())?; }
        let mut transport = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } => Some(SyntheticTransport::open(mailboxes, &store, &saved)?),
            _ => None,
        };
        // The synthetic transport sends as the fixture sender; the wired adapters as the account.
        let sender = |mode: Option<Wired>| mode.map_or((invoice::SendMode::SyntheticTransport, invoice::SYNTHETIC_SENDER), |m| (m.mode(), account.login.as_str()));
        match request.command {
            Action::Open => {},
            Action::Acquire { selection } => {
                if synthetic && !repo.installed_test_transport() { return Err("No fake IMAP server is installed for original acquisition".into()); }
                let (folder, uidvalidity, uid) = invoice::census_location(&saved, account.provider, &selection)?;
                let original = repo.acquire_original(&account, &folder, uidvalidity, uid)?;
                invoice::record_original(&mut saved, &folder, uidvalidity, uid, original)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Preview { selection, target, mode } => {
                let (send_mode, address) = sender(mode);
                let mut preview = invoice::preview(&saved, account.provider, store.keys(), &selection, &target, synthetic, send_mode, address)?;
                let (handle, request) = state.accounts.confirmations.lock().expect("confirmations").offer(invoice::confirmation_plan(&preview))?;
                preview.receipt = handle;
                preview.confirmation = Some(request);
                return Ok(json!(preview));
            },
            Action::Send { selection, target, receipt, confirmation_action, mode } => {
                let (send_mode, address) = sender(mode);
                if saved.backlog.iter().any(|r| !matches!(r.status, crate::backlog::Status::Completed | crate::backlog::Status::Failed)) { return Err("Finish the backlog operation first".into()); }
                if saved.unsubscribe.history.iter().any(|e| matches!(e.status, crate::unsubscribe::Status::Intent | crate::unsubscribe::Status::Uncertain)) { return Err("Reconcile pending unsubscribe intent first".into()); }
                let preview = invoice::preview(&saved, account.provider, store.keys(), &selection, &target, synthetic, send_mode.clone(), address)?;
                if confirmation_action != preview.plan.confirmation_action { return Err("Action changed".into()); }
                let current = invoice::confirmation_plan(&preview);
                let consent = state.accounts.confirmations.lock().expect("confirmations").confirm(&receipt, &current, || {
                    let latest = store.load_state().map_err(|_| "Revalidate invoice state")?;
                    ledger_fresh(&latest, ledger_guard.as_ref())?;
                    invoice::preview(&latest, account.provider, store.keys(), &selection, &target, synthetic, send_mode.clone(), address).map(|p| invoice::confirmation_plan(&p))
                })?;
                if let (Some(mode), Some(permit)) = (mode, permit.as_ref()) {
                    // Resolved before the intent is saved, so a missing server leaves nothing uncertain.
                    let server = match mode { Wired::Smtp => Some(state.accounts.writes.smtp(permit, account.provider)?), Wired::MailApp => None };
                    let id = invoice::record_intent(&mut saved, preview, &consent, &confirmation_action)?;
                    store.save_state(&mut saved).map_err(|e| e.to_string())?;
                    invoice::boundary("before");
                    match server {
                        Some(server) => invoice::submit_smtp(&mut saved, &id, &server, &repo.smtp_login(&account), state.accounts.writes.timeouts())?,
                        None => invoice::hand_off(&mut saved, &id, state.accounts.writes.launcher(permit)?)?,
                    }
                    invoice::boundary("effect");
                    store.save_state(&mut saved).map_err(|e| e.to_string())?;
                    let report = invoice::report(&saved, account.provider, synthetic, store.keys())?;
                    return Ok(json!({"report":report,"sends":saved.invoices.sends,"synthetic":synthetic,"sent_evidence":invoice::sent_evidence(&saved),"targets":targets(&saved)}));
                }
                let id = invoice::record_intent(&mut saved, preview, &consent, &confirmation_action)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
                invoice::boundary("before");
                let send = saved.invoices.sends.last().ok_or("Missing intent")?;
                let installed = transport.as_mut().ok_or("No installed synthetic transport")?;
                installed.submit(&id, &send.plan)?;
                invoice::boundary("effect");
                invoice::observe(&mut saved, installed, &id)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Recover { send } => {
                invoice::observe(&mut saved, transport.as_ref().ok_or("No installed synthetic transport")?, &send)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
            Action::Resolve { send, resolution } => {
                invoice::resolve(&mut saved, &send, resolution)?;
                store.save_state(&mut saved).map_err(|e| e.to_string())?;
            },
        }
        let report = invoice::report(&saved, account.provider, synthetic, store.keys())?;
        Ok::<_, String>(json!({"report":report,"sends":saved.invoices.sends,"synthetic":synthetic,"sent_evidence":invoice::sent_evidence(&saved),"received":transport.as_ref().map(|t| t.received()),"targets":targets(&saved)}))
    }).await {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Invoice action rejected. Reload the report and exact preview. Uncertain submissions require reconciliation, never blind retry."}))).into_response(),
    }
}
