use super::*;
use crate::file_lock::ExclusiveFileLock;
use std::{io::Read, path::Path};

const LIMIT: u64 = 1024 * 1024;
const CONSEQUENCES_EN: &str = "Forget this account locally, including saved headers, original content, rules, decisions, exports, undo and reconciliation history, and independent synthetic mailbox/send/unsubscribe effect stores. No remote undo, unsubscribe or account shutdown occurs. Shared owner-authored addresses.yaml and unrelated accounts survive. Browser-managed downloads, exports and backups cannot be erased here. Stable coordination lock files remain.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Intent {
    Creating {
        account: Account,
    },
    Replacing {
        before: Account,
        after: Account,
    },
    Forgetting {
        account: Account,
        manifest: Vec<Resource>,
    },
    Rotating {
        before: Account,
        after: Account,
        stores: Vec<usize>,
    },
    Forgotten {
        account: AccountId,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleRecord {
    version: u32,
    intent: Intent,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Creating,
    Replacing,
    Forgetting,
    Rotating,
    Forgotten,
}
#[derive(Serialize)]
pub struct Recovery {
    pub account: AccountId,
    pub state: LifecycleState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub path: PathBuf,
    pub bytes: u64,
    pub digest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consequences {
    pub en: String,
    pub de: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetPreview {
    pub account: AccountId,
    pub revision: u64,
    pub generation: u64,
    pub resources: Vec<Resource>,
    pub credential_references: Vec<String>,
    pub consequences: Consequences,
    pub receipt: String,
}

impl Accounts {
    fn intent_path(&self, id: &AccountId) -> PathBuf {
        self.paths
            .config
            .join("lifecycle")
            .join(format!("{}.json", id.as_str()))
    }
    pub(super) fn lock_account(
        &self,
        id: &AccountId,
    ) -> Result<ExclusiveFileLock, AcquisitionError> {
        private_directory(&self.paths.config.join("lifecycle"))?;
        let path = self
            .paths
            .config
            .join("lifecycle")
            .join(format!("{}.lock", id.as_str()));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path).map_err(|_| AcquisitionError::Storage)?;
        if !file
            .metadata()
            .map_err(|_| AcquisitionError::Storage)?
            .is_file()
        {
            return Err(AcquisitionError::Storage);
        }
        ExclusiveFileLock::try_lock(file).map_err(|error| match error {
            fs::TryLockError::WouldBlock => AcquisitionError::LifecyclePending,
            fs::TryLockError::Error(_) => AcquisitionError::Storage,
        })
    }
    pub(super) fn require_ready(&self, id: &AccountId) -> Result<(), AcquisitionError> {
        if self
            .intent_path(id)
            .try_exists()
            .map_err(|_| AcquisitionError::Storage)?
        {
            return Err(AcquisitionError::LifecyclePending);
        }
        Ok(())
    }
    fn intent(&self, id: &AccountId) -> Result<Option<Intent>, AcquisitionError> {
        let path = self.intent_path(id);
        if !path.try_exists().map_err(|_| AcquisitionError::Storage)? {
            return Ok(None);
        }
        let record: LifecycleRecord = serde_json::from_slice(&read_bounded(&path, LIMIT)?)
            .map_err(|_| AcquisitionError::Storage)?;
        if record.version != 1 {
            return Err(AcquisitionError::Storage);
        }
        let value = record.intent;
        match &value {
            Intent::Creating { account } | Intent::Forgetting { account, .. } => {
                self.validate_account(account, id)?
            }
            Intent::Replacing { before, after } | Intent::Rotating { before, after, .. } => {
                self.validate_account(before, id)?;
                self.validate_account(after, id)?;
            }
            Intent::Forgotten { account } if account == id => (),
            _ => return Err(AcquisitionError::Storage),
        }
        Ok(Some(value))
    }
    fn record(&self, id: &AccountId, intent: &Intent) -> Result<(), AcquisitionError> {
        write_json(
            &self.intent_path(id),
            &LifecycleRecord {
                version: 1,
                intent: intent.clone(),
            },
        )?;
        boundary("intent")
    }
    pub(super) fn validate_account(
        &self,
        a: &Account,
        id: &AccountId,
    ) -> Result<(), AcquisitionError> {
        let prefix = id.as_str();
        let password = a.password_ref == format!("{prefix}-password")
            || a.password_ref
                .strip_prefix(&format!("{prefix}-password-"))
                .is_some_and(valid_nonce);
        let secret = if a.generation == 0 {
            format!("{prefix}-census-key")
        } else {
            format!("{prefix}-key-g{}", a.generation)
        };
        if a.id != *id
            || !password
            || a.local_secret_ref != secret
            || a.identity_ref
                .as_ref()
                .is_some_and(|r| r != &format!("{prefix}-identity"))
            || (a.generation > 0 && a.identity_ref.is_none())
            || a.generation > 10000
        {
            return Err(AcquisitionError::InvalidInput);
        }
        Ok(())
    }
    fn publish(&self, a: &Account) -> Result<(), AcquisitionError> {
        write_json(&self.metadata_path(&a.id), a)?;
        boundary("metadata")
    }
    fn put_new(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        match self.backend.get(reference) {
            Err(AcquisitionError::CredentialMissing) => self.backend.put(reference, value),
            Ok(_) => Err(AcquisitionError::InvalidInput),
            Err(e) => Err(e),
        }
    }
    pub(super) fn open_store(
        &self,
        a: &Account,
        directory: &Path,
        kind: usize,
    ) -> Result<EncryptedStore, AcquisitionError> {
        Ok(EncryptedStore::new(
            directory.to_path_buf(),
            self.store_keys(a, kind)?,
        )?)
    }
    fn store_keys(&self, a: &Account, kind: usize) -> Result<CensusKeys, AcquisitionError> {
        let value = self.backend.get(&a.local_secret_ref)?;
        let secret = LocalSecret::from_bytes(value.bytes().to_vec())?;
        let keys = match &a.identity_ref {
            Some(reference) => CensusKeys::rotated(
                &secret,
                &a.id,
                self.backend.get(reference)?.bytes(),
                a.generation,
            )?,
            None => CensusKeys::derive(&secret, &a.id),
        };
        Ok(match kind {
            0 => keys,
            1 => keys.synthetic_mailbox_keys(),
            2 => keys.synthetic_invoice_keys(),
            3 => keys.synthetic_unsubscribe_keys(),
            _ => return Err(AcquisitionError::InvalidInput),
        })
    }
    fn directories(&self, effects: &Path) -> [PathBuf; 4] {
        [
            self.paths.data.clone(),
            effects.to_path_buf(),
            effects.join("invoice-transport"),
            effects.join("unsubscribe-transport"),
        ]
    }
    fn credential_refs(a: &Account) -> Vec<String> {
        let mut refs = vec![a.password_ref.clone(), a.local_secret_ref.clone()];
        refs.extend(a.identity_ref.clone());
        refs
    }
    pub(super) fn create_transaction(
        &self,
        provider: Provider,
        login: String,
        password: &SecretValue,
    ) -> Result<Account, AcquisitionError> {
        validate_password(password)?;
        if login.is_empty() || login.len() > 320 || login.chars().any(char::is_control) {
            return Err(AcquisitionError::InvalidInput);
        }
        private_directory(&self.paths.config)?;
        let id = AccountId::new(&nonce())?;
        let _lease = self.lock_account(&id)?;
        self.require_ready(&id)?;
        if self.metadata_path(&id).exists()
            || self
                .paths
                .data
                .join(format!("{}.census.enc", id.as_str()))
                .exists()
        {
            return Err(AcquisitionError::Storage);
        }
        let a = Account {
            password_ref: format!("{}-password", id.as_str()),
            local_secret_ref: format!("{}-census-key", id.as_str()),
            generation: 0,
            identity_ref: None,
            connection: ConnectionState::Unverified,
            id,
            provider,
            login,
        };
        self.record(&a.id, &Intent::Creating { account: a.clone() })?;
        let mut key = Zeroizing::new(vec![0; 32]);
        OsRng.fill_bytes(&mut key);
        self.put_new(&a.local_secret_ref, &SecretValue::new(key.to_vec()))?;
        boundary("create_key")?;
        self.put_new(&a.password_ref, password)?;
        boundary("create_password")?;
        let store = self.open_store(&a, &self.paths.data, 0)?;
        let mut census = Census::new(a.id.clone());
        census.account_login = Some(a.login.trim().to_lowercase());
        store.save(&census)?;
        boundary("create_data")?;
        self.publish(&a)?;
        self.clear_intent(&a.id)?;
        Ok(a)
    }
    pub fn connection_observed(
        &self,
        id: &AccountId,
        observed: bool,
    ) -> Result<(), AcquisitionError> {
        let _lease = self.lock_account(id)?;
        self.require_ready(id)?;
        let mut account = self.load_metadata(id)?;
        account.connection = if observed {
            ConnectionState::Observed
        } else {
            ConnectionState::Failed
        };
        self.publish(&account)
    }
    pub fn replace_password(
        &self,
        id: &AccountId,
        password: &SecretValue,
    ) -> Result<(), AcquisitionError> {
        validate_password(password)?;
        let _lease = self.lock_account(id)?;
        self.require_ready(id)?;
        let before = self.load_metadata(id)?;
        let _store = self.open_store(&before, &self.paths.data, 0)?;
        _store.load_state()?;
        let mut after = before.clone();
        after.password_ref = format!("{}-password-{}", id.as_str(), nonce());
        after.connection = ConnectionState::Unverified;
        self.record(
            id,
            &Intent::Replacing {
                before: before.clone(),
                after: after.clone(),
            },
        )?;
        self.put_new(&after.password_ref, password)?;
        boundary("replace_password")?;
        self.publish(&after)?;
        self.backend.delete(&before.password_ref)?;
        boundary("retire_password")?;
        self.clear_intent(id)
    }
    fn clear_intent(&self, id: &AccountId) -> Result<(), AcquisitionError> {
        for path in attributable_staging(&self.paths.config, id)?
            .into_iter()
            .chain(attributable_staging(
                &self.paths.config.join("lifecycle"),
                id,
            )?)
        {
            remove_file(&path)?;
        }
        remove_file(&self.intent_path(id))
    }
    fn manifest(&self, a: &Account, effects: &Path) -> Result<Vec<Resource>, AcquisitionError> {
        let mut paths = vec![self.metadata_path(&a.id)];
        for dir in self.directories(effects) {
            paths.push(cipher_path(&dir, &a.id, a.generation));
            paths.extend(attributable_staging(&dir, &a.id)?);
        }
        paths.extend(attributable_staging(&self.paths.config, &a.id)?);
        paths.extend(attributable_staging(
            &self.paths.config.join("lifecycle"),
            &a.id,
        )?);
        let mut resources = Vec::new();
        for path in paths {
            if path.try_exists().map_err(|_| AcquisitionError::Storage)? {
                let bytes = read_bounded(&path, 128 * 1024 * 1024 + 128)?;
                use sha2::{Digest, Sha256};
                resources.push(Resource {
                    path,
                    bytes: bytes.len() as u64,
                    digest: data_encoding::HEXLOWER.encode(&Sha256::digest(bytes)),
                });
            }
        }
        resources.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(resources)
    }
    fn preview_locked(
        &self,
        a: &Account,
        effects: &Path,
    ) -> Result<ForgetPreview, AcquisitionError> {
        let store = self.open_store(a, &self.paths.data, 0)?;
        let revision = store.load_state()?.revision;
        let mut value = ForgetPreview {
            account: a.id.clone(),
            revision,
            generation: a.generation,
            resources: self.manifest(a, effects)?,
            credential_references: Self::credential_refs(a),
            consequences: Consequences {
                en: CONSEQUENCES_EN.into(),
                de: "Dieses Konto lokal vergessen, einschließlich gespeicherter Kopfzeilen, Originalinhalte, Regeln, Entscheidungen, Exporte, Rückgängig- und Abgleichverlauf sowie unabhängiger synthetischer Postfach-, Versand- und Abmeldeeffekte. Keine entfernte Rücknahme, Abmeldung oder Kontoschließung. Gemeinsame eigene addresses.yaml und andere Konten bleiben erhalten. Browser-Downloads, Exporte und Sicherungen können hier nicht gelöscht werden. Stabile Koordinationssperren bleiben.".into(),
            },
            receipt: String::new(),
        };
        value.receipt = store.keys().binding_for(
            b"mailroom/forget-exact-local/v1",
            &serde_json::to_vec(&value).map_err(|_| AcquisitionError::Storage)?,
        );
        Ok(value)
    }
    pub fn forget_preview(
        &self,
        id: &AccountId,
        effects: &Path,
    ) -> Result<ForgetPreview, AcquisitionError> {
        let _lease = self.lock_account(id)?;
        self.require_ready(id)?;
        self.preview_locked(&self.load_metadata(id)?, effects)
    }
    pub fn forget(
        &self,
        id: &AccountId,
        effects: &Path,
        preview: &ForgetPreview,
        confirmed: bool,
    ) -> Result<(), AcquisitionError> {
        use subtle::ConstantTimeEq;
        let _lease = self.lock_account(id)?;
        self.require_ready(id)?;
        let a = self.load_metadata(id)?;
        let current = self.preview_locked(&a, effects)?;
        if !confirmed
            || !bool::from(preview.receipt.as_bytes().ct_eq(current.receipt.as_bytes()))
            || serde_json::to_vec(preview).ok() != serde_json::to_vec(&current).ok()
        {
            return Err(AcquisitionError::InvalidInput);
        }
        // Hold every store lock until deletion is durably complete. Lock paths survive.
        let _stores = self.lock_stores(&a, effects)?;
        self.record(
            id,
            &Intent::Forgetting {
                account: a.clone(),
                manifest: current.resources.clone(),
            },
        )?;
        self.finish_forget(&a, effects, &current.resources)
    }
    fn lock_stores(
        &self,
        a: &Account,
        effects: &Path,
    ) -> Result<Vec<EncryptedStore>, AcquisitionError> {
        self.directories(effects)
            .iter()
            .enumerate()
            .map(|(kind, dir)| self.open_store(a, dir, kind))
            .collect()
    }
    fn finish_forget(
        &self,
        a: &Account,
        effects: &Path,
        manifest: &[Resource],
    ) -> Result<(), AcquisitionError> {
        for resource in manifest {
            let legal = resource.path == self.metadata_path(&a.id)
                || self
                    .directories(effects)
                    .iter()
                    .any(|dir| resource.path == cipher_path(dir, &a.id, a.generation))
                || self
                    .directories(effects)
                    .iter()
                    .chain([&self.paths.config, &self.paths.config.join("lifecycle")])
                    .any(|dir| {
                        resource.path.parent() == Some(dir.as_path())
                            && resource
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| {
                                    n.starts_with(&format!(".{}.stage-", a.id.as_str()))
                                })
                    });
            if !legal {
                return Err(AcquisitionError::Storage);
            }
            if resource.path != self.metadata_path(&a.id) {
                remove_file(&resource.path)?;
                boundary("forget_data")?;
            }
        }
        for reference in Self::credential_refs(a) {
            self.backend.delete(&reference)?;
            boundary("forget_credential")?;
        }
        remove_file(&self.metadata_path(&a.id))?;
        boundary("forget_metadata")?;
        self.record(
            &a.id,
            &Intent::Forgotten {
                account: a.id.clone(),
            },
        )
    }
    pub fn rotate(&self, id: &AccountId, effects: &Path) -> Result<(), AcquisitionError> {
        let _lease = self.lock_account(id)?;
        self.require_ready(id)?;
        let before = self.load_metadata(id)?;
        let stores = self.lock_stores(&before, effects)?;
        let identity = Zeroizing::new(stores[0].keys().identity_material());
        stores[0].load_state()?;
        let present = stores
            .iter()
            .enumerate()
            .filter(|(_, s)| s.path().exists())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut after = before.clone();
        after.generation = before
            .generation
            .checked_add(1)
            .ok_or(AcquisitionError::Storage)?;
        after.local_secret_ref = format!("{}-key-g{}", id.as_str(), after.generation);
        after.identity_ref = Some(format!("{}-identity", id.as_str()));
        self.record(
            id,
            &Intent::Rotating {
                before: before.clone(),
                after: after.clone(),
                stores: present.clone(),
            },
        )?;
        if before.identity_ref.is_none() {
            self.put_new(
                after
                    .identity_ref
                    .as_ref()
                    .ok_or(AcquisitionError::Storage)?,
                &SecretValue::new(identity.to_vec()),
            )?;
        }
        boundary("rotate_identity")?;
        let mut key = Zeroizing::new(vec![0; 32]);
        OsRng.fill_bytes(&mut key);
        self.put_new(&after.local_secret_ref, &SecretValue::new(key.to_vec()))?;
        boundary("rotate_key")?;
        self.finish_rotation(&before, &after, effects, &present, stores)
    }
    fn finish_rotation(
        &self,
        before: &Account,
        after: &Account,
        effects: &Path,
        present: &[usize],
        mut locked: Vec<EncryptedStore>,
    ) -> Result<(), AcquisitionError> {
        let selected = self.load_metadata(&before.id)?.generation == after.generation;
        if !selected {
            // Reuse the stable per-store lock while authenticating both generations.
            for &kind in present {
                let store = locked.get_mut(kind).ok_or(AcquisitionError::Storage)?;
                let mut snapshot = store.load_state()?;
                if kind == 0 {
                    snapshot.revision = snapshot
                        .revision
                        .checked_add(1)
                        .ok_or(AcquisitionError::Storage)?;
                }
                store.replace_keys(self.store_keys(after, kind)?);
                if store.path().exists() {
                    if serde_json::to_vec(&store.load_state()?).ok()
                        != serde_json::to_vec(&snapshot).ok()
                    {
                        return Err(AcquisitionError::Storage);
                    }
                } else {
                    store.install_snapshot(&snapshot)?;
                }
                store.load_state()?;
                boundary(match kind {
                    0 => "rotate_primary",
                    1 => "rotate_mailbox",
                    2 => "rotate_invoice",
                    _ => "rotate_unsubscribe",
                })?;
            }
            self.publish(after)?;
        }
        for &kind in present {
            let store = locked.get_mut(kind).ok_or(AcquisitionError::Storage)?;
            store.replace_keys(self.store_keys(after, kind)?);
            store.load_state()?;
        }
        boundary("rotate_verified")?;
        for (kind, dir) in self.directories(effects).iter().enumerate() {
            if present.contains(&kind) {
                remove_file(&cipher_path(dir, &before.id, before.generation))?;
                boundary("rotate_retire_data")?;
            }
            for path in attributable_staging(dir, &before.id)? {
                remove_file(&path)?;
            }
        }
        self.backend.delete(&before.local_secret_ref)?;
        boundary("rotate_retire_key")?;
        self.clear_intent(&before.id)
    }
    pub fn recovery_status(&self) -> Result<Vec<Recovery>, AcquisitionError> {
        let root = self.paths.config.join("lifecycle");
        if !root.exists() {
            return Ok(vec![]);
        }
        let mut result = vec![];
        for entry in fs::read_dir(root).map_err(|_| AcquisitionError::Storage)? {
            let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let id = AccountId::new(
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or(AcquisitionError::Storage)?,
                )?;
                let state = match self.intent(&id)?.ok_or(AcquisitionError::Storage)? {
                    Intent::Creating { .. } => LifecycleState::Creating,
                    Intent::Replacing { .. } => LifecycleState::Replacing,
                    Intent::Forgetting { .. } => LifecycleState::Forgetting,
                    Intent::Rotating { .. } => LifecycleState::Rotating,
                    Intent::Forgotten { .. } => LifecycleState::Forgotten,
                };
                result.push(Recovery { account: id, state });
            }
        }
        Ok(result)
    }
    pub fn recover(&self, id: &AccountId, effects: &Path) -> Result<(), AcquisitionError> {
        let _lease = self.lock_account(id)?;
        match self.intent(id)? {
            None | Some(Intent::Forgotten { .. }) => Ok(()),
            Some(Intent::Creating { account }) => {
                if self.metadata_path(id).exists() {
                    let saved = self.load_metadata(id)?;
                    if serde_json::to_vec(&saved).ok() != serde_json::to_vec(&account).ok() {
                        return Err(AcquisitionError::Storage);
                    }
                    self.open_store(&account, &self.paths.data, 0)?
                        .load_state()?;
                } else {
                    // Creation never publishes until its resources are complete. Recovery only cleans.
                    let _store_lock = EncryptedStore::lock_directory(&self.paths.data, id)?;
                    for reference in Self::credential_refs(&account) {
                        self.backend.delete(&reference)?;
                    }
                    remove_file(&cipher_path(&self.paths.data, id, 0))?;
                    for path in attributable_staging(&self.paths.data, id)?
                        .into_iter()
                        .chain(attributable_staging(&self.paths.config, id)?)
                    {
                        remove_file(&path)?;
                    }
                }
                self.clear_intent(id)
            }
            Some(Intent::Replacing { before, after }) => {
                let current = self.load_metadata(id)?;
                if current.password_ref == after.password_ref {
                    self.backend.get(&after.password_ref)?;
                    self.backend.delete(&before.password_ref)?;
                } else if current.password_ref == before.password_ref {
                    self.backend.delete(&after.password_ref)?;
                } else {
                    return Err(AcquisitionError::Storage);
                }
                self.clear_intent(id)
            }
            Some(Intent::Forgetting { account, manifest }) => {
                // Keys may already be gone. Acquire file locks without reading or recreating credentials.
                let mut locks = vec![];
                for dir in self.directories(effects) {
                    locks.push(EncryptedStore::lock_directory(&dir, id)?);
                }
                self.finish_forget(&account, effects, &manifest)
            }
            Some(Intent::Rotating {
                before,
                after,
                stores,
            }) => {
                let current = self.load_metadata(id)?;
                if current.generation == before.generation
                    && matches!(
                        self.backend.get(&after.local_secret_ref),
                        Err(AcquisitionError::CredentialMissing)
                    )
                {
                    // No staged ciphertext can exist before its key. Roll back the empty rotation.
                    for dir in self.directories(effects) {
                        if cipher_path(&dir, id, after.generation).exists() {
                            return Err(AcquisitionError::Storage);
                        }
                    }
                    if before.identity_ref.is_none() {
                        self.backend.delete(
                            after
                                .identity_ref
                                .as_ref()
                                .ok_or(AcquisitionError::Storage)?,
                        )?;
                    }
                    return self.clear_intent(id);
                }
                let locked = self.lock_stores(&current, effects)?;
                self.finish_rotation(&before, &after, effects, &stores, locked)
            }
        }
    }
}
fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn nonce() -> String {
    let mut random = [0; 16];
    OsRng.fill_bytes(&mut random);
    data_encoding::HEXLOWER.encode(&random)
}
fn validate_password(password: &SecretValue) -> Result<(), AcquisitionError> {
    if password.bytes().is_empty()
        || password.bytes().len() > 1024
        || password.text()?.chars().any(char::is_control)
    {
        Err(AcquisitionError::InvalidInput)
    } else {
        Ok(())
    }
}
fn cipher_path(dir: &Path, id: &AccountId, generation: u64) -> PathBuf {
    dir.join(if generation == 0 {
        format!("{}.census.enc", id.as_str())
    } else {
        format!("{}.g{generation}.census.enc", id.as_str())
    })
}
fn attributable_staging(dir: &Path, id: &AccountId) -> Result<Vec<PathBuf>, AcquisitionError> {
    if !dir.try_exists().map_err(|_| AcquisitionError::Storage)? {
        return Ok(vec![]);
    }
    let mut paths = vec![];
    for entry in fs::read_dir(dir).map_err(|_| AcquisitionError::Storage)? {
        let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(&format!(".{}.stage-", id.as_str())))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, AcquisitionError> {
    let m = fs::symlink_metadata(path).map_err(|_| AcquisitionError::Storage)?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > limit {
        return Err(AcquisitionError::Storage);
    }
    let mut bytes = vec![];
    fs::File::open(path)
        .and_then(|f| f.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|_| AcquisitionError::Storage)?;
    if bytes.len() as u64 > limit {
        return Err(AcquisitionError::Storage);
    }
    Ok(bytes)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AcquisitionError> {
    let parent = path.parent().ok_or(AcquisitionError::Storage)?;
    private_directory(parent)?;
    let prefix = format!(
        ".{}.stage-",
        path.file_stem()
            .and_then(|s| s.to_str())
            .ok_or(AcquisitionError::Storage)?
    );
    let mut file = tempfile::Builder::new()
        .prefix(&prefix)
        .tempfile_in(parent)
        .map_err(|_| AcquisitionError::Storage)?;
    serde_json::to_writer(&mut file, value).map_err(|_| AcquisitionError::Storage)?;
    file.flush()
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| AcquisitionError::Storage)?;
    file.persist(path).map_err(|_| AcquisitionError::Storage)?;
    sync_dir(parent)
}
#[cfg(test)]
pub(crate) fn write_new_secret(path: &Path, bytes: &[u8]) -> Result<(), AcquisitionError> {
    let parent = path.parent().ok_or(AcquisitionError::Storage)?;
    let reference = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(AcquisitionError::Storage)?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{reference}.stage-"))
        .tempfile_in(parent)
        .map_err(|_| AcquisitionError::Storage)?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| AcquisitionError::Storage)?;
    boundary("credential_staged")?;
    file.persist_noclobber(path)
        .map_err(|_| AcquisitionError::Storage)?;
    sync_dir(parent)
}
pub(crate) fn remove_file(path: &Path) -> Result<(), AcquisitionError> {
    match fs::remove_file(path) {
        Ok(()) => sync_dir(path.parent().ok_or(AcquisitionError::Storage)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AcquisitionError::Storage),
    }
}
fn sync_dir(path: &Path) -> Result<(), AcquisitionError> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| AcquisitionError::Storage)
}
pub(crate) fn boundary(name: &str) -> Result<(), AcquisitionError> {
    #[cfg(test)]
    {
        if FAIL.with(|f| {
            let mut state = f.borrow_mut();
            let Some(target) = state.0.clone() else {
                return false;
            };
            let (phase, nth) = target
                .split_once('#')
                .map(|(p, n)| (p, n.parse::<usize>().unwrap()))
                .unwrap_or((&target, 1));
            if phase != name {
                return false;
            }
            state.1 += 1;
            state.1 == nth
        }) {
            return Err(AcquisitionError::Storage);
        }
        if std::env::var("MAILROOM_LIFECYCLE_BOUNDARY").ok().as_deref() == Some(name) {
            if let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") {
                fs::write(
                    Path::new(&root).join(format!("reached-{name}")),
                    b"synthetic lifecycle boundary",
                )
                .map_err(|_| AcquisitionError::Storage)?;
                while !Path::new(&root).join(format!("continue-{name}")).exists() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    let _ = name;
    Ok(())
}
#[cfg(test)]
thread_local! { static FAIL: std::cell::RefCell<(Option<String>, usize)> = const { std::cell::RefCell::new((None,0)) }; }
#[cfg(test)]
mod tests;
