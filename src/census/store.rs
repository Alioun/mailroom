use super::{AccountId, Census, CensusError};
use crate::file_lock::ExclusiveFileLock;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use rusqlite::Connection;
use sha2::Sha256;
use std::{
    cell::Cell,
    fs,
    io::{Read, Write},
    path::PathBuf,
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"MAILROOM-CENSUS\x01";
const MAX_SNAPSHOT: u64 = 128 * 1024 * 1024;

pub struct LocalSecret(Zeroizing<Vec<u8>>);

impl LocalSecret {
    pub fn random() -> Self {
        let mut secret = Zeroizing::new(vec![0; 32]);
        OsRng.fill_bytes(&mut secret);
        Self(secret)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, CensusError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.len() != 32 {
            return Err(CensusError::Authentication);
        }
        Ok(Self(bytes))
    }
}

impl std::fmt::Debug for LocalSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalSecret([REDACTED])")
    }
}

pub struct CensusKeys {
    account: AccountId,
    encryption: Zeroizing<[u8; 32]>,
    placeholders: Zeroizing<[u8; 32]>,
    identity: Option<Zeroizing<Vec<u8>>>,
    generation: u64,
    recovery_authority: Option<String>,
}

impl CensusKeys {
    pub(super) fn belongs_to(&self, account: &AccountId) -> bool {
        self.account == *account
    }

    pub fn derive(secret: &LocalSecret, account: &AccountId) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(account.as_str().as_bytes()), &secret.0);
        let mut encryption = Zeroizing::new([0; 32]);
        let mut placeholders = Zeroizing::new([0; 32]);
        hkdf.expand(b"mailroom/census/v1/aes-256-gcm", encryption.as_mut())
            .expect("valid HKDF length");
        hkdf.expand(b"mailroom/census/v1/hmac-sha256", placeholders.as_mut())
            .expect("valid HKDF length");
        Self {
            account: account.clone(),
            encryption,
            placeholders,
            identity: None,
            generation: 0,
            recovery_authority: None,
        }
    }

    pub(crate) fn identity_material(&self) -> Vec<u8> {
        let mut value = self.placeholders.to_vec();
        for k in [
            self.synthetic_mailbox_keys(),
            self.synthetic_invoice_keys(),
            self.synthetic_unsubscribe_keys(),
        ] {
            value.extend_from_slice(k.placeholders.as_ref());
        }
        value
    }
    pub(crate) fn rotated(
        secret: &LocalSecret,
        account: &AccountId,
        identity: &[u8],
        generation: u64,
    ) -> Result<Self, CensusError> {
        if identity.len() != 128 {
            return Err(CensusError::Authentication);
        }
        let mut keys = Self::derive(secret, account);
        keys.placeholders.copy_from_slice(&identity[..32]);
        keys.identity = Some(Zeroizing::new(identity.to_vec()));
        keys.generation = generation;
        Ok(keys)
    }
    pub(crate) fn recovery_authority(&mut self, authority: &str) {
        self.recovery_authority = Some(authority.to_owned());
    }

    pub(crate) fn binding(&self, value: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.placeholders.as_ref())
            .expect("valid HMAC key");
        mac.update(b"mailroom/report-binding/v1\0");
        if self.generation > 0 {
            mac.update(&self.generation.to_be_bytes());
        }
        if let Some(authority) = &self.recovery_authority {
            mac.update(authority.as_bytes());
        }
        mac.update(value);
        data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
    }

    pub(crate) fn binding_for(&self, purpose: &[u8], value: &[u8]) -> String {
        if let Some(authority) = &self.recovery_authority {
            let payload = [authority.as_bytes(), &self.generation.to_be_bytes(), value].concat();
            return self.identity_binding_for(purpose, &payload);
        }
        if self.generation == 0 {
            return self.identity_binding_for(purpose, value);
        }
        let payload = [self.generation.to_be_bytes().as_slice(), value].concat();
        self.identity_binding_for(purpose, &payload)
    }
    pub(crate) fn identity_binding_for(&self, purpose: &[u8], value: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.placeholders.as_ref())
            .expect("valid HMAC key");
        mac.update(b"mailroom/purpose-binding/v1\0");
        mac.update(&(purpose.len() as u64).to_be_bytes());
        mac.update(purpose);
        mac.update(value);
        data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
    }

    pub(crate) fn synthetic_mailbox_keys(&self) -> Self {
        self.synthetic_keys(b"mailroom/synthetic-mailbox/v1")
    }

    pub(crate) fn synthetic_invoice_keys(&self) -> Self {
        self.synthetic_keys(b"mailroom/synthetic-invoice-transport/v1")
    }

    pub(crate) fn synthetic_unsubscribe_keys(&self) -> Self {
        self.synthetic_keys(b"mailroom/synthetic-unsubscribe/v1")
    }

    fn synthetic_keys(&self, purpose: &[u8]) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(purpose), self.encryption.as_ref());
        let mut bytes = Zeroizing::new(vec![0; 32]);
        hkdf.expand(b"independent-fake-state", &mut bytes)
            .expect("HKDF length");
        let mut keys = Self::derive(
            &LocalSecret::from_bytes(bytes.to_vec()).expect("key length"),
            &self.account,
        );
        if let Some(identity) = &self.identity {
            let offset = match purpose {
                b"mailroom/synthetic-mailbox/v1" => 32,
                b"mailroom/synthetic-invoice-transport/v1" => 64,
                _ => 96,
            };
            keys.placeholders
                .copy_from_slice(&identity[offset..offset + 32]);
        }
        keys.generation = self.generation;
        keys
    }

    pub fn placeholder(&self, kind: &str, value: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.placeholders.as_ref())
            .expect("valid HMAC key");
        mac.update(kind.as_bytes());
        mac.update(&[0]);
        mac.update(value.trim().to_lowercase().as_bytes());
        let encoded = data_encoding::BASE32_NOPAD.encode(&mac.finalize().into_bytes());
        format!("{kind}:{}", encoded[..6].to_ascii_lowercase())
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sieve_placement: Option<crate::rules::composition::Placement>,
    #[serde(default)]
    pub protocol_backlog: crate::backlog::protocol::Journal,
    #[serde(
        default,
        skip_serializing_if = "crate::backlog::gmail_journal::Journal::is_empty"
    )]
    pub gmail_backlog: crate::backlog::gmail_journal::Journal,
    #[serde(default)]
    pub folder_provisioning: super::folder_provisioning::FolderJournal,
    #[serde(default)]
    pub synthetic_folders: Option<super::folder_provisioning::FakeProvider>,
    #[serde(default)]
    pub sieve_transactions: super::sieve_transaction::Journal,
    #[serde(default)]
    pub synthetic_sieve: Option<super::sieve_transaction::FakeProvider>,
    #[serde(default)]
    pub sieve: super::sieve::Facts,
    #[serde(default)]
    pub proposals: crate::rules::proposals::Decisions,
    #[serde(default)]
    pub winddown: crate::winddown::Aggregate,
    #[serde(default)]
    pub ledger: crate::ledger::Projection,
    #[serde(default)]
    pub invoices: crate::invoice::Aggregate,
    #[serde(default)]
    pub unsubscribe: crate::unsubscribe::Aggregate,
    #[serde(default)]
    pub synthetic_uid_next: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    pub backlog: Vec<crate::backlog::Run>,
    #[serde(default)]
    pub synthetic_effects: std::collections::BTreeMap<String, crate::backlog::Observation>,
    pub revision: u64,
    pub census: Census,
    pub workspace: Option<crate::rules::workspace::Workspace>,
    pub exports: Vec<crate::rules::workspace::ExportRecord>,
}
impl AccountState {
    fn new(census: Census) -> Self {
        Self {
            sieve_placement: None,
            protocol_backlog: Default::default(),
            gmail_backlog: Default::default(),
            folder_provisioning: Default::default(),
            synthetic_folders: None,
            sieve: Default::default(),
            sieve_transactions: Default::default(),
            synthetic_sieve: None,
            proposals: Default::default(),
            revision: 0,
            winddown: Default::default(),
            ledger: Default::default(),
            invoices: Default::default(),
            unsubscribe: Default::default(),
            backlog: Vec::new(),
            synthetic_effects: Default::default(),
            synthetic_uid_next: Default::default(),
            census,
            workspace: None,
            exports: Vec::new(),
        }
    }
}

