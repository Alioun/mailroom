pub(crate) mod exclusive;
#[cfg(target_os = "macos")]
pub(crate) mod keychain;
pub mod lifecycle;
pub(crate) mod recovery;
use super::AcquisitionError;
use crate::census::{AccountId, AccountState, Census, CensusKeys, EncryptedStore, LocalSecret};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Deserializer, Serialize};
use std::{fs, io::Write, path::PathBuf, sync::Arc};
use zeroize::Zeroizing;

pub struct SecretValue(Zeroizing<Vec<u8>>);
impl SecretValue {
    pub fn new(value: Vec<u8>) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub(crate) fn text(&self) -> Result<&str, AcquisitionError> {
        std::str::from_utf8(&self.0).map_err(|_| AcquisitionError::Credentials)
    }
}
impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}
impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Zeroizing::new(String::deserialize(d)?);
        Ok(Self::new(value.as_bytes().to_vec()))
    }
}

pub trait CredentialBackend: Send + Sync {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError>;
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError>;
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError>;
    /// Deletes the entry only when its value satisfies `owned`, and only the entry whose
    /// value was checked, so cleanup never removes an entry it did not prove it created.
    /// Returns whether it deleted. A backend that cannot bind the deletion to the checked
    /// entry keeps it: this default never deletes.
    fn delete_owned(
        &self,
        _reference: &str,
        _owned: &dyn Fn(&[u8]) -> bool,
    ) -> Result<bool, AcquisitionError> {
        Ok(false)
    }
}

