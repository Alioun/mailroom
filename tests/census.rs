use mailroom::census::{
    AccountId, Census, CensusError, CensusKeys, Direction, DkimCoverage, EncryptedStore,
    LocalSecret, ScanState,
    demo::{DemoCensus, DemoMode, fixture},
    parse_headers,
};
use std::{collections::BTreeSet, fs};

fn account() -> AccountId {
    AccountId::new("account-a").unwrap()
}
fn secret() -> LocalSecret {
    LocalSecret::from_bytes(vec![42; 32]).unwrap()
}
fn populated() -> Census {
    let mut census = Census::new(account());
    census
        .reconcile_folder("INBOX", 77, BTreeSet::from([1, 2, 3]))
        .unwrap();
    for uid in 1..=3 {
        census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .insert(fixture(uid).unwrap())
            .unwrap();
    }
    census.confirm_address("owner@example.test", true).unwrap();
    census
}

#[test]
fn unicode_encoded_headers_evidence_and_message_id_hash_survive() {
    let raw = concat!(
        "From: =?UTF-8?Q?Ren=C3=A9?= <rene@example.net>\r\n",
        "Sender: agent@example.test\r\nReply-To: reply@example.test\r\n",
        "To: owner@example.test\r\nCc: colleague@example.net\r\nBcc: hidden@example.net\r\n",
        "Delivered-To: alias@example.test\r\nDelivered-To: second@example.test\r\n",
        "X-Original-To: original@example.test\r\n",
        "Subject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe?=\r\n und 日本語\r\n",
        "Message-ID: <secret-id@example.net>\r\nIn-Reply-To: <previous@example.net>\r\n",
        "Return-Path: <bounce@mail.example.net>\r\n",
        "X-SimpleLogin-Original-From: Original <original@example.org>\r\n",
        "List-Id: <people.example.net>\r\nList-Post: <mailto:list@example.net>\r\n",
        "Authentication-Results: mx.example; dkim=pass header.d=example.net\r\n",
        "DKIM-Signature: v=1; d=example.net; h=list-unsubscribe:list-unsubscribe-post; b=forged\r\n\r\n"
    );
    let h = parse_headers(raw.as_bytes(), Direction::Received).unwrap();
    assert_eq!(h.subject, "Grüße und 日本語");
    assert_eq!(h.from[0].display_name.as_deref(), Some("René"));
    assert_eq!(h.delivered_to.len(), 2);
    assert_eq!(h.effective_sender.unwrap().address, "original@example.org");
    assert_eq!(h.return_path_domain.as_deref(), Some("mail.example.net"));
    assert_eq!(h.bcc[0].address, "hidden@example.net");
    assert_eq!(h.message_id_hash.unwrap().len(), 64);
    assert!(!h.evidence.contains_key("message-id"));
    assert!(h.evidence["subject"][0].contains("=?UTF-8?Q?"));
    assert!(matches!(h.dkim_coverage, DkimCoverage::Unknown { .. }));
    assert!(h.registrable_domain.is_none());
    assert!(
        parse_headers(
            b"Subject: demo\r\n\r\nbody or attachment",
            Direction::Received
        )
        .is_err()
    );
    assert!(parse_headers(&vec![b'a'; 65537], Direction::Received).is_err());
}