pub struct EncryptedStore {
    directory: PathBuf,
    keys: CensusKeys,
    _lock: ExclusiveFileLock,
    loaded: Cell<bool>,
    account_lease: Option<ExclusiveFileLock>,
    #[cfg(test)]
    simulated_provider: bool,
}

impl EncryptedStore {
    pub fn new(directory: PathBuf, keys: CensusKeys) -> Result<Self, CensusError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)
                .map_err(|_| CensusError::Storage)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(&directory).map_err(|_| CensusError::Storage)?;
        let metadata = fs::symlink_metadata(&directory).map_err(|_| CensusError::Storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CensusError::Storage);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| CensusError::Storage)?;
        }
        let lock = Self::lock_directory(&directory, &keys.account)?;
        Ok(Self {
            directory,
            keys,
            _lock: lock,
            loaded: Cell::new(false),
            account_lease: None,
            #[cfg(test)]
            simulated_provider: false,
        })
    }
    /// Marks a store that stands in for a remote server's own storage. Its
    /// writes stay atomic against process death but skip device flushes, which
    /// only guard against power loss, so a fake server answers at server speed
    /// instead of flushing the disk twice per protocol command.
    #[cfg(test)]
    pub(crate) fn simulated_provider(mut self) -> Self {
        self.simulated_provider = true;
        self
    }
    fn sync(&self, file: &fs::File) -> std::io::Result<()> {
        #[cfg(test)]
        if self.simulated_provider {
            return Ok(());
        }
        file.sync_all()
    }

    pub(crate) fn lock_directory(
        directory: &std::path::Path,
        account: &AccountId,
    ) -> Result<ExclusiveFileLock, CensusError> {
        fs::create_dir_all(directory).map_err(|_| CensusError::Storage)?;
        let metadata = fs::symlink_metadata(directory).map_err(|_| CensusError::Storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CensusError::Storage);
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options
            .open(directory.join(format!("{}.lock", account.as_str())))
            .map_err(|_| CensusError::Storage)?;
        if !lock.metadata().map_err(|_| CensusError::Storage)?.is_file() {
            return Err(CensusError::Storage);
        }
        ExclusiveFileLock::try_lock(lock).map_err(|_error| {
            #[cfg(test)]
            eprintln!("census.lock.error={_error:?}");
            CensusError::Storage
        })
    }
    pub(crate) fn retain_account_lease(&mut self, lease: ExclusiveFileLock) {
        self.account_lease = Some(lease);
    }
    pub(crate) fn replace_keys(&mut self, keys: CensusKeys) {
        self.keys = keys;
        self.loaded.set(false);
    }
    pub fn keys(&self) -> &CensusKeys {
        &self.keys
    }

    pub fn path(&self) -> PathBuf {
        self.directory.join(if self.keys.generation == 0 {
            format!("{}.census.enc", self.keys.account.as_str())
        } else {
            format!(
                "{}.g{}.census.enc",
                self.keys.account.as_str(),
                self.keys.generation
            )
        })
    }

    fn context(&self) -> Vec<u8> {
        [MAGIC, self.keys.account.as_str().as_bytes()].concat()
    }

    pub fn save(&self, census: &Census) -> Result<(), CensusError> {
        if census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        let mut state = if self.path().try_exists().map_err(|_| CensusError::Storage)? {
            let loaded = self.loaded.get();
            let state = self.read_state()?;
            if !loaded {
                return Err(CensusError::Storage);
            }
            state
        } else {
            AccountState::new(census.clone())
        };
        state.census = census.clone();
        self.save_state(&mut state)
    }

    pub(crate) fn recovery_pending(&self) -> bool {
        self.keys.recovery_authority.is_some()
    }

    pub fn save_state(&self, state: &mut AccountState) -> Result<(), CensusError> {
        if self.keys.recovery_authority.is_some() {
            return Err(CensusError::Storage);
        }
        if state.census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        if state
            .exports
            .iter()
            .any(|e| e.context_binding.as_ref().is_some_and(|b| b.len() > 128))
        {
            return Err(CensusError::Storage);
        }
        if state.exports.len() > crate::rules::workspace::HISTORY_LIMIT {
            return Err(CensusError::Storage);
        }
        let current = if self.path().try_exists().map_err(|_| CensusError::Storage)? {
            self.read_state()?.revision
        } else {
            0
        };
        if state.revision != current {
            return Err(CensusError::Storage);
        }
        let mut next = state.clone();
        next.revision = current.checked_add(1).ok_or(CensusError::Storage)?;
        self.stage_state(&next)?
            .persist(self.path())
            .map_err(|_| CensusError::Storage)?;
        fs::File::open(&self.directory)
            .and_then(|f| self.sync(&f))
            .map_err(|_| CensusError::Storage)?;
        *state = next;
        self.loaded.set(true);
        Ok(())
    }

    #[cfg(test)]
    fn stage(&self, census: &Census) -> Result<tempfile::NamedTempFile, CensusError> {
        let mut state = self.load_state()?;
        state.census = census.clone();
        self.stage_state(&state)
    }

    pub(crate) fn stage_state(
        &self,
        state: &AccountState,
    ) -> Result<tempfile::NamedTempFile, CensusError> {
        if state.census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        state
            .proposals
            .validate()
            .map_err(|_| CensusError::Storage)?;
        state.sieve.validate(&state.census.account)?;
        state.sieve_transactions.validate(&state.census.account)?;
        if state
            .sieve_placement
            .as_ref()
            .is_some_and(|p| !p.valid_for(""))
        {
            return Err(CensusError::Storage);
        }
        state.folder_provisioning.validate(&state.census.account)?;
        state.protocol_backlog.validate(&state.census.account)?;
        state.gmail_backlog.validate(&state.census.account)?;
        if state
            .synthetic_folders
            .as_ref()
            .is_some_and(|f| !f.validate())
        {
            return Err(CensusError::Storage);
        }
        if state.synthetic_sieve.as_ref().is_some_and(|f| {
            !f.address.ip().is_loopback()
                || f.epoch.is_empty()
                || f.root.len() > 8192
                || f.key.len() > 8192
                || f.commands.len() > 4096
                || !f.image.validate()
        }) {
            return Err(CensusError::Storage);
        }
        if state
            .census
            .folders
            .values()
            .flat_map(|f| f.messages.values())
            .any(|m| !m.delivery.validate())
        {
            return Err(CensusError::Storage);
        }
        state.census.acquisition.validate(&state.census.account)?;
        let snapshot =
            Zeroizing::new(serde_json::to_string(state).map_err(|_| CensusError::Storage)?);
        self.stage_snapshot(18, &snapshot)
    }

    fn stage_snapshot(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<tempfile::NamedTempFile, CensusError> {
        let db = memory_database()?;
        db.execute_batch("CREATE TABLE census (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, account TEXT NOT NULL, snapshot TEXT NOT NULL)").map_err(|_| CensusError::Storage)?;
        db.execute(
            "INSERT INTO census VALUES(1, ?1, ?2, ?3)",
            (version, self.keys.account.as_str(), snapshot),
        )
        .map_err(|_| CensusError::Storage)?;
        let sqlite = db.serialize("main").map_err(|_| CensusError::Storage)?;
        if sqlite.len() as u64 > MAX_SNAPSHOT {
            return Err(CensusError::Storage);
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &sqlite,
                    aad: &self.context(),
                },
            )
            .map_err(|_| CensusError::Storage)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".{}.stage-", self.keys.account.as_str()))
            .tempfile_in(&self.directory)
            .map_err(|_| CensusError::Storage)?;
        temporary
            .write_all(MAGIC)
            .and_then(|_| temporary.write_all(&nonce))
            .and_then(|_| temporary.write_all(&encrypted))
            .map_err(|_| CensusError::Storage)?;
        self.sync(temporary.as_file())
            .map_err(|_| CensusError::Storage)?;
        Ok(temporary)
    }

    pub(crate) fn install_snapshot(&self, state: &AccountState) -> Result<(), CensusError> {
        self.stage_state(state)?
            .persist_noclobber(self.path())
            .map_err(|_| CensusError::Storage)?;
        fs::File::open(&self.directory)
            .and_then(|f| self.sync(&f))
            .map_err(|_| CensusError::Storage)?;
        Ok(())
    }
    pub fn load(&self) -> Result<Census, CensusError> {
        Ok(self.load_state()?.census)
    }

    pub fn load_state(&self) -> Result<AccountState, CensusError> {
        let state = self.read_state()?;
        self.loaded.set(true);
        Ok(state)
    }

    pub(crate) fn schema_version(&self) -> Result<u32, CensusError> {
        self.read_state_versioned().map(|(version, _)| version)
    }

    fn read_state(&self) -> Result<AccountState, CensusError> {
        self.read_state_versioned().map(|(_, state)| state)
    }

    fn read_state_versioned(&self) -> Result<(u32, AccountState), CensusError> {
        let path = self.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| CensusError::Storage)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_SNAPSHOT + 128
        {
            return Err(CensusError::Storage);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|f| f.take(MAX_SNAPSHOT + 129).read_to_end(&mut bytes))
            .map_err(|_| CensusError::Storage)?;
        if bytes.len() < MAGIC.len() + 12 + 16 {
            return Err(CensusError::Authentication);
        }
        if !bytes.starts_with(MAGIC) {
            return Err(CensusError::UnsupportedVersion);
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&bytes[MAGIC.len()..MAGIC.len() + 12]),
                    Payload {
                        msg: &bytes[MAGIC.len() + 12..],
                        aad: &self.context(),
                    },
                )
                .map_err(|_| CensusError::Authentication)?,
        );
        let mut db = memory_database()?;
        db.deserialize_read_exact("main", plaintext.as_slice(), plaintext.len(), true)
            .map_err(|_| CensusError::Storage)?;
        let (version, account, snapshot): (u32, String, String) = db
            .query_row(
                "SELECT version, account, snapshot FROM census WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| CensusError::Storage)?;
        let snapshot = Zeroizing::new(snapshot);
        if !matches!(
            version,
            1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18
        ) {
            return Err(CensusError::UnsupportedVersion);
        }
        if account != self.keys.account.as_str() {
            return Err(CensusError::AccountMismatch);
        }
        if version < 15 {
            let raw: serde_json::Value =
                serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
            fn protocol_fields(v: &serde_json::Value) -> bool {
                match v {
                    serde_json::Value::Object(o) => o.iter().any(|(k, v)| {
                        if k == "protocol_backlog" {
                            v.as_object().is_none_or(|o| o.len() != 1)
                                || v.get("runs")
                                    .is_none_or(|r| r.as_array().is_none_or(|a| !a.is_empty()))
                        } else {
                            k == "execution_proof"
                                || k == "move_evidence"
                                || k == "semantics"
                                    && v.as_str() == Some("imap-move-mapping-readback/v1")
                                || protocol_fields(v)
                        }
                    }),
                    serde_json::Value::Array(a) => a.iter().any(protocol_fields),
                    _ => false,
                }
            }
            if protocol_fields(&raw) {
                return Err(CensusError::UnsupportedVersion);
            }
        }
        if version < 18 {
            let raw: serde_json::Value =
                serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
            fn label_fields(v: &serde_json::Value) -> bool {
                match v {
                    serde_json::Value::Object(o) => o.iter().any(|(k, v)| {
                        k == "gmail_backlog"
                            || k == "semantics"
                                && v.as_str() == Some(crate::backlog::gmail_journal::SEMANTICS)
                            || label_fields(v)
                    }),
                    serde_json::Value::Array(a) => a.iter().any(label_fields),
                    _ => false,
                }
            }
            if label_fields(&raw) {
                return Err(CensusError::UnsupportedVersion);
            }
        }
        if version < 14 {
            let raw: serde_json::Value =
                serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
            if has_delivery_semantics(&raw) {
                return Err(CensusError::UnsupportedVersion);
            }
        }
        let mut state: AccountState = if version == 1 {
            let legacy: serde_json::Value =
                serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
            if legacy.get("folder_provisioning").is_some()
                || legacy.get("synthetic_folders").is_some()
                || legacy.get("sieve").is_some()
                || legacy.get("sieve_transactions").is_some()
                || legacy.get("synthetic_sieve").is_some()
            {
                return Err(CensusError::UnsupportedVersion);
            }
            AccountState::new(serde_json::from_value(legacy).map_err(|_| CensusError::Storage)?)
        } else {
            serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?
        };
        if version < 16
            && (state.sieve_placement.is_some()
                || state.sieve_transactions.history.iter().any(|t| {
                    t.composition.is_some()
                        || t.steps
                            .iter()
                            .any(|s| s.effect == super::sieve_transaction::Effect::UploadOwned)
                }))
        {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 17
            && state.sieve_transactions.history.iter().any(|t| {
                t.composition.as_ref().is_some_and(|c| {
                    c.semantics != crate::rules::composition::LEGACY_SEMANTICS
                        || c.precedence.as_ref().is_some_and(|p| p.copies.is_some())
                })
            })
        {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 13 && state.exports.iter().any(|e| e.context_binding.is_some()) {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 12
            && (state.folder_provisioning != Default::default()
                || state.synthetic_folders.is_some())
        {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 11
            && (state.sieve_transactions != Default::default()
                || state.synthetic_sieve.is_some()
                || state.sieve.invalidated_by_write.is_some())
        {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 10 && state.sieve != Default::default() {
            return Err(CensusError::UnsupportedVersion);
        }
        if version < 9 && !state.proposals.history.is_empty() {
            return Err(CensusError::UnsupportedVersion);
        }
        if state
            .exports
            .iter()
            .any(|e| e.context_binding.as_ref().is_some_and(|b| b.len() > 128))
        {
            return Err(CensusError::Storage);
        }
        if state.exports.len() > crate::rules::workspace::HISTORY_LIMIT {
            return Err(CensusError::Storage);
        }
        state
            .proposals
            .validate()
            .map_err(|_| CensusError::Storage)?;
        state.sieve.validate(&state.census.account)?;
        state.sieve_transactions.validate(&state.census.account)?;
        if state
            .sieve_placement
            .as_ref()
            .is_some_and(|p| !p.valid_for(""))
        {
            return Err(CensusError::Storage);
        }
        state.folder_provisioning.validate(&state.census.account)?;
        state.protocol_backlog.validate(&state.census.account)?;
        state.gmail_backlog.validate(&state.census.account)?;
        if state
            .synthetic_folders
            .as_ref()
            .is_some_and(|f| !f.validate())
        {
            return Err(CensusError::Storage);
        }
        if state.synthetic_sieve.as_ref().is_some_and(|f| {
            !f.address.ip().is_loopback()
                || f.epoch.is_empty()
                || f.root.len() > 8192
                || f.key.len() > 8192
                || f.commands.len() > 4096
                || !f.image.validate()
        }) {
            return Err(CensusError::Storage);
        }
        if state
            .census
            .folders
            .values()
            .flat_map(|f| f.messages.values())
            .any(|m| !m.delivery.validate())
        {
            return Err(CensusError::Storage);
        }
        state.census.acquisition.validate(&state.census.account)?;
        state.sieve.interrupted();
        state.sieve_transactions.interrupted();
        state.folder_provisioning.interrupted();
        state.protocol_backlog.interrupted();
        state.gmail_backlog.interrupted();
        state.census.acquisition.interrupted();
        let census = &mut state.census;
        if census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        // An interrupted scan can resume, but reopening cannot imply a running worker.
        if census.acquisition_state == super::ScanState::Running {
            census.acquisition_state = super::ScanState::Cancelled;
        }
        for folder in census.folders.values_mut() {
            if folder.state == super::ScanState::Running {
                folder.state = super::ScanState::Cancelled;
            }
        }
        for run in &mut state.backlog {
            if run.status == crate::backlog::Status::Running {
                run.status = if run
                    .outcomes
                    .iter()
                    .any(|o| matches!(o, crate::backlog::Outcome::Intent))
                {
                    crate::backlog::Status::Uncertain
                } else {
                    crate::backlog::Status::Prepared
                };
            }
        }
        for send in &mut state.invoices.sends {
            if send.status == crate::invoice::Status::Intent {
                send.status = crate::invoice::Status::Uncertain;
            }
        }
        for entry in &mut state.unsubscribe.history {
            if entry.status == crate::unsubscribe::Status::Intent {
                entry.status = crate::unsubscribe::Status::Uncertain;
            }
        }
        Ok((version, state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn old_schema_rejects_protocol_semantics_inside_a_valid_legacy_plan() {
        let root = tempfile::tempdir().unwrap();
        let input: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
        let census = crate::engine::synthetic_census(&input).unwrap();
        let store = EncryptedStore::new(
            root.path().into(),
            CensusKeys::derive(&LocalSecret::random(), &census.account),
        )
        .unwrap();
        let mut state = AccountState::new(census);
        state.backlog.push(crate::backlog::Run {
            id: "historical-legacy-run".into(),
            plan: crate::backlog::Plan {
                account: state.census.account.as_str().into(),
                provider: crate::acquisition::Provider::MailboxOrg,
                source: "synthetic".into(),
                state_revision: state.revision,
                ledger_version: String::new(),
                mailbox_revision: None,
                semantics: "imap-move-mapping-readback/v1".into(),
                artifact: crate::engine::artifact(&input.documents, &input.context).unwrap(),
                proof_gate: "pass".into(),
                options: crate::backlog::Options::default(),
                changes: vec![],
                excluded: Default::default(),
                unmatched: 0,
                review: 0,
                groups: Default::default(),
                stopped_folders: Default::default(),
                diagnostics: vec![],
            },
            status: crate::backlog::Status::Completed,
            outcomes: vec![],
            undo: vec![],
            undo_requested: false,
            batches: 0,
            error: None,
        });
        let mut value = serde_json::to_value(state).unwrap();
        value.as_object_mut().unwrap().remove("protocol_backlog");
        let mut legacy = value.clone();
        legacy["backlog"][0]["plan"]["semantics"] = "backlog-flags-preserved-no-copies-v1".into();
        store
            .stage_snapshot(14, &serde_json::to_string(&legacy).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(
            serde_json::to_value(store.load_state().unwrap().backlog).unwrap(),
            legacy["backlog"]
        );
        store
            .stage_snapshot(14, &serde_json::to_string(&value).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
    }

    #[test]
    fn protocol_schema_fifteen_keeps_old_absence_and_rejects_nested_authority() {
        for version in 1..=14 {
            let root = tempfile::tempdir().unwrap();
            let input: crate::engine::PreviewInput =
                serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
            let census = crate::engine::synthetic_census(&input).unwrap();
            let store = EncryptedStore::new(
                root.path().into(),
                CensusKeys::derive(&LocalSecret::random(), &census.account),
            )
            .unwrap();
            let mut value = if version == 1 {
                serde_json::to_value(census).unwrap()
            } else {
                serde_json::to_value(AccountState::new(census)).unwrap()
            };
            value.as_object_mut().unwrap().remove("protocol_backlog");
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(store.load_state().unwrap().protocol_backlog.runs.is_empty());
            for injection in [
                serde_json::json!({"protocol_backlog":{"runs":[{}]}}),
                serde_json::json!({"protocol_backlog":{"runs":[],"source":"forged"}}),
                serde_json::json!({"backlog":[{"plan":{"execution_proof":"forged"}}]}),
                serde_json::json!({"exports":[{"artifact":{"move_evidence":{}}}]}),
                serde_json::json!({"backlog":[{"plan":{"semantics":"imap-move-mapping-readback/v1"}}]}),
                serde_json::json!({"exports":[{"artifact":{"semantics":"imap-move-mapping-readback/v1"}}]}),
            ] {
                let mut changed = value.clone();
                changed
                    .as_object_mut()
                    .unwrap()
                    .extend(injection.as_object().unwrap().clone());
                store
                    .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                    .unwrap()
                    .persist(store.path())
                    .unwrap();
                assert!(
                    matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
                    "schema {version}"
                );
            }
        }
    }

    #[test]
    fn gmail_schema_eighteen_keeps_old_absence_and_rejects_label_semantics() {
        for version in 1..=18 {
            let root = tempfile::tempdir().unwrap();
            let input: crate::engine::PreviewInput =
                serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
            let census = crate::engine::synthetic_census(&input).unwrap();
            let store = EncryptedStore::new(
                root.path().into(),
                CensusKeys::derive(&LocalSecret::random(), &census.account),
            )
            .unwrap();
            let value = if version == 1 {
                serde_json::to_value(census).unwrap()
            } else {
                serde_json::to_value(AccountState::new(census)).unwrap()
            };
            assert!(
                value.get("gmail_backlog").is_none(),
                "Empty journal is omitted"
            );
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let mut loaded = store.load_state().unwrap();
            assert!(loaded.gmail_backlog.runs.is_empty());
            if version > 1 {
                store.save_state(&mut loaded).unwrap();
                assert_eq!(store.schema_version().unwrap(), 18);
                assert_eq!(
                    serde_json::to_value(&store.load_state().unwrap().protocol_backlog).unwrap(),
                    serde_json::to_value(&loaded.protocol_backlog).unwrap()
                );
            }
            for injection in [
                serde_json::json!({"gmail_backlog":{"runs":[]}}),
                serde_json::json!({"gmail_backlog":{"runs":[{}]}}),
                serde_json::json!({"backlog":[{"plan":{"semantics":"gmail-label-readback/v1"}}]}),
                serde_json::json!({"exports":[{"artifact":{"semantics":"gmail-label-readback/v1"}}]}),
            ] {
                let mut changed = value.clone();
                changed
                    .as_object_mut()
                    .unwrap()
                    .extend(injection.as_object().unwrap().clone());
                store
                    .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                    .unwrap()
                    .persist(store.path())
                    .unwrap();
                let expected_storage = version == 18;
                let result = store.load_state();
                assert!(
                    if expected_storage {
                        injection
                            .get("gmail_backlog")
                            .is_some_and(|j| j["runs"].as_array().unwrap().is_empty())
                            && result.is_ok()
                            || matches!(result, Err(CensusError::Storage))
                    } else {
                        matches!(result, Err(CensusError::UnsupportedVersion))
                    },
                    "schema {version} {injection}"
                );
            }
        }
    }

    #[test]
    fn delivery_schema_fourteen_preserves_legacy_absence_and_rejects_nested_smuggling() {
        for version in 1..=13 {
            let root = tempfile::tempdir().unwrap();
            let input: crate::engine::PreviewInput =
                serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
            let census = crate::engine::synthetic_census(&input).unwrap();
            let store = EncryptedStore::new(
                root.path().into(),
                CensusKeys::derive(&LocalSecret::random(), &census.account),
            )
            .unwrap();
            let value = if version == 1 {
                serde_json::to_value(&census).unwrap()
            } else {
                serde_json::to_value(AccountState::new(census)).unwrap()
            };
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let loaded = store.load_state().unwrap();
            assert!(
                loaded
                    .census
                    .folders
                    .values()
                    .flat_map(|f| f.messages.values())
                    .all(|m| m.delivery == Default::default())
            );
            for injected in ["message", "context", "tag", "yaml", "backlog"] {
                let mut changed = value.clone();
                if version == 1 {
                    changed["folders"]["INBOX"]["messages"]["1"]["delivery"] =
                        serde_json::json!({"status":"ambiguous"});
                } else {
                    match injected {
                        "message" => {
                            changed["census"]["folders"]["INBOX"]["messages"]["1"]["delivery"] =
                                serde_json::json!({"status":"unsupported"})
                        }
                        "context" => {
                            changed["exports"] = serde_json::json!([{"artifact":{"context":{"recipient_evidence":{"synthetic_envelope":{"source":{}}}}}}])
                        }
                        "tag" => {
                            changed["exports"] = serde_json::json!([{"artifact":{"effective":[{"definition":{"kind":"delivery_tag"}}]}}])
                        }
                        "backlog" => {
                            changed["backlog"] = serde_json::json!([{"plan":{"artifact":{"context":{"recipient_evidence":{"synthetic_envelope":{"source":{}}}}}}}])
                        }
                        _ => {
                            changed["workspace"] = serde_json::json!({"input":{"documents":{"override_yaml":"delivery_tags: [{id: user.injected}]"}}})
                        }
                    }
                }
                store
                    .stage_snapshot(version, &serde_json::to_string(&changed).unwrap())
                    .unwrap()
                    .persist(store.path())
                    .unwrap();
                assert!(
                    matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
                    "schema {version} {injected}"
                );
            }
            store
                .stage_state(&loaded)
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert_eq!(store.schema_version().unwrap(), 18);
            assert_eq!(
                serde_json::to_value(store.load_state().unwrap()).unwrap(),
                serde_json::to_value(loaded).unwrap()
            );
        }
    }

    #[test]
    fn obsolete_export_bytes_and_versions_survive_without_recompilation() {
        for (schema, version, executable) in [
            (
                13,
                "v3",
                "# Historical export retains its original line endings.\r\nkeep;\r\n",
            ),
            (
                14,
                "v4",
                "require [\"envelope\", \"imap4flags\"];\n\n#@ system.role_safety\nif true {\n  keep;\n  stop;\n}\n\n#@ user.route\nif envelope :all :is \"to\" \"route@business.test\" {\n  addflag \"$route\";\n}\n",
            ),
            (
                15,
                "v4",
                "require [\"envelope\", \"imap4flags\"];\r\n#@ system.role_safety\r\nif true {\r\n  keep;\r\n  stop;\r\n}\r\n",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (repo, account, _fake) =
                crate::acquisition::folder_provisioning::tests::install(root.path());
            let store = repo.store(&account).unwrap();
            let mut historical = serde_json::to_value(store.load_state().unwrap()).unwrap();
            before_composition(&mut historical);
            for export in historical["exports"].as_array_mut().unwrap() {
                export["semantics"] = serde_json::json!(format!("mailroom-sieve-subset-{version}"));
                export["proof_semantics"] =
                    serde_json::json!(format!("parsed-emitted-sieve-subset-{version}"));
                export["artifact"]["executable"] = serde_json::json!(executable);
            }
            let expected = historical["exports"].clone();
            store
                .stage_snapshot(schema, &serde_json::to_string(&historical).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let mut loaded = store.load_state().unwrap();
            let context = repo.workspace_context(&account, &store).unwrap();
            assert!(
                crate::rules::workspace::view(&loaded, &context)
                    .exports
                    .iter()
                    .all(|e| !e.usable),
                "{schema} {version}"
            );
            loaded.workspace.as_mut().unwrap().input.baseline =
                crate::rules::proof::Baseline::Previous;
            assert!(crate::rules::workspace::preview(&loaded, &context, store.keys()).is_err());
            store.save_state(&mut loaded).unwrap();
            assert_eq!(store.schema_version().unwrap(), 18);
            assert_eq!(
                serde_json::to_value(store.load_state().unwrap().exports).unwrap(),
                expected,
                "{schema} {version}"
            );
        }
    }

    #[test]
    fn pre_thirteen_exports_default_to_unusable_and_reject_authority_smuggling() {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        let store = repo.store(&account).unwrap();
        let saved = store.load_state().unwrap();
        assert!(!saved.exports.is_empty());
        let mut legacy = serde_json::to_value(&saved).unwrap();
        before_composition(&mut legacy);
        for export in legacy["exports"].as_array_mut().unwrap() {
            export.as_object_mut().unwrap().remove("context_binding");
        }
        for version in [11, 12] {
            let mut compatible = legacy.clone();
            if version < 12 {
                compatible
                    .as_object_mut()
                    .unwrap()
                    .remove("folder_provisioning");
                compatible
                    .as_object_mut()
                    .unwrap()
                    .remove("synthetic_folders");
            }
            store
                .stage_snapshot(version, &serde_json::to_string(&compatible).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let loaded = store.load_state().unwrap();
            assert!(loaded.exports.iter().all(|e| e.context_binding.is_none()));
            let context = repo.workspace_context(&account, &store).unwrap();
            assert!(
                crate::rules::workspace::view(&loaded, &context)
                    .exports
                    .iter()
                    .all(|e| !e.usable)
            );
            compatible["exports"][0]["context_binding"] = serde_json::json!("caller-claim");
            store
                .stage_snapshot(version, &serde_json::to_string(&compatible).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(matches!(
                store.load_state(),
                Err(CensusError::UnsupportedVersion)
            ));
        }
        store
            .stage_state(&saved)
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(store.schema_version().unwrap(), 18);
        assert_eq!(
            serde_json::to_value(store.load_state().unwrap().exports).unwrap(),
            serde_json::to_value(&saved.exports).unwrap()
        );
    }

    #[test]
    fn old_schemas_reject_folder_semantics_and_current_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, account, _fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        crate::acquisition::folder_provisioning::tests::commit_saved_workspace_folders(
            &mut repo,
            &account,
            root.path(),
        );
        let store = repo.store(&account).unwrap();
        let saved = store.load_state().unwrap();
        for version in 1..=11 {
            let mut value = if version == 1 {
                serde_json::to_value(&saved.census).unwrap()
            } else {
                serde_json::to_value(&saved).unwrap()
            };
            if let Some(exports) = value
                .get_mut("exports")
                .and_then(serde_json::Value::as_array_mut)
            {
                for export in exports {
                    export.as_object_mut().unwrap().remove("context_binding");
                }
            }
            value["folder_provisioning"] =
                serde_json::to_value(&saved.folder_provisioning).unwrap();
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(matches!(
                store.load_state(),
                Err(CensusError::UnsupportedVersion)
            ));
        }
        store
            .stage_state(&saved)
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(store.schema_version().unwrap(), 18);
        assert!(store.load_state().unwrap().folder_provisioning == saved.folder_provisioning);
        let mut bad = saved.clone();
        bad.folder_provisioning.history[0].plan.entries[0]
            .name
            .wire
            .push('x');
        assert!(store.stage_state(&bad).is_err());
        let mut bad = saved.clone();
        bad.folder_provisioning.history[0].steps[0]
            .name
            .decoded
            .push('x');
        assert!(store.stage_state(&bad).is_err());
    }

    #[test]
    fn old_schemas_reject_real_transaction_semantics_and_eleven_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) =
            crate::acquisition::sieve_transaction::tests::install(root.path());
        crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
        let store = repo.store(&account).unwrap();
        let state = store.load_state().unwrap();
        for version in 1..=10 {
            let mut value = if version == 1 {
                serde_json::to_value(&state.census).unwrap()
            } else {
                serde_json::to_value(&state).unwrap()
            };
            if version == 1 {
                value["sieve_transactions"] =
                    serde_json::to_value(&state.sieve_transactions).unwrap();
            }
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(matches!(
                store.load_state(),
                Err(CensusError::UnsupportedVersion)
            ));
        }
        store
            .stage_state(&state)
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(store.schema_version().unwrap(), 18);
        assert!(store.load_state().unwrap().sieve_transactions == state.sieve_transactions);
        let mut malformed = state.clone();
        malformed.sieve_transactions.history[0].after.bodies[1]
            .bytes
            .push(1);
        assert!(store.stage_state(&malformed).is_err());
        assert!(store.load_state().unwrap().sieve_transactions == state.sieve_transactions);
    }

    /// Schemas before 16 cannot carry a placement choice or composed transactions.
    fn before_composition(state: &mut serde_json::Value) {
        state.as_object_mut().unwrap().remove("sieve_placement");
        state["sieve_transactions"] = serde_json::json!({"history": []});
    }

    #[test]
    fn composition_needs_schema_sixteen_and_tampered_foreign_bytes_never_load() {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) =
            crate::acquisition::sieve_transaction::tests::install(root.path());
        crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
        let store = repo.store(&account).unwrap();
        let state = store.load_state().unwrap();
        assert!(state.sieve_transactions.history[0].composition.is_some());
        let mut placement_only = state.clone();
        placement_only.sieve_transactions = Default::default();
        let mut composition_only = state.clone();
        composition_only.sieve_placement = None;
        for version in [11, 15] {
            for candidate in [&state, &placement_only, &composition_only] {
                store
                    .stage_snapshot(version, &serde_json::to_string(candidate).unwrap())
                    .unwrap()
                    .persist(store.path())
                    .unwrap();
                assert!(matches!(
                    store.load_state(),
                    Err(CensusError::UnsupportedVersion)
                ));
            }
        }
        let mut legacy = placement_only.clone();
        legacy.sieve_placement = None;
        store
            .stage_snapshot(15, &serde_json::to_string(&legacy).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert_eq!(store.schema_version().unwrap(), 15);
        let mut reopened = store.load_state().unwrap();
        assert!(reopened.sieve_placement.is_none());
        store.save_state(&mut reopened).unwrap();
        assert_eq!(store.schema_version().unwrap(), 18);
        let mut tampered = state.clone();
        let t = &mut tampered.sieve_transactions.history[0];
        for image in std::iter::once(&mut t.after).chain(t.steps.iter_mut().map(|s| &mut s.after)) {
            let host = image
                .bodies
                .iter_mut()
                .find(|b| b.name == "Open-Xchange")
                .unwrap();
            let bytes = String::from_utf8(host.bytes.clone())
                .unwrap()
                .replace("Reisen", "Moved")
                .into_bytes();
            *host = super::super::sieve::Body::new(host.name.clone(), bytes);
        }
        assert!(store.stage_state(&tampered).is_err());
    }

    #[test]
    fn copy_first_composition_needs_schema_seventeen_and_legacy_records_stay_readable() {
        use crate::rules::composition::{Counts, LEGACY_SEMANTICS, SEMANTICS};
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) =
            crate::acquisition::sieve_transaction::tests::install(root.path());
        crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
        let store = repo.store(&account).unwrap();
        let current = store.load_state().unwrap();
        let composition = current.sieve_transactions.history[0]
            .composition
            .as_ref()
            .unwrap();
        assert_eq!(composition.semantics, SEMANTICS);
        assert!(composition.precedence.as_ref().unwrap().copies.is_some());
        let mut legacy = current.clone();
        let c = legacy.sieve_transactions.history[0]
            .composition
            .as_mut()
            .unwrap();
        c.semantics = LEGACY_SEMANTICS.into();
        c.precedence = Some(Counts {
            suppressed_copies: Some(0),
            copies: None,
            ..c.precedence.clone().unwrap()
        });
        let stage = |version, state: &AccountState| {
            store
                .stage_snapshot(version, &serde_json::to_string(state).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
        };
        stage(16, &current);
        assert!(matches!(
            store.load_state(),
            Err(CensusError::UnsupportedVersion)
        ));
        stage(16, &legacy);
        let mut reopened = store.load_state().unwrap();
        assert!(reopened.sieve_transactions == legacy.sieve_transactions);
        store.save_state(&mut reopened).unwrap();
        assert_eq!(store.schema_version().unwrap(), 18);
        assert!(store.load_state().unwrap().sieve_transactions == legacy.sieve_transactions);
        for mixed in [
            Counts {
                suppressed_copies: Some(0),
                ..Default::default()
            },
            Counts {
                copies: None,
                ..Default::default()
            },
        ] {
            let mut bad = current.clone();
            bad.sieve_transactions.history[0]
                .composition
                .as_mut()
                .unwrap()
                .precedence = Some(mixed);
            assert!(store.stage_state(&bad).is_err());
        }
        let mut bad = legacy.clone();
        bad.sieve_transactions.history[0]
            .composition
            .as_mut()
            .unwrap()
            .precedence = Some(Counts {
            suppressed_copies: Some(0),
            copies: Some(Default::default()),
            ..Default::default()
        });
        assert!(store.stage_state(&bad).is_err());
    }

    #[test]
    fn historical_schemas_decode_without_inventing_acquisition_evidence() {
        for version in 1..=10 {
            let directory = tempfile::tempdir().unwrap();
            let account = AccountId::new("historical-schema").unwrap();
            let store = EncryptedStore::new(
                directory.path().into(),
                CensusKeys::derive(&LocalSecret::random(), &account),
            )
            .unwrap();
            let census = Census::new(account);
            let mut value = if version == 1 {
                serde_json::to_value(&census).unwrap()
            } else {
                serde_json::to_value(AccountState::new(census)).unwrap()
            };
            if version == 1 {
                value.as_object_mut().unwrap().remove("acquisition");
            } else {
                value["census"]
                    .as_object_mut()
                    .unwrap()
                    .remove("acquisition");
            }
            if version > 1 {
                value.as_object_mut().unwrap().remove("proposals");
                value.as_object_mut().unwrap().remove("sieve");
            }
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            let mut state = store.load_state().unwrap();
            assert_eq!(state.census.acquisition, Default::default());
            assert!(state.exports.is_empty());
            assert!(state.proposals.history.is_empty());
            store.save_state(&mut state).unwrap();
            assert_eq!(store.schema_version().unwrap(), 18);
            assert_eq!(store.load().unwrap().acquisition, Default::default());
        }
    }

    #[test]
    fn sieve_semantics_cannot_be_smuggled_under_old_schema_or_invented_in_ten() {
        for version in [1, 2, 8, 9, 10] {
            let directory = tempfile::tempdir().unwrap();
            let account = AccountId::new("sieve-schema-boundary").unwrap();
            let store = EncryptedStore::new(
                directory.path().into(),
                CensusKeys::derive(&LocalSecret::random(), &account),
            )
            .unwrap();
            let mut saved = AccountState::new(Census::new(account));
            saved.sieve.version = 1;
            let value = if version == 1 {
                let mut value = serde_json::to_value(&saved.census).unwrap();
                value["sieve"] = serde_json::to_value(&saved.sieve).unwrap();
                value
            } else {
                serde_json::to_value(&saved).unwrap()
            };
            store
                .stage_snapshot(version, &serde_json::to_string(&value).unwrap())
                .unwrap()
                .persist(store.path())
                .unwrap();
            assert!(store.load_state().is_err());
        }
    }

    #[test]
    fn interruption_before_atomic_replacement_retains_the_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("interruption-test").unwrap();
        let store = EncryptedStore::new(
            dir.path().to_owned(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let original = Census::new(account);
        store.save(&original).unwrap();
        let before = fs::read(store.path()).unwrap();
        let mut next = original.clone();
        next.reconcile_folder("private-folder-name", 1, BTreeSet::from([1]))
            .unwrap();
        let staged = store.stage(&next).unwrap();
        let staged_bytes = fs::read(staged.path()).unwrap();
        assert!(
            !staged_bytes
                .windows(19)
                .any(|w| w == b"private-folder-name")
        );
        let (file, orphan) = staged.keep().unwrap();
        drop(file);
        assert_eq!(fs::read(store.path()).unwrap(), before);
        assert_eq!(store.load().unwrap(), original);
        // A crash can also leave an incomplete encrypted staging file.
        fs::write(&orphan, &staged_bytes[..20]).unwrap();
        assert_eq!(store.load().unwrap(), original);
        store.save(&next).unwrap();
        assert_eq!(
            store.load().unwrap().folders["private-folder-name"].pending(10),
            vec![1]
        );
    }
}

fn has_delivery_semantics(value: &serde_json::Value) -> bool {
    fn workspace_has_tags(value: &serde_json::Value) -> bool {
        value
            .pointer("/input/documents/override_yaml")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| serde_yaml_ng::from_str::<serde_json::Value>(text).ok())
            .and_then(|doc| doc.get("delivery_tags").cloned())
            .is_some_and(|tags| tags.as_array().is_none_or(|tags| !tags.is_empty()))
    }
    fn artifact_has_delivery(value: &serde_json::Value) -> bool {
        value
            .pointer("/context/recipient_evidence/synthetic_envelope")
            .is_some()
            || value
                .get("effective")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|rule| {
                    rule.pointer("/definition/kind")
                        .is_some_and(|kind| kind == "delivery_tag")
                })
    }
    let census = value.get("census").unwrap_or(value);
    let messages = census
        .get("folders")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|f| f.values())
        .filter_map(|f| f.get("messages").and_then(serde_json::Value::as_object))
        .flat_map(|m| m.values());
    if messages.into_iter().any(|m| m.get("delivery").is_some()) {
        return true;
    }
    if value.get("workspace").is_some_and(workspace_has_tags) {
        return true;
    }
    value
        .get("exports")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|export| {
            export.get("artifact").is_some_and(artifact_has_delivery)
                || export.get("workspace").is_some_and(workspace_has_tags)
        })
        || value
            .get("backlog")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .any(|run| {
                run.pointer("/plan/artifact")
                    .is_some_and(artifact_has_delivery)
            })
}

fn memory_database() -> Result<Connection, CensusError> {
    let db = Connection::open_in_memory().map_err(|_| CensusError::Storage)?;
    db.execute_batch(
        "PRAGMA temp_store=MEMORY; PRAGMA journal_mode=MEMORY; PRAGMA secure_delete=ON;",
    )
    .map_err(|_| CensusError::Storage)?;
    Ok(db)
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod durable_tests;
