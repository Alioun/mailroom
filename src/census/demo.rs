use super::{
    AccountId, AddressCandidate, Census, CensusError, CensusKeys, Direction, EncryptedStore,
    LocalSecret, MessageRecord, ScanState, parse_headers,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const TOTAL: u32 = 8;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemoMode {
    Normal,
    Error,
}

#[derive(Serialize)]
pub struct DemoStatus {
    pub source: &'static str,
    pub state: ScanState,
    pub completed: usize,
    pub total: u32,
    pub error: Option<&'static str>,
    pub addresses: Vec<AddressCandidate>,
    pub messages: Vec<DemoMessage>,
    pub summary: String,
    pub storage: &'static str,
}

#[derive(Serialize)]
pub struct DemoMessage {
    pub uid: u32,
    pub direction: Direction,
    pub subject: String,
    pub sender: String,
    pub dkim: &'static str,
}

pub struct DemoCensus {
    census: Census,
    keys: CensusKeys,
    store: EncryptedStore,
    _directory: tempfile::TempDir,
    state: ScanState,
    mode: DemoMode,
    generation: u64,
}

impl DemoCensus {
    pub fn new() -> Result<Self, CensusError> {
        let account = AccountId::new("synthetic-demo")?;
        let secret = LocalSecret::random();
        let keys = CensusKeys::derive(&secret, &account);
        let directory = tempfile::tempdir().map_err(|_| CensusError::Storage)?;
        let store = EncryptedStore::new(
            directory.path().to_owned(),
            CensusKeys::derive(&secret, &account),
        )?;
        Ok(Self {
            census: Census::new(account),
            keys,
            store,
            _directory: directory,
            state: ScanState::Idle,
            mode: DemoMode::Normal,
            generation: 0,
        })
    }

    pub fn start(&mut self, mode: DemoMode) -> Result<u64, CensusError> {
        if self.state == ScanState::Running {
            return Err(CensusError::InvalidCheckpoint);
        }
        if matches!(self.state, ScanState::Cancelled | ScanState::Failed) {
            self.census = self.store.load()?;
        } else {
            self.census = Census::new(self.census.account.clone());
        }
        self.census
            .reconcile_folder("INBOX", 7, (1..=TOTAL).collect())?;
        self.store.save(&self.census)?;
        self.state = ScanState::Running;
        self.mode = mode;
        self.generation += 1;
        Ok(self.generation)
    }

    pub fn advance(&mut self, generation: u64) -> Result<bool, CensusError> {
        if generation != self.generation || self.state != ScanState::Running {
            return Ok(false);
        }
        let folder = self.census.folders.get_mut("INBOX").expect("demo folder");
        if matches!(self.mode, DemoMode::Error) && folder.messages.len() >= 2 {
            folder.state = ScanState::Failed;
            self.state = ScanState::Failed;
        } else if let Some(uid) = folder.pending(1).first() {
            folder.insert(fixture(*uid)?)?;
            self.state = folder.state;
        }
        self.store.save(&self.census)?;
        Ok(self.state == ScanState::Running)
    }

    pub fn fail(&mut self) {
        self.state = ScanState::Failed;
    }

    pub fn cancel(&mut self) -> Result<(), CensusError> {
        if self.state == ScanState::Running {
            self.census
                .folders
                .get_mut("INBOX")
                .expect("demo folder")
                .state = ScanState::Cancelled;
            self.store.save(&self.census)?;
            self.state = ScanState::Cancelled;
        }
        Ok(())
    }

    pub fn confirm(&mut self, address: &str, confirmed: bool) -> Result<(), CensusError> {
        self.census.confirm_address(address, confirmed)?;
        self.store.save(&self.census)
    }

    pub fn status(&self) -> DemoStatus {
        let messages: Vec<_> = self
            .census
            .folders
            .values()
            .flat_map(|f| f.messages.values())
            .map(|m| DemoMessage {
                uid: m.uid,
                direction: m.headers.direction,
                subject: m.headers.subject.clone(),
                sender: m
                    .headers
                    .effective_sender
                    .as_ref()
                    .map_or_else(String::new, |a| a.address.clone()),
                dkim: "unknown",
            })
            .collect();
        DemoStatus { source: "synthetic", state: self.state, completed: messages.len(), total: TOTAL,
            error: (self.state == ScanState::Failed).then_some("Synthetic scan stopped. Saved records remain available. Resume with the normal demo."),
            addresses: self.census.own_address_candidates(), messages, summary: self.census.summary(&self.keys).expect("demo account keys"),
            storage: "Encrypted SQLite in a temporary synthetic sandbox. Ephemeral key in memory. No real account or credential store.",
        }
    }
}

pub fn fixture(uid: u32) -> Result<MessageRecord, CensusError> {
    let direction = if uid == TOTAL {
        Direction::Sent
    } else {
        Direction::Received
    };
    let from = if direction == Direction::Sent {
        "Demo owner <owner@example.test>"
    } else if uid <= 4 {
        "Studio <news@studio.example>"
    } else {
        "Mila <mila@example.net>"
    };
    let raw = format!(
        "From: {from}\r\nTo: owner@example.test\r\nDelivered-To: alias@example.test\r\nSubject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe_und_Rechnung?= {uid}\r\nDate: Wed, 21 Oct 2026 10:00:00 +0000\r\nMessage-ID: <demo-{uid}@example.test>\r\nList-Unsubscribe: <https://studio.example/remove?token=synthetic-private-target>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nAuthentication-Results: forged.example; dkim=pass header.d=studio.example\r\nDKIM-Signature: v=1; d=studio.example; s=demo; h=from:list-unsubscribe:list-unsubscribe-post; b=synthetic\r\n\r\n"
    );
    Ok(MessageRecord {
        uid,
        size: 1024,
        internal_date: "2026-10-21T10:00:00Z".to_owned(),
        flags: if uid % 2 == 0 {
            BTreeSet::from(["\\Seen".to_owned()])
        } else {
            BTreeSet::new()
        },
        gmail_message_id: None,
        gmail_thread_id: None,
        gmail_labels: BTreeSet::new(),
        headers: parse_headers(raw.as_bytes(), direction)?,
    })
}
