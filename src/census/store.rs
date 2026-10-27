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
    cell::{Cell, RefCell},
    fs,
    io::{Read, Seek, Write},
    path::PathBuf,
};
use zeroize::Zeroizing;

mod delta;
mod log;
mod patch;

/// Version-1 files hold one snapshot and stay readable. New saves write
/// version 2 (`log::MAGIC_V2`), which can append journal frames.
const MAGIC: &[u8] = b"MAILROOM-CENSUS\x01";
/// Bound on one decrypted SQLite snapshot. A personal-scale census of 100,000
/// messages with full header evidence serializes to about 254 MB.
pub(crate) const MAX_SNAPSHOT: u64 = 512 * 1024 * 1024;
/// Bound on the whole account file: the base snapshot plus frames, which are
/// compacted before they outgrow the base.
pub(crate) const MAX_FILE: u64 = 2 * MAX_SNAPSHOT + 4096;
/// Frames may grow to a quarter of the base, and at least to this, before a
/// save compacts them into a new base. A framed file then stays within 1.25
/// times its base, recovery's resource limits see nearly the same size, and
/// compaction stays amortized over the frames it replaces.
const MIN_FRAME_BUDGET: u64 = 64 * 1024;
/// Schema of new base snapshots. 19 adds Q27 undo retention to the backlog
/// journals: `completed_at` on runs and compacted `summaries`.
pub(crate) const SCHEMA: u32 = 19;

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
    pub census: super::SharedCensus,
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
            census: census.into(),
            workspace: None,
            exports: Vec::new(),
        }
    }
}

/// Every persisted field of `AccountState` except the census, the revision
/// and the two backlog journals. A journal frame carries a structural patch
/// for each one that changed, found by comparing serialized bytes. The census
/// and the backlog journals grow with the mailbox, so `delta` compares them as
/// typed values instead. The exhaustive pattern makes a new field a compile
/// error until it is listed here.
macro_rules! frame_fields {
    ($($field:ident),* $(,)?) => {
        const FRAME_FIELDS: &[&str] = &[$(stringify!($field)),*];
        fn frame_field_bytes(state: &AccountState) -> Result<Vec<Vec<u8>>, CensusError> {
            let AccountState {
                census: _,
                revision: _,
                protocol_backlog: _,
                gmail_backlog: _,
                $($field),*
            } = state;
            Ok(vec![$(serde_json::to_vec($field).map_err(|_| CensusError::Storage)?),*])
        }
        fn frame_field_value(state: &AccountState, name: &str) -> Result<serde_json::Value, CensusError> {
            match name {
                "protocol_backlog" => serde_json::to_value(&state.protocol_backlog).map_err(|_| CensusError::Storage),
                "gmail_backlog" => serde_json::to_value(&state.gmail_backlog).map_err(|_| CensusError::Storage),
                $(stringify!($field) => serde_json::to_value(&state.$field).map_err(|_| CensusError::Storage),)*
                _ => Err(CensusError::UnsupportedVersion),
            }
        }
        fn apply_frame_field(
            state: &mut AccountState,
            name: &str,
            value: serde_json::Value,
        ) -> Result<(), CensusError> {
            let invalid = |_| CensusError::Storage;
            match name {
                "protocol_backlog" => state.protocol_backlog = serde_json::from_value(value).map_err(invalid)?,
                "gmail_backlog" => state.gmail_backlog = serde_json::from_value(value).map_err(invalid)?,
                $(stringify!($field) => {
                    state.$field = serde_json::from_value(value).map_err(invalid)?
                })*
                _ => return Err(CensusError::UnsupportedVersion),
            }
            Ok(())
        }
        /// Rebuilds the persisted state without reading the file.
        fn persisted_state(p: &Persisted) -> Result<AccountState, CensusError> {
            let mut fields = p.fields.iter();
            Ok(AccountState {
                census: p.census.clone(),
                revision: p.revision,
                protocol_backlog: p.protocol_backlog.clone(),
                gmail_backlog: p.gmail_backlog.clone(),
                $($field: serde_json::from_slice(fields.next().ok_or(CensusError::Storage)?)
                    .map_err(|_| CensusError::Storage)?),*
            })
        }
    };
}
frame_fields!(
    sieve_placement,
    folder_provisioning,
    synthetic_folders,
    sieve_transactions,
    synthetic_sieve,
    sieve,
    proposals,
    winddown,
    ledger,
    invoices,
    unsubscribe,
    synthetic_uid_next,
    backlog,
    synthetic_effects,
    workspace,
    exports,
);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    revision: u64,
    fields: std::collections::BTreeMap<String, patch::Patch>,
}

