use super::*;
use crate::acquisition::recovery::{self, Manifest, RestorePreview};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::server) enum Request {
    Status,
    CapturePreview {
        account: AccountId,
    },
    Capture {
        preview: Manifest,
        receipt: String,
    },
    RestorePreview {
        bundle: String,
    },
    Restore {
        preview: RestorePreview,
        receipt: String,
    },
    Resume {
        account: AccountId,
    },
}
fn bundle_path(root: &Path, id: &str) -> Result<PathBuf, AcquisitionError> {
    if !recovery::valid_digest(id) {
        return Err(AcquisitionError::InvalidInput);
    }
    Ok(root.join("bundles").join(format!("{id}.jwe")))
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
    if !state.accounts.synthetic() {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"Recovery creation and restoration denied before credential/native access. Protected native launch and Keychain implementation are missing.","production_available":false}))).into_response();
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
        let AccountSource::Synthetic { mailboxes, .. } = &state.accounts.source else {return Err(AcquisitionError::InvalidInput);};
        let root = mailboxes.parent().ok_or(AcquisitionError::Storage)?.join("synthetic-recovery");
        if matches!(request, Request::Status) {
            let mut view = repo.recovery_view()?;
            let directory = root.join("bundles");
            let mut bundles = vec![];
            if directory.exists() { for entry in std::fs::read_dir(directory).map_err(|_| AcquisitionError::Storage)? {
                let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()).filter(|s| recovery::valid_digest(s)) { bundles.push(id.to_string()); }
            } }
            bundles.sort(); view["bundles"] = json!(bundles);
            return Ok(view);
        }
        let secret = fake_secret(&root, matches!(request, Request::CapturePreview { .. } | Request::Capture { .. }))?;
        let consent = |plan: crate::confirmation::Plan, receipt: &str| {
            state.accounts.confirmations.lock().expect("confirmations").confirm(receipt, &plan, &mut crate::confirmation::SyntheticBackend, || Ok(plan.clone())).map(|_| ()).map_err(|_| AcquisitionError::InvalidInput)
        };
        match request {
            Request::Status => unreachable!(),
            Request::CapturePreview { account } => {
                let (preview, plan) = repo.capture_preview(&account, recovery::now())?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(plan).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation}))
            },
            Request::Capture { preview, receipt } => {
                let bundle = repo.capture(&preview, &secret, &root.join("bundles"), |plan| consent(plan, &receipt))?;
                Ok(json!({"bundle":bundle,"captured":true,"synthetic":true,"limitations":recovery::LIMITATIONS}))
            },
            Request::RestorePreview { bundle } => {
                let preview = repo.restore_preview(&bundle_path(&root, &bundle)?, &secret)?;
                let (receipt, confirmation) = state.accounts.confirmations.lock().expect("confirmations").offer(recovery::restore_plan(&preview)?).map_err(|_| AcquisitionError::InvalidInput)?;
                Ok(json!({"preview":preview,"receipt":receipt,"confirmation":confirmation,"authenticated":true}))
            },
            Request::Restore { preview, receipt } => {
                let restored = repo.restore(&bundle_path(&root, &preview.bundle)?, &secret, &preview, |plan| consent(plan, &receipt))?;
                *state.accounts.confirmations.lock().expect("confirmations") = crate::confirmation::Boundary::synthetic();
                *state.accounts.job.lock().expect("account job") = None;
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
            Request::Resume { account } => {
                let restored = repo.resume_restore(&account, &secret)?;
                *state.accounts.confirmations.lock().expect("confirmations") = crate::confirmation::Boundary::synthetic();
                Ok(json!({"restored":restored,"local_restore_complete":true,"provider_authority":false}))
            },
        }
    }).await {
        Ok(Ok(value)) => Json(value).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"Recovery rejected. Preserve bundle and separate secret. Refresh the exact preview or resume the interrupted restore. Fresh provider evidence is required; no remote retry or compensation is authorized."}))).into_response(),
    }
}

#[cfg(test)]
#[tokio::test]
async fn production_recovery_denies_before_repository_access() {
    use tower::ServiceExt;
    let state = Arc::new(AppState::new(false));
    for action in [
        "status",
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
