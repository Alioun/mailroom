use super::*;
use crate::unsubscribe::{self, Adapters, InstalledSynthetic, ReviewInput, Wired};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {
        as_of: i64,
    },
    Preview {
        input: ReviewInput,
    },
    Confirm {
        input: ReviewInput,
        receipt: String,
        confirmation_action: String,
        mail_confirmation_action: Option<String>,
    },
    Recover {
        entry: String,
        as_of: i64,
    },
    /// Records the owner's finding for an uncertain local unsubscribe message.
    Resolve {
        entry: String,
        resolution: crate::send::Resolution,
        as_of: i64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    account: AccountId,
    command: Action,
}
pub(super) fn wired(state: &AppState) -> bool {
    !state.accounts.synthetic() || state.accounts.writes.fake()
}
pub(super) fn pick<'a>(
    wired: bool,
    wiring: &'a Wired<'a>,
    installed: &'a Option<InstalledSynthetic>,
) -> Option<&'a dyn Adapters> {
    if wired {
        Some(wiring)
    } else {
        installed.as_ref().map(|i| i as &dyn Adapters)
    }
}
pub(super) struct PreparedWired<'a> {
    writes: &'a crate::send::wiring::Endpoints,
    permit: &'a policy::WritePermit,
    server: Option<crate::send::smtp::Server>,
    one_click: Option<(
        &'a dyn unsubscribe::one_click::Network,
        native_tls::TlsConnector,
        std::time::Duration,
    )>,
}
impl<'a> PreparedWired<'a> {
    pub(super) fn new(
        state: &'a AppState,
        permit: &'a policy::WritePermit,
        provider: Provider,
        preview: &unsubscribe::Preview,
    ) -> Result<Self, String> {
        let writes = &state.accounts.writes;
        let method = |m| preview.actions.iter().any(|a| a.method == m);
        Ok(Self {
            writes,
            permit,
            server: method(unsubscribe::Method::LocalMail)
                .then(|| writes.smtp(permit, provider))
                .transpose()?,
            one_click: method(unsubscribe::Method::OneClick)
                .then(|| writes.one_click(permit))
                .transpose()?,
        })
    }
    pub(super) fn execute(
        &self,
        repo: &Accounts,
        account: &Account,
        store: &crate::census::EncryptedStore,
        saved: &mut crate::census::AccountState,
        preview: &unsubscribe::Preview,
        ids: &[String],
        as_of: i64,
    ) -> Result<(), String> {
        for (id, action) in ids.iter().zip(&preview.actions) {
            if !matches!(
                action.method,
                unsubscribe::Method::OneClick
                    | unsubscribe::Method::LocalMail
                    | unsubscribe::Method::MailAppPreparation
            ) {
                continue;
            }
            let uris = unsubscribe::uris(
                &unsubscribe::source(saved, &action.source)
                    .ok_or("Source changed")?
                    .headers,
            );
            let uri = |scheme: &str| {
                uris.iter()
                    .find(|u| u.starts_with(scheme))
                    .cloned()
                    .ok_or("Exact URI missing")
            };
            match action.method {
                unsubscribe::Method::OneClick => {
                    let (network, tls, timeout) =
                        self.one_click.as_ref().ok_or("One-click network")?;
                    unsubscribe::post_one_click(
                        saved,
                        id,
                        &uri("https://")?,
                        *network,
                        tls,
                        *timeout,
                    )?;
                }
                unsubscribe::Method::LocalMail => {
                    unsubscribe::submit_local_mail(
                        saved,
                        id,
                        &uri("mailto:")?,
                        &action.recipient,
                        as_of,
                        self.server.as_ref().ok_or("Submission server")?,
                        &repo.smtp_login(account),
                        self.writes.timeouts(),
                    )?;
                }
                unsubscribe::Method::MailAppPreparation => unsubscribe::hand_off_mailto(
                    saved,
                    id,
                    &uri("mailto:")?,
                    &action.recipient,
                    self.writes.launcher(self.permit)?,
                )?,
                _ => continue,
            }
            unsubscribe::boundary("effect");
            store
                .save_state(saved)
                .map_err(|_| "Persist unsubscribe effect")?;
        }
        Ok(())
    }
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Production and installed fakes use the wired real adapters; the fixture
    // server otherwise keeps its synthetic adapter.
    let wired = wired(&state);
    let permit = match &request.command {
        Action::Open { .. } => None,
        Action::Recover { .. } if wired => return (StatusCode::FORBIDDEN, Json(json!({"error":"Wired unsubscribe effects are resolved by the owner, not reconciled against a fixture"}))).into_response(),
        Action::Recover { .. } => None,
        Action::Resolve { .. } if !wired => return (StatusCode::FORBIDDEN, Json(json!({"error":"Synthetic effects reconcile against their fixture"}))).into_response(),
        _ if wired => match state.accounts.writes.permit(Operation::Unsubscribe) {
            Ok(permit) => Some(permit),
            Err(reason) => return (StatusCode::FORBIDDEN, Json(json!({"error":reason}))).into_response(),
        },
        _ => None,
    };
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) { return Err("Wait for census completion".to_owned()); }
        let repo = state.accounts.repository().map_err(|_| "Account unavailable")?;
        let account = repo.load(&request.account).map_err(|_| "Account unavailable")?;
        let store = repo.store(&account).map_err(|_| "Account storage unavailable")?;
        if store.recovery_pending() && !matches!(request.command, Action::Open { .. }) { return Err("Fresh provider evidence and reconciliation required after recovery".into()); }
        let mut saved = store.load_state().map_err(|_| "Account state unavailable")?;
        let ledger_guard = state.accounts.ledger.as_ref().map(|l| l.lock()).transpose()?;
        sync_ledger(&mut saved, &store, &repo.workspace_context(&account, &store).map_err(|e| e.to_string())?, ledger_guard.as_ref())?;
        let mut installed = match &state.accounts.source { AccountSource::Synthetic { mailboxes, .. } if !wired => Some(InstalledSynthetic::open(mailboxes, &store, &saved)?), _ => None };
        let wiring = Wired { trusted_recipients: state.accounts.synthetic(), authserv: state.accounts.writes.authserv(account.provider) };
        let as_of = match request.command {
            Action::Open { as_of } => as_of,
            Action::Preview { input } => {
                let mut preview = unsubscribe::preview(&saved, account.provider, store.keys(), &input, pick(wired, &wiring, &installed))?;
                let (handle, request) = state.accounts.confirmations.lock().expect("confirmations").offer(unsubscribe::confirmation_plan(&preview))?;
                preview.receipt = handle;
                preview.confirmation = Some(request);
                return Ok(json!(preview));
            },
            Action::Confirm { input, receipt, confirmation_action, mail_confirmation_action } => {
                if saved.invoices.sends.iter().any(|s| !s.status.settled()) || saved.backlog.iter().any(|r| !matches!(r.status, crate::backlog::Status::Completed | crate::backlog::Status::Failed)) { return Err("Reconcile pending invoice/backlog work first".into()); }
                let preview = unsubscribe::preview(&saved, account.provider, store.keys(), &input, pick(wired, &wiring, &installed))?;
                if confirmation_action != preview.confirmation_action || preview.local_mail_confirmation.is_some_and(|a| mail_confirmation_action.as_deref() != Some(a)) { return Err("Action changed".into()); }
                let current = unsubscribe::confirmation_plan(&preview);
                let consent = state.accounts.confirmations.lock().expect("confirmations").confirm(&receipt, &current, || {
                    let latest = store.load_state().map_err(|_| "Revalidate unsubscribe state")?;
                    ledger_fresh(&latest, ledger_guard.as_ref())?;
                    unsubscribe::preview(&latest, account.provider, store.keys(), &input, pick(wired, &wiring, &installed)).map(|p| unsubscribe::confirmation_plan(&p))
                })?;
                if let Some(permit) = permit.as_ref() {
                    let execution = PreparedWired::new(&state, permit, account.provider, &preview)?;
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "Invalid clock")?.as_secs() as i64;
                    let ids = unsubscribe::confirm(&mut saved, &preview, consent.receipt(), &confirmation_action, mail_confirmation_action.as_deref(), now)?;
                    store.save_state(&mut saved).map_err(|_| "Persist unsubscribe intent")?;
                    unsubscribe::boundary("before");
                    execution.execute(&repo, &account, &store, &mut saved, &preview, &ids, input.as_of)?;
                    let report = unsubscribe::report(&saved, account.provider, store.keys(), input.as_of, pick(wired, &wiring, &installed))?;
                    return Ok(json!({"report":report,"history":saved.unsubscribe.history,"mutes":saved.unsubscribe.mutes,"synthetic":false,"wired":true,"production_available":false,"q20_scope":"separate_subscriptions"}));
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "Invalid clock")?.as_secs() as i64;
                let ids = unsubscribe::confirm(&mut saved, &preview, consent.receipt(), &confirmation_action, mail_confirmation_action.as_deref(), now)?;
                store.save_state(&mut saved).map_err(|_| "Persist unsubscribe intent")?;
                unsubscribe::boundary("before");
                for (id, action) in ids.iter().zip(preview.actions.iter()) {
                    if matches!(action.method, unsubscribe::Method::OneClick | unsubscribe::Method::LocalMail) {
                        let entry = saved.unsubscribe.history.iter().find(|e| &e.id == id).ok_or("Missing intent")?.clone();
                        let source = unsubscribe::source(&saved, &action.source).ok_or("Source changed")?;
                        let adapter = installed.as_mut().ok_or("No installed fake transport")?;
                        adapter.submit(id, &entry, source, &action.recipient)?;
                        unsubscribe::boundary("effect");
                        unsubscribe::observe(&mut saved, adapter, id)?;
                        store.save_state(&mut saved).map_err(|_| "Persist unsubscribe observation")?;
                    }
                }
                input.as_of
            },
            Action::Resolve { entry, resolution, as_of } => {
                unsubscribe::resolve(&mut saved, &entry, resolution)?;
                store.save_state(&mut saved).map_err(|_| "Persist resolution")?;
                as_of
            },
            Action::Recover { entry, as_of } => {
                unsubscribe::observe(&mut saved, installed.as_ref().ok_or("No installed fake transport")?, &entry)?;
                store.save_state(&mut saved).map_err(|_| "Persist recovery observation")?;
                as_of
            },
        };
        let report = unsubscribe::report(&saved, account.provider, store.keys(), as_of, pick(wired, &wiring, &installed))?;
        Ok::<_, String>(json!({"report":report,"history":saved.unsubscribe.history,"mutes":saved.unsubscribe.mutes,"synthetic":installed.is_some(),"wired":wired,"received":installed.as_ref().map(|a| a.received()),"production_available":false,"q20_scope":"separate_subscriptions"}))
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Unsubscribe review rejected. Reload the exact preview. Reconcile uncertain effects without retrying. User-edited mutes are preserved."}))).into_response(),
    }
}