/// The file a store last read or wrote. Change times are included so an
/// in-place rewrite of the same length is noticed and the file read again.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    len: u64,
    modified: i64,
    changed: i64,
}
impl FileIdentity {
    fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                len: metadata.len(),
                modified: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                changed: metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                device: 0,
                inode: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos() as u64),
                len: metadata.len(),
                modified: 0,
                changed: 0,
            }
        }
    }
}

/// Where new frames go in a version-2 file.
#[derive(Clone, Copy)]
struct Tail {
    base_tag: [u8; 16],
    base_len: u64,
    valid_end: u64,
    next_sequence: u64,
}

/// What this store last read or wrote. A save trusts it only while the file
/// keeps the identity recorded here, so the revision check costs one `stat`.
struct Persisted {
    revision: u64,
    file: FileIdentity,
    tail: Option<Tail>,
    census: super::SharedCensus,
    /// Serialized frame fields as persisted, in `FRAME_FIELDS` order.
    fields: Vec<Vec<u8>>,
    protocol_backlog: crate::backlog::protocol::Journal,
    gmail_backlog: crate::backlog::gmail_journal::Journal,
}
impl Persisted {
    fn of(
        state: &AccountState,
        file: FileIdentity,
        tail: Option<Tail>,
    ) -> Result<Self, CensusError> {
        Ok(Self {
            revision: state.revision,
            file,
            tail,
            census: state.census.clone(),
            fields: frame_field_bytes(state)?,
            protocol_backlog: state.protocol_backlog.clone(),
            gmail_backlog: state.gmail_backlog.clone(),
        })
    }
}