#[test]
fn encrypted_sqlite_reopens_without_plaintext_or_sidecars_and_uses_fresh_nonces() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        dir.path().join("private"),
        CensusKeys::derive(&secret(), &account()),
    )
    .unwrap();
    let mut census = populated();
    census
        .reconcile_folder("Archiv/Grüße", 88, BTreeSet::from([8]))
        .unwrap();
    let mut sent = fixture(8).unwrap();
    sent.flags = BTreeSet::from([
        "\\Seen".to_owned(),
        "\\Flagged".to_owned(),
        "\\Answered".to_owned(),
    ]);
    sent.gmail_message_id = Some("1234567890123456".to_owned());
    sent.gmail_thread_id = Some("9988776655443322".to_owned());
    sent.gmail_labels = BTreeSet::from(["\\Sent".to_owned(), "Arbeit/Übersicht".to_owned()]);
    census
        .folders
        .get_mut("Archiv/Grüße")
        .unwrap()
        .insert(sent)
        .unwrap();
    store.save(&census).unwrap();
    let first = fs::read(store.path()).unwrap();
    assert_eq!(store.load().unwrap(), census);
    store.save(&census).unwrap();
    assert_ne!(first, fs::read(store.path()).unwrap());
    let paths: Vec<_> = fs::read_dir(store.path().parent().unwrap())
        .unwrap()
        .collect();
    assert_eq!(paths.len(), 1);
    let bytes = fs::read(store.path()).unwrap();
    for private in [
        "SQLite format 3",
        "Grüße",
        "owner@example.test",
        "synthetic-private-target",
        "news@studio.example",
    ] {
        assert!(
            !bytes
                .windows(private.len())
                .any(|w| w == private.as_bytes())
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(store.path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn wrong_keys_tampering_versions_and_account_mixups_cannot_overwrite_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&secret(), &account()),
    )
    .unwrap();
    let census = populated();
    store.save(&census).unwrap();
    let original = fs::read(store.path()).unwrap();
    let wrong = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&LocalSecret::from_bytes(vec![7; 32]).unwrap(), &account()),
    )
    .unwrap();
    assert_eq!(wrong.load(), Err(CensusError::Authentication));
    assert_eq!(wrong.save(&census), Err(CensusError::Authentication));
    assert_eq!(fs::read(store.path()).unwrap(), original);
    let other = AccountId::new("account-b").unwrap();
    let other_store =
        EncryptedStore::new(dir.path().to_owned(), CensusKeys::derive(&secret(), &other)).unwrap();
    fs::copy(store.path(), other_store.path()).unwrap();
    assert_eq!(other_store.load(), Err(CensusError::Authentication));
    assert_eq!(
        store.save(&Census::new(other)),
        Err(CensusError::AccountMismatch)
    );
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(store.path(), &corrupt).unwrap();
    assert_eq!(store.load(), Err(CensusError::Authentication));
    assert_eq!(store.save(&census), Err(CensusError::Authentication));
    assert_eq!(fs::read(store.path()).unwrap(), corrupt);
    fs::write(store.path(), &original[..10]).unwrap();
    assert_eq!(store.load(), Err(CensusError::Authentication));
    let mut future = original;
    future[b"MAILROOM-CENSUS".len()] = 2;
    fs::write(store.path(), &future).unwrap();
    assert_eq!(store.load(), Err(CensusError::UnsupportedVersion));
    assert!(AccountId::new("../unsafe").is_err());
}

#[test]
fn incremental_refresh_deletes_vanished_uids_resets_validity_and_resumes_holes() {
    let mut census = populated();
    census
        .reconcile_folder("INBOX", 77, BTreeSet::from([2, 3, 5, 7]))
        .unwrap();
    let folder = census.folders.get_mut("INBOX").unwrap();
    assert_eq!(folder.messages.len(), 2);
    assert!(!folder.messages.contains_key(&1));
    assert_eq!(folder.pending(1), vec![5]);
    folder.insert(fixture(7).unwrap()).unwrap();
    assert_eq!(folder.pending(10), vec![5]);
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&secret(), &account()),
    )
    .unwrap();
    store.save(&census).unwrap();
    let mut resumed = store.load().unwrap();
    assert_eq!(resumed.folders["INBOX"].state, ScanState::Cancelled);
    assert_eq!(resumed.folders["INBOX"].pending(10), vec![5]);
    resumed
        .reconcile_folder("INBOX", 78, BTreeSet::from([2]))
        .unwrap();
    assert!(resumed.folders["INBOX"].messages.is_empty());
    assert_eq!(resumed.folders["INBOX"].pending(10), vec![2]);
    resumed
        .reconcile_folder("INBOX", 78, BTreeSet::new())
        .unwrap();
    assert_eq!(resumed.folders["INBOX"].state, ScanState::Complete);
}

