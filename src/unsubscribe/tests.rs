use super::*;
use crate::{
    census::{AccountId, CensusKeys, EncryptedStore, LocalSecret},
    engine::{self, PreviewInput},
    rules::workspace::{Workspace, WorkspaceInput},
};
fn fixture() -> (
    tempfile::TempDir,
    EncryptedStore,
    AccountState,
    InstalledSynthetic,
) {
    let dir = tempfile::tempdir().unwrap();
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
    let mut input: PreviewInput = serde_json::from_value(sample.clone()).unwrap();
    input.context.account = "unsubscribe-fixture".into();
    input.messages.clear();
    for i in 1..=6 {
        input.messages.push(engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: format!("message-{i}"), headers: format!("From: News <news@shop.example>\nDelivered-To: team@home.example.test\nTo: team@home.example.test\nDate: 20 Oct 2026 10:00:00 +0000\nMessage-ID: <unsubscribe-{i}@example.test>\nList-Id: <offers.shop.example>\nList-Unsubscribe: <https://shop.example/remove?token=synthetic-private-token>, <mailto:leave@shop.example?subject=unsubscribe&body=synthetic-mail-token>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\nDKIM-Signature: v=1; d=shop.example; s=fixture; h=list-unsubscribe:list-unsubscribe-post; b=synthetic-signature\n"), current_folder: "INBOX".into() });
    }
    let census = engine::synthetic_census(&input).unwrap();
    let store = EncryptedStore::new(
        dir.path().join("journal"),
        CensusKeys::derive(
            &LocalSecret::random(),
            &AccountId::new("unsubscribe-fixture").unwrap(),
        ),
    )
    .unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: 1792886400,
            baseline: crate::rules::proof::Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
    let installed = InstalledSynthetic::open(dir.path(), &store, &state).unwrap();
    (dir, store, state, installed)
}
fn review(
    state: &AccountState,
    keys: &CensusKeys,
    installed: &InstalledSynthetic,
    method: Method,
) -> ReviewInput {
    let r = report(
        state,
        Provider::MailboxOrg,
        keys,
        1792886400,
        Some(installed),
    )
    .unwrap();
    ReviewInput {
        as_of: 1792886400,
        selections: vec![Selection {
            subscription: r.rows[0].id.clone(),
            method,
        }],
        folders: Folders {
            personal: "Muted".into(),
            business: "Quiet".into(),
        },
    }
}
#[test]
fn exact_candidates_flags_classification_dates_and_subscription_boundaries() {
    let (_dir, store, mut state, installed) = fixture();
    let row = report(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        1792886400,
        Some(&installed),
    )
    .unwrap()
    .rows
    .remove(0);
    assert!(row.recommended);
    assert_eq!(row.count_12_months, 6);
    assert_eq!(row.read_share, Some(0.0));
    assert_eq!(row.rank, 1);
    assert!(row.methods.contains(&Method::OneClick));
    let real = report(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        1792886400,
        None::<&InstalledSynthetic>,
    )
    .unwrap()
    .rows
    .remove(0);
    assert!(!real.methods.contains(&Method::OneClick));
    assert!(!real.recommended);
    let original = state.clone();
    for count in [4, 5] {
        state = original.clone();
        state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .retain(|uid, _| *uid <= count);
        state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .live_uids
            .retain(|uid| *uid <= count);
        let row = report(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            1792886400,
            Some(&installed),
        )
        .unwrap()
        .rows
        .remove(0);
        assert_eq!(row.recommended, count == 5);
    }
    for missing in [false, true] {
        state = original.clone();
        let m = state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .get_mut(&1)
            .unwrap();
        if missing {
            m.flags_observed = false;
        } else {
            m.flags.insert("\\Seen".into());
        }
        assert!(
            !report(
                &state,
                Provider::MailboxOrg,
                store.keys(),
                1792886400,
                Some(&installed)
            )
            .unwrap()
            .rows[0]
                .recommended
        );
    }
    state = original.clone();
    let m = state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap();
    m.flags.insert("\\Seen".into());
    m.headers.date = Some("25 Apr 2026 00:00:00 +0000".into());
    assert!(
        !report(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            1792886400,
            Some(&installed)
        )
        .unwrap()
        .rows[0]
            .recommended,
        "six month boundary inclusive"
    );
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .date = Some("24 Apr 2026 23:59:59 +0000".into());
    assert!(
        report(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            1792886400,
            Some(&installed)
        )
        .unwrap()
        .rows[0]
            .recommended
    );
    for (key, value) in [
        ("list-post", "<mailto:people@example.test>"),
        ("list-unsubscribe-post", "absent"),
    ] {
        state = original.clone();
        for m in state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .values_mut()
        {
            m.headers.evidence.insert(key.into(), vec![value.into()]);
            m.headers
                .evidence
                .insert("auto-submitted".into(), vec!["auto-generated".into()]);
        }
        assert!(
            !report(
                &state,
                Provider::MailboxOrg,
                store.keys(),
                1792886400,
                Some(&installed)
            )
            .unwrap()
            .rows[0]
                .recommended
        );
    }
    state = original.clone();
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .evidence
        .insert("list-id".into(), vec!["<other.shop.example>".into()]);
    assert_eq!(
        report(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            1792886400,
            Some(&installed)
        )
        .unwrap()
        .rows
        .len(),
        2,
        "same domain is not same subscription"
    );
    state.census.acquisition_state = crate::census::ScanState::Cancelled;
    assert!(
        report(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            1792886400,
            Some(&installed)
        )
        .unwrap()
        .rows
        .iter()
        .all(|r| !r.recommended)
    );
}
#[test]
fn provider_observation_must_correlate_to_one_passing_signature_covering_both_headers() {
    let (_dir, _store, state, installed) = fixture();
    let mut h = state.census.folders["INBOX"].messages[&1].headers.clone();
    let signature = h.evidence["dkim-signature"][0].clone();
    assert!(installed.authentication(&h).contains("pass_both"));
    assert!(synthetic::auth_test(&h, &signature, false).contains("fail"));
    assert!(synthetic::auth_test(&h, "wrong-signature", true).contains("wrong_signature"));
    h.evidence.insert(
        "dkim-signature".into(),
        vec![
            "v=1; h=list-unsubscribe; b=one".into(),
            "v=1; h=list-unsubscribe-post; b=two".into(),
        ],
    );
    assert!(
        synthetic::auth_test(&h, &h.evidence["dkim-signature"][0], true).contains("incomplete")
    );
    h.evidence.remove("dkim-signature");
    assert!(installed.authentication(&h).contains("unknown"));
    h.evidence.insert(
        "authentication-results".into(),
        vec!["provider; dkim=pass header.d=shop.example".into()],
    );
    assert!(installed.authentication(&h).contains("unknown"));
}
#[test]
fn https_dns_and_checked_peer_refuse_ambiguous_private_and_mapped_destinations() {
    for raw in [
        "http://shop.example/x",
        "https://user:password@shop.example/x",
        "https://shop.example:444/x",
        "https://shop.example:/x",
        "https://shop.example:garbage/x",
        "https://shop.example/x%",
        "https://shop.example/x%zz",
        "https://shop.example/x#fragment",
        "https://shop.example\\@127.0.0.1/x",
        "https://127.0.0.1/x",
        "https://10.0.0.1/x",
        "https://[::1]/x",
        "https://[::ffff:8.8.8.8]/x",
        "https://[fe80::1]/x",
        "https://[fc00::1]/x",
        "https://2130706433/x",
        "https://0177.0.0.1/x",
        "https://0x7f000001/x",
        "https://shop.example/%0d\n",
    ] {
        assert!(safety::Endpoint::parse(raw).is_err(), "Rejected unsafe URL");
    }
    for ip in [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "127.1.2.3",
        "169.254.1.1",
        "172.31.1.1",
        "192.168.1.1",
        "198.18.0.1",
        "224.1.1.1",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "64:ff9b::a00:1",
        "2001:db8::1",
        "2002:7f00:1::",
        "3fff:1::1",
    ] {
        assert!(
            !safety::public(ip.parse().unwrap()),
            "Nonpublic or transition IP"
        );
    }
    let raw = "https://shop.example/remove?token=synthetic-private-token";
    assert!(safety::Endpoint::parse(raw).unwrap().bind(&[]).is_err());
    assert!(
        safety::Endpoint::parse(raw)
            .unwrap()
            .bind(&["8.8.8.8".parse().unwrap(), "127.0.0.1".parse().unwrap()])
            .is_err()
    );
    let post = safety::Endpoint::parse(raw)
        .unwrap()
        .bind(&["8.8.8.8".parse().unwrap()])
        .unwrap();
    assert!(post.verify_peer("1.1.1.1".parse().unwrap()).is_err());
    assert!(post.verify_peer("8.8.8.8".parse().unwrap()).is_ok());
    assert!(post.follow_redirect("https://127.0.0.1/").is_err());
    assert_eq!(post.body(), "List-Unsubscribe=One-Click");
    assert_eq!(post.headers().len(), 1);
    assert!(
        !format!("{:?}", safety::Endpoint::parse(raw).unwrap()).contains("synthetic-private-token")
    );
}
#[test]
fn confirmation_token_safe_history_independent_effects_and_recovery() {
    let (dir, store, mut state, mut installed) = fixture();
    let input = review(&state, store.keys(), &installed, Method::OneClick);
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    assert_eq!(p.receipt.len(), 64);
    assert!(
        confirm(
            &mut state.clone(),
            &p,
            &"x".repeat(64),
            CONFIRM,
            None,
            1792886400
        )
        .is_err()
    );
    let ids = confirm(&mut state, &p, &p.receipt, CONFIRM, None, 1792886400).unwrap();
    store.save_state(&mut state).unwrap();
    let mut recovered = store.load_state().unwrap();
    assert_eq!(recovered.unsubscribe.history[0].status, Status::Uncertain);
    observe(&mut recovered, &installed, &ids[0]).unwrap();
    assert_eq!(recovered.unsubscribe.history[0].status, Status::Uncertain);
    let entry = state.unsubscribe.history[0].clone();
    let source = candidates::source(&state, &p.actions[0].source).unwrap();
    installed
        .submit(&ids[0], &entry, source, &p.actions[0].recipient)
        .unwrap();
    installed
        .submit(&ids[0], &entry, source, &p.actions[0].recipient)
        .unwrap();
    assert_eq!(installed.received().len(), 1);
    let ack = installed.received()[&ids[0]].clone();
    assert_eq!(ack.verb, "POST");
    assert_eq!(ack.form.as_deref(), Some("List-Unsubscribe=One-Click"));
    assert_eq!(ack.http_status, Some(202));
    drop(installed);
    let installed = InstalledSynthetic::open(dir.path(), &store, &state).unwrap();
    observe(&mut recovered, &installed, &ids[0]).unwrap();
    assert_eq!(
        recovered.unsubscribe.history[0].status,
        Status::HttpObserved
    );
    assert_eq!(recovered.unsubscribe.history[0].success, "unknown");
    for encoded in [
        serde_json::to_string(&recovered.unsubscribe).unwrap(),
        serde_json::to_string(&installed.saved_for_test()).unwrap(),
    ] {
        assert!(!encoded.contains("synthetic-private-token"));
        assert!(!encoded.contains("synthetic-mail-token"));
        assert!(!encoded.contains("https://"));
    }
    let fresh = preview(
        &recovered,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    assert_ne!(fresh.receipt, p.receipt);
    assert!(
        confirm(
            &mut recovered,
            &fresh,
            &p.receipt,
            CONFIRM,
            None,
            1792886400
        )
        .is_err()
    );
    let mail = review(&recovered, store.keys(), &installed, Method::LocalMail);
    let p = preview(
        &recovered,
        Provider::MailboxOrg,
        store.keys(),
        &mail,
        Some(&installed),
    )
    .unwrap();
    assert!(
        confirm(
            &mut recovered.clone(),
            &p,
            &p.receipt,
            CONFIRM,
            None,
            1792886400
        )
        .is_err()
    );
    assert!(
        confirm(
            &mut recovered,
            &p,
            &p.receipt,
            CONFIRM,
            Some(MAIL_CONFIRM),
            1792886400
        )
        .is_ok()
    );
    assert!(
        safety::Mail::parse(
            "mailto:leave@shop.example?subject=x%0d%0aBcc:evil@example.test",
            "team@home.example.test"
        )
        .is_err()
    );
    assert!(
        safety::Mail::parse(
            "mailto:leave@shop.example?bcc=evil@example.test",
            "team@home.example.test"
        )
        .is_err()
    );
}
#[test]
fn mute_compiles_exact_list_before_bulk_marks_seen_and_preserves_user_edits() {
    let (_dir, store, mut state, installed) = fixture();
    let input = review(&state, store.keys(), &installed, Method::Mute);
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    let proof = p.proof.as_ref().unwrap();
    assert!(proof.artifact.executable().contains(
        "header :is :comparator \"i;ascii-casemap\" \"list-id\" \"<offers.shop.example>\""
    ));
    assert!(
        proof
            .decisions
            .iter()
            .all(|d| d.mark_read && d.destination.label() == "Personal/Muted")
    );
    confirm(&mut state, &p, &p.receipt, CONFIRM, None, 1792886400).unwrap();
    store.save_state(&mut state).unwrap();
    let remove = ReviewInput {
        selections: vec![Selection {
            subscription: input.selections[0].subscription.clone(),
            method: Method::RemoveMute,
        }],
        ..input.clone()
    };
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &remove,
        Some(&installed),
    )
    .unwrap();
    assert!(
        p.proof
            .as_ref()
            .unwrap()
            .decisions
            .iter()
            .all(|d| !d.mark_read)
    );
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml = state
        .workspace
        .as_ref()
        .unwrap()
        .input
        .documents
        .override_yaml
        .replace("folder: Muted", "folder: Edited");
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            &remove,
            Some(&installed)
        )
        .is_err()
    );
}
#[test]
fn fourteen_day_stragglers_require_new_source_and_new_internal_arrival() {
    let (_dir, store, mut state, installed) = fixture();
    let input = review(&state, store.keys(), &installed, Method::OneClick);
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    let time = 1792886400;
    confirm(&mut state, &p, &p.receipt, CONFIRM, None, time).unwrap();
    state.unsubscribe.history[0].status = Status::HttpObserved;
    let view = |s: &AccountState, at| {
        report(s, Provider::MailboxOrg, store.keys(), at, Some(&installed))
            .unwrap()
            .rows
            .remove(0)
            .straggler
    };
    assert_eq!(view(&state, time + 13 * 86400), "wait_14_days");
    assert_eq!(
        view(&state, time + 14 * 86400),
        "no_new_arrivals_old_mail_is_not_a_straggler"
    );
    let f = state.census.folders.get_mut("INBOX").unwrap();
    let mut m = f.messages[&1].clone();
    m.uid = 70;
    m.gmail_message_id = Some("70".into());
    m.internal_date = "23-Oct-2026 10:00:00 +0000".into();
    f.messages.insert(70, m);
    f.live_uids.insert(70);
    assert_eq!(
        view(&state, time + 14 * 86400),
        "no_new_arrivals_old_mail_is_not_a_straggler"
    );
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&70)
        .unwrap()
        .internal_date = "25-Oct-2026 10:00:00 +0000".into();
    assert_eq!(
        view(&state, time + 14 * 86400),
        "no_new_arrivals_old_mail_is_not_a_straggler",
        "Changing a physical source cannot make a known Message-ID new"
    );
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&70)
        .unwrap()
        .headers
        .message_id_hash = Some(digest(b"new-message-identity"));
    assert_eq!(view(&state, time + 14 * 86400), "propose_mute_new_arrival");
    state.unsubscribe.history[0].status = Status::Uncertain;
    assert_eq!(
        view(&state, time + 14 * 86400),
        "unknown_request_observation"
    );
}

