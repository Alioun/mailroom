use super::*;
use crate::{census::EncryptedStore, rules::proof::Baseline};
use crate::{
    census::LocalSecret,
    engine::{self, PreviewInput},
    rules::workspace::{Workspace, WorkspaceInput},
};
fn fixture() -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let input: PreviewInput =
        serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
    let census = engine::synthetic_census(&input).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let keys = CensusKeys::derive(&LocalSecret::random(), &census.account);
    let store = EncryptedStore::new(dir.path().join("journal"), keys).unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: input.as_of,
            baseline: Baseline::Current,
        },
    });
    initialize_synthetic(&mut state);
    store.save_state(&mut state).unwrap();
    (dir, store, state)
}
#[test]
fn report_keeps_predicted_copy_unknown_handoff_roles_and_mime_separate() {
    let (_dir, store, mut state) = fixture();
    let report = report(&state, Provider::MailboxOrg, true, store.keys()).unwrap();
    assert!(
        report
            .rows
            .iter()
            .any(|r| !r.predicted_targets.is_empty() && r.handoff == "real_handoff_unknown")
    );
    assert!(
        report
            .rows
            .iter()
            .any(|r| r.role == "unresolved" && !r.send_available)
    );
    assert!(
        report
            .rows
            .iter()
            .all(|r| r.accounting_acceptance == "unknown")
    );
    let real = super::report(&state, Provider::MailboxOrg, false, store.keys()).unwrap();
    assert_eq!(real.eligible_business, 0);
    assert!(
        real.rows
            .iter()
            .all(|r| r.evidence == ContentEvidence::Unknown && !r.send_available)
    );
    let selection = "INBOX/1/1";
    let prior = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        SendMode::SyntheticTransport,
        SYNTHETIC_SENDER,
    )
    .unwrap();
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .message_id_hash = Some("f".repeat(64));
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            selection,
            "accounting",
            true,
            SendMode::SyntheticTransport,
            SYNTHETIC_SENDER,
        )
        .is_err()
    );
    let changed = super::report(&state, Provider::MailboxOrg, true, store.keys()).unwrap();
    assert_ne!(changed.version, prior.plan.report_version);
    assert_eq!(
        changed
            .rows
            .iter()
            .find(|r| r.id == selection)
            .unwrap()
            .evidence,
        ContentEvidence::Unknown
    );
}
#[test]
fn exact_send_hmac_rejects_state_content_targets_sender_and_other_account() {
    let (_dir, store, mut state) = fixture();
    let selection = "INBOX/1/1";
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        SendMode::SyntheticTransport,
        SYNTHETIC_SENDER,
    )
    .unwrap();
    let receipt = p.receipt.clone();
    let action = p.plan.confirmation_action.clone();
    state
        .invoices
        .sources
        .get_mut(selection)
        .unwrap()
        .original
        .push_str("edited");
    let fresh = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        SendMode::SyntheticTransport,
        SYNTHETIC_SENDER,
    )
    .unwrap();
    assert!(
        record_intent(
            &mut state,
            fresh,
            &crate::confirmation::Confirmed::for_test(&receipt),
            &action
        )
        .is_err()
    );
    let other_keys = CensusKeys::derive(&LocalSecret::random(), &state.census.account);
    let other = preview(
        &state,
        Provider::MailboxOrg,
        &other_keys,
        selection,
        "accounting",
        true,
        SendMode::SyntheticTransport,
        SYNTHETIC_SENDER,
    )
    .unwrap();
    assert!(
        record_intent(
            &mut state,
            other,
            &crate::confirmation::Confirmed::for_test(&receipt),
            &action
        )
        .is_err()
    );
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/6",
            "accounting",
            true,
            SendMode::SyntheticTransport,
            SYNTHETIC_SENDER,
        )
        .is_err()
    );
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            selection,
            "accounting",
            false,
            SendMode::SyntheticTransport,
            SYNTHETIC_SENDER,
        )
        .is_err()
    );
}
#[test]
fn durable_effect_reconciles_exact_bytes_and_missing_ack_never_retries() {
    let (dir, store, mut state) = fixture();
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        "INBOX/1/1",
        "accounting",
        true,
        SendMode::SyntheticTransport,
        SYNTHETIC_SENDER,
    )
    .unwrap();
    let receipt = p.receipt.clone();
    let action = p.plan.confirmation_action.clone();
    let exact = p.plan.clone();
    let id = record_intent(
        &mut state,
        p,
        &crate::confirmation::Confirmed::for_test(&receipt),
        &action,
    )
    .unwrap();
    store.save_state(&mut state).unwrap();
    let mut state = store.load_state().unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Uncertain);
    let mut transport = SyntheticTransport::open(dir.path(), &store, &state).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Uncertain);
    assert!(transport.received().is_empty());
    transport.submit(&id, &exact).unwrap();
    drop(transport);
    let transport = SyntheticTransport::open(dir.path(), &store, &state).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Completed);
    assert_eq!(transport.received().len(), 1);
    assert_eq!(transport.received()[&id], exact);
    store.save_state(&mut state).unwrap();
    let bytes = std::fs::read(store.path()).unwrap();
    assert!(
        !bytes
            .windows(b"SYNTHETIC-INVOICE-PRIVATE-BYTES".len())
            .any(|w| w == b"SYNTHETIC-INVOICE-PRIVATE-BYTES")
    );
}

