use mailroom::census::{
    AccountId, Census, CensusKeys, Direction, LocalSecret, MessageRecord, ScanState, parse_headers,
    report::SenderClass,
};
use std::collections::BTreeSet;

const AS_OF: i64 = 1792886400; // 25 October 2026 UTC.
fn keys(account: &AccountId) -> CensusKeys {
    CensusKeys::derive(&LocalSecret::from_bytes(vec![42; 32]).unwrap(), account)
}
fn message(uid: u32, sender: &str, extra: &str) -> MessageRecord {
    MessageRecord {
        uid, size: 100, internal_date: "24-Oct-2026 10:00:00 +0000".into(), flags: BTreeSet::new(), gmail_message_id: Some(uid.to_string()), gmail_thread_id: None, gmail_labels: BTreeSet::from(["Private label token".into()]),
        headers: parse_headers(format!("From: {sender}\r\nTo: owner@private.example\r\nSubject: =?UTF-8?Q?Private_Secret_Order_123_123_123?=\r\nDate: 24 Oct 2026 10:00:00 +0000\r\n{extra}\r\n").as_bytes(), Direction::Received).unwrap(),
    }
}
fn fixture() -> Census {
    let mut census = Census::new(AccountId::new("fake-report").unwrap());
    let bulk = "List-Unsubscribe: <mailto:remove@shop.example>,\r\n <https://shop.example/remove?token=private-token>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nList-Id: <private-list.shop.example>\r\nFeedback-ID: something:AmazonSES\r\n";
    let records = (1..=3).map(|uid| message(uid, "offers@mail.shop.co.uk", bulk))
        .chain((4..=6).map(|uid| message(uid, "=?UTF-8?Q?Alex_M=C3=BCster?= <alex@people.example>", "In-Reply-To: <private@example.test>\r\n")))
        .chain((7..=9).map(|uid| message(uid, "team@community.example", "List-Id: <people.community.example>\r\nList-Post: <mailto:team@community.example>\r\n")))
        .chain((10..=12).map(|uid| message(uid, "receipts@platform.example", "Auto-Submitted: auto-generated\r\nReturn-Path: <bounce@amazonses.com>\r\n")))
        .chain((13..=15).map(|uid| message(uid, "unknown@unknown.example", "")))
        .chain((16..=17).map(|uid| message(uid, "small@tail.example", "")))
        .chain((18..=20).map(|uid| message(uid, "rewritten@relay.example", &format!("{bulk}X-SimpleLogin-Original-From: Claimed <claim@claimed.example>\r\n"))))
        .collect::<Vec<_>>();
    census
        .reconcile_folder("Private folder token", 7, (1..=21).collect())
        .unwrap();
    let folder = census.folders.get_mut("Private folder token").unwrap();
    for mut m in records {
        if m.uid <= 2 {
            m.flags = BTreeSet::from(["\\Seen".into(), "\\Flagged".into(), "\\Answered".into()]);
        }
        folder.insert(m).unwrap();
    }
    let mut sent = message(21, "owner@private.example", "Cc: alex@people.example\r\n");
    sent.headers.direction = Direction::Sent;
    folder.insert(sent).unwrap();
    census
        .confirmed_own_addresses
        .insert("owner@private.example".into());
    census.acquisition_state = ScanState::Complete;
    census
}

#[test]
fn report_counts_classification_keywords_psl_and_sent_evidence_are_deterministic() {
    let census = fixture();
    let key = keys(&census.account);
    let report = census.report(&key, AS_OF, true).unwrap();
    assert!(report.complete);
    assert_eq!(report.received.messages, 20);
    assert_eq!(report.sent, 1);
    assert_eq!(report.received.seen, 2);
    assert_eq!(report.received.flagged, 2);
    assert_eq!(report.received.answered, 2);
    assert_eq!(report.gmail_arrivals["2026-10"], 20);
    let shop = report
        .senders
        .iter()
        .find(|s| s.domain.as_deref() == Some("shop.co.uk"))
        .unwrap();
    assert_eq!(shop.class, SenderClass::Bulk);
    assert_eq!(shop.one_click_pair, 3);
    assert_eq!(shop.list_unsubscribe, 3);
    assert_eq!(shop.subject_keywords["###"], 3);
    assert_eq!(shop.subject_keywords["private"], 3);
    assert_eq!(shop.counts.last_12_months, 3);
    assert_eq!(shop.counts.latest_read_message_date, shop.counts.last_date);
    assert_eq!(
        report
            .senders
            .iter()
            .find(|s| s.sender.as_deref() == Some("alex@people.example"))
            .unwrap()
            .class,
        SenderClass::Person
    );
    assert_eq!(
        report
            .senders
            .iter()
            .find(|s| s.domain.as_deref() == Some("community.example"))
            .unwrap()
            .class,
        SenderClass::ListWithPeople
    );
    assert_eq!(
        report
            .senders
            .iter()
            .find(|s| s.domain.as_deref() == Some("platform.example"))
            .unwrap()
            .class,
        SenderClass::Platform
    );
    assert_eq!(
        report
            .senders
            .iter()
            .find(|s| s.domain.as_deref() == Some("unknown.example"))
            .unwrap()
            .class,
        SenderClass::Unknown
    );
    let relay = report
        .senders
        .iter()
        .find(|s| !s.relay_claims.is_empty())
        .unwrap();
    assert!(relay.raw_from.contains("rewritten@relay.example"));
    assert_eq!(relay.sender.as_deref(), Some("claim@claimed.example"));
    assert!(
        !report
            .clear_candidates
            .iter()
            .any(|c| c.domain == "claimed.example")
    );
    assert_eq!(
        report.version,
        census
            .report(&keys(&census.account), AS_OF, true)
            .unwrap()
            .version
    );
}