#[test]
fn ambiguous_lists_receiving_targets_and_gmail_observations_do_not_broaden_selection() {
    let (_dir, store, mut state, installed) = fixture();
    let f = state.census.folders.get_mut("INBOX").unwrap();
    f.messages
        .get_mut(&1)
        .unwrap()
        .headers
        .evidence
        .remove("list-id");
    f.messages.get_mut(&2).unwrap().headers.evidence.insert(
        "list-id".into(),
        vec!["<one.example>".into(), "<two.example>".into()],
    );
    let mut duplicate = f.messages[&3].clone();
    duplicate.uid = 70;
    duplicate.flags.insert("\\Seen".into());
    f.messages.insert(70, duplicate);
    f.live_uids.insert(70);
    let report = report(
        &state,
        Provider::Gmail,
        store.keys(),
        1792886400,
        Some(&installed),
    )
    .unwrap();
    assert_eq!(
        report.rows.len(),
        3,
        "Absent, exact and ambiguous List-Id targets remain distinct"
    );
    let ambiguous = report
        .rows
        .iter()
        .find(|r| r.list_evidence == "ambiguous_no_from_fallback")
        .unwrap();
    assert!(!ambiguous.methods.contains(&Method::Mute));
    assert!(!ambiguous.methods.contains(&Method::OneClick));
    assert!(!ambiguous.recommended);
    let exact = report.rows.iter().find(|r| r.list_id.is_some()).unwrap();
    assert_eq!(exact.count_12_months, 4);
    assert_eq!(
        exact.read_share,
        Some(0.25),
        "Gmail duplicate observations union Seen instead of selecting the first unread copy"
    );
    let f = state.census.folders.get_mut("INBOX").unwrap();
    let m = f.messages.get_mut(&4).unwrap();
    m.headers.delivered_to.push(crate::census::Mailbox {
        address: "second@home.example.test".into(),
        display_name: None,
    });
    let report = super::report(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        1792886400,
        Some(&installed),
    )
    .unwrap();
    let mixed = report.rows.iter().find(|r| r.recipient.is_empty()).unwrap();
    assert!(!mixed.methods.contains(&Method::OneClick));
    assert!(!mixed.methods.contains(&Method::LocalMail));
    assert!(!mixed.methods.contains(&Method::Mute));
}

