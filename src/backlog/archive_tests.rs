use super::*;
fn mailbox_address(address: &str) -> crate::census::Mailbox {
    crate::census::Mailbox {
        address: address.into(),
        display_name: None,
    }
}

fn prepared(
    gmail: bool,
) -> (
    tempfile::TempDir,
    EncryptedStore,
    AccountState,
    SyntheticMailbox,
) {
    let (root, store, mut state, mut mailbox) = fixture(1, gmail);
    state.workspace.as_mut().unwrap().input.as_of = 1792800000;
    let census = &mut mailbox.state.census;
    census.folder_roles.remove("Archive fixture");
    census.folder_roles.insert(
        if gmail { "[Gmail]/All Mail" } else { "Archiv" }.into(),
        [if gmail { "all" } else { "archive" }.into()].into(),
    );
    let m = census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap();
    m.headers = crate::census::parse_headers(b"From: person@unmatched.example.test\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: A personal note\r\nMessage-ID: <old-unmatched@example.test>\r\nDate: Wed, 01 May 2024 00:00:00 +0000\r\n\r\n", crate::census::Direction::Received).unwrap();
    m.internal_date = "01-May-2024 00:00:00 +0000".into();
    state.census = census.clone();
    mailbox.persist().unwrap();
    store.save_state(&mut state).unwrap();
    (root, store, state, mailbox)
}
fn archive_options() -> Options {
    Options {
        scope: Scope::Inbox,
        intent: Intent::ArchiveUnmatched,
        ..Default::default()
    }
}
fn archive(
    state: &AccountState,
    store: &EncryptedStore,
    mailbox: &SyntheticMailbox,
    gmail: bool,
) -> Preview {
    preview(
        state,
        if gmail {
            Provider::Gmail
        } else {
            Provider::MailboxOrg
        },
        store.keys(),
        archive_options(),
        Some(mailbox),
    )
    .unwrap()
}

#[test]
fn archive_is_a_separate_receipt_and_preserves_historical_sort_options() {
    let (_root, store, mut state, mailbox) = prepared(false);
    let sort = p(&state, &store, &mailbox, false);
    assert!(sort.plan.changes.is_empty());
    let a = archive(&state, &store, &mailbox, false);
    assert_eq!(a.plan.changes.len(), 1);
    assert_eq!(a.plan.changes[0].destination, "Archiv/2024");
    assert_ne!(sort.receipt, a.receipt);
    assert!(start(&mut state, a, &sort.receipt).is_err());
    let legacy = r#"{"scope":"inbox_unfiled","protect_flagged":true,"selected":null}"#;
    assert_eq!(serde_json::to_string(&Options::default()).unwrap(), legacy);
    assert_eq!(
        serde_json::from_str::<Options>(legacy).unwrap(),
        Options::default()
    );
    let mut wrong = archive_options();
    wrong.scope = Scope::EveryFolder;
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            wrong,
            Some(&mailbox)
        )
        .is_err()
    );
}

#[test]
fn protocol_projection_binds_the_explicit_method_without_changing_filing_choices() {
    let (_root, store, state, mailbox) = prepared(false);
    let artifact = archive(&state, &store, &mailbox, false).plan.artifact;
    let proof = crate::rules::proof::prove(
        artifact,
        &mailbox.state.census,
        state.workspace.as_ref().unwrap().input.as_of,
        &[],
        crate::rules::proof::Baseline::Current,
        None,
    )
    .unwrap();
    let moved = protocol::project(
        &state,
        &mailbox.state.census,
        &proof,
        archive_options(),
        MoveMethod::UidMove,
    )
    .unwrap();
    let mut copied = protocol::project(
        &state,
        &mailbox.state.census,
        &proof,
        archive_options(),
        MoveMethod::CopyUidExpunge,
    )
    .unwrap();
    assert_eq!(copied.changes.len(), 1);
    assert_eq!(copied.changes[0].destination, "Archiv/2024");
    assert_eq!(
        copied.changes[0].operation,
        Operation::Imap {
            method: MoveMethod::CopyUidExpunge
        }
    );
    assert_eq!(copied.semantics, protocol::COPY_SEMANTICS);
    assert_ne!(copied.source, moved.source);
    copied.source = moved.source.clone();
    copied.semantics = moved.semantics.clone();
    copied.changes[0].operation = Operation::Imap {
        method: MoveMethod::UidMove,
    };
    assert_eq!(
        serde_json::to_vec(&copied).unwrap(),
        serde_json::to_vec(&moved).unwrap()
    );
}

#[test]
fn archive_uses_strict_calendar_arrival_boundary_and_never_falls_back_to_header_date() {
    let (_root, store, state, mut mailbox) = prepared(false);
    for (date, eligible) in [
        ("23-Oct-2025 23:59:59 +0000", true),
        ("24-Oct-2025 00:00:00 +0000", false),
        ("25-Oct-2025 00:00:00 +0000", false),
        ("24-Oct-2027 00:00:00 +0000", false),
        ("invalid", false),
    ] {
        mailbox
            .state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .get_mut(&1)
            .unwrap()
            .internal_date = date.into();
        assert_eq!(
            !archive(&state, &store, &mailbox, false)
                .plan
                .changes
                .is_empty(),
            eligible,
            "{date}"
        );
    }
}

