use super::*;
use crate::ledger::InstalledLedger;

pub(super) struct Services {
    pub accounts: Arc<Accounts>,
    pub ledger: InstalledLedger,
}

fn eligible(
    core_dumps_disabled: bool,
    hardened: bool,
    helper: Option<&crate::native::Helper>,
) -> bool {
    core_dumps_disabled
        && hardened
        && helper.is_some_and(|h| matches!(h.pin, crate::native::Pin::AdHocCdHash(_)))
}

pub(super) fn installed() -> Result<Option<Services>, AcquisitionError> {
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
    #[cfg(target_os = "macos")]
    {
        // Check the running image before resolving any private paths. A configured pin
        // is a prerequisite, not proof of a successful helper launch or Keychain access.
        let helper = crate::native::installed().ok();
        install_if(
            eligible(
                crate::native::core_dumps_disabled(),
                crate::native::own_process_hardened(),
                helper.as_ref(),
            ),
            || {
                let paths = AccountPaths::per_user()?;
                Ok(Arc::new(Accounts::new(paths, Arc::new(OsCredentials))))
            },
        )
    }
}

fn install_if(
    qualified: bool,
    repository: impl FnOnce() -> Result<Arc<Accounts>, AcquisitionError>,
) -> Result<Option<Services>, AcquisitionError> {
    if !qualified {
        return Ok(None);
    }
    from_repository(repository()?)
        .map(Some)
        .map_err(|_| AcquisitionError::Storage)
}