#[test]
fn exact_effect_digest_rejects_changed_sender_recipient_and_mail_content() {
    let (_dir, store, mut state, mut installed) = fixture();
    let input = review(&state, store.keys(), &installed, Method::LocalMail);
    let initial = state.clone();
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    let ids = confirm(
        &mut state,
        &p,
        &p.receipt,
        CONFIRM,
        Some(MAIL_CONFIRM),
        1792886400,
    )
    .unwrap();
    let entry = &state.unsubscribe.history[0];
    let source = candidates::source(&state, &p.actions[0].source).unwrap();
    assert!(
        installed
            .submit(&ids[0], entry, source, "changed@home.example.test")
            .is_err()
    );
    let mut changed = source.clone();
    changed.headers.evidence.insert(
        "list-unsubscribe".into(),
        vec!["<mailto:other@shop.example?subject=changed&body=changed>".into()],
    );
    assert!(
        installed
            .submit(&ids[0], entry, &changed, &p.actions[0].recipient)
            .is_err()
    );
    assert!(installed.received().is_empty());
    let mut changed_state = initial;
    changed_state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .from[0]
        .address = "changed@shop.example".into();
    let fresh = preview(
        &changed_state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    assert!(
        confirm(
            &mut changed_state,
            &fresh,
            &p.receipt,
            CONFIRM,
            Some(MAIL_CONFIRM),
            1792886400
        )
        .is_err()
    );
}

#[test]
fn mute_pin_conflicts_block_confirmation_and_from_fallback_excludes_unseen_lists() {
    let (_dir, store, mut state, installed) = fixture();
    for m in state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .values_mut()
    {
        m.headers.evidence.remove("list-id");
    }
    let input = review(&state, store.keys(), &installed, Method::Mute);
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    assert!(
        p.proof
            .as_ref()
            .unwrap()
            .artifact
            .executable()
            .contains("not exists \"list-id\"")
    );
    let mut proposed = state.clone();
    confirm(&mut proposed, &p, &p.receipt, CONFIRM, None, 1792886400).unwrap();
    let mut new_message = proposed.census.folders["INBOX"].messages[&1].clone();
    new_message.uid = 70;
    new_message.headers.message_id_hash = Some(digest(b"new-unseen-list"));
    new_message
        .headers
        .evidence
        .insert("list-id".into(), vec!["<unseen.shop.example>".into()]);
    let f = proposed.census.folders.get_mut("INBOX").unwrap();
    f.live_uids.insert(70);
    f.messages.insert(70, new_message);
    let proof = mute::proof(&proposed, Provider::MailboxOrg, 1792886400, true).unwrap();
    let future = proof
        .decisions
        .iter()
        .find(|d| d.message_id_hash.as_deref() == Some(&digest(b"new-unseen-list")))
        .unwrap();
    assert!(!future.mark_read);
    assert_eq!(future.destination.label(), "Personal/Newsletters");
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .pins
        .push(crate::rules::proof::Pin {
            account: state.census.account.as_str().into(),
            message_id_hash: state.census.folders["INBOX"].messages[&1]
                .headers
                .message_id_hash
                .clone()
                .unwrap(),
            expected: crate::rules::proof::Destination::Inbox,
        });
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        &input,
        Some(&installed),
    )
    .unwrap();
    assert_eq!(p.proof.as_ref().unwrap().gate, "blocked");
    assert!(confirm(&mut state, &p, &p.receipt, CONFIRM, None, 1792886400).is_err());
    assert!(state.unsubscribe.mutes.is_empty());
}