pub struct OsCredentials;
/// Every access goes through the pinned Keychain helper. Without one, every access is refused.
#[cfg(target_os = "macos")]
impl CredentialBackend for OsCredentials {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        keychain::installed()?.get(reference)
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        keychain::installed()?.put(reference, value)
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        keychain::installed()?.delete(reference)
    }
    fn delete_owned(
        &self,
        reference: &str,
        owned: &dyn Fn(&[u8]) -> bool,
    ) -> Result<bool, AcquisitionError> {
        keychain::installed()?.delete_owned(reference, owned)
    }
}
#[cfg(not(target_os = "macos"))]
impl CredentialBackend for OsCredentials {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        keyring::Entry::new("mailroom", reference)
            .and_then(|entry| entry.get_secret())
            .map(SecretValue::new)
            .map_err(|e| {
                if matches!(e, keyring::Error::NoEntry) {
                    AcquisitionError::CredentialMissing
                } else {
                    AcquisitionError::Credentials
                }
            })
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        keyring::Entry::new("mailroom", reference)
            .and_then(|entry| entry.set_secret(value.bytes()))
            .map_err(|_| AcquisitionError::Credentials)
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        match keyring::Entry::new("mailroom", reference).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AcquisitionError::Credentials),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    MailboxOrg,
    Gmail,
}
impl Provider {
    pub(crate) fn host(self) -> &'static str {
        match self {
            Self::MailboxOrg => "imap.mailbox.org",
            Self::Gmail => "imap.gmail.com",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: AccountId,
    pub provider: Provider,
    pub login: String,
    password_ref: String,
    local_secret_ref: String,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    identity_ref: Option<String>,
    #[serde(default)]
    connection: ConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restored: Option<recovery::Restored>,
    /// Binds the identity item to the encryption key; see `CensusKeys::identity_commitment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_commitment: Option<String>,
}

impl Account {
    /// Identity bytes saved before identity commitments existed. Such an account does not
    /// open until the owner adopts its identity.
    pub(crate) fn uncommitted(&self) -> bool {
        self.identity_ref.is_some() && self.identity_commitment.is_none()
    }
    /// The references of keys pinned to this binary: the local secret and any identity.
    pub(crate) fn key_references(&self) -> Vec<&str> {
        let mut references = vec![self.local_secret_ref.as_str()];
        references.extend(self.identity_ref.as_deref());
        references
    }
    pub(crate) fn credential_references(&self) -> Vec<&str> {
        let mut references = vec![self.password_ref.as_str(), self.local_secret_ref.as_str()];
        references.extend(self.identity_ref.as_deref());
        references
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    #[default]
    Unverified,
    Observed,
    Failed,
}

#[derive(Clone)]
pub struct AccountPaths {
    pub config: PathBuf,
    pub data: PathBuf,
}
impl AccountPaths {
    pub fn per_user() -> Result<Self, AcquisitionError> {
        let dirs = directories::BaseDirs::new().ok_or(AcquisitionError::Storage)?;
        #[cfg(target_os = "macos")]
        let config = dirs.home_dir().join(".config/mailroom");
        #[cfg(not(target_os = "macos"))]
        let config = dirs.config_dir().join("mailroom");
        Ok(Self {
            config,
            data: dirs.data_local_dir().join("mailroom"),
        })
    }
}

pub struct Accounts {
    pub(super) label_source:
        Option<Arc<super::folder_provisioning::transport::gmail_label::InstalledSource>>,
    pub(super) backlog_source:
        Option<Arc<super::folder_provisioning::transport::message_move::InstalledSource>>,
    pub(super) workspace_authority: Option<super::workspace_authority::SyntheticWorkspaceAuthority>,
    pub(super) folder_source: Option<Arc<super::folder_provisioning::transport::InstalledSource>>,
    pub(crate) paths: AccountPaths,
    backend: Arc<dyn CredentialBackend>,
    pub(super) sieve_write: Option<Arc<super::managesieve::FixtureOwnership>>,
    pub(super) sieve_source: Option<Arc<super::managesieve::InstalledSource>>,
    #[cfg(test)]
    pub(super) imap_factory: Option<super::imap::TestConnection>,
    /// Injected wall clock in seconds, for retention tests.
    #[cfg(test)]
    pub(crate) clock: Option<Arc<std::sync::atomic::AtomicU64>>,
}
impl Accounts {
    /// Seconds since the Unix epoch, used for Q27 undo retention.
    pub(crate) fn now(&self) -> u64 {
        #[cfg(test)]
        if let Some(clock) = &self.clock {
            return clock.load(std::sync::atomic::Ordering::SeqCst);
        }
        crate::backlog::retention::now()
    }
    /// Applies Q27 retention to both backlog journals and saves once if it
    /// stamped or compacted a run.
    pub(crate) fn retain_backlog(
        &self,
        store: &EncryptedStore,
        saved: &mut AccountState,
    ) -> Result<(), AcquisitionError> {
        let now = self.now();
        let protocol = saved.protocol_backlog.compact(now);
        let gmail = saved.gmail_backlog.compact(now);
        if protocol || gmail {
            store.save_state(saved)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn fixture_sieve_store(
        &self,
        id: &AccountId,
        root: &std::path::Path,
    ) -> Result<EncryptedStore, AcquisitionError> {
        Ok(self
            .open_store(&self.load(id)?, &root.join("mailboxes"), 1)?
            .simulated_provider())
    }
    pub fn new(paths: AccountPaths, backend: Arc<dyn CredentialBackend>) -> Self {
        Self {
            paths,
            backend,
            folder_source: None,
            backlog_source: None,
            label_source: None,
            workspace_authority: None,
            sieve_source: None,
            sieve_write: None,
            #[cfg(test)]
            imap_factory: None,
            #[cfg(test)]
            clock: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn track_credential_accesses(&mut self) -> Arc<std::sync::atomic::AtomicUsize> {
        self.track_secret_accesses(None)
    }
    /// Counts only reads of the account's mailbox password, not its local key.
    #[cfg(test)]
    pub(crate) fn track_password_accesses(
        &mut self,
        account: &Account,
    ) -> Arc<std::sync::atomic::AtomicUsize> {
        self.track_secret_accesses(Some(account.password_ref.clone()))
    }
    #[cfg(test)]
    fn track_secret_accesses(
        &mut self,
        only: Option<String>,
    ) -> Arc<std::sync::atomic::AtomicUsize> {
        struct Tracked {
            backend: Arc<dyn CredentialBackend>,
            accesses: Arc<std::sync::atomic::AtomicUsize>,
            only: Option<String>,
        }
        impl Tracked {
            fn count(&self, reference: &str) {
                if self.only.as_deref().is_none_or(|only| only == reference) {
                    self.accesses
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        impl CredentialBackend for Tracked {
            fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
                self.count(reference);
                self.backend.get(reference)
            }
            fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
                self.count(reference);
                self.backend.put(reference, value)
            }
            fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
                self.count(reference);
                self.backend.delete(reference)
            }
            fn delete_owned(
                &self,
                reference: &str,
                owned: &dyn Fn(&[u8]) -> bool,
            ) -> Result<bool, AcquisitionError> {
                self.count(reference);
                self.backend.delete_owned(reference, owned)
            }
        }
        let accesses = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.backend = Arc::new(Tracked {
            backend: self.backend.clone(),
            accesses: accesses.clone(),
            only,
        });
        accesses
    }
    pub fn create(
        &self,
        provider: Provider,
        login: String,
        password: &SecretValue,
    ) -> Result<Account, AcquisitionError> {
        self.create_transaction(provider, login, password)
    }
    fn metadata_path(&self, id: &AccountId) -> PathBuf {
        self.paths.config.join(format!("{}.json", id.as_str()))
    }
    pub fn load(&self, id: &AccountId) -> Result<Account, AcquisitionError> {
        self.load_metadata(id)
    }
    fn load_metadata(&self, id: &AccountId) -> Result<Account, AcquisitionError> {
        let path = self.metadata_path(id);
        let metadata = fs::symlink_metadata(&path).map_err(|_| AcquisitionError::Storage)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(AcquisitionError::Storage);
        }
        let account: Account =
            serde_json::from_slice(&fs::read(path).map_err(|_| AcquisitionError::Storage)?)
                .map_err(|_| AcquisitionError::Storage)?;
        self.validate_account(&account, id)?;
        Ok(account)
    }
    pub fn list(&self) -> Result<Vec<Account>, AcquisitionError> {
        if !self
            .paths
            .config
            .try_exists()
            .map_err(|_| AcquisitionError::Storage)?
        {
            return Ok(vec![]);
        }
        let entries = fs::read_dir(&self.paths.config).map_err(|_| AcquisitionError::Storage)?;
        let mut accounts = Vec::new();
        for entry in entries {
            let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let id = AccountId::new(
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or(AcquisitionError::Storage)?,
                )?;
                accounts.push(self.load(&id)?);
            }
        }
        Ok(accounts)
    }
    pub(super) fn sieve_authority(
        &self,
        account: &Account,
        store: &EncryptedStore,
    ) -> Result<String, AcquisitionError> {
        let identity = serde_json::to_vec(&(
            account.id.as_str(),
            account.provider,
            &account.password_ref,
            &account.local_secret_ref,
            account.generation,
            &account.restored,
        ))
        .map_err(|_| AcquisitionError::Storage)?;
        Ok(store
            .keys()
            .binding_for(b"mailroom/sieve-source/v1", &identity))
    }
    pub(crate) fn password(&self, account: &Account) -> Result<SecretValue, AcquisitionError> {
        self.backend.get(&account.password_ref)
    }
    /// The account's SMTP login: its user, backend and password reference, never the password.
    /// Only `send::smtp_secret` reads the password through it, after TLS is up.
    pub(crate) fn smtp_login<'a>(&'a self, account: &'a Account) -> crate::send::smtp::Login<'a> {
        crate::send::smtp::Login {
            user: &account.login,
            backend: self.backend.as_ref(),
            reference: &account.password_ref,
        }
    }
    pub fn store(&self, account: &Account) -> Result<EncryptedStore, AcquisitionError> {
        let lease = self.lock_account(&account.id)?;
        self.require_ready(&account.id)?;
        let current = self.load_metadata(&account.id)?;
        if serde_json::to_vec(&current).ok() != serde_json::to_vec(account).ok() {
            return Err(AcquisitionError::InvalidInput);
        }
        let mut store = self.open_store(&current, &self.paths.data, 0)?;
        store.retain_account_lease(lease);
        Ok(store)
    }
}

#[cfg(test)]
mod provider_attempt_tests {
    use super::*;
    use crate::{acquisition::Cancellation, census::ScanState};

    #[test]
    fn missing_credential_and_pre_cancelled_scan_record_new_attempts_without_refreshing_success() {
        let root = tempfile::tempdir().unwrap();
        let mut repo = super::process_tests::persistent_accounts(root.path());
        crate::acquisition::install_provider_fixture(&mut repo, root.path());
        let account = repo
            .create(
                Provider::Gmail,
                "attempt@example.test".into(),
                &SecretValue::new(b"invented".to_vec()),
            )
            .unwrap();
        repo.scan(&account.id, Cancellation::default(), |_| {})
            .unwrap();
        let good = repo.store(&account).unwrap().load().unwrap();
        let id = good.acquisition.latest.as_ref().unwrap().id.clone();
        repo.backend.delete(&account.password_ref).unwrap();
        assert_eq!(
            repo.scan(&account.id, Cancellation::default(), |_| {}),
            Err(AcquisitionError::CredentialMissing)
        );
        let saved = repo.store(&account).unwrap().load().unwrap();
        assert_eq!(
            saved.acquisition.latest.as_ref().unwrap().state,
            ScanState::Failed
        );
        assert_ne!(saved.acquisition.latest.as_ref().unwrap().id, id);
        assert_eq!(
            saved.acquisition.last_successful,
            good.acquisition.last_successful
        );
        let id = saved.acquisition.latest.unwrap().id;
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(
            repo.scan(&account.id, cancel, |_| {}),
            Err(AcquisitionError::Cancelled)
        );
        let saved = repo.store(&account).unwrap().load().unwrap();
        assert_eq!(
            saved.acquisition.latest.as_ref().unwrap().state,
            ScanState::Cancelled
        );
        assert_ne!(saved.acquisition.latest.unwrap().id, id);
        assert_eq!(
            saved.acquisition.last_successful,
            good.acquisition.last_successful
        );
    }
}

fn private_directory(path: &std::path::Path) -> Result<(), AcquisitionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|_| AcquisitionError::Storage)?;
        let metadata = fs::symlink_metadata(path).map_err(|_| AcquisitionError::Storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(AcquisitionError::Storage);
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| AcquisitionError::Storage)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path).map_err(|_| AcquisitionError::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex};
    #[derive(Default)]
    struct FakeBackend(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
    impl CredentialBackend for FakeBackend {
        fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
            self.0.lock().unwrap().remove(reference);
            Ok(())
        }
        fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
            self.0
                .lock()
                .unwrap()
                .get(reference)
                .map(|v| SecretValue::new(v.to_vec()))
                .ok_or(AcquisitionError::CredentialMissing)
        }
        fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.to_owned(), Zeroizing::new(value.bytes().to_vec()));
            Ok(())
        }
        fn delete_owned(
            &self,
            reference: &str,
            owned: &dyn Fn(&[u8]) -> bool,
        ) -> Result<bool, AcquisitionError> {
            let mut values = self.0.lock().unwrap();
            let checked = values.get(reference).is_some_and(|v| owned(v));
            Ok(checked && values.remove(reference).is_some())
        }
    }
    #[test]
    fn process_reconstruction_uses_stable_backend_keys_and_keeps_metadata_secret_free() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AccountPaths {
            config: directory.path().join("config"),
            data: directory.path().join("data"),
        };
        let backend = Arc::new(FakeBackend::default());
        let accounts = Accounts::new(paths.clone(), backend.clone());
        let account = accounts
            .create(
                Provider::MailboxOrg,
                "owner@example.test".to_owned(),
                &SecretValue::new(b"synthetic-password".to_vec()),
            )
            .unwrap();
        let store = accounts.store(&account).unwrap();
        let mut census = store.load().unwrap();
        census
            .reconcile_folder("INBOX", 7, [1].into_iter().collect())
            .unwrap();
        census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .insert(crate::census::demo::fixture(1).unwrap())
            .unwrap();
        store.save(&census).unwrap();
        drop(store);
        let metadata = fs::read_to_string(accounts.metadata_path(&account.id)).unwrap();
        assert!(!metadata.contains("synthetic-password"));
        drop(accounts);
        drop(account);
        let reconstructed = Accounts::new(paths.clone(), backend.clone());
        let account = reconstructed.list().unwrap().pop().unwrap();
        assert_eq!(
            reconstructed.store(&account).unwrap().load().unwrap(),
            census
        );
        assert_eq!(
            backend
                .get(&account.local_secret_ref)
                .unwrap()
                .bytes()
                .len(),
            32
        );
        let other = reconstructed
            .create(
                Provider::Gmail,
                "other@example.test".to_owned(),
                &SecretValue::new(b"other-synthetic-password".to_vec()),
            )
            .unwrap();
        assert_ne!(account.id, other.id);
        let original = fs::read(reconstructed.store(&account).unwrap().path()).unwrap();
        let other_store = reconstructed.store(&other).unwrap();
        fs::write(other_store.path(), &original).unwrap();
        assert!(other_store.load().is_err());
        assert!(other_store.save(&Census::new(other.id)).is_err());
        backend.0.lock().unwrap().remove(&account.local_secret_ref);
        assert!(matches!(
            reconstructed.store(&account),
            Err(AcquisitionError::CredentialMissing)
        ));
        assert_eq!(
            fs::read(
                paths
                    .data
                    .join(format!("{}.census.enc", account.id.as_str()))
            )
            .unwrap(),
            original
        );
        assert_eq!(
            format!("{:?}", SecretValue::new(b"synthetic-password".to_vec())),
            "SecretValue([REDACTED])"
        );
        assert_eq!(backend.0.lock().unwrap().len(), 3);
    }
}

