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
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        self.0
            .lock()
            .expect("synthetic credentials")
            .remove(reference);
        Ok(())
    }
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        self.0
            .lock()
            .expect("synthetic credentials")
            .get(reference)
            .map(|v| SecretValue::new(v.to_vec()))
            .ok_or(AcquisitionError::CredentialMissing)
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
            roles: BTreeSet::new(),
            wire: "INBOX".into(),
            name: "INBOX".into(),
            sent: false,
            all: false,
            excluded: false,
        }])
    }
    fn inventory(&mut self, _: &Folder) -> Result<(u32, BTreeSet<u32>), AcquisitionError> {
        let count = 100;
        #[cfg(test)]
        let count = if cancel_fixture() { 601 } else { count };
        Ok((7, (1..=count).collect()))
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
                        flags: if uid % 3 == 0 {
                            BTreeSet::from(["\\Seen".into(), "\\Flagged".into()])
                        } else {
                            BTreeSet::new()
                        },
                        gmail_id: Some(uid.to_string()),
                        thread_id: None,
                        labels: if *uid == 99 {
                            BTreeSet::from(["\\Sent".into()])
                        } else {
                            BTreeSet::from(["Synthetic label".into(), "\\Inbox".into()])
                        },
                    },
                )
            })
            .collect())
    }
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError> {
        Ok(uids.iter().map(|uid| {
            let category = *uid;
            #[cfg(test)]
            let category = if cancel_fixture() && *uid > 100 { 1 } else { category };
            let (sender, markers) = match category {
                1..=40 => ("Synthetic shop <news@shop.example>", "List-Id: <offers.shop.example>\r\nList-Unsubscribe: <https://shop.example/remove?token=synthetic-private-token>, <mailto:leave@shop.example?subject=unsubscribe&body=synthetic-mail-token>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nDKIM-Signature: v=1; d=shop.example; s=fixture; h=list-unsubscribe:list-unsubscribe-post; b=synthetic-signature\r\nFeedback-ID: fixture:AmazonSES\r\n"),
                41..=60 => ("=?UTF-8?Q?Alex_M=C3=BCster?= <alex@people.example>", "In-Reply-To: <synthetic@example.test>\r\n"),
                61..=70 => ("Receipts <receipts@platform.example>", "Auto-Submitted: auto-generated\r\nReturn-Path: <bounce@amazonses.com>\r\n"),
                71..=80 => ("Discussion <team@community.example>", "List-Id: <team.community.example>\r\nList-Post: <mailto:team@community.example>\r\n"),
                81..=90 => ("Relay <rewritten@relay.example>", "X-SimpleLogin-Original-From: Claimed shop <news@claimed.example>\r\nList-Unsubscribe: <https://claimed.example/remove?token=synthetic-relay-token>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n"),
                91..=92 => ("Small <small@tail.example>", ""),
                93..=94 => ("Other <other@tail.example>", ""),
                99 => ("owner@example.test", "Cc: alex@people.example\r\n"),
                _ => ("sender@example.test", ""),
            };
            let date = if *uid == 98 { "invalid date".into() } else { format!("{} Oct 2026 10:00:00 +0000", 1 + uid % 23) };
            let recipient = if (21..=30).contains(uid) { "alias@home.example.test" } else { "team@home.example.test" };
            let markers = if (31..=40).contains(uid) { markers.replace("<offers.shop.example>", "<updates.shop.example>") } else { markers.into() };
            (*uid, format!("From: {sender}\r\nTo: owner@example.test\r\nDelivered-To: {recipient}\r\nDate: {date}\r\nSubject: Synthetic account header {uid}\r\nMessage-ID: <synthetic-account-{uid}@example.test>\r\n{markers}\r\n").into_bytes())
        }).collect())
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
    super::lifecycle::boundary("acquisition_running")?;
    scan::run(
        &mut SyntheticMail,
        account.provider,
        &mut census,
        &store,
        &cancel,
        progress,
    )?;
    #[cfg(test)]
    if std::env::var("MAILROOM_WINDDOWN_FIXTURE").ok().as_deref() == Some("1")
        && std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE").is_some()
    {
        crate::winddown::synthetic::install(&mut census, account.provider);
        store.save(&census)?;
    }
    Ok(census)
}

#[cfg(test)]
fn cancel_fixture() -> bool {
    std::env::var("MAILROOM_BACKLOG_CANCEL_FIXTURE").is_ok_and(|v| v == "1")
        && std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE").is_some()
}