mod adapters {
    use super::*;
    use crate::send::{
        handoff::Recorder,
        smtp::{
            Login, Outcome, Timeouts,
            tests::{DUMMY, Dummy, Fake, Mode},
        },
    };
    use std::{sync::atomic::AtomicUsize, time::Duration};

    const URI: &str = "https://shop.example/remove?token=synthetic-private-token";
    const MAILTO: &str = "mailto:leave@shop.example?subject=unsubscribe&body=synthetic-mail-token";

    fn confirmed(
        state: &mut AccountState,
        store: &EncryptedStore,
        installed: &InstalledSynthetic,
        method: Method,
    ) -> (String, String) {
        let input = review(state, store.keys(), installed, method.clone());
        // Real mail adapters bind what they hand over; one-click keeps the fixture's authority.
        let wired = Wired {
            trusted_recipients: true,
            authserv: None,
        };
        let p = if method == Method::OneClick {
            preview(
                state,
                Provider::MailboxOrg,
                store.keys(),
                &input,
                Some(installed),
            )
        } else {
            preview(
                state,
                Provider::MailboxOrg,
                store.keys(),
                &input,
                Some(&wired),
            )
        }
        .unwrap();
        let recipient = p.actions[0].recipient.clone();
        let ids = confirm(
            state,
            &p,
            &p.receipt.clone(),
            p.confirmation_action,
            p.local_mail_confirmation,
            1792886400,
        )
        .unwrap();
        store.save_state(state).unwrap();
        (ids[0].clone(), recipient)
    }
    fn saved(store: &EncryptedStore, id: &str) -> Entry {
        store
            .load_state()
            .unwrap()
            .unsubscribe
            .history
            .into_iter()
            .find(|e| e.id == id)
            .unwrap()
    }