#[test]
fn summary_only_releases_approved_domains_and_keys_every_other_value() {
    let census = fixture();
    let key = keys(&census.account);
    let report = census.report(&key, AS_OF, true).unwrap();
    let none = report.summary_preview(&key, &BTreeSet::new()).unwrap();
    for private in [
        "shop.co.uk",
        "platform.example",
        "people.example",
        "private.example",
        "unknown.example",
        "private-token",
        "Private",
        "Secret",
        "123",
        "private-list",
        "alex@",
        "rewritten@",
        "private-list.shop.example",
    ] {
        assert!(!none.summary.contains(private), "{private}");
    }
    assert!(none.summary.contains("Long tail 2 messages"));
    assert!(none.summary.contains("person:"));
    let selection = BTreeSet::from([report
        .clear_candidates
        .iter()
        .find(|c| c.domain == "shop.co.uk")
        .unwrap()
        .id
        .clone()]);
    let allowed = report.summary_preview(&key, &selection).unwrap();
    assert!(allowed.summary.contains("shop.co.uk"));
    assert!(!allowed.summary.contains("platform.example"));
    assert_ne!(allowed.receipt, none.receipt);
    assert!(
        report
            .summary_preview(
                &key,
                &BTreeSet::from(["https://unsafe.example/token".into()])
            )
            .is_err()
    );
    let rebuilt = census
        .report(&keys(&census.account), AS_OF, true)
        .unwrap()
        .summary_preview(&keys(&census.account), &selection)
        .unwrap();
    assert_eq!(allowed.summary, rebuilt.summary);
    assert_eq!(allowed.receipt, rebuilt.receipt);
    let other = AccountId::new("other-account").unwrap();
    assert!(census.report(&keys(&other), AS_OF, true).is_err());
}

#[test]
fn gmail_deduplication_unions_labels_and_sent_is_never_received() {
    let mut census = fixture();
    let key = keys(&census.account);
    let mut duplicate = census.folders["Private folder token"].messages[&1].clone();
    duplicate.uid = 1;
    duplicate.gmail_labels.insert("Other label".into());
    census
        .reconcile_folder("Other label", 8, BTreeSet::from([1]))
        .unwrap();
    census
        .folders
        .get_mut("Other label")
        .unwrap()
        .insert(duplicate)
        .unwrap();
    let report = census.report(&key, AS_OF, true).unwrap();
    assert_eq!(report.saved_records, 22);
    assert_eq!(report.canonical_messages, 21);
    assert_eq!(report.received.messages, 20);
    assert!(
        report
            .senders
            .iter()
            .find(|s| s.domain.as_deref() == Some("shop.co.uk"))
            .unwrap()
            .gmail_labels
            .contains("Other label")
    );
    let report = census.report(&key, AS_OF, false).unwrap();
    assert_eq!(report.received.messages, 21);
    census
        .folders
        .get_mut("Other label")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .direction = Direction::Sent;
    let report = census.report(&key, AS_OF, true).unwrap();
    assert_eq!(report.sent, 2);
    assert_eq!(report.received.messages, 19);
}

