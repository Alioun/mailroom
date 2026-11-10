//! The local web setup: the browser creates and unlocks the vault, finishes an interrupted
//! account creation, and retries or corrects a connection the provider refused. Every
//! account route reaches credentials through `repository()`, which refuses while locked.
use super::*;
use crate::acquisition::{
    lifecycle::LifecycleState,
    vault::{Vault, VaultError},
};

impl AccountController {
    /// Opens the vault, which takes the process lease, before any other file is touched.
    pub(in crate::server) fn local_vault() -> Result<Self, VaultError> {
        let paths = AccountPaths::local_web().map_err(|_| VaultError::Storage)?;
        let vault = Arc::new(Vault::open(&paths.config)?);
        Self::with_vault(Accounts::new(paths, vault.clone()), vault)
    }
    fn with_vault(accounts: Accounts, vault: Arc<Vault>) -> Result<Self, VaultError> {
        crate::private_fs::prepare(&accounts.paths.data).map_err(|_| VaultError::Unsafe)?;
        let ledger = crate::ledger::InstalledLedger::install_under(&accounts.paths.data)
            .map_err(|_| VaultError::Storage)?;
        let accounts = Arc::new(accounts);
        Ok(Self::assemble(
            AccountSource::Vault { accounts, vault },
            Some(ledger),
        ))
    }
}

pub(super) fn connect_allowed() -> Result<(), AcquisitionError> {
    if policy::authorize(Operation::Connect).allowed {
        Ok(())
    } else {
        Err(AcquisitionError::InvalidInput)
    }
}

/// Accounts whose creation stopped before it finished, for the browser to finish or clean up.
pub(super) fn interrupted(accounts: &AccountController) -> Vec<AccountId> {
    if accounts.vault().is_none() || accounts.vault_locked() {
        return vec![];
    }
    let Ok(repo) = accounts.repository() else {
        return vec![];
    };
    repo.recovery_status()
        .map(|pending| {
            pending
                .into_iter()
                .filter(|r| matches!(r.state, LifecycleState::Creating))
                .map(|r| r.account)
                .collect()
        })
        .unwrap_or_default()
}

fn not_local_web() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(
            json!({"error":"This server does not run the local web setup.","code":"not_local_web"}),
        ),
    )
        .into_response()
}

fn refused(error: VaultError) -> Response {
    let (status, message) = match error {
        VaultError::Busy => (
            StatusCode::TOO_MANY_REQUESTS,
            "Another vault action is running. Wait for it to finish, then retry.",
        ),
        VaultError::Exists => (
            StatusCode::CONFLICT,
            "A Mailroom vault already exists. Unlock it instead.",
        ),
        VaultError::Absent => (
            StatusCode::CONFLICT,
            "No Mailroom vault exists yet. Create one first.",
        ),
        VaultError::InUse => (
            StatusCode::CONFLICT,
            "Another Mailroom process holds the vault.",
        ),
        VaultError::UnlockFailed => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "The master password is wrong, or the vault file was changed. Nothing was unlocked.",
        ),
        VaultError::WeakPassword => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Use a master password of at least 12 characters, without control characters.",
        ),
        VaultError::Mismatch => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "The two master passwords differ. Type the same password twice.",
        ),
        VaultError::Unavailable | VaultError::Unsafe | VaultError::Storage => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The vault file is unsafe, damaged or unreadable. Mailroom left it unchanged.",
        ),
    };
    (
        status,
        Json(json!({"error": message, "code": error.code()})),
    )
        .into_response()
}

async fn transition(
    state: Arc<AppState>,
    run: impl FnOnce(&Vault) -> Result<(), VaultError> + Send + 'static,
) -> Response {
    let Some(vault) = state.accounts.vault().cloned() else {
        return not_local_web();
    };
    match tokio::task::spawn_blocking(move || run(&vault)).await {
        Ok(Ok(())) => status(State(state)).await,
        Ok(Err(error)) => refused(error),
        Err(_) => refused(VaultError::Storage),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct CreateRequest {
    password: SecretValue,
    confirmation: SecretValue,
}
pub(in crate::server) async fn create(
    State(state): State<Arc<AppState>>,
    body: Result<Json<CreateRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    transition(state, move |vault| {
        vault.create(&request.password, &request.confirmation)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct UnlockRequest {
    password: SecretValue,
}
pub(in crate::server) async fn unlock(
    State(state): State<Arc<AppState>>,
    body: Result<Json<UnlockRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    transition(state, move |vault| vault.unlock(&request.password)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ReconnectRequest {
    account: AccountId,
    #[serde(default)]
    password: Option<SecretValue>,
}
/// Probes a saved account again, after replacing its app password when the browser sent one.
pub(in crate::server) async fn reconnect(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ReconnectRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if connect_allowed().is_err() {
        return failure(AcquisitionError::InvalidInput);
    }
    launch_connection(state, move |repo| {
        if let Some(password) = &request.password {
            repo.replace_password_reviewed(&request.account, password, |_, _, _| {
                connect_allowed()
            })?;
        }
        repo.load(&request.account)
    })
    .await
}

/// Finishes or cleans up one interrupted account creation, and nothing else.
pub(in crate::server) async fn finish_interrupted(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let worker = state.clone();
    match tokio::task::spawn_blocking(move || {
        let _admission = worker.accounts.admission.lock().expect("account admission");
        let _operation = worker.accounts.operation.lock().expect("account operation");
        let repo = worker.accounts.repository()?;
        repo.recover_reviewed(&request.account, &repo.paths.data, |_, pending, _| {
            if matches!(pending, LifecycleState::Creating) {
                Ok(())
            } else {
                Err(AcquisitionError::LifecyclePending)
            }
        })
    })
    .await
    {
        Ok(Ok(())) => status(State(state)).await,
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[cfg(test)]
mod tests;
