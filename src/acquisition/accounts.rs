use super::AcquisitionError;
use crate::census::{AccountId, Census, CensusKeys, EncryptedStore, LocalSecret};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Deserializer, Serialize};
use std::{fs, io::Write, path::PathBuf, sync::Arc};
use zeroize::Zeroizing;

pub struct SecretValue(Zeroizing<Vec<u8>>);
impl SecretValue {
    pub fn new(value: Vec<u8>) -> Self {
        Self(Zeroizing::new(value))
    }
    pub(crate) fn bytes(&self) -> &[u8] {
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
}

pub struct OsCredentials;
impl CredentialBackend for OsCredentials {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        keyring::Entry::new("mailroom", reference)
            .and_then(|entry| entry.get_secret())
            .map(SecretValue::new)
            .map_err(|_| AcquisitionError::Credentials)
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        keyring::Entry::new("mailroom", reference)
            .and_then(|entry| entry.set_secret(value.bytes()))
            .map_err(|_| AcquisitionError::Credentials)
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
    pub(crate) paths: AccountPaths,
    backend: Arc<dyn CredentialBackend>,
}
impl Accounts {
    pub fn new(paths: AccountPaths, backend: Arc<dyn CredentialBackend>) -> Self {
        Self { paths, backend }
    }
    pub fn create(
        &self,
        provider: Provider,
        login: String,
        password: &SecretValue,
    ) -> Result<Account, AcquisitionError> {
        if login.is_empty()
            || login.len() > 320
            || login.chars().any(char::is_control)
            || password.bytes().is_empty()
            || password.bytes().len() > 1024
            || password.text()?.chars().any(char::is_control)
        {
            return Err(AcquisitionError::InvalidInput);
        }
        private_directory(&self.paths.config)?;
        let mut random = [0; 16];
        OsRng.fill_bytes(&mut random);
        let id = AccountId::new(&data_encoding::HEXLOWER.encode(&random))?;
        let account = Account {
            password_ref: format!("{}-password", id.as_str()),
            local_secret_ref: format!("{}-census-key", id.as_str()),
            id,
            provider,
            login,
        };
        let mut key = Zeroizing::new(vec![0; 32]);
        OsRng.fill_bytes(&mut key);
        self.backend
            .put(&account.local_secret_ref, &SecretValue::new(key.to_vec()))?;
        self.backend.put(&account.password_ref, password)?;
        let store = self.store(&account)?;
        let mut census = Census::new(account.id.clone());
        census.account_login = Some(account.login.trim().to_lowercase());
        store.save(&census)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.paths.config)
            .map_err(|_| AcquisitionError::Storage)?;
        serde_json::to_writer(&mut file, &account).map_err(|_| AcquisitionError::Storage)?;
        file.flush()
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| AcquisitionError::Storage)?;
        file.persist_noclobber(self.metadata_path(&account.id))
            .map_err(|_| AcquisitionError::Storage)?;
        fs::File::open(&self.paths.config)
            .and_then(|f| f.sync_all())
            .map_err(|_| AcquisitionError::Storage)?;
        Ok(account)
    }
    fn metadata_path(&self, id: &AccountId) -> PathBuf {
        self.paths.config.join(format!("{}.json", id.as_str()))
    }
    pub fn load(&self, id: &AccountId) -> Result<Account, AcquisitionError> {
        let path = self.metadata_path(id);
        let metadata = fs::symlink_metadata(&path).map_err(|_| AcquisitionError::Storage)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(AcquisitionError::Storage);
        }
        let account: Account =
            serde_json::from_slice(&fs::read(path).map_err(|_| AcquisitionError::Storage)?)
                .map_err(|_| AcquisitionError::Storage)?;
        if account.id != *id
            || account.password_ref != format!("{}-password", id.as_str())
            || account.local_secret_ref != format!("{}-census-key", id.as_str())
        {
            return Err(AcquisitionError::InvalidInput);
        }
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
    pub(crate) fn password(&self, account: &Account) -> Result<SecretValue, AcquisitionError> {
        self.backend.get(&account.password_ref)
    }
    pub fn store(&self, account: &Account) -> Result<EncryptedStore, AcquisitionError> {
        let value = self.backend.get(&account.local_secret_ref)?;
        let secret = LocalSecret::from_bytes(value.bytes().to_vec())?;
        Ok(EncryptedStore::new(
            self.paths.data.clone(),
            CensusKeys::derive(&secret, &account.id),
        )?)
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
        fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
            self.0
                .lock()
                .unwrap()
                .get(reference)
                .map(|v| SecretValue::new(v.to_vec()))
                .ok_or(AcquisitionError::Credentials)
        }
        fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.to_owned(), Zeroizing::new(value.bytes().to_vec()));
            Ok(())
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
            Err(AcquisitionError::Credentials)
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
    struct FileFake(PathBuf);
    impl CredentialBackend for FileFake {
        fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
            fs::read(self.0.join(reference))
                .map(SecretValue::new)
                .map_err(|_| AcquisitionError::Credentials)
        }
        fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
            fs::create_dir_all(&self.0)
                .and_then(|_| fs::write(self.0.join(reference), value.bytes()))
                .map_err(|_| AcquisitionError::Credentials)
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
            let proof =
                crate::rules::workspace::preview(&state, account.provider, store.keys()).unwrap();
            crate::rules::workspace::confirm_export(
                &mut state,
                account.provider,
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
                crate::rules::workspace::preview(&state, account.provider, store.keys())
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