#[cfg(test)]
pub(crate) mod process_tests {
    use super::*;
    pub(crate) struct FileFake(pub(crate) PathBuf);
    impl CredentialBackend for FileFake {
        fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
            lifecycle::remove_file(&self.0.join(reference))?;
            if self.0.exists() {
                for entry in fs::read_dir(&self.0).map_err(|_| AcquisitionError::Storage)? {
                    let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
                    if path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.starts_with(&format!(".{reference}.stage-")))
                    {
                        lifecycle::remove_file(&path)?;
                    }
                }
            }
            Ok(())
        }
        fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
            fs::read(self.0.join(reference))
                .map(SecretValue::new)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        AcquisitionError::CredentialMissing
                    } else {
                        AcquisitionError::Credentials
                    }
                })
        }
        fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
            private_directory(&self.0)?;
            lifecycle::write_new_secret(&self.0.join(reference), value.bytes())
        }
        /// Never unlinks. The entry is moved, by an exclusive rename between open
        /// directories, into a fresh `.<reference>.discarded-<random>` directory, and the file
        /// read is the one in that directory. An owned entry stays discarded there; any other
        /// is moved back only if its name is still free, and otherwise stays there too. So no
        /// entry another process puts at either name is ever removed or replaced.
        fn delete_owned(
            &self,
            reference: &str,
            owned: &dyn Fn(&[u8]) -> bool,
        ) -> Result<bool, AcquisitionError> {
            use exclusive::{Dir, Moved, move_exclusive};
            if !self.0.try_exists().map_err(|_| AcquisitionError::Storage)? {
                return Ok(false);
            }
            let store = Dir::open(&self.0)?;
            if !store.has(reference)? {
                return Ok(false);
            }
            let folder =
                store.create_fresh(&format!(".{reference}.discarded-{}", lifecycle::nonce()))?;
            match move_exclusive(&store, reference, &folder, reference)? {
                Moved::Moved => {}
                Moved::SourceMissing => return Ok(false),
                Moved::DestinationTaken => return Err(AcquisitionError::Storage),
            }
            let value = folder.read(reference, 1024 * 1024)?;
            if owned(&value) {
                folder.sync()?;
                store.sync()?;
                return Ok(true);
            }
            move_exclusive(&folder, reference, &store, reference)?;
            Ok(false)
        }
    }
    pub(crate) fn persistent_accounts(root: &std::path::Path) -> Accounts {
        Accounts::new(
            AccountPaths {
                config: root.join("config"),
                data: root.join("data"),
            },
            Arc::new(FileFake(root.join("fake-credentials"))),
        )
    }
    #[test]
    fn synthetic_process_worker() {
        let Some(directory) = std::env::var_os("MAILROOM_SYNTHETIC_PROCESS_DIR") else {
            return;
        };
        let root = PathBuf::from(directory);
        let accounts = persistent_accounts(&root);
        if !root.join("account-id").exists() {
            let account = accounts
                .create(
                    Provider::MailboxOrg,
                    "owner@example.test".into(),
                    &SecretValue::new(b"synthetic-password".to_vec()),
                )
                .unwrap();
            let store = accounts.store(&account).unwrap();
            let mut census = store.load().unwrap();
            census
                .reconcile_folder("INBOX", 7, [1].into_iter().collect())
                .unwrap();
            census
                .folders
                .get_mut("INBOX")
                .unwrap()
                .insert(crate::census::demo::fixture(1).unwrap())
                .unwrap();
            census.confirm_address("owner@example.test", true).unwrap();
            store.save(&census).unwrap();
            let mut state = store.load_state().unwrap();
            let sample: serde_json::Value =
                serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
            state.workspace = Some(crate::rules::workspace::Workspace {
                revision: 1,
                input: crate::rules::workspace::WorkspaceInput {
                    documents: serde_json::from_value(sample["documents"].clone()).unwrap(),
                    pins: vec![],
                    as_of: 1792886400,
                    baseline: crate::rules::proof::Baseline::Current,
                },
            });
            store.save_state(&mut state).unwrap();
            let proof = crate::rules::workspace::preview(
                &state,
                &crate::rules::workspace::WorkspaceContext::local(account.provider),
                store.keys(),
            )
            .unwrap();
            crate::rules::workspace::confirm_export(
                &mut state,
                &crate::rules::workspace::WorkspaceContext::local(account.provider),
                store.keys(),
                &proof.receipt,
            )
            .unwrap();
            store.save_state(&mut state).unwrap();
            fs::write(root.join("account-id"), account.id.as_str()).unwrap();
        } else {
            let id = AccountId::new(&fs::read_to_string(root.join("account-id")).unwrap()).unwrap();
            let account = accounts.load(&id).unwrap();
            let store = accounts.store(&account).unwrap();
            let state = store.load_state().unwrap();
            assert_eq!(state.workspace.as_ref().unwrap().revision, 1);
            assert_eq!(state.exports.len(), 1);
            assert_eq!(
                state.exports[0].artifact.executable(),
                crate::rules::workspace::preview(
                    &state,
                    &crate::rules::workspace::WorkspaceContext::local(account.provider),
                    store.keys()
                )
                .unwrap()
                .proof
                .artifact
                .executable()
            );
            let census = state.census;
            assert_eq!(census.folders["INBOX"].messages.len(), 1);
            assert!(
                census
                    .confirmed_own_addresses
                    .contains("owner@example.test")
            );
        }
    }
    #[test]
    fn fake_backend_reopens_encrypted_state_across_actual_process_restarts() {
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "acquisition::accounts::process_tests::synthetic_process_worker",
                ])
                .env("MAILROOM_SYNTHETIC_PROCESS_DIR", dir.path())
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "Synthetic persistence subprocess failed"
            );
        }
    }
}
