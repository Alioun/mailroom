pub mod demo;
mod headers;
mod store;

pub use headers::{HEADER_FIELDS, parse_headers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use store::{CensusKeys, EncryptedStore, LocalSecret};

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
    pub uid: u32,
    pub size: u64,
    pub internal_date: String,
    pub flags: BTreeSet<String>,
    pub gmail_message_id: Option<String>,
    pub gmail_thread_id: Option<String>,
    pub gmail_labels: BTreeSet<String>,
    pub headers: HeaderRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
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
    pub account: AccountId,
    pub folders: BTreeMap<String, FolderCensus>,
    pub confirmed_own_addresses: BTreeSet<String>,
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
            account,
            folders: BTreeMap::new(),
            confirmed_own_addresses: BTreeSet::new(),
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
        let mut candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
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

    pub fn summary(&self, keys: &CensusKeys) -> Result<String, CensusError> {
        if !keys.belongs_to(&self.account) {
            return Err(CensusError::AccountMismatch);
        }
        let mut text = String::from(
            "# Census summary\n\nAll values are placeholders. No clear values approved. Subjects and unsubscribe targets are omitted.\n\n",
        );
        for (folder, census) in &self.folders {
            text.push_str(&format!(
                "- {}: {} messages\n",
                keys.placeholder("folder", folder),
                census.messages.len()
            ));
        }
        text.push_str("\nConfirmed own addresses\n\n");
        for address in &self.confirmed_own_addresses {
            if let Some((local, domain)) = address.rsplit_once('@') {
                text.push_str(&format!(
                    "- {}@{}\n",
                    keys.placeholder("alias", local),
                    keys.placeholder("own-domain", domain)
                ));
            }
        }
        text.push_str("\nSenders with at least 3 messages\n\n");
        let mut senders: BTreeMap<String, usize> = BTreeMap::new();
        for message in self.folders.values().flat_map(|f| f.messages.values()) {
            if message.headers.direction == Direction::Received {
                if let Some(sender) = &message.headers.effective_sender {
                    *senders.entry(sender.address.clone()).or_default() += 1;
                }
            }
        }
        let mut tail = 0;
        for (sender, count) in senders {
            if count < 3 {
                tail += count;
            } else {
                text.push_str(&format!(
                    "- {}: {} messages\n",
                    keys.placeholder("person", &sender),
                    count
                ));
            }
        }
        text.push_str(&format!("\nLong tail: {tail} messages\n\nLimitations: no pinned PSL, sender classification, monthly statistics or approved clear values yet. DKIM coverage is unknown.\n"));
        Ok(text)
    }
}
