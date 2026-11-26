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
    fn with_vault(mut accounts: Accounts, vault: Arc<Vault>) -> Result<Self, VaultError> {
        crate::private_fs::prepare(&accounts.paths.data).map_err(|_| VaultError::Unsafe)?;
        accounts.live_transports = true;
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
        Ok(Ok(())) => {
            state.accounts.touch_activity();
            state.accounts.notify_change();
            status(State(state)).await
        }
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
    state.accounts.set_live_writes(false);
    transition(state, move |vault| vault.unlock(&request.password)).await
}

pub(in crate::server) async fn lock(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmptyRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    if let Err(e) = input(body) {
        return e;
    }
    state.accounts.set_live_writes(false);
    transition(state, |vault| vault.lock()).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ChangePasswordRequest {
    current: SecretValue,
    password: SecretValue,
    confirmation: SecretValue,
}

pub(in crate::server) async fn change_password(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ChangePasswordRequest>, JsonRejection>,
) -> Response {
    if state.accounts.vault().is_none() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    transition(state, move |vault| {
        vault.change_password(&request.current, &request.password, &request.confirmation)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct WriteModeRequest {
    live_writes: bool,
    #[serde(default)]
    confirmed: bool,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

pub(in crate::server) async fn write_mode(
    State(state): State<Arc<AppState>>,
    body: Result<Json<WriteModeRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Some(secs) = request.timeout_secs {
        if !valid_timeout_secs(secs) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "Choose an auto-disarm timeout between 1 minute and 24 hours, or 0 to disable."})),
            )
                .into_response();
        }
    }
    if !request.live_writes {
        state.accounts.set_live_writes(false);
        return status(State(state)).await;
    }
    if !request.confirmed {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "Confirm explicitly before arming live provider writes."})),
        )
            .into_response();
    }
    if state.accounts.vault_locked() {
        return (
            StatusCode::LOCKED,
            Json(json!({"error": "Unlock the local vault before arming live provider writes.", "code": "vault_locked"})),
        )
            .into_response();
    }
    if !state.accounts.vault_live() && !state.accounts.synthetic() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Live browser write mode requires the local vault or synthetic mode."})),
        )
            .into_response();
    }
    state.accounts.arm_live_writes(true, request.timeout_secs);
    status(State(state)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct SafetySettingsRequest {
    #[serde(default)]
    live_writes_timeout_secs: Option<u64>,
    #[serde(default)]
    vault_idle_timeout_secs: Option<u64>,
}

pub(in crate::server) async fn safety_settings(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SafetySettingsRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if request
        .live_writes_timeout_secs
        .is_some_and(|secs| !valid_timeout_secs(secs))
        || request
            .vault_idle_timeout_secs
            .is_some_and(|secs| !valid_timeout_secs(secs))
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "Timeouts must be 0 (off) or between 60 and 86400 seconds."})),
        )
            .into_response();
    }
    state.accounts.set_safety_timeouts(
        request.live_writes_timeout_secs,
        request.vault_idle_timeout_secs,
    );
    status(State(state)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct PickFileRequest {
    kind: String,
}

#[cfg(target_os = "macos")]
static PICKER_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opens the native macOS file picker for Takeout `.mbox` selection on explicit user click.
pub(in crate::server) async fn pick_file(
    State(state): State<Arc<AppState>>,
    body: Result<Json<PickFileRequest>, JsonRejection>,
) -> Response {
    if !state.accounts.vault_live() && !state.accounts.synthetic() {
        return not_local_web();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if request.kind != "takeout_mbox" {
        return failure(AcquisitionError::InvalidInput);
    }
    if std::env::var_os("MAILROOM_DISABLE_NATIVE_PICKER").is_some() {
        return Json(json!({"supported": false, "path": null})).into_response();
    }
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::Ordering;
        if PICKER_OPEN.swap(true, Ordering::AcqRel) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "A file picker dialog is already open."})),
            )
                .into_response();
        }
        struct PickerGuard;
        impl Drop for PickerGuard {
            fn drop(&mut self) {
                PICKER_OPEN.store(false, Ordering::Release);
            }
        }
        let outcome = tokio::task::spawn_blocking(|| {
            let _guard = PickerGuard;
            let mut child = std::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg("POSIX path of (choose file with prompt \"Select an extracted Google Takeout .mbox file\")")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            loop {
                match child.try_wait()? {
                    Some(_) => return child.wait_with_output(),
                    None if std::time::Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "file picker timed out",
                        ));
                    }
                    None => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
        })
        .await;
        match outcome {
            Ok(Ok(output)) if output.status.success() => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if path.is_empty() {
                    Json(json!({"supported": true, "cancelled": true, "path": null})).into_response()
                } else {
                    Json(json!({"supported": true, "cancelled": false, "path": path})).into_response()
                }
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("(-128)") || stderr.contains("User canceled") {
                    Json(json!({"supported": true, "cancelled": true, "path": null})).into_response()
                } else {
                    Json(json!({"supported": false, "cancelled": false, "path": null}))
                        .into_response()
                }
            }
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({"error": "File picker timed out. Try again or paste the .mbox path manually."})),
            )
                .into_response(),
            _ => Json(json!({"supported": false, "path": null})).into_response(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Json(json!({"supported": false, "path": null})).into_response()
    }
}