fn from_repository(accounts: Arc<Accounts>) -> Result<Services, String> {
    let ledger = InstalledLedger::install_under(&accounts.paths.data)?;
    Ok(Services { accounts, ledger })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        os::unix::fs::{PermissionsExt, symlink},
    };
    use tower::ServiceExt;

    const DUMMY: &[u8] = b"LOCAL-SERVICES-DUMMY-CREDENTIAL-7c06";
    const AS_OF: i64 = crate::winddown::synthetic::AS_OF;

    #[test]
    fn private_paths_require_disabled_core_dumps_hardening_and_supported_pin() {
        let helper = crate::native::Helper {
            path: "/unused/dummy-helper".into(),
            pin: crate::native::Pin::AdHocCdHash([7; 20]),
        };
        for core in [false, true] {
            for hardened in [false, true] {
                assert_eq!(eligible(core, hardened, Some(&helper)), core && hardened);
                assert!(!eligible(core, hardened, None));
            }
        }
        let unsupported = crate::native::Helper {
            path: helper.path,
            pin: crate::native::Pin::DeveloperIdRequirement("dummy requirement"),
        };
        assert!(!eligible(true, true, Some(&unsupported)));
    }

    #[tokio::test]
    async fn installation_failure_never_falls_back_to_unchecked_projection() {
        assert!(
            install_if(false, || panic!(
                "unqualified process resolved private paths"
            ))
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            install_if(true, || Err(AcquisitionError::Storage)),
            Err(AcquisitionError::Storage)
        ));
        let f = Fixture::new();
        std::fs::set_permissions(&f.repo.paths.data, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(matches!(
            install_if(true, || Ok(f.repo.clone())),
            Err(AcquisitionError::Storage)
        ));
        assert!(!f.repo.paths.data.join("ledger").exists());
        let reads = f
            .credentials
            .reads
            .load(std::sync::atomic::Ordering::SeqCst);
        let mut state = AppState::new(false);
        state.accounts.source = AccountSource::Unavailable;
        let app = Arc::new(state);
        let (status, _) = post(&app, "proof/preview", json!({"account":f.account.id}), true).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            f.credentials
                .reads
                .load(std::sync::atomic::Ordering::SeqCst),
            reads
        );
        assert!(!app.accounts.local_bookkeeping());
    }

    #[test]
    fn installed_root_is_private_and_fixed_before_ledger_creation() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(InstalledLedger::install_under(&data).is_err());
        assert!(!data.join("ledger").exists());
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = root.path().join("alias");
        symlink(&data, &alias).unwrap();
        assert!(InstalledLedger::install_under(&alias).is_err());
        assert!(!data.join("ledger").exists());
        let ledger = InstalledLedger::install_under(&data).unwrap();
        assert!(!ledger.lock().unwrap().read().unwrap().document.enabled);
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 1);
    }

    #[derive(Default)]
    struct Credentials {
        values: Mutex<BTreeMap<String, Vec<u8>>>,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl acquisition::CredentialBackend for Credentials {
        fn get(&self, key: &str) -> Result<SecretValue, AcquisitionError> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.values
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .map(SecretValue::new)
                .ok_or(AcquisitionError::CredentialMissing)
        }
        fn put(&self, key: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.into(), value.bytes().to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), AcquisitionError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }
    struct Fixture {
        _root: tempfile::TempDir,
        repo: Arc<Accounts>,
        credentials: Arc<Credentials>,
        account: Account,
        other: Account,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let credentials = Arc::new(Credentials::default());
            let repo = Arc::new(Accounts::new(
                AccountPaths {
                    config: root.path().join("config"),
                    data: root.path().join("data"),
                },
                credentials.clone(),
            ));
            let account = repo
                .create(
                    Provider::Gmail,
                    "old@synthetic.example.test".into(),
                    &SecretValue::new(DUMMY.to_vec()),
                )
                .unwrap();
            let other = repo
                .create(
                    Provider::MailboxOrg,
                    "new@synthetic.example.test".into(),
                    &SecretValue::new(DUMMY.to_vec()),
                )
                .unwrap();
            for a in [&account, &other] {
                acquisition::synthetic::scan(&repo, &a.id, Cancellation::default(), |_| {})
                    .unwrap();
                let store = repo.store(a).unwrap();
                let mut saved = store.load_state().unwrap();
                crate::winddown::synthetic::install(&mut saved.census, a.provider);
                let sample: serde_json::Value =
                    serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
                saved.workspace = Some(Workspace {
                    revision: 1,
                    input: WorkspaceInput {
                        documents: serde_json::from_value(sample["documents"].clone()).unwrap(),
                        pins: vec![],
                        as_of: AS_OF,
                        baseline: crate::rules::proof::Baseline::Current,
                    },
                });
                store.save_state(&mut saved).unwrap();
            }
            Self {
                _root: root,
                repo,
                credentials,
                account,
                other,
            }
        }
        fn app(&self) -> Arc<AppState> {
            let mut app = AppState::new(false);
            let services = from_repository(self.repo.clone()).unwrap();
            app.accounts.source = AccountSource::Installed {
                accounts: services.accounts,
            };
            app.accounts.installed_ledger = Some(services.ledger);
            assert!(!app.accounts.synthetic());
            assert!(app.accounts.local_bookkeeping());
            Arc::new(app)
        }
    }
    async fn post(
        app: &Arc<AppState>,
        path: &str,
        body: serde_json::Value,
        token: bool,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/accounts/{path}"))
            .header("Host", super::super::super::ADDRESS)
            .header("Origin", super::super::super::ORIGIN)
            .header("Content-Type", "application/json");
        if token {
            request = request.header("X-Mailroom-Token", app.token.as_str());
        }
        let response = super::super::super::router(app.clone())
            .oneshot(
                request
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap();
        assert!(
            !bytes.windows(DUMMY.len()).any(|w| w == DUMMY),
            "dummy credential in HTTP response"
        );
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn ledger(
        app: &Arc<AppState>,
        account: &AccountId,
        command: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        post(
            app,
            "ledger",
            json!({"account":account,"command":command}),
            true,
        )
        .await
    }
    async fn winddown(
        app: &Arc<AppState>,
        account: &AccountId,
        command: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        post(
            app,
            "winddown",
            json!({"account":account,"as_of":AS_OF,"command":command}),
            true,
        )
        .await
    }

    #[tokio::test]
    async fn installed_ledger_exact_review_reopens_and_invalidates_stale_or_cross_account_receipts()
    {
        let f = Fixture::new();
        let app = f.app();
        let id = &f.account.id;
        let (status, view) = ledger(&app, id, json!({"action":"open"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(view["snapshot"]["document"]["enabled"], false);
        let batch = json!({"content_version":view["snapshot"]["content_version"],"revision":view["revision"],"enabled":true,"entries":[{
            "address":"local@signup.example.test","service":"Private dummy service","folder":"Purchases","first_seen":"2026-10-01","status":"active","note":"private dummy note"
        }]});
        let (status, reviewed) = ledger(&app, id, json!({"action":"preview","batch":batch})).await;
        assert_eq!(status, StatusCode::OK);
        let confirm = json!({"action":"confirm","batch":batch,"receipt":reviewed["receipt"],"confirmed":true});
        assert_eq!(
            ledger(&app, &f.other.id, confirm.clone()).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut altered = confirm.clone();
        altered["batch"]["entries"][0]["note"] = "changed dummy note".into();
        assert_eq!(
            ledger(&app, id, altered).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(ledger(&app, id, confirm.clone()).await.0, StatusCode::OK);
        assert_eq!(
            ledger(&app, id, confirm).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        drop(app);
        let reopened = f.app();
        let (status, saved) = ledger(&reopened, id, json!({"action":"open"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            saved["snapshot"]["document"]["entries"][0]["note"],
            "private dummy note"
        );
        let mut next_batch = batch;
        next_batch["revision"] = saved["revision"].clone();
        next_batch["content_version"] = saved["snapshot"]["content_version"].clone();
        let (_, preview) = ledger(
            &reopened,
            id,
            json!({"action":"preview","batch":next_batch}),
        )
        .await;
        {
            let guard = reopened
                .accounts
                .installed_ledger
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            let snapshot = guard.read().unwrap();
            let mut document = snapshot.document;
            document.entries[0].note = "external private edit".into();
            guard.replace(&snapshot.content_version, &document).unwrap();
        }
        assert_eq!(ledger(&reopened, id, json!({"action":"confirm","batch":next_batch,"receipt":preview["receipt"],"confirmed":true})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
        let cipher = std::fs::read(
            f.repo
                .paths
                .data
                .join(format!("{}.census.enc", id.as_str())),
        )
        .unwrap();
        for text in ["private dummy note", "external private edit"] {
            assert!(
                !cipher.windows(text.len()).any(|w| w == text.as_bytes()),
                "private ledger data in plaintext census"
            );
        }
    }

    #[tokio::test]
    async fn installed_winddown_keeps_unknowns_and_requires_its_own_current_disclosure() {
        let f = Fixture::new();
        let app = f.app();
        let id = &f.account.id;
        let (status, opened) = winddown(&app, id, json!({"action":"open"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(
            opened["report"]["verdict"],
            "evidence_supports_owner_retirement_review"
        );
        assert_ne!(
            opened["report"]["sender_data_version"],
            "synthetic-reviewed-sender-data/v1"
        );
        assert!(
            opened["report"]["sender_data_version"]
                .as_str()
                .unwrap()
                .starts_with("sender-core-v1/")
        );
        let catalog = &opened["report"]["sender_catalog"];
        assert_eq!(catalog["license"], "CC0-1.0");
        assert!(catalog["rows"].as_u64().unwrap() >= 120);
        assert_eq!(catalog["seed_complete"], true);
        assert_eq!(catalog["missing_categories"].as_array().unwrap().len(), 0);
        let change = json!({"kind":"settings","settings":{"winding_down":true,"comparisons":{}}});
        let (status, reviewed) = winddown(
            &app,
            id,
            json!({"action":"preview","version":opened["report"]["version"],"change":change}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(winddown(&app, id, json!({"action":"confirm","version":opened["report"]["version"],"change":change,"receipt":reviewed["receipt"],"confirmed":true})).await.0, StatusCode::OK);
        drop(app);
        let app = f.app();
        let (_, opened) = winddown(&app, id, json!({"action":"open"})).await;
        assert_eq!(opened["report"]["settings"]["winding_down"], true);
        let (status, preview) = winddown(&app, id, json!({"action":"export_preview","version":opened["report"]["version"],"disclosure":"agent_summary"})).await;
        assert_eq!(status, StatusCode::OK);
        let text = preview["text"].as_str().unwrap();
        assert!(!text.contains("@people.example"));
        assert!(!text.contains("@gmail.com"));
        assert_eq!(winddown(&app, id, json!({"action":"export","version":opened["report"]["version"],"disclosure":"agent_summary","receipt":reviewed["receipt"],"confirmed":true})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(winddown(&app, id, json!({"action":"export","version":opened["report"]["version"],"disclosure":"agent_summary","receipt":preview["receipt"],"confirmed":true})).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn installed_local_services_do_not_enable_provider_lifecycle_or_recovery_writes() {
        let f = Fixture::new();
        let app = f.app();
        let reads = f
            .credentials
            .reads
            .load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            post(
                &app,
                "ledger",
                json!({"account":f.account.id,"command":{"action":"open"}}),
                false
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        for (path, command) in [
            (
                "invoices",
                json!({"account":f.account.id,"command":{"action":"preview","selection":"INBOX/1/1","target":"accounting","mode":"smtp"}}),
            ),
            ("lifecycle", json!({"action":"status"})),
            ("recovery", json!({"action":"status"})),
        ] {
            assert_eq!(
                post(&app, path, command, true).await.0,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            f.credentials
                .reads
                .load(std::sync::atomic::Ordering::SeqCst),
            reads
        );
        assert_eq!(
            app.accounts
                .repository_accesses
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
    #[tokio::test]
    async fn missing_dummy_keys_never_recreate_state_or_expose_credentials() {
        let f = Fixture::new();
        let app = f.app();
        let store = f.repo.store(&f.account).unwrap();
        let path = store.path();
        let before = std::fs::read(&path).unwrap();
        drop(store);
        f.credentials.values.lock().unwrap().clear();
        assert_eq!(
            ledger(&app, &f.account.id, json!({"action":"open"}))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            winddown(&app, &f.account.id, json!({"action":"open"}))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(f.credentials.values.lock().unwrap().is_empty());
        assert!(std::fs::read(path).unwrap() == before);
    }

    #[tokio::test]
    async fn installed_reports_and_ledger_preserve_the_restored_account_hold() {
        let f = Fixture::new();
        let separate_dummy_secret = SecretValue::new(vec![0x71; 32]);
        let (manifest, _) = f.repo.capture_preview(&f.account.id, AS_OF as u64).unwrap();
        let directory = f._root.path().join("dummy-bundles");
        let bundle = f
            .repo
            .capture(&manifest, &separate_dummy_secret, &directory, |_| Ok(()))
            .unwrap();
        let path = acquisition::recovery::bundle_file(&directory, &bundle).unwrap();
        let fresh_root = tempfile::tempdir().unwrap();
        let fresh = Arc::new(acquisition::synthetic::accounts(fresh_root.path()));
        let preview = fresh
            .restore_preview(&path, &separate_dummy_secret)
            .unwrap();
        fresh
            .restore(&path, &separate_dummy_secret, &preview, |_| Ok(()))
            .unwrap();
        let local = from_repository(fresh.clone()).unwrap();
        let mut state = AppState::new(false);
        state.accounts.source = AccountSource::Installed {
            accounts: local.accounts,
        };
        state.accounts.installed_ledger = Some(local.ledger);
        let app = Arc::new(state);
        let store = fresh.store(&fresh.load(&f.account.id).unwrap()).unwrap();
        let before = std::fs::read(store.path()).unwrap();
        assert!(store.recovery_pending());
        drop(store);
        for (status, response) in [
            ledger(&app, &f.account.id, json!({"action":"open"})).await,
            winddown(&app, &f.account.id, json!({"action":"open"})).await,
        ] {
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(
                response["error"],
                "Fresh provider evidence and reconciliation required after recovery"
            );
        }
        let store = fresh.store(&fresh.load(&f.account.id).unwrap()).unwrap();
        assert!(store.recovery_pending());
        assert!(std::fs::read(store.path()).unwrap() == before);
    }
}