    #[test]
    fn one_click_posts_through_the_native_adapter_and_logs_status_and_host_only() {
        use super::super::one_click::tests::{PUBLIC, fake, network};
        for (reply, delay, status, http) in [
            (
                "HTTP/1.1 200 OK\r\n\r\n",
                Duration::ZERO,
                Status::HttpObserved,
                Some(200),
            ),
            (
                "HTTP/1.1 302 Found\r\nLocation: https://10.0.0.1/\r\n\r\n",
                Duration::ZERO,
                Status::HttpObserved,
                Some(302),
            ),
            (
                "HTTP/1.1 200 OK\r\n\r\n",
                Duration::from_millis(1500),
                Status::Uncertain,
                None,
            ),
        ] {
            let (_dir, store, mut state, installed) = fixture();
            let (id, _) = confirmed(&mut state, &store, &installed, Method::OneClick);
            assert_eq!(
                saved(&store, &id).status,
                Status::Uncertain,
                "a saved intent reloads as uncertain"
            );
            let server = fake(reply, delay);
            let net = network(&[("shop.example", &[PUBLIC])], &[(PUBLIC, server.address)]);
            assert!(
                post_one_click(
                    &mut state,
                    &id,
                    "https://shop.example/remove?token=changed",
                    &net,
                    &server.identity.connector(),
                    Duration::from_millis(500)
                )
                .is_err()
            );
            post_one_click(
                &mut state,
                &id,
                URI,
                &net,
                &server.identity.connector(),
                Duration::from_millis(500),
            )
            .unwrap();
            store.save_state(&mut state).unwrap();
            assert!(
                post_one_click(
                    &mut state,
                    &id,
                    URI,
                    &net,
                    &server.identity.connector(),
                    Duration::from_millis(500)
                )
                .is_err(),
                "never repeated"
            );
            let entry = saved(&store, &id);
            assert_eq!(
                (entry.status, entry.http_status, entry.host.as_deref()),
                (status, http, Some("shop.example"))
            );
            assert_eq!(server.requests.lock().unwrap().len(), 1);
            let history =
                serde_json::to_string(&store.load_state().unwrap().unsubscribe.history).unwrap();
            assert!(
                !history.contains("synthetic-private-token") && !history.contains("/remove"),
                "full URI never logged"
            );
        }
        let (_dir, store, mut state, installed) = fixture();
        let (id, _) = confirmed(&mut state, &store, &installed, Method::OneClick);
        let net = network(&[("shop.example", &["10.1.2.3"])], &[]);
        post_one_click(
            &mut state,
            &id,
            URI,
            &net,
            &crate::fake_tls::identity("shop.example").connector(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            state.unsubscribe.history.last().unwrap().status,
            Status::NotSent
        );
    }

    #[test]
    fn local_unsubscribe_mail_goes_through_smtp_from_the_receiving_address() {
        for (mode, expected) in [
            (Mode::Accept, Status::Submitted),
            (Mode::DropAfterData, Status::Uncertain),
            (Mode::RcptReject, Status::NotSent),
        ] {
            let (_dir, store, mut state, installed) = fixture();
            let (id, recipient) = confirmed(&mut state, &store, &installed, Method::LocalMail);
            let fake = Fake::start(mode, true);
            let backend = Dummy(AtomicUsize::new(0));
            let login = Login {
                user: "books@work.example.test",
                backend: &backend,
                reference: "account-password-ref",
            };
            let timeouts = Timeouts {
                command: Duration::from_secs(5),
                final_reply: Duration::from_millis(500),
            };
            submit_local_mail(
                &mut state,
                &id,
                MAILTO,
                &recipient,
                1792886400,
                &fake.server(true),
                &login,
                timeouts,
            )
            .unwrap();
            store.save_state(&mut state).unwrap();
            assert!(
                submit_local_mail(
                    &mut state,
                    &id,
                    MAILTO,
                    &recipient,
                    1792886400,
                    &fake.server(true),
                    &login,
                    timeouts
                )
                .is_err()
            );
            let seen = fake.finish();
            let entry = saved(&store, &id);
            assert_eq!(entry.status, expected, "{mode:?}");
            if mode == Mode::Accept {
                assert_eq!(
                    entry.smtp,
                    Some(Outcome::Accepted {
                        code: 250,
                        enhanced: Some("2.0.0".into())
                    })
                );
                let bytes = seen.message.unwrap();
                assert_eq!(
                    crate::unsubscribe::digest(&bytes),
                    entry.payload_digest,
                    "the confirmed digest is the SHA-256 of exactly the submitted bytes"
                );
                let message = String::from_utf8(bytes).unwrap();
                assert!(message.starts_with(&format!(
                    "From: <{recipient}>\r\nTo: <leave@shop.example>\r\nSubject: unsubscribe\r\n"
                )));
                assert!(message.ends_with("\r\n\r\nsynthetic-mail-token\r\n"));
                assert!(seen.commands.contains(&format!("MAIL FROM:<{recipient}>")));
            }
            assert!(
                !serde_json::to_string(&store.load_state().unwrap())
                    .unwrap()
                    .contains(DUMMY)
            );
        }
    }

    #[test]
    fn a_changed_review_time_changes_the_bytes_and_is_refused_before_sending() {
        let (_dir, store, mut state, installed) = fixture();
        let (id, recipient) = confirmed(&mut state, &store, &installed, Method::LocalMail);
        let backend = Dummy(AtomicUsize::new(0));
        let login = Login {
            user: "books@work.example.test",
            backend: &backend,
            reference: "account-password-ref",
        };
        let fake = Fake::start(Mode::Accept, true);
        let outcome = submit_local_mail(
            &mut state,
            &id,
            MAILTO,
            &recipient,
            1792886401,
            &fake.server(true),
            &login,
            Timeouts::default(),
        );
        assert!(outcome.is_err());
        assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(fake);
    }

    #[test]
    fn mailto_decoding_preserves_unicode_fields_and_stable_framing() {
        let mail = safety::Mail::parse(
            "mailto:leave@shop.example?subject=Gr%C3%BC%C3%9Fe&body=token%3Ddummy%26x%0D%0Asecond%0Athird",
            "alias@home.example.test",
        )
        .unwrap();
        let _: &zeroize::Zeroizing<String> = &mail.subject;
        let _: &zeroize::Zeroizing<String> = &mail.body;
        assert_eq!(mail.subject.as_str(), "Grüße");
        assert_eq!(mail.body.as_str(), "token=dummy&x\r\nsecond\nthird");
        assert_eq!(
            mail.digest(),
            digest("[\"leave@shop.example\",\"alias@home.example.test\",\"Grüße\",\"token=dummy&x\\r\\nsecond\\nthird\"]".as_bytes())
        );
        let message = mail.message(1792886400).unwrap();
        let text = std::str::from_utf8(&message).unwrap();
        assert!(text.contains("\r\nSubject: =?UTF-8?B?R3LDvMOfZQ==?=\r\n"));
        assert!(text.ends_with("\r\n\r\ntoken=dummy&x\r\nsecond\r\nthird\r\n"));
    }

    #[test]
    fn malformed_mailto_text_is_refused_after_valid_token_bytes() {
        for query in [
            "subject=dummy%FF",
            "body=dummy%FF",
            "body=dummy%0",
            "body=dummy%GG",
            "subject=dummy&body=dummy%FF",
            "subject=dummy&subject=other",
        ] {
            assert!(
                safety::Mail::parse(
                    &format!("mailto:leave@shop.example?{query}"),
                    "alias@home.example.test",
                )
                .is_err()
            );
        }
    }

    #[test]
    fn a_fragment_in_a_mailto_link_is_refused() {
        assert!(
            safety::Mail::parse(
                "mailto:leave@shop.example?subject=a#b",
                "alias@home.example.test"
            )
            .is_err()
        );
        assert!(
            safety::Mail::parse(
                "mailto:leave@shop.example?subject=a%23b",
                "alias@home.example.test"
            )
            .is_ok()
        );
    }

    #[test]
    fn mailto_handoff_composes_in_mail_from_the_receiving_address() {
        let (_dir, store, mut state, installed) = fixture();
        let (id, recipient) = confirmed(&mut state, &store, &installed, Method::MailAppPreparation);
        assert_eq!(saved(&store, &id).status, Status::Prepared);
        let recorder = Recorder::default();
        assert!(
            hand_off_mailto(
                &mut state,
                &id,
                "mailto:other@shop.example",
                &recipient,
                &recorder
            )
            .is_err()
        );
        hand_off_mailto(&mut state, &id, MAILTO, &recipient, &recorder).unwrap();
        store.save_state(&mut state).unwrap();
        assert_eq!(saved(&store, &id).status, Status::HandedOff);
        let launched = recorder.0.lock().unwrap();
        assert_eq!(launched.len(), 1);
        let crate::send::handoff::Request::ComposeInMail {
            to,
            from,
            subject,
            body,
        } = &launched[0];
        assert_eq!(
            (to.as_str(), from.as_str(), subject.as_str(), body.as_str()),
            (
                "leave@shop.example",
                recipient.as_str(),
                "unsubscribe",
                "synthetic-mail-token"
            ),
            "Mail gets the confirmed fields from the confirmed sender, not the raw link"
        );
    }
}
