use super::*;
use crate::census::{AccountId, Census, EncryptedStore, LocalSecret, ScanState};
fn fixture(root: &std::path::Path, id: &str, provider: Provider) -> (EncryptedStore, AccountState) {
    let account = AccountId::new(id).unwrap();
    let store = EncryptedStore::new(
        root.join(id),
        CensusKeys::derive(&LocalSecret::from_bytes(vec![32; 32]).unwrap(), &account),
    )
    .unwrap();
    let mut census = Census::new(account);
    census
        .reconcile_folder("INBOX", 7, BTreeSet::from([1]))
        .unwrap();
    census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .insert(crate::census::demo::fixture(1).unwrap())
        .unwrap();
    census.acquisition_state = ScanState::Complete;
    synthetic::install(&mut census, provider);
    store.save(&census).unwrap();
    let state = store.load_state().unwrap();
    (store, state)
}
fn snapshot() -> Snapshot {
    Snapshot {
        content_version: "synthetic-ledger-version".into(),
        document: Default::default(),
    }
}

#[test]
fn bundled_catalog_projection_binds_reviews_and_refreshes_historical_links() {
    const EMPTY: &str = "domain,country,priority,settings_url,source,checked\n";
    const SERVICES: &str = "domain,source,checked\n";
    let root = tempfile::tempdir().unwrap();
    let (store, mut saved) = fixture(root.path(), "catalog", Provider::MailboxOrg);
    let record = saved
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap();
    record.headers = crate::census::parse_headers(b"From: alerts@example.com\r\nTo: owner@example.test\r\nSubject: Receipt\r\nAuto-Submitted: auto-generated\r\nDate: Fri, 23 Oct 2026 12:00:00 +0000\r\nMessage-ID: <catalog-fixture@example.test>\r\n\r\n", Direction::Received).unwrap();
    let money =
        format!("{EMPTY}example.com,DE,1,https://example.com/settings,contrib,2026-10-01\n");
    let services = format!("{SERVICES}example.com,contrib,2026-10-01\n");
    let data = |notes: &str, services: &str, as_of| {
        crate::sender_data::Catalog::parse(crate::sender_data::Package {
            release: "schema: 1\nversion: 1.0.0\nreleased: 2026-10-29\nlicense: CC0-1.0\n",
            license: "SPDX-License-Identifier: CC0-1.0",
            sources: notes,
            categories: [&money, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY],
            services,
        })
        .unwrap()
        .winddown(as_of)
        .unwrap()
    };
    let reviewed = data("Synthetic fixture provenance", &services, synthetic::AS_OF);
    let unreviewed = data("Synthetic fixture provenance", SERVICES, synthetic::AS_OF);
    let sender = "alerts@example.com";
    let report = derive(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &snapshot(),
        &reviewed,
        synthetic::AS_OF,
    )
    .unwrap();
    assert_eq!(row(&report, sender).group, Group::Service);
    assert_eq!(
        row(&report, sender).settings_link.as_deref(),
        Some("https://example.com/settings")
    );
    let uncertain = derive(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &snapshot(),
        &unreviewed,
        synthetic::AS_OF,
    )
    .unwrap();
    assert_eq!(row(&uncertain, sender).group, Group::Unresolved);
    let change = Change::Dismiss {
        rows: BTreeSet::from([row(&report, sender).id.clone()]),
        dismissed: true,
    };
    let decision = review(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &[],
        &snapshot(),
        &reviewed,
        synthetic::AS_OF,
        &report.version,
        &change,
    )
    .unwrap();
    let changed = data(
        "Changed synthetic provenance only",
        &services,
        synthetic::AS_OF,
    );
    assert!(
        review(
            &source(&saved, &store, Provider::MailboxOrg),
            &[],
            &[],
            &snapshot(),
            &changed,
            synthetic::AS_OF,
            &report.version,
            &change
        )
        .is_err()
    );
    let refreshed = derive(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &snapshot(),
        &changed,
        synthetic::AS_OF,
    )
    .unwrap();
    assert_ne!(refreshed.version, report.version);
    assert_ne!(
        refreshed
            .export_preview(store.keys(), Disclosure::LocalFile)
            .receipt,
        report
            .export_preview(store.keys(), Disclosure::LocalFile)
            .receipt
    );
    let summary = report.summary(store.keys());
    assert!(!summary.contains("alerts@example.com"));
    assert!(!summary.contains("https://example.com/settings"));
    assert!(!summary.contains("Synthetic fixture provenance"));
    let stale_as_of = DateTime::parse_from_rfc3339("2028-10-01T00:00:00Z")
        .unwrap()
        .timestamp();
    let stale_data = data("Synthetic fixture provenance", &services, stale_as_of);
    let stale = derive(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &snapshot(),
        &stale_data,
        stale_as_of,
    )
    .unwrap();
    assert_eq!(row(&stale, sender).group, Group::Unresolved);
    assert!(
        review(
            &source(&saved, &store, Provider::MailboxOrg),
            &[],
            &[],
            &snapshot(),
            &stale_data,
            stale_as_of,
            &stale.version,
            &change,
        )
        .is_err(),
        "Expired service evidence cannot authorize a service dismissal"
    );
    saved.winddown.known.insert(
        row(&report, sender).id.clone(),
        row(&report, sender).clone(),
    );
    saved
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .clear();
    let bundled = crate::sender_data::bundled()
        .unwrap()
        .winddown(synthetic::AS_OF)
        .unwrap();
    let historical = derive(
        &source(&saved, &store, Provider::MailboxOrg),
        &[],
        &snapshot(),
        &bundled,
        synthetic::AS_OF,
    )
    .unwrap();
    assert!(row(&historical, sender).historical);
    assert_eq!(row(&historical, sender).group, Group::Unresolved);
    assert!(
        row(&historical, sender).settings_link.is_none(),
        "Removed catalog links cannot survive through cached history"
    );
    assert!(historical.terminal().contains("Sender catalog"));
    println!(
        "MAILROOM_CATALOG_DISPLAY_FIXTURE {}",
        serde_json::json!({ "reviewed": report, "unreviewed": uncertain, "stale": stale, "historical": historical, "decision": decision })
    );
}
fn source<'a>(
    state: &'a AccountState,
    store: &'a EncryptedStore,
    provider: Provider,
) -> Source<'a> {
    Source {
        state,
        keys: store.keys(),
        provider,
    }
}
fn connect(state: &mut AccountState, other: &AccountState) {
    state.winddown.settings = Settings {
        winding_down: true,
        comparisons: BTreeMap::from([(
            other.census.account.as_str().into(),
            BTreeSet::from(["new@destination.example.test".into()]),
        )]),
    };
}
fn row<'a>(report: &'a Report, sender: &str) -> &'a Row {
    report
        .rows
        .iter()
        .find(|r| r.sender.as_deref() == Some(sender))
        .unwrap()
}
#[test]
fn calendar_statuses_private_join_and_exact_observations() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    let (b, other) = fixture(root.path(), "new", Provider::MailboxOrg);
    connect(&mut state, &other);
    state.census.winddown_evidence.spam_count = Some(0);
    let report = derive(
        &source(&state, &a, Provider::Gmail),
        &[source(&other, &b, Provider::MailboxOrg)],
        &snapshot(),
        &SenderData::default(),
        synthetic::AS_OF,
    )
    .unwrap();
    assert_eq!(date(Some(report.boundary)), "2025-09-25T00:00:00+00:00");
    assert_eq!(
        row(&report, "waiting@people.example").status,
        Status::Waiting
    );
    let observations = &row(&report, "waiting@people.example").observations;
    assert!(
        observations
            .iter()
            .any(|o| o.address == "own.er@gmail.com" && o.field == "to")
    );
    assert!(
        observations
            .iter()
            .any(|o| o.address == "owner@googlemail.com" && o.field == "to")
    );
    assert!(
        observations
            .iter()
            .any(|o| o.address == "own.er@gmail.com" && o.field == "x-original-to")
    );
    assert_eq!(row(&report, "quiet@people.example").status, Status::Quiet);
    assert_eq!(row(&report, "boundary@people.example").recent, 1);
    assert_eq!(
        row(&report, "boundary@people.example").status,
        Status::Waiting
    );
    assert_eq!(
        row(&report, "future@people.example").status,
        Status::Unresolved
    );
    assert_eq!(row(&report, "future@people.example").future_dates, 1);
    assert_eq!(row(&report, "invalid@people.example").invalid_dates, 1);
    for sender in [
        "forwarded@people.example",
        "ambiguous@people.example",
        "missing@people.example",
    ] {
        let r = row(&report, sender);
        assert_eq!(r.status, Status::Waiting);
        assert!(r.migration.is_empty());
    }
    assert_ne!(report.verdict, "evidence_supports_owner_retirement_review");
    assert_ne!(
        a.keys().placeholder("sender", "alex@people.example"),
        b.keys().placeholder("sender", "alex@people.example")
    );
}
#[test]
fn trusted_direct_fact_moves_only_distinct_messages_and_selected_exact_address() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    let (b, mut other) = fixture(root.path(), "new", Provider::MailboxOrg);
    connect(&mut state, &other);
    // The synthetic helper gives this sender correspondence in the source.
    let mut sent = state.census.folders["INBOX"].messages[&1101].clone();
    sent.uid = 2000;
    sent.headers.to[0].address = "alex@people.example".into();
    sent.gmail_message_id = Some("source-sent-alex".into());
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .live_uids
        .insert(sent.uid);
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .insert(sent.uid, sent);
    let mut received = other.census.folders["INBOX"].messages[&101].clone();
    received.uid = 2001;
    received.gmail_message_id = Some("source-alex".into());
    received.headers.message_id_hash = Some("source-distinct".into());
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .live_uids
        .insert(received.uid);
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .insert(received.uid, received);
    let run = |s: &AccountState, o: &AccountState| {
        derive(
            &source(s, &a, Provider::Gmail),
            &[source(o, &b, Provider::MailboxOrg)],
            &snapshot(),
            &SenderData::default(),
            synthetic::AS_OF,
        )
        .unwrap()
    };
    let r = run(&state, &other);
    assert_eq!(row(&r, "alex@people.example").status, Status::Moved);
    let census = &mut *other.census;
    let m = census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&101)
        .unwrap();
    m.headers.message_id_hash = Some("source-distinct".into());
    census.winddown_evidence.deliveries.insert(
        message_identity(m),
        Delivery::Direct {
            address: "new@destination.example.test".into(),
        },
    );
    assert_eq!(
        row(&run(&state, &other), "alex@people.example").status,
        Status::Waiting
    );
    other
        .census
        .confirmed_own_addresses
        .insert("n.ew@destination.example.test".into());
    state
        .winddown
        .settings
        .comparisons
        .values_mut()
        .next()
        .unwrap()
        .clear();
    state
        .winddown
        .settings
        .comparisons
        .values_mut()
        .next()
        .unwrap()
        .insert("n.ew@destination.example.test".into());
    assert_eq!(
        row(&run(&state, &other), "alex@people.example").status,
        Status::Waiting
    );
}
#[test]
fn receipt_binds_sources_decisions_ledger_purpose_and_exact_output() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    let (b, other) = fixture(root.path(), "new", Provider::MailboxOrg);
    connect(&mut state, &other);
    let ledger = snapshot();
    let data = SenderData::default();
    let current = derive(
        &source(&state, &a, Provider::Gmail),
        &[source(&other, &b, Provider::MailboxOrg)],
        &ledger,
        &data,
        synthetic::AS_OF,
    )
    .unwrap();
    let id = row(&current, "waiting@people.example").id.clone();
    let change = Change::Dismiss {
        rows: BTreeSet::from([id.clone()]),
        dismissed: true,
    };
    let make = |state: &AccountState, other: &AccountState, ledger: &Snapshot, version: &str| {
        review(
            &source(state, &a, Provider::Gmail),
            &[source(other, &b, Provider::MailboxOrg)],
            &[source(other, &b, Provider::MailboxOrg)],
            ledger,
            &data,
            synthetic::AS_OF,
            version,
            &change,
        )
    };
    let preview = make(&state, &other, &ledger, &current.version).unwrap();
    assert_eq!(preview.receipt.len(), 64);
    assert_eq!(
        row(&preview.resulting_report, "waiting@people.example").status,
        Status::Dismissed
    );
    let local = current.export_preview(a.keys(), Disclosure::LocalFile);
    let summary = current.export_preview(a.keys(), Disclosure::AgentSummary);
    assert_eq!(local.text, current.terminal());
    assert_ne!(local.receipt, summary.receipt);
    assert_ne!(local.receipt, preview.receipt);
    for private in [
        "people.example",
        "owner@example.test",
        "destination.example",
        "gmail.com",
        "synthetic-ledger-version",
    ] {
        assert!(!summary.text.contains(private));
    }
    let mut changed = other.clone();
    changed.census.winddown_evidence.spam_count = Some(123);
    assert!(make(&state, &changed, &ledger, &current.version).is_err());
    let mut changed_ledger = snapshot();
    changed_ledger.content_version = "changed-external-file".into();
    assert!(make(&state, &other, &changed_ledger, &current.version).is_err());
    apply(&mut state, &preview);
    a.save_state(&mut state).unwrap();
    assert!(make(&state, &other, &ledger, &current.version).is_err());
    assert_eq!(state.winddown.history.len(), 1);
    let mut census = a.load().unwrap();
    census.account_login = Some("refreshed@example.test".into());
    a.save(&census).unwrap();
    assert!(a.load_state().unwrap().winddown.dismissed.contains(&id));
    let encrypted = std::fs::read(a.path()).unwrap();
    assert!(!encrypted.windows(id.len()).any(|v| v == id.as_bytes()));
}
#[test]
fn history_coverage_and_missing_spam_remain_unknown_not_zero() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    state.census.winddown_evidence.spam_count = None;
    let run = |state: &AccountState| {
        derive(
            &source(state, &a, Provider::Gmail),
            &[],
            &snapshot(),
            &SenderData::default(),
            synthetic::AS_OF,
        )
        .unwrap()
    };
    let initial = run(&state);
    assert_eq!(
        row(&initial, "quiet@people.example").status,
        Status::Waiting
    );
    assert!(initial.blockers.contains("spam_count_unknown"));
    assert!(initial.blockers.contains("comparison_accounts_absent"));
    for r in initial.rows {
        state.winddown.known.insert(r.id.clone(), r);
    }
    let quiet = state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .remove(&102)
        .unwrap();
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .live_uids
        .remove(&quiet.uid);
    let aged = run(&state);
    assert!(row(&aged, "quiet@people.example").historical);
    assert_eq!(
        row(&aged, "quiet@people.example").status,
        Status::Unresolved
    );
    state.census.winddown_evidence.spam_count = Some(0);
    state.census.winddown_evidence.complete_since = None;
    assert!(run(&state).blockers.contains("historical_coverage_unknown"));
}