#[test]
fn invalid_future_dates_calendar_windows_partial_state_and_version_changes_are_visible() {
    let mut census = fixture();
    let key = keys(&census.account);
    let old = census.report(&key, AS_OF, true).unwrap();
    let messages = &mut census
        .folders
        .get_mut("Private folder token")
        .unwrap()
        .messages;
    messages.get_mut(&1).unwrap().headers.date = Some("31 Feb 2026 10:00:00 +0000".into());
    messages.get_mut(&2).unwrap().headers.date = Some("24 Oct 2027 10:00:00 +0000".into());
    messages.get_mut(&3).unwrap().headers.date = Some("24 Sep 2025 10:00:00 +0000".into());
    messages.get_mut(&4).unwrap().internal_date = "invalid".into();
    let report = census.report(&key, AS_OF, true).unwrap();
    assert_eq!(report.received.invalid_dates, 1);
    assert_eq!(report.received.future_dates, 1);
    assert_eq!(report.invalid_arrival_dates, 1);
    let shop = report
        .senders
        .iter()
        .find(|s| s.domain.as_deref() == Some("shop.co.uk"))
        .unwrap();
    assert_eq!(shop.counts.last_12_months, 0);
    assert_eq!(shop.counts.last_13_months, 0);
    assert_eq!(shop.counts.monthly["2025-09"], 1);
    assert_ne!(old.version, report.version);
    assert_ne!(
        report.version,
        census.report(&key, AS_OF - 1, true).unwrap().version
    );
    for state in [
        ScanState::Cancelled,
        ScanState::Failed,
        ScanState::Running,
        ScanState::Idle,
    ] {
        census.acquisition_state = state;
        let r = census.report(&key, AS_OF, true).unwrap();
        assert!(!r.complete);
        assert_eq!(r.state, state);
    }
    census.acquisition_state = ScanState::Complete;
    census
        .folders
        .get_mut("Private folder token")
        .unwrap()
        .state = ScanState::Cancelled;
    assert!(!census.report(&key, AS_OF, true).unwrap().complete);
    assert!(census.report(&key, i64::MAX, true).is_err());
}

#[test]
fn ownership_is_required_for_sent_correspondence_and_own_domains_cannot_be_approved() {
    let mut census = fixture();
    let key = keys(&census.account);
    census.confirmed_own_addresses.clear();
    let report = census.report(&key, AS_OF, true).unwrap();
    assert_eq!(
        report
            .senders
            .iter()
            .find(|s| s.sender.as_deref() == Some("alex@people.example"))
            .unwrap()
            .class,
        SenderClass::Unknown
    );
    census
        .confirmed_own_addresses
        .insert("owner@shop.co.uk".into());
    assert!(
        !census
            .report(&key, AS_OF, true)
            .unwrap()
            .clear_candidates
            .iter()
            .any(|c| c.domain == "shop.co.uk")
    );
}

#[test]
fn psl_private_suffixes_wildcards_exceptions_idna_and_invalid_hosts_stay_distinct() {
    let mut census = Census::new(AccountId::new("psl-fixture").unwrap());
    let senders = [
        "sender@mail.shop.co.uk",
        "sender@a.user.github.io",
        "sender@a.b.ck",
        "sender@a.www.ck",
        "sender@post.bücher.de",
        "sender@127.0.0.1",
        "sender@bad_host.example",
    ];
    census
        .reconcile_folder("Synthetic", 1, (1..=7).collect())
        .unwrap();
    for (i, sender) in senders.iter().enumerate() {
        let uid = i as u32 + 1;
        census
            .folders
            .get_mut("Synthetic")
            .unwrap()
            .insert(message(uid, sender, ""))
            .unwrap();
    }
    let report = census.report(&keys(&census.account), AS_OF, false).unwrap();
    for expected in [
        "shop.co.uk",
        "user.github.io",
        "a.b.ck",
        "www.ck",
        "xn--bcher-kva.de",
    ] {
        assert!(report.domains.contains_key(expected), "{expected}");
    }
    assert_eq!(report.domains["unknown"].messages, 2);
}

#[test]
fn mixed_and_incomplete_bulk_evidence_stays_unknown_and_never_becomes_an_approved_domain() {
    let mut census = fixture();
    let messages = &mut census
        .folders
        .get_mut("Private folder token")
        .unwrap()
        .messages;
    messages
        .get_mut(&1)
        .unwrap()
        .headers
        .evidence
        .remove("list-unsubscribe");
    messages
        .get_mut(&1)
        .unwrap()
        .headers
        .evidence
        .remove("feedback-id");
    messages.get_mut(&2).unwrap().headers.evidence.insert(
        "list-unsubscribe".into(),
        vec!["<http://shop.example/remove>".into()],
    );
    messages.get_mut(&3).unwrap().headers.evidence.insert(
        "list-unsubscribe-post".into(),
        vec![
            "List-Unsubscribe=One-Click".into(),
            "List-Unsubscribe=One-Click".into(),
        ],
    );
    let key = keys(&census.account);
    let report = census.report(&key, AS_OF, false).unwrap();
    let shop = report
        .senders
        .iter()
        .find(|s| s.domain.as_deref() == Some("shop.co.uk"))
        .unwrap();
    assert_eq!(shop.one_click_pair, 0);
    assert_eq!(shop.class, SenderClass::Unknown);
    assert!(
        !report
            .clear_candidates
            .iter()
            .any(|c| c.domain == "shop.co.uk")
    );
    assert!(
        report
            .summary_preview(
                &keys(&AccountId::new("different-account").unwrap()),
                &BTreeSet::new()
            )
            .is_err()
    );
}