pub struct EncryptedStore {
    directory: PathBuf,
    keys: CensusKeys,
    _lock: ExclusiveFileLock,
    loaded: Cell<bool>,
    persisted: RefCell<Option<Persisted>>,
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
            persisted: RefCell::new(None),
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
        #[cfg(test)]
        io::record(|s| s.fsyncs += 1);
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
        self.persisted.replace(None);
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
            if !self.loaded.get() {
                self.read_state()?;
                return Err(CensusError::Storage);
            }
            match self.unchanged_state()? {
                Some(state) => state,
                None => self.read_state()?,
            }
        } else {
            AccountState::new(census.clone())
        };
        state.census = census.clone().into();
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
        let current = self.current_revision()?;
        if state.revision != current {
            return Err(CensusError::Storage);
        }
        let next = current.checked_add(1).ok_or(CensusError::Storage)?;
        #[cfg(test)]
        let began = std::time::Instant::now();
        if !self.append_frame(state, next)? {
            state.revision = next;
            if let Err(e) = self.rewrite(state, false) {
                state.revision = current;
                return Err(e);
            }
        }
        state.revision = next;
        self.loaded.set(true);
        #[cfg(test)]
        if !self.simulated_provider {
            io::record(|s| {
                s.saves += 1;
                s.save_us += began.elapsed().as_micros() as u64;
            });
            if std::env::var_os("MAILROOM_SCALE_TRACE").is_some() {
                eprintln!(
                    "scale.save revision={} us={}",
                    state.revision,
                    began.elapsed().as_micros()
                );
            }
        }
        Ok(())
    }

    /// The state this store last read or wrote, with the reopening
    /// adjustments, while the file is still that one.
    fn unchanged_state(&self) -> Result<Option<AccountState>, CensusError> {
        let metadata = fs::symlink_metadata(self.path()).map_err(|_| CensusError::Storage)?;
        let persisted = self.persisted.borrow();
        let Some(p) = persisted
            .as_ref()
            .filter(|p| metadata.is_file() && p.file == FileIdentity::of(&metadata))
        else {
            return Ok(None);
        };
        let mut state = persisted_state(p)?;
        normalize_interrupted(&mut state);
        Ok(Some(state))
    }

    /// The revision on disk. Uses the recorded state while the file is the
    /// one this store last read or wrote, and reads the file otherwise.
    fn current_revision(&self) -> Result<u64, CensusError> {
        let metadata = match fs::symlink_metadata(self.path()) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.persisted.replace(None);
                return Ok(0);
            }
            Err(_) => return Err(CensusError::Storage),
        };
        if let Some(p) = self.persisted.borrow().as_ref() {
            if metadata.is_file() && p.file == FileIdentity::of(&metadata) {
                return Ok(p.revision);
            }
        }
        Ok(self.read_state()?.revision)
    }

    /// Appends the changed fields as one synced frame when the census is
    /// unchanged and the frames still fit their budget. Returns false when the
    /// save needs a new base instead.
    fn append_frame(&self, state: &AccountState, next: u64) -> Result<bool, CensusError> {
        let mut persisted = self.persisted.borrow_mut();
        let Some(p) = persisted.as_mut() else {
            return Ok(false);
        };
        let Some(tail) = p.tail.as_mut() else {
            return Ok(false);
        };
        let census_changed = state.census != p.census;
        self.validate(state, census_changed)?;
        let fields = frame_field_bytes(state)?;
        let mut changes = std::collections::BTreeMap::new();
        for ((name, now), before) in FRAME_FIELDS.iter().zip(&fields).zip(&p.fields) {
            if now == before {
                continue;
            }
            let parse = |bytes: &[u8]| {
                serde_json::from_slice::<serde_json::Value>(bytes).map_err(|_| CensusError::Storage)
            };
            let (old, new) = (parse(before)?, parse(now)?);
            let Some(change) = patch::diff(&old, &new) else {
                continue;
            };
            let mut check = old;
            patch::apply(&mut check, change.clone())?;
            if check != new {
                return Err(CensusError::Storage);
            }
            changes.insert(*name, change);
        }
        for (name, delta) in [
            (
                "protocol_backlog",
                delta::protocol_journal(&p.protocol_backlog, &state.protocol_backlog)?,
            ),
            (
                "gmail_backlog",
                delta::gmail_journal(&p.gmail_backlog, &state.gmail_backlog)?,
            ),
            (
                "census",
                if census_changed {
                    delta::census(&p.census, &state.census)?
                } else {
                    delta::Delta::Same
                },
            ),
        ] {
            match delta {
                delta::Delta::Same => (),
                delta::Delta::Patch(change) => {
                    changes.insert(name, change);
                }
                delta::Delta::Rewrite => return Ok(false),
            }
        }
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&serde_json::json!({"revision": next, "fields": changes}))
                .map_err(|_| CensusError::Storage)?,
        );
        if plaintext.len() + 16 > log::MAX_FRAME {
            return Ok(false);
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let frame = log::encode_frame(
            &cipher,
            self.keys.account.as_str(),
            &tail.base_tag,
            tail.next_sequence,
            &plaintext,
        )?;
        let frames = tail.valid_end - tail.base_len + frame.len() as u64;
        if frames > (tail.base_len / 4).max(MIN_FRAME_BUDGET)
            || tail.valid_end + frame.len() as u64 > MAX_FILE
        {
            return Ok(false);
        }
        let mut options = fs::OpenOptions::new();
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(self.path())
            .map_err(|_| CensusError::Storage)?;
        let metadata = file.metadata().map_err(|_| CensusError::Storage)?;
        if !metadata.is_file() || FileIdentity::of(&metadata) != p.file {
            return Err(CensusError::Storage);
        }
        file.set_len(tail.valid_end)
            .and_then(|_| file.seek(std::io::SeekFrom::Start(tail.valid_end)))
            .map_err(|_| CensusError::Storage)?;
        #[cfg(test)]
        if kill_requested("frame_half") {
            let _ = file.write_all(&frame[..frame.len() / 2]);
            let _ = file.sync_all();
            kill_point("frame_half");
        }
        file.write_all(&frame).map_err(|_| CensusError::Storage)?;
        self.sync(&file).map_err(|_| CensusError::Storage)?;
        #[cfg(test)]
        if !self.simulated_provider {
            io::record(|s| s.bytes_written += frame.len() as u64);
        }
        let written = file.metadata().map_err(|_| CensusError::Storage)?;
        decoded::appended(
            &self.path(),
            &decoded::key(&self.keys),
            p.file.len,
            tail.valid_end,
            &frame,
            state,
            next,
            FileIdentity::of(&written),
        );
        tail.valid_end += frame.len() as u64;
        tail.next_sequence += 1;
        p.file = FileIdentity::of(&written);
        p.revision = next;
        p.fields = fields;
        p.census = state.census.clone();
        p.protocol_backlog = state.protocol_backlog.clone();
        p.gmail_backlog = state.gmail_backlog.clone();
        Ok(true)
    }

    /// Writes the whole state as a new base and atomically replaces the file.
    fn rewrite(&self, state: &AccountState, noclobber: bool) -> Result<(), CensusError> {
        let revision = state.revision;
        self.validate(state, true)?;
        let snapshot =
            Zeroizing::new(serde_json::to_string(state).map_err(|_| CensusError::Storage)?);
        let (staged, tag, bytes) = self.stage_encoded(SCHEMA, &snapshot)?;
        drop(snapshot);
        #[cfg(test)]
        kill_point("base_staged");
        let path = self.path();
        if noclobber {
            staged
                .persist_noclobber(&path)
                .map_err(|_| CensusError::Storage)?;
        } else {
            staged.persist(&path).map_err(|_| CensusError::Storage)?;
        }
        #[cfg(test)]
        kill_point("base_renamed");
        fs::File::open(&self.directory)
            .and_then(|f| self.sync(&f))
            .map_err(|_| CensusError::Storage)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| CensusError::Storage)?;
        let file = FileIdentity::of(&metadata);
        debug_assert_eq!(revision, state.revision);
        let tail = Tail {
            base_tag: tag,
            base_len: file.len,
            valid_end: file.len,
            next_sequence: 0,
        };
        self.persisted
            .replace(Some(Persisted::of(state, file, Some(tail))?));
        decoded::put(
            path,
            decoded::key(&self.keys),
            bytes,
            SCHEMA,
            state,
            Some(tail),
            FileIdentity::of(&metadata),
        );
        Ok(())
    }

    #[cfg(test)]
    fn stage(&self, census: &Census) -> Result<tempfile::NamedTempFile, CensusError> {
        let mut state = self.load_state()?;
        state.census = census.clone().into();
        self.stage_state(&state)
    }

    /// Checks the bounded journals and, when `census` is set, every census
    /// record. A frame skips the census because it never changes it.
    fn validate(&self, state: &AccountState, census: bool) -> Result<(), CensusError> {
        if state.census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        if state
            .exports
            .iter()
            .any(|e| e.context_binding.as_ref().is_some_and(|b| b.len() > 128))
            || state.exports.len() > crate::rules::workspace::HISTORY_LIMIT
        {
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
        if census {
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
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn stage_state(
        &self,
        state: &AccountState,
    ) -> Result<tempfile::NamedTempFile, CensusError> {
        self.stage_base(state).map(|(staged, _)| staged)
    }

    #[cfg(test)]
    fn stage_base(
        &self,
        state: &AccountState,
    ) -> Result<(tempfile::NamedTempFile, [u8; 16]), CensusError> {
        self.validate(state, true)?;
        let snapshot =
            Zeroizing::new(serde_json::to_string(state).map_err(|_| CensusError::Storage)?);
        self.stage_versioned(SCHEMA, &snapshot)
    }

    #[cfg(test)]
    fn stage_snapshot(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<tempfile::NamedTempFile, CensusError> {
        self.stage_versioned(version, snapshot)
            .map(|(staged, _)| staged)
    }

    fn sqlite_image(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<Zeroizing<Vec<u8>>, CensusError> {
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
        Ok(Zeroizing::new(sqlite.to_vec()))
    }

    fn stage_bytes(&self, bytes: &[u8]) -> Result<tempfile::NamedTempFile, CensusError> {
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".{}.stage-", self.keys.account.as_str()))
            .tempfile_in(&self.directory)
            .map_err(|_| CensusError::Storage)?;
        temporary
            .write_all(bytes)
            .map_err(|_| CensusError::Storage)?;
        #[cfg(test)]
        if !self.simulated_provider {
            io::record(|s| s.bytes_written += bytes.len() as u64);
        }
        self.sync(temporary.as_file())
            .map_err(|_| CensusError::Storage)?;
        Ok(temporary)
    }

    #[cfg(test)]
    fn stage_versioned(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<(tempfile::NamedTempFile, [u8; 16]), CensusError> {
        self.stage_encoded(version, snapshot)
            .map(|(staged, tag, _)| (staged, tag))
    }

    /// Stages a base and also returns the file's bytes.
    fn stage_encoded(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<(tempfile::NamedTempFile, [u8; 16], Vec<u8>), CensusError> {
        let sqlite = self.sqlite_image(version, snapshot)?;
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let (bytes, tag) = log::encode_base(&cipher, self.keys.account.as_str(), &sqlite)?;
        Ok((self.stage_bytes(&bytes)?, tag, bytes))
    }

    /// Writes a version-1 file, the format older builds saved.
    #[cfg(test)]
    fn stage_v1(
        &self,
        version: u32,
        snapshot: &str,
    ) -> Result<tempfile::NamedTempFile, CensusError> {
        let sqlite = self.sqlite_image(version, snapshot)?;
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
        self.stage_bytes(&[MAGIC, &nonce, &encrypted].concat())
    }

    pub(crate) fn install_snapshot(&self, state: &AccountState) -> Result<(), CensusError> {
        self.rewrite(state, true)
    }
    pub fn load(&self) -> Result<Census, CensusError> {
        Ok(self.load_state()?.census.into_inner())
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
        #[cfg(test)]
        let began = std::time::Instant::now();
        let result = self.read_raw().map(|(version, mut state)| {
            normalize_interrupted(&mut state);
            (version, state)
        });
        #[cfg(test)]
        if !self.simulated_provider {
            let bytes = fs::metadata(self.path()).map(|m| m.len()).unwrap_or(0);
            io::record(|s| {
                s.reads += 1;
                s.bytes_read += bytes;
                s.read_us += began.elapsed().as_micros() as u64;
            });
        }
        result
    }

    /// Reads the persisted state exactly, without the reopening adjustments,
    /// and records it as the state this store last saw.
    fn read_raw(&self) -> Result<(u32, AccountState), CensusError> {
        let path = self.path();
        let mut file = fs::File::open(&path).map_err(|_| CensusError::Storage)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| CensusError::Storage)?;
        let opened = file.metadata().map_err(|_| CensusError::Storage)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || FileIdentity::of(&metadata) != FileIdentity::of(&opened)
            || metadata.len() > MAX_FILE
        {
            return Err(CensusError::Storage);
        }
        // Ciphertext, so it is not zeroized: wiping a personal-scale file
        // byte by byte cost more than reading it.
        let key = decoded::key(&self.keys);
        #[cfg(test)]
        if self.simulated_provider {
            if let Some((version, state, tail)) =
                decoded::get_stamped(&path, &key, FileIdentity::of(&metadata))
            {
                self.persisted.replace(Some(Persisted::of(
                    &state,
                    FileIdentity::of(&metadata),
                    tail,
                )?));
                return Ok((version, state));
            }
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| CensusError::Storage)?;
        if bytes.len() as u64 != metadata.len() {
            return Err(CensusError::Storage);
        }
        if let Some((version, state, tail)) = decoded::get(&path, &key, &bytes) {
            self.persisted.replace(Some(Persisted::of(
                &state,
                FileIdentity::of(&metadata),
                tail,
            )?));
            return Ok((version, state));
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let (plaintext, decoded) = if bytes.starts_with(log::MAGIC_V2) {
            let mut decoded = log::decode(&cipher, self.keys.account.as_str(), &bytes)?;
            (std::mem::take(&mut decoded.sqlite), Some(decoded))
        } else if bytes.starts_with(MAGIC) {
            if bytes.len() as u64 > MAX_SNAPSHOT + 128 {
                return Err(CensusError::Storage);
            }
            if bytes.len() < MAGIC.len() + 12 + 16 {
                return Err(CensusError::Authentication);
            }
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
            (plaintext, None)
        } else if bytes.len() < MAGIC.len() + 12 + 16 {
            return Err(CensusError::Authentication);
        } else {
            return Err(CensusError::UnsupportedVersion);
        };
        let ciphertext = bytes;
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
            1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19
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
        if version < 19 {
            let raw: serde_json::Value =
                serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
            let retained = |journal: Option<&serde_json::Value>| {
                journal.is_some_and(|j| {
                    j.get("summaries")
                        .is_some_and(|s| s.as_array().is_none_or(|a| !a.is_empty()))
                        || j.get("runs")
                            .and_then(|r| r.as_array())
                            .is_some_and(|runs| {
                                runs.iter().any(|r| r.get("completed_at").is_some())
                            })
                })
            };
            if retained(raw.get("protocol_backlog")) || retained(raw.get("gmail_backlog")) {
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
        let frames = decoded.as_ref().map_or(&[][..], |d| &d.frames[..]);
        if !frames.is_empty() && !matches!(version, 18 | SCHEMA) {
            return Err(CensusError::UnsupportedVersion);
        }
        let mut patched = std::collections::BTreeMap::new();
        for frame in frames {
            let frame: Frame = serde_json::from_slice(frame).map_err(|_| CensusError::Storage)?;
            if Some(frame.revision) != state.revision.checked_add(1) {
                return Err(CensusError::Storage);
            }
            state.revision = frame.revision;
            for (name, change) in frame.fields {
                if name == "census" {
                    delta::apply_census(&mut state.census, change)?;
                    continue;
                }
                let value = match patched.entry(name) {
                    std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::btree_map::Entry::Vacant(e) => {
                        let value = frame_field_value(&state, e.key())?;
                        e.insert(value)
                    }
                };
                patch::apply(value, change)?;
            }
        }
        for (name, value) in patched {
            apply_frame_field(&mut state, &name, value)?;
        }
        if version < 19
            && (!state.protocol_backlog.summaries.is_empty()
                || !state.gmail_backlog.summaries.is_empty()
                || state
                    .protocol_backlog
                    .runs
                    .iter()
                    .any(|r| r.completed_at.is_some())
                || state
                    .gmail_backlog
                    .runs
                    .iter()
                    .any(|r| r.completed_at.is_some()))
        {
            return Err(CensusError::UnsupportedVersion);
        }
        self.validate(&state, true)?;
        // Frames append only to a base of the current schema, so the first
        // save after an upgrade writes a new base.
        let tail = decoded.filter(|_| version == SCHEMA).map(|d| Tail {
            base_tag: d.base_tag,
            base_len: d.base_len,
            valid_end: d.valid_end,
            next_sequence: d.frames.len() as u64,
        });
        self.persisted.replace(Some(Persisted::of(
            &state,
            FileIdentity::of(&metadata),
            tail,
        )?));
        decoded::put(
            path,
            key,
            ciphertext,
            version,
            &state,
            tail,
            FileIdentity::of(&metadata),
        );
        Ok((version, state))
    }
}

/// Reopening a store never implies a running worker or an unanswered intent.
/// Recently decoded account files. Every request opens a fresh store and
/// reads its account file. Decrypting and parsing a personal-scale file takes
/// seconds, and a fresh census copy would also miss the census digest memo
/// (`census::binding`). An entry is reused only when the file's bytes equal
/// the bytes it was decoded from, byte for byte, under the same key, so a
/// changed, replaced or damaged file is always decoded again. Entries hold
/// ciphertext and the decoded state, so only a few are kept.
mod decoded {
    use super::{AccountState, CensusKeys, Tail};
    use sha2::{Digest, Sha256};
    use std::{path::PathBuf, sync::Mutex};

    struct Entry {
        path: PathBuf,
        key: [u8; 32],
        bytes: Vec<u8>,
        version: u32,
        state: AccountState,
        tail: Option<Tail>,
        stamp: Stamp,
    }
    /// Only stores that stand in for a remote server in tests trust the
    /// file's identity and change times in place of its bytes: their fixture
    /// rereads its whole mailbox for every protocol command.
    pub(super) type Stamp = super::FileIdentity;
    #[cfg(test)]
    pub(super) fn get_stamped(
        path: &std::path::Path,
        key: &[u8; 32],
        stamp: Stamp,
    ) -> Option<(u32, AccountState, Option<Tail>)> {
        let entries = ENTRIES.lock().expect("decoded cache");
        let entry = entries
            .iter()
            .find(|e| e.path == path && e.key == *key && e.stamp == stamp)?;
        Some((entry.version, entry.state.clone(), entry.tail))
    }
    static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    #[cfg(not(test))]
    const CAPACITY: usize = 4;
    /// Parallel tests share the cache; a larger one keeps an entry a test
    /// just wrote from being evicted before it reads it back.
    #[cfg(test)]
    const CAPACITY: usize = 64;

    pub(super) fn key(keys: &CensusKeys) -> [u8; 32] {
        Sha256::new()
            .chain_update(b"mailroom/decoded-cache/v1\0")
            .chain_update(keys.account.as_str())
            .chain_update([0])
            .chain_update(keys.encryption.as_ref())
            .finalize()
            .into()
    }
    pub(super) fn get(
        path: &std::path::Path,
        key: &[u8; 32],
        bytes: &[u8],
    ) -> Option<(u32, AccountState, Option<Tail>)> {
        let mut entries = ENTRIES.lock().expect("decoded cache");
        let at = entries
            .iter()
            .position(|e| e.path == path && e.key == *key && e.bytes == bytes)?;
        let entry = entries.remove(at);
        let hit = (entry.version, entry.state.clone(), entry.tail);
        entries.push(entry);
        Some(hit)
    }
    /// Keeps the entry for a file current after this process appended a
    /// frame to it. The entry is updated only if it holds exactly the bytes
    /// the file had before the append, and its new state is the state the
    /// frame was computed from, which reading the file again reproduces.
    pub(super) fn appended(
        path: &std::path::Path,
        key: &[u8; 32],
        len_before: u64,
        valid_end: u64,
        frame: &[u8],
        state: &AccountState,
        revision: u64,
        stamp: Stamp,
    ) {
        let mut entries = ENTRIES.lock().expect("decoded cache");
        let Some(at) = entries
            .iter()
            .position(|e| e.path == path && e.key == *key && e.bytes.len() as u64 == len_before)
        else {
            return;
        };
        let mut entry = entries.remove(at);
        let Some(tail) = entry.tail.as_mut() else {
            return;
        };
        entry.bytes.truncate(valid_end as usize);
        entry.bytes.extend_from_slice(frame);
        tail.valid_end = valid_end + frame.len() as u64;
        tail.next_sequence += 1;
        entry.state = state.clone();
        entry.state.revision = revision;
        entry.stamp = stamp;
        entries.push(entry);
    }
    /// Forgets the entry for `path`, so the next read decodes the file.
    #[cfg(test)]
    pub(super) fn forget(path: &std::path::Path) {
        ENTRIES
            .lock()
            .expect("decoded cache")
            .retain(|e| e.path != path);
    }
    #[cfg(test)]
    pub(super) fn holds(path: &std::path::Path) -> bool {
        ENTRIES
            .lock()
            .expect("decoded cache")
            .iter()
            .any(|e| e.path == path)
    }
    pub(super) fn put(
        path: PathBuf,
        key: [u8; 32],
        bytes: Vec<u8>,
        version: u32,
        state: &AccountState,
        tail: Option<Tail>,
        stamp: Stamp,
    ) {
        let mut entries = ENTRIES.lock().expect("decoded cache");
        entries.retain(|e| e.path != path);
        if entries.len() >= CAPACITY {
            entries.remove(0);
        }
        entries.push(Entry {
            path,
            key,
            bytes,
            version,
            state: state.clone(),
            tail,
            stamp,
        });
    }
}

fn normalize_interrupted(state: &mut AccountState) {
    state.sieve.interrupted();
    state.sieve_transactions.interrupted();
    state.folder_provisioning.interrupted();
    state.protocol_backlog.interrupted();
    state.gmail_backlog.interrupted();
    // An interrupted scan can resume, but reopening cannot imply a running worker.
    // Only a running census is touched, so an idle one stays shared with
    // the store's record and compares in constant time.
    let census = &state.census;
    if census.acquisition.running()
        || census.acquisition_state == super::ScanState::Running
        || census
            .folders
            .values()
            .any(|f| f.state == super::ScanState::Running)
    {
        let census = &mut *state.census;
        census.acquisition.interrupted();
        if census.acquisition_state == super::ScanState::Running {
            census.acquisition_state = super::ScanState::Cancelled;
        }
        for folder in census.folders.values_mut() {
            if folder.state == super::ScanState::Running {
                folder.state = super::ScanState::Cancelled;
            }
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
                assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
            assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
            assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
        assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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
            assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
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

/// Test processes started with `MAILROOM_KILL_AT=<point>` kill themselves
/// with SIGKILL when a save reaches that point, so crash tests land exactly
/// there instead of at a random moment.
#[cfg(test)]
fn kill_requested(point: &str) -> bool {
    std::env::var("MAILROOM_KILL_AT").is_ok_and(|p| p == point)
}
#[cfg(test)]
fn kill_point(point: &str) {
    if kill_requested(point) {
        unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
        // Delivery to a multi-threaded process can lag the call, and this
        // thread must not write anything more before it lands.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// Per-thread counters of the product store's own I/O, for measurements.
/// Simulated provider stores are not counted.
#[cfg(test)]
pub(crate) mod io {
    use std::cell::Cell;
    #[derive(Clone, Copy, Debug, Default, serde::Serialize)]
    pub(crate) struct Stats {
        pub saves: u64,
        pub bytes_written: u64,
        pub fsyncs: u64,
        pub save_us: u64,
        pub reads: u64,
        pub bytes_read: u64,
        pub read_us: u64,
    }
    thread_local! {
        static STATS: Cell<Stats> = Cell::new(Stats::default());
    }
    pub(crate) fn record(change: impl FnOnce(&mut Stats)) {
        STATS.with(|s| {
            let mut v = s.get();
            change(&mut v);
            s.set(v);
        });
    }
    pub(crate) fn take() -> Stats {
        STATS.with(|s| s.take())
    }
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod durable_tests;

#[cfg(test)]
#[path = "store/journal_tests.rs"]
mod journal_tests;