fn expand_takeout_path(raw: &str) -> Result<std::path::PathBuf, (&'static str, &'static str)> {
    let mut trimmed = raw.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
    if trimmed.is_empty() || trimmed.len() > 2048 || trimmed.contains('\0') {
        return Err(("Enter a path to an extracted .mbox file.", "invalid_path"));
    }
    let resolved = if trimmed == "~" {
        let Some(home) = std::env::var_os("HOME") else {
            return Err((
                "Could not resolve '~'. Enter the full absolute path to the .mbox file.",
                "invalid_path",
            ));
        };
        std::path::PathBuf::from(home)
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let Some(home) = std::env::var_os("HOME") else {
            return Err((
                "Could not resolve '~/'. Enter the full absolute path to the .mbox file.",
                "invalid_path",
            ));
        };
        std::path::PathBuf::from(home).join(rest)
    } else {
        std::path::PathBuf::from(trimmed)
    };
    let lower = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if lower.ends_with(".zip") || lower.ends_with(".tgz") || lower.ends_with(".gz") {
        return Err((
            "That looks like a compressed archive (.zip/.gz). Extract it first and pass the .mbox file inside.",
            "archive_file",
        ));
    }
    match std::fs::symlink_metadata(&resolved) {
        Ok(meta) if meta.file_type().is_symlink() => Err((
            "Symlinks are not accepted for Takeout import. Pass the real .mbox file path.",
            "symlink",
        )),
        Ok(meta) if meta.is_dir() => Err((
            "That path is a folder, not an .mbox file. Pass the .mbox file inside it.",
            "is_directory",
        )),
        Ok(meta) if !meta.is_file() => Err((
            "That path is not a regular file. Pass an extracted .mbox file.",
            "invalid_path",
        )),
        Ok(_) => Ok(resolved),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err((
            "File not found at that path. Check the path and try again.",
            "not_found",
        )),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Err((
            "Mailroom does not have permission to read that file.",
            "permission_denied",
        )),
        Err(_) => Err((
            "Could not access that path. Check the path and try again.",
            "not_found",
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct TakeoutRequest {
    account: AccountId,
    path: String,
}

pub(in crate::server) async fn takeout(
    State(state): State<Arc<AppState>>,
    body: Result<Json<TakeoutRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let resolved = match expand_takeout_path(&request.path) {
        Ok(path) => path,
        Err((message, code)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": message, "code": code})),
            )
                .into_response();
        }
    };
    let worker = state.clone();
    match tokio::task::spawn_blocking(move || {
        let _admission = worker.accounts.admission.lock().expect("account admission");
        if worker.accounts.job_running() {
            return Err((
                "Wait for the active scan or account operation to finish before importing Takeout.",
                "scan_running",
            ));
        }
        let _operation = worker.accounts.operation.lock().expect("account operation");
        let repo = worker
            .accounts
            .repository()
            .map_err(|_| ("Account repository unavailable.", "storage"))?;
        repo.import_takeout(&request.account, &resolved).map_err(|e| match e {
            AcquisitionError::LifecyclePending => (
                "Finish pending account recovery before importing Takeout.",
                "recovery_pending",
            ),
            AcquisitionError::InvalidInput => (
                "That file does not look like a valid .mbox export (no RFC 4155 'From ' message headers found).",
                "not_mbox",
            ),
            _ => (
                "Could not import that Takeout file. Check that the file is readable and try again.",
                "storage",
            ),
        })
    })
    .await
    {
        Ok(Ok(summary)) => Json(json!({"summary": summary})).into_response(),
        Ok(Err((message, code))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": message, "code": code})),
        )
            .into_response(),
        Err(_) => failure(AcquisitionError::Storage),
    }
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
