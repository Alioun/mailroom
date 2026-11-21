use super::*;
use crate::acquisition::recovery::{
    self, Manifest, RestorePreview,
    selected::{SelectedManifest, SelectedRestorePreview, selected_restore_plan},
};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::server) enum Request {
    Status,
    SelectedCapturePreview {
        accounts: Vec<AccountId>,
        include_ledger: bool,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    SelectedCapture {
        preview: SelectedManifest,
        receipt: String,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    SelectedRestorePreview {
        bundle: String,
        restore_ledger: bool,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    SelectedRestore {
        preview: SelectedRestorePreview,
        receipt: String,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    SelectedResume {
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    CapturePreview {
        account: AccountId,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    Capture {
        preview: Manifest,
        receipt: String,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    RestorePreview {
        bundle: String,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    Restore {
        preview: RestorePreview,
        receipt: String,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
    Resume {
        account: AccountId,
        #[serde(default)]
        passphrase: Option<SecretValue>,
    },
}
impl Request {
    fn passphrase(&self) -> Option<&SecretValue> {
        match self {
            Self::Status => None,
            Self::SelectedCapturePreview { passphrase, .. }
            | Self::SelectedCapture { passphrase, .. }
            | Self::SelectedRestorePreview { passphrase, .. }
            | Self::SelectedRestore { passphrase, .. }
            | Self::SelectedResume { passphrase }
            | Self::CapturePreview { passphrase, .. }
            | Self::Capture { passphrase, .. }
            | Self::RestorePreview { passphrase, .. }
            | Self::Restore { passphrase, .. }
            | Self::Resume { passphrase, .. } => passphrase.as_ref(),
        }
    }
}
fn derive_recovery_secret(
    passphrase: Option<&SecretValue>,
) -> Result<SecretValue, AcquisitionError> {
    let pass = passphrase.ok_or(AcquisitionError::InvalidInput)?;
    let text = std::str::from_utf8(pass.bytes()).map_err(|_| AcquisitionError::InvalidInput)?;
    if text.chars().count() < 12 || pass.bytes().len() > 1024 || text.chars().any(char::is_control)
    {
        return Err(AcquisitionError::InvalidInput);
    }
    let params =
        argon2::Params::new(65536, 3, 4, Some(32)).map_err(|_| AcquisitionError::Storage)?;
    let mut blocks = zeroize::Zeroizing::new(vec![argon2::Block::default(); params.block_count()]);
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    const SALT: &[u8; 16] = b"mailroom-rec-v1!";
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into_with_memory(pass.bytes(), SALT, key.as_mut(), blocks.as_mut_slice())
        .map_err(|_| AcquisitionError::Storage)?;
    Ok(SecretValue::new(key.to_vec()))
}
fn bundle_path(root: &Path, id: &str) -> Result<PathBuf, AcquisitionError> {
    recovery::bundle_file(&root.join("bundles"), id)
}
fn fake_secret(root: &Path, create: bool) -> Result<SecretValue, AcquisitionError> {
    use rand::RngCore;
    let path = root.join("separately-held-fake-recovery-key");
    if path.exists() {
        let meta = std::fs::symlink_metadata(&path).map_err(|_| AcquisitionError::Storage)?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != 32 {
            return Err(AcquisitionError::Credentials);
        }
        return std::fs::read(path)
            .map(SecretValue::new)
            .map_err(|_| AcquisitionError::Credentials);
    }
    if !create
        || root
            .join("bundles")
            .try_exists()
            .map_err(|_| AcquisitionError::Storage)?
    {
        return Err(AcquisitionError::CredentialMissing);
    }
    std::fs::create_dir_all(root).map_err(|_| AcquisitionError::Storage)?;
    let mut bytes = zeroize::Zeroizing::new(vec![0; 32]);
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut f| {
            f.write_all(&bytes)?;
            f.sync_all()
        })
        .map_err(|_| AcquisitionError::Storage)?;
    Ok(SecretValue::new(bytes.to_vec()))
}
pub(in crate::server) async fn handle(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let is_vault = state.accounts.vault_live();
    if !state.accounts.synthetic() && !is_vault {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Recovery creation and restoration denied before credential/native access. Protected recovery setup and validation are required.","production_available":false}))).into_response();
    }
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match tokio::task::spawn_blocking(move || -> Result<serde_json::Value, AcquisitionError> {
        let status_only = matches!(request, Request::Status);
        let _admission = if status_only { None } else { Some(state.accounts.admission.lock().expect("account admission")) };
        if !status_only && state.accounts.job.lock().expect("account job").as_ref().is_some_and(|j| j.progress.state == ScanState::Running) {return Err(AcquisitionError::LifecyclePending);}
        let _operation = state.accounts.operation.lock().expect("account operation");
        let repo = state.accounts.repository()?;
        let (root, synthetic) = match &state.accounts.source {
            AccountSource::Synthetic { mailboxes, .. } => (mailboxes.parent().ok_or(AcquisitionError::Storage)?.join("synthetic-recovery"), true),
            AccountSource::Vault { .. } => (repo.paths.data.join("recovery"), false),
            _ => return Err(AcquisitionError::InvalidInput),
        };
        if matches!(request, Request::Status) {
            let mut view = repo.recovery_view()?;
            view["selected_pending"] = json!(repo.selected_pending()?);
            view["production_available"] = json!(!synthetic);
            view["limitations"] = json!(recovery::selected::SELECTED_LIMITATIONS);
            let directory = root.join("bundles");
            let mut bundles = vec![];
            if directory.exists() { for entry in std::fs::read_dir(directory).map_err(|_| AcquisitionError::Storage)? {
                let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()).filter(|s| recovery::valid_digest(s)) { bundles.push(id.to_string()); }
            } }
            bundles.sort(); view["bundles"] = json!(bundles);
            return Ok(view);
        }
        let secret = if synthetic {
            fake_secret(&root, matches!(request, Request::CapturePreview { .. } | Request::Capture { .. } | Request::SelectedCapturePreview { .. } | Request::SelectedCapture { .. }))?
        } else {
            derive_recovery_secret(request.passphrase())?
        };
        let consent = |plan: crate::confirmation::Plan, receipt: &str| {
            state.accounts.confirmations.lock().expect("confirmations").confirm(receipt, &plan, || Ok(plan.clone())).map(|_| ()).map_err(|_| AcquisitionError::InvalidInput)
        };
        let reset_boundary = || {
            if synthetic {
                crate::confirmation::Boundary::synthetic()
            } else {
                crate::confirmation::Boundary::browser()
            }
        };
        match request {
            Request::Status => unreachable!(),
            Request::SelectedCapturePreview { accounts, include_ledger, .. } => {
                let (preview, plan) = repo.selected_capture_preview(&accounts, include_ledger, state.accounts.ledger()?, recovery::now())?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(plan).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation}))
            },
            Request::SelectedCapture { preview, receipt, .. } => {
                crate::private_fs::prepare(&root.join("bundles")).map_err(|_| AcquisitionError::Storage)?;
                let bundle = repo.selected_capture(&preview, &secret, &root.join("bundles"), state.accounts.ledger()?, |plan| consent(plan, &receipt))?;
                Ok(json!({"bundle":bundle,"captured":true,"synthetic":synthetic,"limitations":recovery::selected::SELECTED_LIMITATIONS}))
            },
            Request::SelectedRestorePreview { bundle, restore_ledger, .. } => {
                let preview = repo.selected_restore_preview(&bundle_path(&root, &bundle)?, &secret, restore_ledger, state.accounts.ledger()?)?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(selected_restore_plan(&preview)?).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation,"authenticated":true}))
            },
            Request::SelectedRestore { preview, receipt, .. } => {
                let restored = repo.selected_restore(&bundle_path(&root, &preview.bundle)?, &secret, &preview, state.accounts.ledger()?, |plan| consent(plan, &receipt))?;
                *state.accounts.confirmations.lock().expect("confirmations") = reset_boundary();
                *state.accounts.job.lock().expect("account job") = None;
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
            Request::SelectedResume { .. } => {
                let restored = repo.selected_resume_reviewed(&secret, state.accounts.ledger()?, |_, _| Ok(()))?;
                *state.accounts.confirmations.lock().expect("confirmations") = reset_boundary();
                *state.accounts.job.lock().expect("account job") = None;
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
            Request::CapturePreview { account, .. } => {
                let (preview, plan) = repo.capture_preview(&account, recovery::now())?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(plan).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation}))
            },
            Request::Capture { preview, receipt, .. } => {
                crate::private_fs::prepare(&root.join("bundles")).map_err(|_| AcquisitionError::Storage)?;
                let bundle = repo.capture(&preview, &secret, &root.join("bundles"), |plan| consent(plan, &receipt))?;
                Ok(json!({"bundle":bundle,"captured":true,"synthetic":synthetic,"limitations":recovery::LIMITATIONS}))
            },
            Request::RestorePreview { bundle, .. } => {
                let preview = repo.restore_preview(&bundle_path(&root, &bundle)?, &secret)?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(recovery::restore_plan(&preview)?).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation,"authenticated":true}))
            },
            Request::Restore { preview, receipt, .. } => {
                let restored = repo.restore(&bundle_path(&root, &preview.bundle)?, &secret, &preview, |plan| consent(plan, &receipt))?;
                *state.accounts.confirmations.lock().expect("confirmations") = reset_boundary();
                *state.accounts.job.lock().expect("account job") = None;
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
            Request::Resume { account, .. } => {
                let restored = repo.resume_restore(&account, &secret)?;
                *state.accounts.confirmations.lock().expect("confirmations") = reset_boundary();
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
        }
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        result => {
            #[cfg(test)]
            if let Ok(Err(error)) = &result { eprintln!("Synthetic recovery rejection: {error:?}"); }
            let _ = result;
            (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Recovery rejected. Preserve bundle and separate secret. Refresh the exact preview or resume the interrupted restore. Fresh provider evidence is required; no remote retry or compensation is authorized."}))).into_response()
        },
    }
}

#[cfg(test)]
#[tokio::test]
async fn production_recovery_denies_before_repository_access() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(false));
    for action in [
        "status",
        "selected_capture_preview",
        "selected_capture",
        "selected_restore_preview",
        "selected_restore",
        "selected_resume",
        "capture_preview",
        "capture",
        "restore_preview",
        "restore",
        "resume",
    ] {
        let response = super::super::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/accounts/recovery")
                    .header("host", super::super::ADDRESS)
                    .header("content-type", "application/json")
                    .header("x-mailroom-token", state.token.as_str())
                    .body(axum::body::Body::from(json!({"action":action}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            state
                .accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}

#[cfg(test)]
#[test]
fn missing_fake_recovery_key_never_replaces_a_captured_bundle_key() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let original = fake_secret(root, true).unwrap();
    std::fs::create_dir(root.join("bundles")).unwrap();
    std::fs::rename(
        root.join("separately-held-fake-recovery-key"),
        root.join("lost-fake-recovery-key-preserved"),
    )
    .unwrap();
    assert!(matches!(
        fake_secret(root, true),
        Err(AcquisitionError::CredentialMissing)
    ));
    assert!(!root.join("separately-held-fake-recovery-key").exists());
    assert!(
        std::fs::read(root.join("lost-fake-recovery-key-preserved"))
            .unwrap()
            .as_slice()
            == original.bytes()
    );
}

#[cfg(test)]
#[tokio::test]
async fn recovery_status_is_available_while_account_admission_is_closed() {
    let state = Arc::new(AppState::new(true));
    let admission = state.accounts.admission.lock().unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        handle(State(state.clone()), Ok(Json(Request::Status))),
    )
    .await;
    drop(admission);
    assert_eq!(response.unwrap().status(), StatusCode::OK);
}
