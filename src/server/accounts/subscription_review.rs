use super::*;
use crate::{
    census::{AccountState, EncryptedStore},
    unsubscribe::{
        self, Folders, InstalledSynthetic, Method, Preview, ReviewInput, Selection, Wired,
    },
};
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;
const CONFIRM: &str = "Confirm exactly this list of separate subscription actions (synthetic only)";
const WIRED_CONFIRM: &str = "Confirm exactly this list of separate subscription actions";
fn confirmation_action(state: &AppState) -> &'static str {
    if super::unsubscribe::wired(state) {
        WIRED_CONFIRM
    } else {
        CONFIRM
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    account: AccountId,
    subscription: String,
    method: Method,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Input {
    as_of: i64,
    selections: Vec<Target>,
    folders: Folders,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {
        as_of: i64,
    },
    Preview {
        input: Input,
    },
    Confirm {
        input: Input,
        receipt: String,
        confirmation_action: String,
        mail_confirmation_action: Option<String>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Request {
    command: Action,
}
struct AccountReview {
    account: Account,
    store: EncryptedStore,
    saved: AccountState,
    installed: Option<InstalledSynthetic>,
    preview: Preview,
}
fn confirmation_plan(reviews: &[AccountReview], receipt: &str) -> crate::confirmation::Plan {
    crate::confirmation::Plan {
        exact_receipt: receipt.into(),
        actions: reviews
            .iter()
            .flat_map(|r| unsubscribe::confirmation_plan(&r.preview).actions)
            .collect(),
    }
}
fn revalidate(
    state: &AppState,
    reviews: &[AccountReview],
    input: &Input,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<crate::confirmation::Plan, String> {
    let mut previews = Vec::new();
    for r in reviews {
        let saved = r
            .store
            .load_state()
            .map_err(|_| "Revalidate subscription state")?;
        ledger_fresh(&saved, guard)?;
        let single = ReviewInput {
            as_of: input.as_of,
            selections: input
                .selections
                .iter()
                .filter(|t| t.account == r.account.id)
                .map(|t| Selection {
                    subscription: t.subscription.clone(),
                    method: t.method.clone(),
                })
                .collect(),
            folders: input.folders.clone(),
        };
        let wiring = Wired {
            trusted_recipients: state.accounts.synthetic(),
            authserv: state.accounts.writes.authserv(r.account.provider),
        };
        previews.push((
            &r.account.id,
            unsubscribe::preview(
                &saved,
                r.account.provider,
                r.store.keys(),
                &single,
                super::unsubscribe::pick(super::unsubscribe::wired(state), &wiring, &r.installed),
            )?,
        ));
    }
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&("separate-subscriptions-v1", input, &previews))
            .map_err(|_| "Review encoding")?,
    );
    Ok(crate::confirmation::Plan {
        exact_receipt: reviews[0]
            .store
            .keys()
            .binding_for(b"mailroom/unsubscribe-list-confirm/v1", &payload),
        actions: previews
            .iter()
            .flat_map(|(_, p)| unsubscribe::confirmation_plan(p).actions)
            .collect(),
    })
}
fn prepare(
    state: &AppState,
    input: &Input,
    guard: Option<&crate::ledger::file::LockedLedger<'_>>,
) -> Result<(Vec<AccountReview>, String), String> {
    if input.selections.is_empty() || input.selections.len() > 64 {
        return Err("Choose 1..64 separate subscriptions".into());
    }
    let mut selected: BTreeMap<String, Vec<Selection>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for t in &input.selections {
        if !seen.insert((t.account.as_str(), &t.subscription)) {
            return Err("Duplicate exact subscription".into());
        }
        selected
            .entry(t.account.as_str().into())
            .or_default()
            .push(Selection {
                subscription: t.subscription.clone(),
                method: t.method.clone(),
            });
    }
    let repo = state
        .accounts
        .repository()
        .map_err(|_| "Account repository unavailable")?;
    let wired = super::unsubscribe::wired(state);
    let mut reviews = Vec::new();
    for (id, selections) in selected {
        let account = repo
            .load(&AccountId::new(&id).map_err(|_| "Invalid account")?)
            .map_err(|_| "Account unavailable")?;
        let store = repo
            .store(&account)
            .map_err(|_| "Account storage unavailable")?;
        if store.recovery_pending() {
            return Err(
                "Fresh provider evidence and reconciliation required after recovery".into(),
            );
        }
        let mut saved = store.load_state().map_err(|_| "Census unavailable")?;
        sync_ledger(
            &mut saved,
            &store,
            &repo
                .workspace_context(&account, &store)
                .map_err(|e| e.to_string())?,
            guard,
        )?;
        let installed = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } if !wired => {
                Some(InstalledSynthetic::open(mailboxes, &store, &saved)?)
            }
            _ => None,
        };
        let wiring = Wired {
            trusted_recipients: state.accounts.synthetic(),
            authserv: state.accounts.writes.authserv(account.provider),
        };
        let single = ReviewInput {
            as_of: input.as_of,
            selections,
            folders: input.folders.clone(),
        };
        let preview = unsubscribe::preview(
            &saved,
            account.provider,
            store.keys(),
            &single,
            super::unsubscribe::pick(wired, &wiring, &installed),
        )?;
        reviews.push(AccountReview {
            account,
            store,
            saved,
            installed,
            preview,
        });
    }
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(
            "separate-subscriptions-v1",
            input,
            reviews
                .iter()
                .map(|r| (&r.account.id, &r.preview))
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| "Review encoding")?,
    );
    let receipt = reviews[0]
        .store
        .keys()
        .binding_for(b"mailroom/unsubscribe-list-confirm/v1", &payload);
    Ok((reviews, receipt))
}
fn view(state: &AppState, as_of: i64) -> Result<serde_json::Value, String> {
    let wired = super::unsubscribe::wired(state);
    let repo = state
        .accounts
        .repository()
        .map_err(|_| "Account repository unavailable")?;
    let mut accounts = Vec::new();
    for account in repo.list().map_err(|_| "Accounts unavailable")? {
        let store = repo
            .store(&account)
            .map_err(|_| "Account storage unavailable")?;
        if !store.path().exists() {
            continue;
        }
        if store.recovery_pending() {
            return Err(
                "Fresh provider evidence and reconciliation required after recovery".into(),
            );
        }
        let mut saved = store.load_state().map_err(|_| "Census unavailable")?;
        let snapshot = state
            .accounts
            .ledger
            .as_ref()
            .map(|l| l.lock())
            .transpose()?;
        sync_ledger(
            &mut saved,
            &store,
            &repo
                .workspace_context(&account, &store)
                .map_err(|e| e.to_string())?,
            snapshot.as_ref(),
        )?;
        drop(snapshot);
        if saved.census.folders.is_empty() {
            continue;
        }
        let installed = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } if !wired => {
                Some(InstalledSynthetic::open(mailboxes, &store, &saved)?)
            }
            _ => None,
        };
        let wiring = Wired {
            trusted_recipients: state.accounts.synthetic(),
            authserv: state.accounts.writes.authserv(account.provider),
        };
        let report = unsubscribe::report(
            &saved,
            account.provider,
            store.keys(),
            as_of,
            super::unsubscribe::pick(wired, &wiring, &installed),
        )?;
        accounts.push(json!({"account":account.id,"report":report,"history":saved.unsubscribe.history,"mutes":saved.unsubscribe.mutes,"synthetic":installed.is_some(),"wired":wired,"production_available":false,"q20_scope":"separate_subscriptions"}));
    }
    Ok(
        json!({"accounts":accounts,"synthetic":!wired,"review_available":!wired || state.accounts.writes.permit(Operation::Unsubscribe).is_ok(),"q20_scope":"separate_subscriptions"}),
    )
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let permit =
        if super::unsubscribe::wired(&state) && !matches!(request.command, Action::Open { .. }) {
            match state.accounts.writes.permit(Operation::Unsubscribe) {
                Ok(permit) => Some(permit),
                Err(reason) => {
                    return (StatusCode::FORBIDDEN, Json(json!({"error":reason}))).into_response();
                }
            }
        } else {
            None
        };
    match tokio::task::spawn_blocking(move || {
        let _operation=state.accounts.operation.lock().expect("account operation");
        if state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j|j.progress.state==ScanState::Running) {return Err("Wait for census completion".to_owned());}
        let ledger_guard = state.accounts.ledger.as_ref().map(|l| l.lock()).transpose()?;
        let as_of=match request.command {
            Action::Open {as_of}=>as_of,
            Action::Preview {input}=>{
                let (reviews,receipt)=prepare(&state,&input,ledger_guard.as_ref())?;
                let mail=reviews.iter().find_map(|r|r.preview.local_mail_confirmation);
                let (handle,request)=state.accounts.confirmations.lock().expect("confirmations").offer(confirmation_plan(&reviews,&receipt))?;
                return Ok(json!({"previews":reviews.iter().map(|r|&r.preview).collect::<Vec<_>>(),"receipt":handle,"confirmation_action":confirmation_action(&state),"local_mail_confirmation":mail,"confirmation":request}));
            },
            Action::Confirm {input,receipt,confirmation_action,mail_confirmation_action}=>{
                let (mut reviews,current)=prepare(&state,&input,ledger_guard.as_ref())?;
                if confirmation_action!=self::confirmation_action(&state) || reviews.iter().any(|r|r.preview.local_mail_confirmation.is_some_and(|a|mail_confirmation_action.as_deref()!=Some(a))) {return Err("Exact multi-account subscription list changed".into());}
                let current_plan=confirmation_plan(&reviews,&current);
                let consent=state.accounts.confirmations.lock().expect("confirmations").confirm(&receipt,&current_plan,|| {
                    revalidate(&state,&reviews,&input,ledger_guard.as_ref())
                })?;
                if consent.receipt().as_bytes().ct_eq(current.as_bytes()).unwrap_u8()!=1 {return Err("Exact subscription receipt changed".into());}
                let executions = reviews.iter().map(|r| {
                    permit.as_ref().map(|p| super::unsubscribe::PreparedWired::new(&state, p, r.account.provider, &r.preview)).transpose()
                }).collect::<Result<Vec<_>, _>>()?;
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "Invalid clock")?.as_secs() as i64;
                let mut intents=Vec::new();
                for r in &mut reviews {
                    if r.saved.invoices.sends.iter().any(|s| !s.status.settled()) || r.saved.backlog.iter().any(|b|!matches!(b.status,crate::backlog::Status::Completed|crate::backlog::Status::Failed)) {return Err("Reconcile pending account work first".into());}
                    let ids=unsubscribe::confirm(&mut r.saved,&r.preview,&r.preview.receipt,r.preview.confirmation_action,mail_confirmation_action.as_deref(),now)?;
                    intents.push(ids);
                }
                for r in &mut reviews { r.store.save_state(&mut r.saved).map_err(|_| "Persist exact subscription intents")?; }
                unsubscribe::boundary("before");
                let repo = state.accounts.repository().map_err(|_| "Account repository unavailable")?;
                for ((r,ids),execution) in reviews.iter_mut().zip(intents).zip(executions) {
                    if let Some(execution) = execution {
                        execution.execute(&repo, &r.account, &r.store, &mut r.saved, &r.preview, &ids, input.as_of)?;
                        continue;
                    }
                    for (id,action) in ids.iter().zip(&r.preview.actions) {
                        if matches!(action.method,Method::OneClick|Method::LocalMail) {
                            let entry=r.saved.unsubscribe.history.iter().find(|e|&e.id==id).ok_or("Missing intent")?;
                            let source=unsubscribe::source(&r.saved,&action.source).ok_or("Source changed")?;
                            let installed = r.installed.as_mut().ok_or("No installed fake transport")?;
                            installed.submit(id,entry,source,&action.recipient)?;
                            unsubscribe::boundary("effect");
                            unsubscribe::observe(&mut r.saved,installed,id)?;
                            r.store.save_state(&mut r.saved).map_err(|_| "Persist effect observation")?;
                        }
                    }
                }
                input.as_of
            },
        };
        drop(ledger_guard);
        view(&state,as_of)
    }).await {
        Ok(Ok(value))=>Json(value).into_response(),
        _=>(StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"error":"Separate subscription review rejected. Refresh the exact list. No additional subscription is authorized; reconcile uncertain intents without retry."}))).into_response(),
    }
}