#[test]
fn adequate_empty_waiting_set_can_only_recommend_owner_review_and_uncertainty_still_blocks() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    let (b, mut other) = fixture(root.path(), "new", Provider::MailboxOrg);
    connect(&mut state, &other);
    for s in [&mut state, &mut other] {
        let folder = s.census.folders.get_mut("INBOX").unwrap();
        folder.messages.clear();
        folder.live_uids.clear();
        s.census.winddown_evidence.spam_count = Some(0);
    }
    let run = |s: &AccountState, o: &AccountState| {
        derive(
            &source(s, &a, Provider::Gmail),
            &[source(o, &b, Provider::MailboxOrg)],
            &snapshot(),
            &SenderData::default(),
            synthetic::AS_OF,
        )
        .unwrap()
    };
    let ready = run(&state, &other);
    assert!(ready.blockers.is_empty());
    assert_eq!(ready.verdict, "evidence_supports_owner_retirement_review");
    assert!(ready.effects.contains("No retirement"));
    other.census.acquisition_state = ScanState::Cancelled;
    assert_eq!(
        run(&state, &other).verdict,
        "insufficient_evidence_or_waiting"
    );
    assert!(
        run(&state, &other)
            .blockers
            .contains("incomplete_acquisition")
    );
    other.census.winddown_evidence.complete_through = None;
    assert!(
        run(&state, &other)
            .blockers
            .contains("source_stale_or_coverage_unknown")
    );
    state.winddown.settings.comparisons.insert(
        "missing-account".into(),
        BTreeSet::from(["missing@example.test".into()]),
    );
    assert!(
        run(&state, &other)
            .blockers
            .contains("selected_comparison_unavailable")
    );
    other.census.confirmed_own_addresses.clear();
    assert!(
        run(&state, &other)
            .blockers
            .contains("comparison_ownership_changed")
    );
}
#[test]
fn canonical_copies_keep_counts_but_conflicting_delivery_facts_never_establish_migration() {
    let root = tempfile::tempdir().unwrap();
    let (a, mut state) = fixture(root.path(), "old", Provider::Gmail);
    let original = state.census.folders["INBOX"].clone();
    state
        .census
        .folders
        .insert("Gmail duplicate label".into(), original);
    let report = derive(
        &source(&state, &a, Provider::Gmail),
        &[],
        &snapshot(),
        &SenderData::default(),
        synthetic::AS_OF,
    )
    .unwrap();
    assert_eq!(row(&report, "waiting@people.example").messages, 1);
    let (b, mut other) = fixture(root.path(), "new", Provider::MailboxOrg);
    let direct = other.census.folders["INBOX"].messages[&101].clone();
    assert!(matches!(
        delivery_fact(&other.census, &direct, true),
        Some(Delivery::Direct { .. })
    ));
    let mut duplicate = other.census.folders["INBOX"].clone();
    duplicate.messages.get_mut(&101).unwrap().headers.to[0].address =
        "different@destination.example.test".into();
    other
        .census
        .folders
        .insert("Other Gmail label".into(), duplicate);
    assert!(matches!(
        delivery_fact(&other.census, &direct, true),
        Some(Delivery::Ambiguous)
    ));
    let source = source(&other, &b, Provider::Gmail);
    assert_eq!(
        source
            .state
            .census
            .canonical_messages(true)
            .values()
            .filter(|(m, _)| m.gmail_message_id == direct.gmail_message_id)
            .count(),
        1
    );
}