#[test]
fn keyword_forwards_follow_acquired_pdf_xml_evidence_and_stay_omitted_without_mime() {
    use crate::rules::{Engine, EngineContext, EvidenceSource, proof};
    let (_dir, _store, state) = fixture();
    let mut documents = state.workspace.as_ref().unwrap().input.documents.clone();
    documents.override_yaml = documents
        .override_yaml
        .replace("home.example.test: personal", "home.example.test: business");
    let documents = &documents;
    let mut context =
        EngineContext::local(state.census.account.as_str().into(), Engine::MailboxSieve);
    context.recipient_evidence = EvidenceSource::SyntheticDeliveredTo;
    context.capabilities = [
        "fileinto",
        "copy",
        "duplicate",
        "relational",
        "comparator-i;ascii-numeric",
        "imap4flags",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let without = engine::artifact(documents, &context).unwrap();
    assert!(
        without
            .omissions()
            .iter()
            .any(|o| o.rule == "forward.keywords" && o.reason.contains("mime extension"))
    );
    assert!(!without.executable().contains("mime"));
    context.capabilities.insert("mime".into());
    let with = engine::artifact(documents, &context).unwrap();
    assert!(
        !with
            .omissions()
            .iter()
            .any(|o| o.rule == "forward.keywords")
    );
    assert!(with.executable().contains("\"mime\""));
    assert!(with.executable().contains(
        "header :mime :anychild :contenttype :is :comparator \"i;ascii-casemap\" \"content-type\" \"application/pdf\""
    ));
    let prove = |evidence: &proof::MimeEvidence| {
        proof::prove_with_evidence(
            with.clone(),
            &state.census,
            state.workspace.as_ref().unwrap().input.as_of,
            &[],
            Baseline::Current,
            None,
            evidence,
        )
        .unwrap()
    };
    let unknown = prove(&proof::MimeEvidence::new());
    let keyword = |p: &proof::Proof, id: &str| {
        p.decisions
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .copy_eligibility
            .iter()
            .find(|e| e.rule == "forward.keywords")
            .map(|e| e.status)
    };
    let candidates: Vec<_> = unknown
        .decisions
        .iter()
        .filter(|d| keyword(&unknown, &d.id) == Some("unknown") && !d.review)
        .map(|d| d.id.clone())
        .collect();
    assert!(
        !candidates.is_empty(),
        "keyword business mail with unknown MIME keeps its copy open"
    );
    let all_ids = |types: &[&str]| -> proof::MimeEvidence {
        unknown
            .decisions
            .iter()
            .map(|d| (d.id.clone(), types.iter().map(|t| t.to_string()).collect()))
            .collect()
    };
    let pdf = prove(&all_ids(&["text/plain", "application/pdf"]));
    let none = prove(&all_ids(&["text/plain", "image/png"]));
    let copied = |p: &proof::Proof, id: &str| {
        p.decisions
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .copies
            .iter()
            .any(|c| c.rule == "forward.keywords")
    };
    for id in &candidates {
        assert_eq!(keyword(&pdf, id) == Some("eligible"), copied(&pdf, id));
        assert_eq!(keyword(&none, id), Some("ineligible"));
        assert!(!copied(&none, id) && !copied(&unknown, id));
    }
    assert!(
        candidates.iter().any(|id| copied(&pdf, id)),
        "a PDF part makes the keyword copy predictable"
    );
    let destinations = |p: &proof::Proof| {
        p.decisions
            .iter()
            .map(|d| (d.id.clone(), d.destination.clone(), d.review))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        destinations(&unknown),
        destinations(&pdf),
        "unknown MIME structure never changes filing or review"
    );
    assert_eq!(destinations(&unknown), destinations(&none));
}

mod adapters {
    use super::*;
    use crate::send::smtp::{
        Login, Outcome, Stage, Timeouts,
        tests::{DUMMY, Dummy, Fake, Mode},
    };
    use std::{sync::atomic::AtomicUsize, time::Duration};

    const SENDER: &str = "books@work.example.test";
    /// Replaces the fixture source of INBOX/1/1 with an acquired original in canonical CRLF form.
    fn acquire(state: &mut AccountState, store: &EncryptedStore) {
        let raw = b"From: billing@shop.example.org\r\nTo: invoices@work.example.test\r\nSubject: Studio invoice 104\r\nMessage-ID: <acquired-1@shop.example.org>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n.See attachment.\r\n--b\r\nContent-Type: application/pdf; name=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b--\r\n".to_vec();
        let original = crate::original::Original::parse(raw).unwrap();
        state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .get_mut(&1)
            .unwrap()
            .headers
            .message_id_hash = original.headers.message_id_hash.clone();
        record_original(state, "INBOX", 1, 1, original).unwrap();
        store.save_state(state).unwrap();
    }
    fn confirmed(state: &mut AccountState, store: &EncryptedStore, mode: SendMode) -> String {
        acquire(state, store);
        let p = preview(
            state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/1",
            "accounting",
            true,
            mode,
            SENDER,
        )
        .unwrap();
        let id = consent(state, p);
        store.save_state(state).unwrap();
        id
    }
    /// Confirms `p` through the synthetic boundary and records its intent.
    fn consent(state: &mut AccountState, p: Preview) -> String {
        let action = p.plan.confirmation_action.clone();
        let mut boundary = crate::confirmation::Boundary::synthetic();
        let plan = confirmation_plan(&p);
        let (handle, _) = boundary.offer(plan.clone()).unwrap();
        let consent = boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .unwrap();
        record_intent(state, p, &consent, &action).unwrap()
    }
    fn timeouts() -> Timeouts {
        Timeouts {
            command: Duration::from_secs(5),
            final_reply: Duration::from_millis(500),
        }
    }
    fn status(store: &EncryptedStore, id: &str) -> (Status, Option<Outcome>) {
        let s = store.load_state().unwrap();
        let send = s.invoices.sends.iter().find(|s| s.id == id).unwrap();
        (send.status.clone(), send.smtp.clone())
    }

    #[test]
    fn smtp_send_on_records_intent_before_data_and_its_reply_after() {
        for (mode, implicit, expected) in [
            (Mode::Accept, true, Status::Completed),
            (Mode::Reject, false, Status::NotSent),
            (Mode::DropAfterData, true, Status::Uncertain),
            (Mode::Slow, false, Status::Uncertain),
            (Mode::NoStartTls, false, Status::NotSent),
        ] {
            let (_dir, store, mut state) = fixture();
            let id = confirmed(&mut state, &store, SendMode::Smtp);
            let original = state.invoices.sends[0].plan.source.original.clone();
            assert_eq!(
                status(&store, &id).0,
                Status::Uncertain,
                "{mode:?}: a saved intent reloads as uncertain"
            );
            let fake = Fake::start(mode, implicit);
            let backend = Dummy(AtomicUsize::new(0));
            let login = Login {
                user: SENDER,
                backend: &backend,
                reference: "account-password-ref",
            };
            submit_smtp(&mut state, &id, &fake.server(implicit), &login, timeouts()).unwrap();
            store.save_state(&mut state).unwrap();
            assert!(
                submit_smtp(&mut state, &id, &fake.server(implicit), &login, timeouts()).is_err(),
                "never resubmitted"
            );
            let seen = fake.finish();
            let (saved, outcome) = status(&store, &id);
            assert_eq!(saved, expected, "{mode:?}");
            match mode {
                Mode::Accept => assert_eq!(
                    outcome,
                    Some(Outcome::Accepted {
                        code: 250,
                        enhanced: Some("2.0.0".into())
                    })
                ),
                Mode::NoStartTls => assert_eq!(
                    outcome,
                    Some(Outcome::NotSent {
                        stage: Stage::StartTls,
                        code: None
                    })
                ),
                _ => {}
            }
            if matches!(
                mode,
                Mode::Accept | Mode::Reject | Mode::DropAfterData | Mode::Slow
            ) {
                assert_eq!(
                    seen.message.as_deref(),
                    Some(original.as_bytes()),
                    "{mode:?}: exact previewed original"
                );
                assert!(seen.commands.contains(&format!("MAIL FROM:<{SENDER}>")));
                assert!(
                    seen.commands
                        .contains(&"RCPT TO:<archive@example.net>".to_owned())
                );
            }
            let saved = serde_json::to_string(&store.load_state().unwrap()).unwrap();
            assert!(
                !saved.contains(DUMMY),
                "{mode:?}: the password never reaches saved state"
            );
            let report = report(
                &store.load_state().unwrap(),
                Provider::MailboxOrg,
                true,
                store.keys(),
            )
            .unwrap();
            let row = report.rows.iter().find(|r| r.id == "INBOX/1/1");
            match expected {
                Status::Completed => {
                    assert!(row.is_none_or(|r| r.handoff == "smtp_accepted" && !r.send_available))
                }
                Status::Uncertain => assert_eq!(row.unwrap().handoff, "uncertain_submission"),
                _ => assert!(
                    row.unwrap().send_available,
                    "a send that certainly failed can be previewed again"
                ),
            }
        }
    }

    #[test]
    fn a_crash_at_any_boundary_leaves_the_send_uncertain_and_blocks_a_second_send() {
        // Before submission: the saved intent is all a restarted process finds.
        let (_dir, store, mut state) = fixture();
        let id = confirmed(&mut state, &store, SendMode::Smtp);
        let mut restarted = store.load_state().unwrap();
        assert_eq!(restarted.invoices.sends[0].status, Status::Uncertain);
        let fake = Fake::start(Mode::Accept, true);
        let backend = Dummy(AtomicUsize::new(0));
        let login = Login {
            user: SENDER,
            backend: &backend,
            reference: "account-password-ref",
        };
        assert!(submit_smtp(&mut restarted, &id, &fake.server(true), &login, timeouts()).is_err());
        let p = preview(
            &restarted,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/2",
            "accounting",
            true,
            SendMode::Smtp,
            SENDER,
        );
        assert!(
            p.is_err() || push_intent(&mut restarted, p.unwrap()).is_err(),
            "an uncertain send blocks the next one"
        );
        // After the server accepted, before the outcome was saved.
        let (_dir, store, mut state) = fixture();
        let id = confirmed(&mut state, &store, SendMode::Smtp);
        submit_smtp(&mut state, &id, &fake.server(true), &login, timeouts()).unwrap();
        assert_eq!(
            state.invoices.sends[0].status,
            Status::Completed,
            "{:?}",
            state.invoices.sends[0].smtp
        );
        drop(state);
        assert_eq!(
            status(&store, &id).0,
            Status::Uncertain,
            "an unsaved acceptance is not assumed"
        );
        assert_eq!(fake.finish().message.iter().count(), 1);
    }

    #[test]
    fn a_mail_handoff_is_refused_because_mail_cannot_bind_the_original_and_sender() {
        let (_dir, store, mut state) = fixture();
        acquire(&mut state, &store);
        let refused = preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/1",
            "accounting",
            true,
            SendMode::MailAppHandoff,
            SENDER,
        );
        assert!(
            refused.is_err_and(|e| e.contains("cannot be bound")),
            "no confirmation is offered for a handoff Mail would resolve by Message-ID alone"
        );
    }

    #[test]
    fn the_owner_resolves_only_uncertain_sends_and_sees_sent_folder_copies() {
        use crate::send::Resolution;
        let (_dir, store, mut state) = fixture();
        let id = confirmed(&mut state, &store, SendMode::Smtp);
        assert!(
            resolve(&mut state, &id, Resolution::Sent).is_err(),
            "a send still in progress is not resolved"
        );
        let mut state = store.load_state().unwrap();
        assert_eq!(state.invoices.sends[0].status, Status::Uncertain);
        assert_eq!(sent_evidence(&state)[&id], Vec::<String>::new());
        let mut sent = state.census.folders["INBOX"].clone();
        sent.messages.retain(|uid, _| *uid == 1);
        sent.messages.get_mut(&1).unwrap().headers.direction = crate::census::Direction::Sent;
        state.census.folders.insert("Gesendet".into(), sent);
        assert_eq!(
            sent_evidence(&state)[&id],
            vec!["Gesendet/1/1".to_owned()],
            "a Sent copy with the original's Message-ID is offered as evidence"
        );
        resolve(&mut state, &id, Resolution::Sent).unwrap();
        assert_eq!(
            (
                &state.invoices.sends[0].status,
                state.invoices.sends[0].resolved
            ),
            (&Status::Completed, Some(Resolution::Sent))
        );
        assert!(
            resolve(&mut state, &id, Resolution::NotSent).is_err(),
            "a resolved send stays resolved"
        );
        assert!(sent_evidence(&state).is_empty());
        let row = report(&state, Provider::MailboxOrg, true, store.keys())
            .unwrap()
            .rows
            .into_iter()
            .find(|r| r.id == "INBOX/1/1");
        assert!(
            row.is_none(),
            "an owner-recorded send leaves the missed-invoice report"
        );

        let (_dir, store, mut state) = fixture();
        let id = confirmed(&mut state, &store, SendMode::Smtp);
        drop(state);
        let mut state = store.load_state().unwrap();
        resolve(&mut state, &id, Resolution::NotSent).unwrap();
        store.save_state(&mut state).unwrap();
        let state = store.load_state().unwrap();
        assert_eq!(
            (
                &state.invoices.sends[0].status,
                state.invoices.sends[0].resolved
            ),
            (&Status::NotSent, Some(Resolution::NotSent))
        );
        let row = report(&state, Provider::MailboxOrg, true, store.keys())
            .unwrap()
            .rows
            .into_iter()
            .find(|r| r.id == "INBOX/1/1")
            .unwrap();
        assert!(
            row.send_available,
            "a fresh preview and confirmation may send it again"
        );
        let again = preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/1",
            "accounting",
            true,
            SendMode::Smtp,
            SENDER,
        )
        .unwrap();
        assert_ne!(
            again.receipt, id,
            "the earlier confirmation cannot be reused"
        );

        let (_dir, store, mut state) = fixture();
        let p = preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/1",
            "accounting",
            true,
            SendMode::SyntheticTransport,
            SYNTHETIC_SENDER,
        )
        .unwrap();
        let id = consent(&mut state, p);
        store.save_state(&mut state).unwrap();
        let mut state = store.load_state().unwrap();
        assert!(
            resolve(&mut state, &id, Resolution::NotSent).is_err(),
            "synthetic sends reconcile against their fixture instead"
        );
    }

    #[test]
    fn wired_modes_name_the_exact_message_bytes_and_need_an_acquired_original() {
        let (_dir, store, mut state) = fixture();
        for mode in [SendMode::Smtp] {
            assert!(
                preview(
                    &state,
                    Provider::MailboxOrg,
                    store.keys(),
                    "INBOX/1/1",
                    "accounting",
                    true,
                    mode,
                    SENDER
                )
                .is_err(),
                "a fixture source is not handed to a real adapter"
            );
        }
        acquire(&mut state, &store);
        let original = state.invoices.sources["INBOX/1/1"].original.clone();
        for (mode, purpose) in [(SendMode::Smtp, "smtp_send")] {
            let p = preview(
                &state,
                Provider::MailboxOrg,
                store.keys(),
                "INBOX/1/1",
                "accounting",
                true,
                mode,
                SENDER,
            )
            .unwrap();
            let action = serde_json::to_value(&confirmation_plan(&p).actions[0]).unwrap();
            assert_eq!(action["purpose"], purpose);
            assert_eq!(action["target"], "archive@example.net");
            assert_eq!(action["sending_identity"], SENDER);
            assert_eq!(action["content_digest"], digest(original.as_bytes()));
        }
    }

    #[test]
    fn acquired_originals_replace_fixtures_only_when_their_message_id_matches() {
        let (_dir, store, mut state) = fixture();
        let raw = b"From: billing@shop.example.org\r\nTo: invoices@work.example.test\r\nSubject: Invoice\r\nMessage-ID: <acquired-1@shop.example.org>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attachment.\r\n--b\r\nContent-Type: application/pdf; name=a.pdf\r\n\r\n%PDF\r\n--b--\r\n".to_vec();
        let original = crate::original::Original::parse(raw.clone()).unwrap();
        assert!(
            record_original(
                &mut state,
                "INBOX",
                1,
                1,
                crate::original::Original::parse(raw.clone()).unwrap()
            )
            .is_err(),
            "a different message is refused"
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
            .message_id_hash = original.headers.message_id_hash.clone();
        assert!(
            record_original(
                &mut state,
                "INBOX",
                2,
                1,
                crate::original::Original::parse(raw.clone()).unwrap()
            )
            .is_err(),
            "a stale UIDVALIDITY is refused"
        );
        let id = record_original(&mut state, "INBOX", 1, 1, original).unwrap();
        let source = &state.invoices.sources[&id];
        assert!(source.acquired && source.original.as_bytes() == raw.as_slice());
        assert!(
            matches!(&source.evidence, ContentEvidence::Known { attachments, .. } if attachments[0].media_type == "application/pdf")
        );
        store.save_state(&mut state).unwrap();
        let real = super::super::report(
            &store.load_state().unwrap(),
            Provider::MailboxOrg,
            false,
            store.keys(),
        )
        .unwrap();
        assert!(
            real.rows
                .iter()
                .any(|r| r.id == id && matches!(r.evidence, ContentEvidence::Known { .. })),
            "a real account uses acquired facts"
        );
    }
}
