use super::*;
use crate::census::{Census, parse_headers};

pub(crate) const AS_OF: i64 = 1792886400;
pub(crate) fn install(census: &mut Census, provider: Provider) {
    let comparison = provider == Provider::MailboxOrg;
    let own = if comparison {
        "new@destination.example.test"
    } else {
        "owner@example.test"
    };
    census.confirmed_own_addresses.insert(own.into());
    let specs = if comparison {
        vec![
            (
                101,
                "alex@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: new@destination.example.test\r\n",
                Some(Delivery::Direct {
                    address: own.into(),
                }),
            ),
            (
                102,
                "forwarded@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: owner@example.test\r\nDelivered-To: new@destination.example.test\r\n",
                Some(Delivery::Forwarded),
            ),
            (
                103,
                "ambiguous@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: new@destination.example.test\r\nCc: another@destination.example.test\r\n",
                Some(Delivery::Ambiguous),
            ),
            (
                104,
                "missing@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: new@destination.example.test\r\n",
                None,
            ),
        ]
    } else {
        vec![
            (
                101,
                "waiting@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: own.er@gmail.com, owner@googlemail.com\r\nX-Original-To: own.er@gmail.com\r\n",
                None,
            ),
            (
                102,
                "quiet@people.example",
                "24 Sep 2025 23:59:59 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                103,
                "forwarded@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                104,
                "ambiguous@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                105,
                "missing@people.example",
                "24 Oct 2026 10:00:00 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                106,
                "boundary@people.example",
                "25 Sep 2025 00:00:00 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                107,
                "future@people.example",
                "25 Oct 2027 00:00:00 +0000",
                "To: owner@example.test\r\n",
                None,
            ),
            (
                108,
                "invalid@people.example",
                "invalid",
                "To: owner@example.test\r\n",
                None,
            ),
        ]
    };
    let mut records = Vec::new();
    for (uid, sender, date, fields, delivery) in specs {
        let headers = parse_headers(format!("From: {sender}\r\nDate: {date}\r\nMessage-ID: <winddown-{}-{uid}@example.test>\r\n{fields}\r\n", if comparison { "comparison" } else { "source" }).as_bytes(), Direction::Received).unwrap();
        let message = MessageRecord {
            delivery: Default::default(),
            uid,
            size: 100,
            internal_date: "24-Oct-2026 10:00:00 +0000".into(),
            flags_observed: true,
            flags: BTreeSet::new(),
            gmail_message_id: Some(format!("winddown-{uid}")),
            gmail_thread_id: None,
            gmail_labels: BTreeSet::new(),
            headers,
        };
        if let Some(delivery) = delivery {
            census
                .winddown_evidence
                .deliveries
                .insert(message_identity(&message), delivery);
        }
        records.push(message);
        let sent_uid = uid + 1000;
        records.push(MessageRecord {
            delivery: Default::default(),
            uid: sent_uid,
            size: 100,
            internal_date: "24-Oct-2026 10:00:00 +0000".into(),
            flags_observed: true,
            flags: BTreeSet::new(),
            gmail_message_id: Some(format!("winddown-sent-{uid}")),
            gmail_thread_id: None,
            gmail_labels: BTreeSet::new(),
            headers: parse_headers(
                format!("From: {own}\r\nTo: {sender}\r\nDate: 24 Oct 2026 10:00:00 +0000\r\n\r\n")
                    .as_bytes(),
                Direction::Sent,
            )
            .unwrap(),
        });
    }
    if let Some(folder) = census.folders.get_mut("INBOX") {
        for m in records {
            folder.live_uids.insert(m.uid);
            folder.messages.insert(m.uid, m);
        }
    }
    census.winddown_evidence.complete_since = Some(1704067200);
    census.winddown_evidence.complete_through = Some(AS_OF);
    census.winddown_evidence.spam_count = if comparison { None } else { Some(0) };
}