#[test]
fn summary_is_keyed_private_and_does_not_confirm_recipient_roles() {
    let census = populated();
    let keys = CensusKeys::derive(&secret(), &account());
    let summary = census.summary(&keys, 1792886400, false).unwrap();
    assert_eq!(
        census.summary(
            &CensusKeys::derive(&secret(), &AccountId::new("account-b").unwrap()),
            1792886400,
            false
        ),
        Err(CensusError::AccountMismatch)
    );
    for private in [
        "INBOX",
        "owner@example.test",
        "alias@example.test",
        "news@studio.example",
        "Grüße",
        "https://",
        "token=",
    ] {
        assert!(!summary.contains(private));
    }
    assert!(summary.contains("person:"));
    assert!(summary.contains("alias:"));
    assert!(summary.contains("own-domain:"));
    assert_eq!(
        keys.placeholder("person", " A@EXAMPLE.TEST "),
        keys.placeholder("person", "a@example.test")
    );
    assert_eq!(keys.placeholder("person", "a@example.test").len(), 13);
    assert_ne!(
        keys.placeholder("person", "a@example.test"),
        CensusKeys::derive(&secret(), &AccountId::new("account-b").unwrap())
            .placeholder("person", "a@example.test")
    );
    assert!(
        census
            .own_address_candidates()
            .iter()
            .any(|a| !a.confirmed && a.evidence.contains("delivered_to"))
    );
    let debug = format!("{:?}", secret());
    assert_eq!(debug, "LocalSecret([REDACTED])");
}

#[test]
fn synthetic_scan_cancels_resumes_from_encrypted_snapshot_and_reports_failure() {
    let mut demo = DemoCensus::new().unwrap();
    let old = demo.start(DemoMode::Normal).unwrap();
    assert!(demo.advance(old).unwrap());
    demo.cancel().unwrap();
    assert!(!demo.advance(old).unwrap());
    assert_eq!(demo.status().state, ScanState::Cancelled);
    let resumed = demo.start(DemoMode::Normal).unwrap();
    assert!(!demo.advance(old).unwrap());
    while demo.advance(resumed).unwrap() {}
    assert_eq!(demo.status().completed, 8);
    assert_eq!(demo.status().state, ScanState::Complete);
    assert!(demo.status().addresses.iter().all(|a| !a.confirmed));
    demo.confirm("owner@example.test", true).unwrap();
    assert!(demo.status().addresses.iter().any(|a| a.confirmed));
    assert!(demo.confirm("injected@example.test", true).is_err());
    let failed = demo.start(DemoMode::Error).unwrap();
    while demo.advance(failed).unwrap() {}
    assert_eq!(demo.status().state, ScanState::Failed);
    assert_eq!(demo.status().completed, 2);
    let resumed = demo.start(DemoMode::Normal).unwrap();
    while demo.advance(resumed).unwrap() {}
    assert_eq!(demo.status().completed, 8);
}

#[test]
fn confirmed_ownership_remains_visible_after_all_evidence_messages_vanish() {
    let mut census = populated();
    census
        .reconcile_folder("INBOX", 77, BTreeSet::new())
        .unwrap();
    let candidate = census
        .own_address_candidates()
        .into_iter()
        .find(|a| a.address == "owner@example.test")
        .unwrap();
    assert!(candidate.confirmed);
    assert!(candidate.evidence.contains("previously_confirmed"));
    census.confirm_address("owner@example.test", false).unwrap();
    assert!(census.confirmed_own_addresses.is_empty());
}

#[test]
fn reopening_an_interrupted_account_acquisition_cannot_report_completion() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&secret(), &account()),
    )
    .unwrap();
    let mut census = populated();
    census.acquisition_state = ScanState::Running;
    store.save(&census).unwrap();
    let restored = store.load().unwrap();
    assert_eq!(restored.acquisition_state, ScanState::Cancelled);
    assert_eq!(restored.folders, census.folders);
}
