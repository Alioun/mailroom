use super::{
    AccountPaths, Accounts, AcquisitionError, CredentialBackend, SecretValue,
    imap::{Folder, MailRead, Metadata},
    scan::{self, Cancellation, ScanProgress},
};
use crate::census::{AccountId, Census};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryCredentials(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl CredentialBackend for MemoryCredentials {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        self.0
            .lock()
            .expect("synthetic credentials")
            .get(reference)
            .map(|v| SecretValue::new(v.to_vec()))
            .ok_or(AcquisitionError::Credentials)
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        self.0
            .lock()
            .expect("synthetic credentials")
            .insert(reference.to_owned(), Zeroizing::new(value.bytes().to_vec()));
        Ok(())
    }
}

pub(crate) fn accounts(root: &std::path::Path) -> Accounts {
    Accounts::new(
        AccountPaths {
            config: root.join("config"),
            data: root.join("data"),
        },
        Arc::new(MemoryCredentials::default()),
    )
}
struct SyntheticMail;
impl MailRead for SyntheticMail {
    fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError> {
        Ok(vec![Folder {
            wire: "INBOX".into(),
            name: "INBOX".into(),
            sent: false,
            all: false,
            excluded: false,
        }])
    }
    fn inventory(&mut self, _: &Folder) -> Result<(u32, BTreeSet<u32>), AcquisitionError> {
        Ok((7, (1..=100).collect()))
    }
    fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError> {
        Ok(uids
            .iter()
            .map(|uid| {
                (
                    *uid,
                    Metadata {
                        uid: *uid,
                        size: 1024,
                        date: "24-Oct-2026 10:00:00 +0000".into(),
                        flags: BTreeSet::new(),
                        gmail_id: None,
                        thread_id: None,
                        labels: BTreeSet::new(),
                    },
                )
            })
            .collect())
    }
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError> {
        Ok(uids.iter().map(|uid| (*uid, format!("From: Synthetic sender <sender@example.test>\r\nTo: owner@example.test\r\nDelivered-To: alias@example.test\r\nSubject: Synthetic account header {uid}\r\n\r\n").into_bytes())).collect())
    }
}
pub(crate) fn scan(
    accounts: &Accounts,
    id: &AccountId,
    cancel: Cancellation,
    progress: impl FnMut(ScanProgress),
) -> Result<Census, AcquisitionError> {
    let account = accounts.load(id)?;
    let store = accounts.store(&account)?;
    let mut census = store.load()?;
    scan::run(
        &mut SyntheticMail,
        account.provider,
        &mut census,
        &store,
        &cancel,
        progress,
    )?;
    Ok(census)
}
