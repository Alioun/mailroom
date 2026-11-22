use super::super::AppState;
use super::local;
use crate::{
    acquisition::{
        self, Account, AccountPaths, Accounts, AcquisitionError, Cancellation, OsCredentials,
        ScanProgress, vault::Vault,
    },
    census::{AccountId, ScanState},
};
use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) const DEFAULT_LIVE_WRITES_TIMEOUT_SECS: u64 = 15 * 60;
pub(super) const MIN_TIMEOUT_SECS: u64 = 60;
pub(super) const MAX_TIMEOUT_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub(super) struct SafetyState {
    pub(super) live_writes: bool,
    pub(super) live_writes_armed_at: Option<Instant>,
    pub(super) live_writes_timeout_secs: u64,
    pub(super) vault_idle_timeout_secs: u64,
    pub(super) last_activity: Instant,
}

impl Default for SafetyState {
    fn default() -> Self {
        Self {
            live_writes: false,
            live_writes_armed_at: None,
            live_writes_timeout_secs: DEFAULT_LIVE_WRITES_TIMEOUT_SECS,
            vault_idle_timeout_secs: 0,
            last_activity: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct SafetySnapshot {
    pub(super) live_writes: bool,
    pub(super) live_writes_expires_in_secs: Option<u64>,
    pub(super) live_writes_timeout_secs: u64,
    pub(super) vault_idle_timeout_secs: u64,
    pub(super) vault_locks_in_secs: Option<u64>,
}

pub(in crate::server) struct AccountController {
    #[cfg(test)]
    pub(super) assistant_cli: Option<crate::agent::t3::Client>,
    pub(super) source: AccountSource,
    pub(super) job: Mutex<Option<Job>>,
    pub(super) operation: Mutex<()>,
    pub(super) admission: Mutex<()>,
    pub(super) worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Read only through `ledger()`, which refuses while the vault is locked.
    pub(super) installed_ledger: Option<crate::ledger::InstalledLedger>,
    pub(super) confirmations: Mutex<crate::confirmation::Boundary>,
    /// The real send, one-click and mail-app adapters, each behind a write permit.
    pub(super) writes: crate::send::wiring::Endpoints,
    /// Session-only safety toggle & inactivity timeouts. Always starts disarmed.
    safety: Mutex<SafetyState>,
    /// Monotonic event sequence for instant UI updates via `/api/accounts/events`.
    events: tokio::sync::watch::Sender<u64>,
    #[cfg(test)]
    pub(super) repository_accesses: std::sync::atomic::AtomicUsize,
}

pub(super) enum AccountSource {
    Os,
    Unavailable,
    Installed {
        accounts: Arc<Accounts>,
    },
    Synthetic {
        accounts: Arc<Accounts>,
        _directory: tempfile::TempDir,
        mailboxes: std::path::PathBuf,
    },
    /// The browser setup: its own roots, every credential in the vault.
    Vault {
        accounts: Arc<Accounts>,
        vault: Arc<Vault>,
    },
}

#[derive(Clone, Serialize)]
pub(super) struct Job {
    pub(super) operation: &'static str,
    #[serde(skip)]
    pub(super) backlog_account: Option<AccountId>,
    #[serde(skip)]
    pub(super) folder_account: Option<AccountId>,
    #[serde(skip)]
    pub(super) sieve_account: Option<AccountId>,
    pub(super) account: Option<Account>,
    pub(super) progress: ScanProgress,
    pub(super) error: Option<AcquisitionError>,
    #[serde(skip)]
    pub(super) cancel: Cancellation,
}

/// Asks a person through the confirmation boundary before a previewed mailbox change runs.
/// The repository then recomputes the plan and requires this exact receipt.
pub(super) fn mailbox_consent(
    state: &AppState,
    account: &AccountId,
    operation: &str,
    receipt: &str,
) -> Result<(), AcquisitionError> {
    let plan = crate::confirmation::Plan {
        actions: vec![crate::confirmation::ExactAction::MailboxChange {
            account: account.as_str().into(),
            operation: operation.into(),
            plan_receipt: receipt.into(),
        }],
        exact_receipt: receipt.into(),
    };
    let mut boundary = state.accounts.confirmations.lock().expect("confirmations");
    let (handle, _) = boundary
        .offer(plan.clone())
        .map_err(|_| AcquisitionError::InvalidInput)?;
    boundary
        .confirm(&handle, &plan, || Ok(plan.clone()))
        .map(|_| ())
        .map_err(|_| AcquisitionError::InvalidInput)
}

impl AccountController {
    pub fn new(synthetic: bool) -> Self {
        let mut installed_ledger = None;
        let source = if synthetic {
            let directory = tempfile::tempdir().expect("synthetic account directory");
            AccountSource::Synthetic {
                accounts: Arc::new(acquisition::synthetic::accounts(directory.path())),
                mailboxes: directory.path().join("mailboxes"),
                _directory: directory,
            }
        } else {
            match local::installed() {
                Ok(Some(local::Services { accounts, ledger })) => {
                    installed_ledger = Some(ledger);
                    AccountSource::Installed { accounts }
                }
                Ok(None) => AccountSource::Os,
                Err(_) => AccountSource::Unavailable,
            }
        };
        let ledger = match &source {
            AccountSource::Synthetic { mailboxes, .. } => {
                std::fs::create_dir_all(mailboxes).expect("synthetic mailbox directory");
                Some(
                    crate::ledger::InstalledLedger::install(mailboxes.join("ledger"))
                        .expect("synthetic private ledger"),
                )
            }
            _ => installed_ledger,
        };
        Self::assemble(source, ledger)
    }

    pub(super) fn assemble(
        source: AccountSource,
        ledger: Option<crate::ledger::InstalledLedger>,
    ) -> Self {
        let (events, _) = tokio::sync::watch::channel(1u64);
        Self {
            #[cfg(test)]
            assistant_cli: None,
            confirmations: Mutex::new(match &source {
                AccountSource::Synthetic { .. } => crate::confirmation::Boundary::synthetic(),
                AccountSource::Vault { .. } => crate::confirmation::Boundary::browser(),
                _ => crate::confirmation::Boundary::native(),
            }),
            writes: crate::send::wiring::Endpoints::Real,
            safety: Mutex::new(SafetyState::default()),
            events,
            #[cfg(test)]
            repository_accesses: std::sync::atomic::AtomicUsize::new(0),
            installed_ledger: ledger,
            source,
            job: Mutex::new(None),
            operation: Mutex::new(()),
            admission: Mutex::new(()),
            worker: Mutex::new(None),
        }
    }

    pub(super) fn repository(&self) -> Result<Arc<Accounts>, AcquisitionError> {
        self.enforce_timeouts();
        #[cfg(test)]
        self.repository_accesses
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match &self.source {
            AccountSource::Unavailable => Err(AcquisitionError::Storage),
            AccountSource::Os => Ok(Arc::new(Accounts::new(
                AccountPaths::per_user()?,
                Arc::new(OsCredentials),
            ))),
            AccountSource::Installed { accounts } | AccountSource::Synthetic { accounts, .. } => {
                Ok(accounts.clone())
            }
            AccountSource::Vault { accounts, vault } if vault.unlocked() => Ok(accounts.clone()),
            AccountSource::Vault { .. } => Err(AcquisitionError::VaultLocked),
        }
    }

    /// The installed ledger, refused while the vault is locked like every account route.
    pub(super) fn ledger(
        &self,
    ) -> Result<Option<&crate::ledger::InstalledLedger>, AcquisitionError> {
        if self.vault_locked() {
            return Err(AcquisitionError::VaultLocked);
        }
        Ok(self.installed_ledger.as_ref())
    }

    pub(super) fn vault(&self) -> Option<&Arc<Vault>> {
        match &self.source {
            AccountSource::Vault { vault, .. } => Some(vault),
            _ => None,
        }
    }

    pub(super) fn job_running(&self) -> bool {
        self.job
            .lock()
            .expect("account job")
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
    }

    pub(in crate::server) fn touch_activity(&self) {
        let mut safety = self.safety.lock().expect("safety state");
        safety.last_activity = Instant::now();
    }

    pub(in crate::server) fn enforce_timeouts(&self) {
        if self.job_running() {
            return;
        }
        let now = Instant::now();
        let mut changed = false;
        let mut should_lock_vault = false;
        {
            let mut safety = self.safety.lock().expect("safety state");
            if safety.live_writes && safety.live_writes_timeout_secs > 0 {
                if let Some(armed_at) = safety.live_writes_armed_at {
                    if now.saturating_duration_since(armed_at)
                        >= Duration::from_secs(safety.live_writes_timeout_secs)
                    {
                        safety.live_writes = false;
                        safety.live_writes_armed_at = None;
                        changed = true;
                    }
                }
            }
            if safety.vault_idle_timeout_secs > 0
                && self.vault().is_some_and(|vault| vault.unlocked())
                && now.saturating_duration_since(safety.last_activity)
                    >= Duration::from_secs(safety.vault_idle_timeout_secs)
            {
                safety.live_writes = false;
                safety.live_writes_armed_at = None;
                should_lock_vault = true;
                changed = true;
            }
        }
        if should_lock_vault {
            if let Some(vault) = self.vault() {
                let _ = vault.lock();
            }
        }
        if changed {
            self.notify_change();
        }
    }

    pub(in crate::server) fn vault_locked(&self) -> bool {
        self.enforce_timeouts();
        self.vault().is_some_and(|vault| !vault.unlocked())
    }

    pub(in crate::server) fn live_writes(&self) -> bool {
        self.enforce_timeouts();
        self.safety.lock().expect("safety state").live_writes
    }

    pub(super) fn set_live_writes(&self, enabled: bool) {
        self.arm_live_writes(enabled, None);
    }

    pub(super) fn arm_live_writes(&self, enabled: bool, timeout_secs: Option<u64>) {
        {
            let mut safety = self.safety.lock().expect("safety state");
            if let Some(secs) = timeout_secs {
                safety.live_writes_timeout_secs = secs;
            }
            safety.live_writes = enabled;
            safety.live_writes_armed_at = if enabled { Some(Instant::now()) } else { None };
            safety.last_activity = Instant::now();
        }
        self.notify_change();
    }

    pub(super) fn set_safety_timeouts(
        &self,
        live_writes_timeout_secs: Option<u64>,
        vault_idle_timeout_secs: Option<u64>,
    ) {
        {
            let mut safety = self.safety.lock().expect("safety state");
            if let Some(secs) = live_writes_timeout_secs {
                safety.live_writes_timeout_secs = secs;
                if safety.live_writes {
                    safety.live_writes_armed_at = Some(Instant::now());
                }
            }
            if let Some(secs) = vault_idle_timeout_secs {
                safety.vault_idle_timeout_secs = secs;
            }
            safety.last_activity = Instant::now();
        }
        self.notify_change();
    }

    pub(super) fn safety_snapshot(&self) -> SafetySnapshot {
        self.enforce_timeouts();
        let now = Instant::now();
        let vault_unlocked = self.vault().is_some_and(|v| v.unlocked());
        let safety = self.safety.lock().expect("safety state");
        let live_writes_expires_in_secs =
            if safety.live_writes && safety.live_writes_timeout_secs > 0 {
                safety.live_writes_armed_at.map(|armed_at| {
                    let elapsed = now.saturating_duration_since(armed_at).as_secs();
                    safety.live_writes_timeout_secs.saturating_sub(elapsed)
                })
            } else {
                None
            };
        let vault_locks_in_secs = if vault_unlocked && safety.vault_idle_timeout_secs > 0 {
            let elapsed = now
                .saturating_duration_since(safety.last_activity)
                .as_secs();
            Some(safety.vault_idle_timeout_secs.saturating_sub(elapsed))
        } else {
            None
        };
        SafetySnapshot {
            live_writes: safety.live_writes,
            live_writes_expires_in_secs,
            live_writes_timeout_secs: safety.live_writes_timeout_secs,
            vault_idle_timeout_secs: safety.vault_idle_timeout_secs,
            vault_locks_in_secs,
        }
    }

    pub(super) fn notify_change(&self) {
        self.events.send_modify(|seq| *seq = seq.wrapping_add(1));
    }

    pub(super) fn subscribe_events(&self) -> tokio::sync::watch::Receiver<u64> {
        self.events.subscribe()
    }

    pub(super) fn event_seq(&self) -> u64 {
        *self.events.borrow()
    }

    #[cfg(test)]
    pub(super) fn rewind_safety_timers_for_test(&self, secs: u64) {
        let mut safety = self.safety.lock().expect("safety state");
        let delta = Duration::from_secs(secs);
        if let Some(armed_at) = safety.live_writes_armed_at {
            safety.live_writes_armed_at = Some(armed_at.checked_sub(delta).unwrap_or(armed_at));
        }
        safety.last_activity = safety
            .last_activity
            .checked_sub(delta)
            .unwrap_or(safety.last_activity);
    }

    pub(super) fn vault_live(&self) -> bool {
        matches!(self.source, AccountSource::Vault { .. })
    }

    pub(super) fn synthetic(&self) -> bool {
        matches!(self.source, AccountSource::Synthetic { .. })
    }

    pub(super) fn local_bookkeeping(&self) -> bool {
        self.installed_ledger.is_some()
            && matches!(
                self.source,
                AccountSource::Installed { .. }
                    | AccountSource::Synthetic { .. }
                    | AccountSource::Vault { .. }
            )
    }

    pub(super) fn reserve(
        &self,
        operation: &'static str,
    ) -> Result<Cancellation, AcquisitionError> {
        let mut job = self.job.lock().expect("account job");
        if job
            .as_ref()
            .is_some_and(|j| j.progress.state == ScanState::Running)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let cancel = Cancellation::default();
        *job = Some(Job {
            operation,
            backlog_account: None,
            folder_account: None,
            sieve_account: None,
            account: None,
            progress: ScanProgress {
                state: ScanState::Running,
                saved: 0,
                folders: 0,
            },
            error: None,
            cancel: cancel.clone(),
        });
        drop(job);
        self.notify_change();
        Ok(cancel)
    }

    pub(super) fn finish(&self, result: Result<(), AcquisitionError>) {
        let mut current = self.job.lock().expect("account job");
        let job = current.as_mut().expect("reserved account job");
        match result {
            Ok(()) => job.progress.state = ScanState::Complete,
            Err(error) => {
                let error = if job.cancel.is_cancelled()
                    && matches!(
                        error,
                        AcquisitionError::Protocol
                            | AcquisitionError::Tls
                            | AcquisitionError::Authentication
                    ) {
                    AcquisitionError::Cancelled
                } else {
                    error
                };
                job.progress.state = if error == AcquisitionError::Cancelled {
                    ScanState::Cancelled
                } else {
                    ScanState::Failed
                };
                job.error = Some(error);
            }
        }
        drop(current);
        self.touch_activity();
        self.notify_change();
    }
}

pub(super) fn valid_timeout_secs(secs: u64) -> bool {
    secs == 0 || (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs)
}

pub(super) fn failure(error: AcquisitionError) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"error": error.to_string()})),
    )
        .into_response()
}

pub(super) fn input<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    match value {
        Ok(Json(value)) => Ok(value),
        Err(error) if error.status() == StatusCode::PAYLOAD_TOO_LARGE => Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error":"Account request exceeds the body limit"})),
        )
            .into_response()),
        Err(_) => Err(failure(AcquisitionError::InvalidInput)),
    }
}
