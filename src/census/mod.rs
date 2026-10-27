pub mod acquisition;
pub mod delivery;
pub mod demo;
pub mod folder_provisioning;
mod headers;
pub mod report;
#[cfg(test)]
pub(crate) mod scale;
pub mod sieve;
pub mod sieve_transaction;
mod store;

pub use headers::{HEADER_FIELDS, parse_headers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
pub(crate) use store::io;
pub use store::{AccountState, CensusKeys, EncryptedStore, LocalSecret};
pub(crate) use store::{MAX_FILE, SCHEMA};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct AccountId(String);

impl AccountId {
    pub fn new(value: &str) -> Result<Self, CensusError> {
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(CensusError::InvalidAccount);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AccountId {
    type Error = CensusError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CensusError {
    InvalidAccount,
    InvalidHeaders,
    InvalidCheckpoint,
    Storage,
    Authentication,
    AccountMismatch,
    UnsupportedVersion,
}

impl std::fmt::Display for CensusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidAccount => "Invalid account identifier",
            Self::InvalidHeaders => "Invalid or excessive header block",
            Self::InvalidCheckpoint => "Invalid census checkpoint",
            Self::Storage => "Census storage failed; retain existing files and retry",
            Self::Authentication => "Census authentication failed; the file was not changed",
            Self::AccountMismatch => "Census belongs to a different account",
            Self::UnsupportedVersion => "Unsupported census version; the file was not changed",
        })
    }
}
impl std::error::Error for CensusError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Received,
    Sent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    pub address: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DkimCoverage {
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderRecord {
    pub direction: Direction,
    pub subject: String,
    pub date: Option<String>,
    pub message_id_hash: Option<String>,
    pub in_reply_to: bool,
    pub from: Vec<Mailbox>,
    pub sender: Vec<Mailbox>,
    pub reply_to: Vec<Mailbox>,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub delivered_to: Vec<Mailbox>,
    pub original_to: Vec<Mailbox>,
    pub return_path_domain: Option<String>,
    pub effective_sender: Option<Mailbox>,
    pub registrable_domain: Option<String>,
    pub dkim_coverage: DkimCoverage,
    // Keep duplicate, encoded and relay evidence for later owner-reviewed routing.
    pub evidence: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    #[serde(default, skip_serializing_if = "delivery_unavailable")]
    pub delivery: delivery::DeliveryEvidence,
    pub uid: u32,
    pub size: u64,
    pub internal_date: String,
    #[serde(default)]
    pub flags_observed: bool,
    pub flags: BTreeSet<String>,
    pub gmail_message_id: Option<String>,
    pub gmail_thread_id: Option<String>,
    pub gmail_labels: BTreeSet<String>,
    pub headers: HeaderRecord,
}

fn delivery_unavailable(value: &delivery::DeliveryEvidence) -> bool {
    matches!(value, delivery::DeliveryEvidence::Unavailable)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    #[default]
    Idle,
    Running,
    Cancelled,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCensus {
    pub uidvalidity: u32,
    pub state: ScanState,
    pub live_uids: BTreeSet<u32>,
    pub messages: BTreeMap<u32, MessageRecord>,
}

impl FolderCensus {
    pub fn reconcile(
        &mut self,
        uidvalidity: u32,
        live_uids: BTreeSet<u32>,
    ) -> Result<(), CensusError> {
        if uidvalidity == 0 || live_uids.contains(&0) {
            return Err(CensusError::InvalidCheckpoint);
        }
        if self.uidvalidity != uidvalidity {
            self.messages.clear();
        }
        self.uidvalidity = uidvalidity;
        self.messages.retain(|uid, _| live_uids.contains(uid));
        self.live_uids = live_uids;
        self.state = if self.pending(1).is_empty() {
            ScanState::Complete
        } else {
            ScanState::Running
        };
        Ok(())
    }
    pub fn pending(&self, limit: usize) -> Vec<u32> {
        self.live_uids
            .iter()
            .filter(|uid| !self.messages.contains_key(uid))
            .take(limit)
            .copied()
            .collect()
    }
    pub fn insert(&mut self, record: MessageRecord) -> Result<(), CensusError> {
        if self.state != ScanState::Running || !self.live_uids.contains(&record.uid) {
            return Err(CensusError::InvalidCheckpoint);
        }
        self.messages.insert(record.uid, record);
        if self.pending(1).is_empty() {
            self.state = ScanState::Complete;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Census {
    #[serde(default)]
    pub acquisition: acquisition::AcquisitionFacts,
    #[serde(default)]
    pub winddown_evidence: crate::winddown::AcquisitionEvidence,
    #[serde(default)]
    pub folder_roles: BTreeMap<String, BTreeSet<String>>,
    pub account: AccountId,
    pub folders: BTreeMap<String, FolderCensus>,
    pub confirmed_own_addresses: BTreeSet<String>,
    #[serde(default)]
    pub account_login: Option<String>,
    #[serde(default)]
    pub acquisition_state: ScanState,
    #[serde(default)]
    pub last_completed_at: Option<i64>,
}

/// A census shared between a loaded `AccountState` and the store's record of
/// what it last persisted. Cloning shares it, so an untouched census compares
/// equal by pointer in constant time. The first mutable access copies it.
#[derive(Clone, Debug)]
pub struct SharedCensus(std::sync::Arc<Census>);

impl SharedCensus {
    pub fn into_inner(self) -> Census {
        std::sync::Arc::unwrap_or_clone(self.0)
    }
}
impl From<Census> for SharedCensus {
    fn from(census: Census) -> Self {
        Self(std::sync::Arc::new(census))
    }
}
impl std::ops::Deref for SharedCensus {
    type Target = Census;
    fn deref(&self) -> &Census {
        &self.0
    }
}
impl std::ops::DerefMut for SharedCensus {
    fn deref_mut(&mut self) -> &mut Census {
        std::sync::Arc::make_mut(&mut self.0)
    }
}
impl PartialEq for SharedCensus {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}
impl Eq for SharedCensus {}
impl PartialEq<Census> for SharedCensus {
    fn eq(&self, other: &Census) -> bool {
        *self.0 == *other
    }
}
impl Serialize for SharedCensus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if binding::active() {
            return serializer.serialize_str(&binding::label(&self.digest()));
        }
        self.0.serialize(serializer)
    }
}
impl SharedCensus {
    /// SHA-256 of the census's JSON. Computed once per shared census: every
    /// copy of a loaded state shares it, and a mutation makes a new one.
    pub fn digest(&self) -> [u8; 32] {
        binding::digest(&self.0)
    }
}

/// Bindings (receipts, material digests) commit to the whole account state.
/// Inside `bound`, a shared census serializes as its digest, so a binding
/// costs the size of the state around the census, and the census is hashed
/// once per loaded version instead of once per binding.
pub(crate) mod binding {
    use super::Census;
    use sha2::{Digest, Sha256};
    use std::{
        cell::Cell,
        sync::{Arc, Mutex, Weak},
    };

    thread_local! {
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
    }
    pub(super) fn active() -> bool {
        ACTIVE.with(Cell::get)
    }
    pub(super) fn label(digest: &[u8; 32]) -> String {
        format!("census-sha256:{}", data_encoding::HEXLOWER.encode(digest))
    }
    /// Runs `f` with shared censuses serializing as their digests.
    pub(crate) fn bound<T>(f: impl FnOnce() -> T) -> T {
        struct Reset(bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                ACTIVE.with(|a| a.set(self.0));
            }
        }
        let _reset = Reset(ACTIVE.with(|a| a.replace(true)));
        f()
    }

    struct Hasher(Sha256);
    impl std::io::Write for Hasher {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A few recent censuses by allocation. The weak reference keeps the
    /// allocation from being reused while its entry exists, so a live census
    /// at the same address is the same census.
    static DIGESTS: Mutex<Vec<(Weak<Census>, [u8; 32])>> = Mutex::new(Vec::new());

    pub(super) fn digest(census: &Arc<Census>) -> [u8; 32] {
        let mut cache = DIGESTS.lock().expect("census digests");
        cache.retain(|(weak, _)| weak.strong_count() > 0);
        if let Some((_, digest)) = cache
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(census)))
        {
            return *digest;
        }
        drop(cache);
        let was = ACTIVE.with(|a| a.replace(false));
        let mut hasher = Hasher(Sha256::new());
        serde_json::to_writer(&mut hasher, &**census).expect("census serializes");
        ACTIVE.with(|a| a.set(was));
        let digest: [u8; 32] = hasher.0.finalize().into();
        let mut cache = DIGESTS.lock().expect("census digests");
        if cache.len() >= 8 {
            cache.remove(0);
        }
        cache.push((Arc::downgrade(census), digest));
        digest
    }
}
impl<'de> Deserialize<'de> for SharedCensus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Census::deserialize(deserializer).map(Self::from)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddressCandidate {
    pub address: String,
    pub evidence: BTreeSet<String>,
    pub confirmed: bool,
}

impl Census {
    pub fn new(account: AccountId) -> Self {
        Self {
            winddown_evidence: Default::default(),
            acquisition: Default::default(),
            account,
            folder_roles: BTreeMap::new(),
            folders: BTreeMap::new(),
            confirmed_own_addresses: BTreeSet::new(),
            account_login: None,
            acquisition_state: ScanState::Idle,
            last_completed_at: None,
        }
    }

    pub fn reconcile_folder(
        &mut self,
        folder: &str,
        uidvalidity: u32,
        uids: BTreeSet<u32>,
    ) -> Result<(), CensusError> {
        if uidvalidity == 0 || uids.contains(&0) {
            return Err(CensusError::InvalidCheckpoint);
        }
        let entry = self
            .folders
            .entry(folder.to_owned())
            .or_insert_with(|| FolderCensus {
                uidvalidity,
                state: ScanState::Idle,
                live_uids: BTreeSet::new(),
                messages: BTreeMap::new(),
            });
        entry.reconcile(uidvalidity, uids)
    }

    pub fn own_address_candidates(&self) -> Vec<AddressCandidate> {
        let mut candidates: BTreeMap<String, BTreeSet<String>> = self
            .confirmed_own_addresses
            .iter()
            .map(|address| {
                (
                    address.clone(),
                    BTreeSet::from(["previously_confirmed".to_owned()]),
                )
            })
            .collect();
        if let Some(login) = &self.account_login {
            candidates
                .entry(login.clone())
                .or_default()
                .insert("account_login_unverified".to_owned());
        }
        for message in self.folders.values().flat_map(|f| f.messages.values()) {
            let h = &message.headers;
            let sources = if h.direction == Direction::Sent {
                vec![("sent_from", &h.from)]
            } else {
                vec![
                    ("delivered_to", &h.delivered_to),
                    ("original_to", &h.original_to),
                    ("to_unverified", &h.to),
                    ("cc_unverified", &h.cc),
                ]
            };
            for (source, addresses) in sources {
                for address in addresses {
                    candidates
                        .entry(address.address.clone())
                        .or_default()
                        .insert(source.to_owned());
                }
            }
        }
        candidates
            .into_iter()
            .map(|(address, evidence)| AddressCandidate {
                confirmed: self.confirmed_own_addresses.contains(&address),
                address,
                evidence,
            })
            .collect()
    }

    pub fn confirm_address(&mut self, address: &str, confirmed: bool) -> Result<(), CensusError> {
        if !self
            .own_address_candidates()
            .iter()
            .any(|a| a.address == address)
        {
            return Err(CensusError::InvalidHeaders);
        }
        if confirmed {
            self.confirmed_own_addresses.insert(address.to_owned());
        } else {
            self.confirmed_own_addresses.remove(address);
        }
        Ok(())
    }

    pub fn summary(
        &self,
        keys: &CensusKeys,
        as_of: i64,
        gmail: bool,
    ) -> Result<String, CensusError> {
        Ok(self
            .report(keys, as_of, gmail)?
            .summary_preview(keys, &BTreeSet::new())?
            .summary)
    }
}
