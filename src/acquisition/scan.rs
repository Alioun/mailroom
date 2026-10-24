use super::{
    Accounts, AcquisitionError, Provider,
    imap::{BATCH, MailRead, ReadOnlyImap, record},
};
use crate::{
    census::{AccountId, Census, EncryptedStore, ScanState},
    policy,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub(super) fn check(&self) -> Result<(), AcquisitionError> {
        if self.is_cancelled() {
            Err(AcquisitionError::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Serialize)]
pub struct ScanProgress {
    pub state: ScanState,
    pub saved: usize,
    pub folders: usize,
}
impl ScanProgress {
    fn current(census: &Census, state: ScanState) -> Self {
        Self {
            state,
            saved: census.folders.values().map(|f| f.messages.len()).sum(),
            folders: census.folders.len(),
        }
    }
}
impl Accounts {
    pub fn probe(&self, id: &AccountId, cancel: Cancellation) -> Result<usize, AcquisitionError> {
        let account = self.load(id)?;
        let permit = policy::read_permit(account.provider)?;
        let password = self.password(&account)?;
        retry_read(&cancel, || {
            let mut source =
                ReadOnlyImap::connect(&permit, &account.login, &password, cancel.clone())?;
            Ok(source.folders()?.len())
        })
    }
    pub fn scan(
        &self,
        id: &AccountId,
        cancel: Cancellation,
        mut progress: impl FnMut(ScanProgress),
    ) -> Result<Census, AcquisitionError> {
        let account = self.load(id)?;
        let permit = policy::read_permit(account.provider)?;
        let store = self.store(&account)?;
        // Missing keys or files never start a replacement census.
        let mut census = store.load()?;
        let password = self.password(&account)?;
        census.acquisition_state = ScanState::Running;
        store.save(&census)?;
        let outcome = retry_read(&cancel, || {
            let mut source =
                ReadOnlyImap::connect(&permit, &account.login, &password, cancel.clone())?;
            census = store.load()?;
            run(
                &mut source,
                account.provider,
                &mut census,
                &store,
                &cancel,
                &mut progress,
            )
        });
        if let Err(error) = outcome {
            let error = if cancel.is_cancelled()
                && matches!(
                    error,
                    AcquisitionError::Tls
                        | AcquisitionError::Protocol
                        | AcquisitionError::Incomplete
                        | AcquisitionError::Authentication
                ) {
                AcquisitionError::Cancelled
            } else {
                error
            };
            let mut saved = store.load()?;
            saved.acquisition_state = if error == AcquisitionError::Cancelled {
                ScanState::Cancelled
            } else {
                ScanState::Failed
            };
            store.save(&saved)?;
            return Err(error);
        }
        Ok(census)
    }
}

fn retry_read<T>(
    cancel: &Cancellation,
    mut action: impl FnMut() -> Result<T, AcquisitionError>,
) -> Result<T, AcquisitionError> {
    for attempt in 0..3 {
        cancel.check()?;
        match action() {
            Err(
                AcquisitionError::Tls | AcquisitionError::Protocol | AcquisitionError::Incomplete,
            ) if attempt < 2 => {
                for _ in 0..(50 << attempt) {
                    cancel.check()?;
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            result => return result,
        }
    }
    unreachable!("bounded retry returns on its final attempt")
}

pub(super) fn run(
    source: &mut impl MailRead,
    provider: Provider,
    census: &mut Census,
    store: &EncryptedStore,
    cancel: &Cancellation,
    mut progress: impl FnMut(ScanProgress),
) -> Result<(), AcquisitionError> {
    census.acquisition_state = ScanState::Running;
    store.save(census)?;
    let outcome = (|| {
        cancel.check()?;
        let folders = source.folders()?;
        let all = if provider == Provider::Gmail {
            folders
                .iter()
                .find(|f| f.all && !f.excluded)
                .map(|f| f.name.clone())
        } else {
            None
        };
        let selected: Vec<_> = folders
            .into_iter()
            .filter(|f| !f.excluded && all.as_ref().is_none_or(|name| *name == f.name))
            .collect();
        let names: BTreeSet<_> = selected.iter().map(|f| f.name.clone()).collect();
        let mut gmail_ids = BTreeSet::new();
        for folder in selected {
            cancel.check()?;
            let (validity, inventory) = source.inventory(&folder)?;
            let mut metadata = BTreeMap::new();
            for batch in inventory.iter().copied().collect::<Vec<_>>().chunks(BATCH) {
                cancel.check()?;
                let values = source.metadata(batch)?;
                if values.keys().copied().collect::<BTreeSet<_>>()
                    != batch.iter().copied().collect()
                {
                    return Err(AcquisitionError::Incomplete);
                }
                metadata.extend(values);
                throttle(cancel)?;
            }
            let eligible: BTreeSet<_> = metadata
                .iter()
                .filter_map(|(uid, m)| {
                    if m.excluded() {
                        return None;
                    }
                    if provider == Provider::Gmail && !gmail_ids.insert(m.gmail_id.clone()?) {
                        return None;
                    }
                    Some(*uid)
                })
                .collect();
            census.reconcile_folder(&folder.name, validity, eligible)?;
            let saved = census
                .folders
                .get_mut(&folder.name)
                .expect("reconciled folder");
            for (uid, message) in &mut saved.messages {
                metadata[uid].apply(message, &folder);
            }
            store.save(census)?;
            progress(ScanProgress::current(census, ScanState::Running));
            loop {
                cancel.check()?;
                let pending = census.folders[&folder.name].pending(BATCH);
                if pending.is_empty() {
                    break;
                }
                let headers = source.headers(&pending)?;
                if headers.keys().copied().collect::<BTreeSet<_>>()
                    != pending.iter().copied().collect()
                {
                    return Err(AcquisitionError::Incomplete);
                }
                let records = pending
                    .iter()
                    .map(|uid| record(&metadata[uid], &folder, &headers[uid]))
                    .collect::<Result<Vec<_>, _>>()?;
                for message in records {
                    census
                        .folders
                        .get_mut(&folder.name)
                        .expect("reconciled folder")
                        .insert(message)?;
                }
                store.save(census)?;
                progress(ScanProgress::current(census, ScanState::Running));
                throttle(cancel)?;
            }
        }
        cancel.check()?;
        // A failed folder scan must not turn a partial LIST into deletion of other saved folders.
        census.folders.retain(|name, _| names.contains(name));
        census.acquisition_state = ScanState::Complete;
        store.save(census)?;
        progress(ScanProgress::current(census, ScanState::Complete));
        Ok(())
    })();
    let outcome = outcome.map_err(|error| {
        if cancel.is_cancelled()
            && matches!(
                error,
                AcquisitionError::Protocol
                    | AcquisitionError::Tls
                    | AcquisitionError::Authentication
            )
        {
            AcquisitionError::Cancelled
        } else {
            error
        }
    });
    if let Err(error) = outcome {
        let state = if error == AcquisitionError::Cancelled {
            ScanState::Cancelled
        } else {
            ScanState::Failed
        };
        census.acquisition_state = state;
        for folder in census
            .folders
            .values_mut()
            .filter(|f| f.state == ScanState::Running)
        {
            folder.state = state;
        }
        store.save(census)?;
        progress(ScanProgress::current(census, state));
    }
    outcome
}
fn throttle(cancel: &Cancellation) -> Result<(), AcquisitionError> {
    for _ in 0..5 {
        cancel.check()?;
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acquisition::imap::{Folder, Metadata},
        census::{CensusKeys, LocalSecret},
    };
    struct FakeMail {
        folders: Vec<Folder>,
        validity: u32,
        metadata: BTreeMap<u32, Metadata>,
        headers_requested: Vec<u32>,
        fail_metadata: bool,
    }
    impl MailRead for FakeMail {
        fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError> {
            Ok(self.folders.clone())
        }
        fn inventory(&mut self, _: &Folder) -> Result<(u32, BTreeSet<u32>), AcquisitionError> {
            Ok((self.validity, self.metadata.keys().copied().collect()))
        }
        fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError> {
            if self.fail_metadata {
                return Err(AcquisitionError::Incomplete);
            }
            Ok(uids
                .iter()
                .map(|uid| (*uid, self.metadata[uid].clone()))
                .collect())
        }
        fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError> {
            self.headers_requested.extend(uids);
            Ok(uids.iter().map(|uid| (*uid, format!("From: sender@example.test\r\nTo: owner@example.test\r\nSubject: Synthetic {uid}\r\n\r\n").into_bytes())).collect())
        }
    }
    fn source() -> FakeMail {
        FakeMail {
            folders: vec![Folder {
                wire: "Inbox".into(),
                name: "Inbox".into(),
                all: false,
                excluded: false,
                sent: false,
            }],
            validity: 7,
            metadata: (1..=3)
                .map(|uid| {
                    (
                        uid,
                        Metadata {
                            uid,
                            size: 100,
                            date: "synthetic date".into(),
                            flags: BTreeSet::new(),
                            gmail_id: Some(uid.to_string()),
                            thread_id: Some("1".into()),
                            labels: BTreeSet::new(),
                        },
                    )
                })
                .collect(),
            headers_requested: vec![],
            fail_metadata: false,
        }
    }
    #[test]
    fn acquisition_resumes_holes_refreshes_metadata_removes_vanished_and_resets_validity() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("fake-account").unwrap();
        let store = EncryptedStore::new(
            dir.path().to_owned(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let mut census = Census::new(account);
        let mut fake = source();
        let cancel = Cancellation::default();
        census
            .reconcile_folder("Inbox", 7, [1, 2, 3].into_iter().collect())
            .unwrap();
        census
            .folders
            .get_mut("Inbox")
            .unwrap()
            .insert(crate::census::demo::fixture(3).unwrap())
            .unwrap();
        census
            .reconcile_folder("Vanished", 7, BTreeSet::new())
            .unwrap();
        store.save(&census).unwrap();
        let mut census = store.load().unwrap();
        run(
            &mut fake,
            Provider::MailboxOrg,
            &mut census,
            &store,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(fake.headers_requested, vec![1, 2]);
        assert!(!census.folders.contains_key("Vanished"));
        census.confirm_address("owner@example.test", true).unwrap();
        fake.metadata.remove(&1);
        fake.metadata
            .get_mut(&2)
            .unwrap()
            .flags
            .insert("\\Seen".into());
        fake.headers_requested.clear();
        run(
            &mut fake,
            Provider::MailboxOrg,
            &mut census,
            &store,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(fake.headers_requested.is_empty());
        assert!(!census.folders["Inbox"].messages.contains_key(&1));
        assert!(
            census.folders["Inbox"].messages[&2]
                .flags
                .contains("\\Seen")
        );
        fake.validity = 8;
        run(
            &mut fake,
            Provider::MailboxOrg,
            &mut census,
            &store,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(fake.headers_requested, vec![2, 3]);
        assert!(
            census
                .confirmed_own_addresses
                .contains("owner@example.test")
        );
    }
    #[test]
    fn incomplete_inventory_and_cancellation_retain_saved_data_without_reporting_completion() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("fake-account").unwrap();
        let store = EncryptedStore::new(
            dir.path().to_owned(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let mut census = Census::new(account);
        let mut fake = source();
        let cancel = Cancellation::default();
        run(
            &mut fake,
            Provider::MailboxOrg,
            &mut census,
            &store,
            &cancel,
            |_| {},
        )
        .unwrap();
        let original = census.clone();
        fake.fail_metadata = true;
        fake.metadata.remove(&1);
        let mut state = ScanState::Idle;
        assert_eq!(
            run(
                &mut fake,
                Provider::MailboxOrg,
                &mut census,
                &store,
                &cancel,
                |p| state = p.state
            ),
            Err(AcquisitionError::Incomplete)
        );
        assert_eq!(state, ScanState::Failed);
        assert_eq!(census.folders, original.folders);
        assert_eq!(census.acquisition_state, ScanState::Failed);
        cancel.cancel();
        assert_eq!(
            run(
                &mut fake,
                Provider::MailboxOrg,
                &mut census,
                &store,
                &cancel,
                |p| state = p.state
            ),
            Err(AcquisitionError::Cancelled)
        );
        assert_eq!(state, ScanState::Cancelled);
        let saved = store.load().unwrap();
        assert_eq!(saved.folders, original.folders);
        assert_eq!(saved.acquisition_state, ScanState::Cancelled);
    }
    #[test]
    fn gmail_scans_all_mail_once_deduplicates_and_excludes_system_labels() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("fake-account").unwrap();
        let store = EncryptedStore::new(
            dir.path().to_owned(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let mut census = Census::new(account);
        let mut fake = source();
        fake.folders[0].all = true;
        let mut extra = fake.folders[0].clone();
        extra.name = "Other Label".into();
        extra.all = false;
        fake.folders.push(extra);
        fake.metadata
            .get_mut(&1)
            .unwrap()
            .labels
            .insert("\\Trash".into());
        fake.metadata
            .get_mut(&2)
            .unwrap()
            .labels
            .insert("\\Sent".into());
        fake.metadata.get_mut(&3).unwrap().gmail_id = Some("2".into());
        run(
            &mut fake,
            Provider::Gmail,
            &mut census,
            &store,
            &Cancellation::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(fake.headers_requested, vec![2]);
        assert_eq!(census.folders.len(), 1);
        assert_eq!(
            census.folders["Inbox"].messages[&2].headers.direction,
            crate::census::Direction::Sent
        );
        fake.folders[0].all = false;
        fake.metadata.get_mut(&2).unwrap().labels.clear();
        fake.headers_requested.clear();
        run(
            &mut fake,
            Provider::Gmail,
            &mut census,
            &store,
            &Cancellation::default(),
            |_| {},
        )
        .unwrap();
        assert!(fake.headers_requested.is_empty());
        assert!(census.folders["Other Label"].messages.is_empty());
        assert_eq!(
            census.folders["Inbox"].messages[&2].headers.direction,
            crate::census::Direction::Received
        );
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn transient_read_failures_back_off_and_authentication_never_retries() {
        let cancel = Cancellation::default();
        let mut attempts = 0;
        let start = std::time::Instant::now();
        assert_eq!(
            retry_read(&cancel, || {
                attempts += 1;
                if attempts == 1 {
                    Err(AcquisitionError::Incomplete)
                } else {
                    Ok(7)
                }
            }),
            Ok(7)
        );
        assert_eq!(attempts, 2);
        assert!(start.elapsed() >= Duration::from_secs(1));
        attempts = 0;
        assert_eq!(
            retry_read::<()>(&cancel, || {
                attempts += 1;
                Err(AcquisitionError::Authentication)
            }),
            Err(AcquisitionError::Authentication)
        );
        assert_eq!(attempts, 1);
        assert_eq!(
            retry_read::<()>(&cancel, || {
                cancel.cancel();
                Err(AcquisitionError::Protocol)
            }),
            Err(AcquisitionError::Cancelled)
        );
    }
}