#[test]
fn archive_excludes_held_matched_protected_deleted_and_ambiguous_archive_topology() {
    let (_root, store, state, mailbox) = prepared(false);
    for case in [
        "unknown_recipient",
        "mixed_recipient",
        "matched",
        "flagged",
        "deleted",
        "ambiguous_archive",
        "missing_archive",
    ] {
        let mut census = mailbox.state.census.clone();
        let m = census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .get_mut(&1)
            .unwrap();
        match case {
            "unknown_recipient" => {
                m.headers.delivered_to = vec![mailbox_address("nobody@unknown.example.test")]
            }
            "mixed_recipient" => {
                m.headers.delivered_to = vec![
                    mailbox_address("owner@home.example.test"),
                    mailbox_address("owner@work.example.test"),
                ]
            }
            "matched" => {
                m.headers.from = vec![mailbox_address("billing@shop.example.org")];
                m.headers.subject = "invoice".into();
            }
            "flagged" => {
                m.flags.insert("\\Flagged".into());
            }
            "deleted" => {
                m.flags.insert("\\Deleted".into());
            }
            "ambiguous_archive" => {
                census
                    .folder_roles
                    .insert("Archive".into(), ["archive".into()].into());
            }
            "missing_archive" => {
                census.folder_roles.clear();
            }
            _ => unreachable!(),
        }
        let proof = rules::proof::prove(
            crate::engine::artifact(
                &state.workspace.as_ref().unwrap().input.documents,
                &p(&state, &store, &mailbox, false)
                    .plan
                    .artifact
                    .context()
                    .clone(),
            )
            .unwrap(),
            &census,
            state.workspace.as_ref().unwrap().input.as_of,
            &[],
            Baseline::Current,
            None,
        )
        .unwrap();
        let plan = protocol::project(
            &state,
            &census,
            &proof,
            archive_options(),
            MoveMethod::UidMove,
        )
        .unwrap();
        assert!(plan.changes.is_empty(), "{case}");
    }
}

#[test]
fn archive_execution_reopens_and_conditional_undo_preserves_later_flags_and_gmail_labels() {
    for gmail in [false, true] {
        let (_root, store, mut state, mut mailbox) = prepared(gmail);
        let a = archive(&state, &store, &mailbox, gmail);
        assert!(a.execution_available);
        if gmail {
            assert!(
                matches!(&a.plan.changes[0].operation,Operation::Gmail{add,remove} if add.is_empty() && remove == &BTreeSet::from(["\\Inbox".into()]))
            );
        }
        let receipt = a.receipt.clone();
        let id = start(&mut state, a, &receipt).unwrap();
        store.save_state(&mut state).unwrap();
        advance(&store, &mut state, &mut mailbox, &id).unwrap();
        state = store.load_state().unwrap();
        assert_eq!(state.backlog[0].status, Status::Completed);
        let destination = match &state.backlog[0].outcomes[0] {
            Outcome::Applied { observation } => observation.destination.clone(),
            _ => panic!("Archive not observed"),
        };
        let m = mailbox
            .state
            .census
            .folders
            .get_mut(&destination.folder)
            .unwrap()
            .messages
            .get_mut(&destination.uid)
            .unwrap();
        assert!(m.flags.contains("\\Answered"));
        assert!(!m.flags.contains("\\Seen"));
        if gmail {
            assert_eq!(m.gmail_labels, BTreeSet::from(["Unrelated".into()]));
            m.gmail_labels.insert("Added later".into());
        }
        m.flags.insert("\\Seen".into());
        mailbox.persist().unwrap();
        let compensation = undo_preview(&state, &mailbox, store.keys(), &id).unwrap();
        undo(
            &store,
            &mut state,
            &mut mailbox,
            &id,
            Some(&compensation.receipt),
        )
        .unwrap();
        let m = mailbox.state.census.folders["INBOX"]
            .messages
            .values()
            .next()
            .unwrap();
        assert!(m.flags.contains("\\Seen") && m.flags.contains("\\Answered"));
        if gmail {
            assert_eq!(
                m.gmail_labels,
                BTreeSet::from(["\\Inbox".into(), "Unrelated".into(), "Added later".into()])
            );
        }
    }
}

#[test]
fn archive_folder_plan_names_only_the_reviewed_arrival_year_and_existing_root() {
    use crate::census::folder_provisioning::{
        ExactName, FolderPlan, Inventory, Mailbox, Namespace, Namespaces,
    };
    let (_root, store, state, mailbox) = prepared(false);
    let plan = archive(&state, &store, &mailbox, false).plan;
    let leaves = plan.changes.iter().map(|c| c.destination.clone()).collect();
    let mut inventory = Inventory {
        capabilities: ["IMAP4REV1".into(), "NAMESPACE".into()].into(),
        namespaces: Namespaces {
            personal: vec![Namespace {
                prefix: String::new(),
                delimiter: Some("/".into()),
            }],
            other: vec![],
            shared: vec![],
        },
        mailboxes: vec![Mailbox {
            name: ExactName::new("Archiv").unwrap(),
            delimiter: Some("/".into()),
            attributes: BTreeSet::new(),
        }],
    };
    let create = FolderPlan::build(&leaves, &inventory).unwrap();
    assert_eq!(
        create
            .entries
            .iter()
            .map(|e| (e.name.decoded.as_str(), e.existing.is_some()))
            .collect::<Vec<_>>(),
        [("Archiv", true), ("Archiv/2024", false)]
    );
    inventory.mailboxes.push(Mailbox {
        name: ExactName::new("Archiv/2024").unwrap(),
        delimiter: Some("/".into()),
        attributes: BTreeSet::new(),
    });
    assert!(
        FolderPlan::build(&leaves, &inventory)
            .unwrap()
            .entries
            .iter()
            .all(|e| e.existing.is_some())
    );
}
