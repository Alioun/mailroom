use mailroom::{
    census::delivery::{DeliveryEvidence, Source},
    engine::{self, PreviewInput},
    rules::{
        self, Engine, EvidenceSource, config,
        proof::{self, Baseline, Destination, Pin},
    },
};
use serde_json::json;
fn input() -> PreviewInput {
    let mut input: PreviewInput =
        serde_json::from_str(include_str!("../examples/sample.json")).unwrap();
    input.context.recipient_evidence = EvidenceSource::SyntheticEnvelope {
        source: Source {
            account: input.context.account.clone(),
            provider: "mailbox_org".into(),
            identity: "invented-smtp-delivery".into(),
            profile: "synthetic-envelope/v1".into(),
            epoch: "epoch-one".into(),
            authority: "explicit-preview-fixture-only".into(),
        },
    };
    input.context.capabilities.insert("envelope".into());
    input.pins.clear();
    input.messages = vec![
        fixture(
            "direct",
            Some("invoices@work.example.test"),
            "known",
            "Invoice",
            "same-id",
            Some(vec![
                "\\Seen",
                "\\Flagged",
                "\\Answered",
                "unrelated",
                "$ROUTE",
            ]),
        ),
        fixture(
            "forwarded",
            Some("invoices@work.example.test"),
            "known",
            "Invoice",
            "same-id",
            None,
        ),
        fixture(
            "different",
            Some("team@work.example.test"),
            "known",
            "Invoice",
            "different-id",
            None,
        ),
    ];
    edit(&mut input, |o| {
        o.delivery_tags.push(serde_json::from_value(json!({"id":"user.route", "accounts":["demo"], "envelope_to":"invoices@work.example.test", "add_keyword":"$route"})).unwrap());
    });
    let (_, mut o) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml).unwrap();
    o.delivery_tags[0].accounts = [input.context.account.clone()].into();
    input.documents.override_yaml = serde_yaml_ng::to_string(&o).unwrap();
    input
}
fn fixture(
    id: &str,
    recipient: Option<&str>,
    status: &str,
    subject: &str,
    message_id: &str,
    flags: Option<Vec<&str>>,
) -> engine::Fixture {
    let delivery = if let Some(recipient) = recipient {
        json!({"status":status,"recipient":recipient})
    } else {
        json!({"status":status})
    };
    serde_json::from_value(json!({"id":id,"headers":format!("From: billing@shop.example.org\r\nTo: forged@personal.test\r\nDelivered-To: invoices@work.example.test\r\nX-Original-To: invoices@work.example.test\r\nSubject: {subject}\r\nMessage-ID: <{message_id}@example.test>\r\nDate: 24 Oct 2026 12:00:00 +0000\r\n\r\n"),"current_folder":"INBOX","delivery":delivery,"observed_flags":flags})).unwrap()
}
fn edit(input: &mut PreviewInput, apply: impl FnOnce(&mut config::Overrides)) {
    let (_, mut o) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml).unwrap();
    apply(&mut o);
    input.documents.override_yaml = serde_yaml_ng::to_string(&o).unwrap();
}
#[test]
fn emitted_envelope_and_keywords_preserve_one_folder_and_destination_copy_deduplication() {
    let mut i = input();
    edit(&mut i, |o| {
        o.targets.insert(
            "alias".into(),
            config::Target {
                address: "ARCHIVE@example.net".into(),
            },
        );
        o.forwards[1].target = "alias".into();
        let mut tag = o.delivery_tags[0].clone();
        tag.id = "user.route-again".into();
        tag.add_keyword = config::Keyword::try_from("$ROUTE".to_owned()).unwrap();
        o.delivery_tags.push(tag);
    });
    let p = engine::preview(&i).unwrap();
    assert_eq!(p.decisions[0].keyword_additions, ["$route"]);
    assert_eq!(p.decisions[0].copies.len(), 1);
    assert!(p.decisions[1].copies.is_empty());
    assert_eq!(p.decisions[2].copies.len(), 1);
    assert!(p.decisions[2].keyword_additions.is_empty());
    assert_eq!(p.decisions[2].delivery_tags[0].status, "no_match");
    assert_eq!(
        p.decisions[0].destination,
        Destination::Folder {
            name: "Business/Invoices".into()
        }
    );
    assert_eq!(p.decisions[0].observed_flags.as_ref().unwrap().len(), 5);
    assert_eq!(p.decisions[1].observed_flags, None);
    assert!(!p.decisions[0].mark_read);
    assert!(
        p.artifact.executable().find("addflag \"$route\"").unwrap()
            < p.artifact.executable().find("fileinto ").unwrap()
    );
    for forbidden in ["setflag", "removeflag", ":flags", "STORE"] {
        assert!(!p.artifact.executable().contains(forbidden));
    }
    edit(&mut i, |o| {
        o.targets.get_mut("alias").unwrap().address = "second@example.test".into()
    });
    let distinct = engine::preview(&i).unwrap();
    assert_eq!(distinct.decisions[0].copies.len(), 2);
    assert!(distinct.decisions[1].copies.is_empty());
}
#[test]
fn unavailable_ambiguous_unsupported_and_incompatible_evidence_stay_unknown_without_fallthrough() {
    let mut i = input();
    i.messages = vec![
        fixture("missing", None, "unavailable", "Invoice", "one", None),
        fixture(
            "multiple-same-role",
            None,
            "ambiguous",
            "Invoice",
            "two",
            None,
        ),
        fixture("unsupported", None, "unsupported", "Invoice", "three", None),
    ];
    let p = engine::preview(&i).unwrap();
    assert_eq!(p.counts.review, 3);
    assert_eq!(p.counts.copies, 0);
    assert_eq!(p.counts.filed, 0);
    for d in &p.decisions {
        assert!(d.keyword_additions.is_empty());
        assert!(!d.mark_read);
        assert_eq!(d.delivery_tags[0].status, "unknown");
    }
    let hash = p.decisions[0].message_id_hash.clone().unwrap();
    i.pins = vec![Pin {
        account: i.context.account.clone(),
        message_id_hash: hash,
        expected: Destination::Inbox,
    }];
    assert_eq!(engine::preview(&i).unwrap().pins[0].status, "unknown");
    let i = input();
    let mut census = engine::synthetic_census(&i).unwrap();
    let record = census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap();
    if let DeliveryEvidence::Known { source, .. } = &mut record.delivery {
        source.epoch.push('2');
    }
    let p = proof::prove(
        engine::artifact(&i.documents, &i.context).unwrap(),
        &census,
        i.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap();
    assert!(p.decisions[0].review);
    assert_eq!(p.decisions[0].delivery_status, "incompatible");
    assert!(p.decisions[0].copies.is_empty());
    assert!(p.decisions[0].keyword_additions.is_empty());
    for raw in [
        json!({"status":"known","recipient":["one@work.example.test","two@work.example.test"]}),
        json!({"status":"known","recipient":"one@work.example.test","candidates":["one@work.example.test"]}),
    ] {
        assert!(serde_json::from_value::<engine::FixtureDelivery>(raw).is_err());
    }
}
#[test]
fn complete_tag_omissions_scope_and_roles_do_not_promote_headers_or_route_membership() {
    for missing in ["envelope", "imap4flags", "fileinto"] {
        let mut i = input();
        i.context.capabilities.remove(missing);
        let p = engine::preview(&i).unwrap();
        assert!(
            p.artifact
                .omissions()
                .iter()
                .any(|o| o.rule == "user.route")
        );
        assert!(!p.artifact.executable().contains("addflag \"$route\""));
    }
    let mut i = input();
    i.context.engine = Engine::GmailBacklog;
    if let EvidenceSource::SyntheticEnvelope { source } = &mut i.context.recipient_evidence {
        source.provider = "gmail".into();
    }
    let p = engine::preview(&i).unwrap();
    assert_eq!(p.counts.copies, 0);
    assert!(!p.artifact.executable().contains("addflag \"$route\""));
    for scope in ["disabled", "other-account"] {
        let mut i = input();
        edit(&mut i, |o| {
            if scope == "disabled" {
                o.delivery_tags[0].enabled = false;
            } else {
                o.delivery_tags[0].accounts = ["another".into()].into();
            }
        });
        let p = engine::preview(&i).unwrap();
        assert!(
            p.artifact
                .omissions()
                .iter()
                .any(|o| o.rule == "user.route")
        );
    }
    let mut i = input();
    edit(&mut i, |o| {
        o.roles
            .addresses
            .insert("invoices@work.example.test".into(), config::Role::Unknown);
    });
    let p = engine::preview(&i).unwrap();
    assert!(p.decisions[0].review);
    assert_eq!(p.decisions[0].delivery_tags[0].status, "held_tagged");
    assert_eq!(p.decisions[0].keyword_additions, ["$route"]);
    assert!(p.decisions[0].copies.is_empty());
    edit(&mut i, |o| {
        o.roles
            .addresses
            .insert("invoices@work.example.test".into(), config::Role::Personal);
    });
    let p = engine::preview(&i).unwrap();
    assert!(!p.decisions[0].review);
    assert!(p.decisions[0].copies.is_empty());
    assert_eq!(p.decisions[0].keyword_additions, ["$route"]);
}
#[test]
fn keyword_boundary_and_tag_resource_validation() {
    for valid in ["$via-gmail", "$route", "a+b", "a-b", "A.B", "[allowed"] {
        assert!(
            config::Keyword::try_from(valid.to_owned()).is_ok(),
            "{valid}"
        );
    }
    for invalid in [
        "", "a b", "a\t", "a\r\n", "ü", "(x", "x)", "{x", "x]", "a%", "*", "\"", "\\", "\\Seen",
        "Recent", "sEeN", "Answered", "Flagged", "Deleted", "Draft",
    ] {
        assert!(
            config::Keyword::try_from(invalid.to_owned()).is_err(),
            "{invalid}"
        );
    }
    for invalid in ["id", "duplicate", "scope", "recipient", "bound"] {
        let mut i = input();
        edit(&mut i, |o| match invalid {
            "id" => o.delivery_tags[0].id = "system.reserved".into(),
            "duplicate" => o.delivery_tags[0].id = o.forwards[0].id.clone(),
            "scope" => o.delivery_tags[0].accounts.clear(),
            "recipient" => o.delivery_tags[0].envelope_to = "Name <route@example.test>".into(),
            _ => o.delivery_tags = vec![o.delivery_tags[0].clone(); 33],
        });
        assert!(engine::preview(&i).is_err(), "{invalid}");
    }
}
#[test]
fn independent_mark_read_guards_bulk_unmatched_and_plus_addresses() {
    let mut i = input();
    i.messages = vec![
        fixture(
            "guard",
            Some("invoices@work.example.test"),
            "known",
            "Your login code",
            "guard",
            None,
        ),
        fixture(
            "bulk",
            Some("invoices@work.example.test"),
            "known",
            "Invoice",
            "bulk",
            None,
        ),
        fixture(
            "plus",
            Some("invoices+tag@work.example.test"),
            "known",
            "Unmatched",
            "plus",
            None,
        ),
    ];
    i.messages[1].headers = i.messages[1]
        .headers
        .replace("\r\n\r\n", "\r\nList-Id: <letters.example.test>\r\n\r\n");
    let p = engine::preview(&i).unwrap();
    assert_eq!(p.decisions[0].destination, Destination::Inbox);
    assert_eq!(p.decisions[0].keyword_additions, ["$route"]);
    assert!(!p.decisions[0].mark_read);
    assert!(p.decisions[2].keyword_additions.is_empty());
    assert_eq!(p.decisions[2].delivery_tags[0].status, "no_match");
    edit(&mut i, |o| {
        o.additions.insert(0,serde_json::from_value(json!({"id":"user.read","category":"money","stage":"from","when":[{"kind":"from_address","value":"billing@shop.example.org"}],"mark_read":true})).unwrap())
    });
    let p = engine::preview(&i).unwrap();
    assert!(p.decisions[1].mark_read);
    assert_eq!(p.decisions[1].keyword_additions, ["$route"]);
    assert!(!p.decisions[0].mark_read);
    assert!(rules::config::mailbox("invoices+tag@work.example.test"));
}

fn evidence_matrix(held: bool) -> proof::Proof {
    let mut i = input();
    i.messages = vec![
        fixture(
            "match",
            Some("invoices@work.example.test"),
            "known",
            "Invoice",
            "match",
            Some(vec!["unrelated"]),
        ),
        fixture(
            "different",
            Some("team@work.example.test"),
            "known",
            "Invoice",
            "different",
            None,
        ),
        fixture(
            "unavailable",
            None,
            "unavailable",
            "Invoice",
            "unavailable",
            None,
        ),
        fixture("ambiguous", None, "ambiguous", "Invoice", "ambiguous", None),
        fixture(
            "unsupported",
            None,
            "unsupported",
            "Invoice",
            "unsupported",
            None,
        ),
        fixture(
            "incompatible",
            Some("invoices@work.example.test"),
            "known",
            "Invoice",
            "incompatible",
            None,
        ),
    ];
    edit(&mut i, |o| {
        o.additions.insert(0, serde_json::from_value(json!({"id":"user.read","category":"money","stage":"from","when":[{"kind":"from_address","value":"billing@shop.example.org"}],"mark_read":true})).unwrap());
        if held {
            o.roles
                .addresses
                .insert("invoices@work.example.test".into(), config::Role::Unknown);
            o.roles
                .addresses
                .insert("team@work.example.test".into(), config::Role::Unfiltered);
        }
    });
    let mut census = engine::synthetic_census(&i).unwrap();
    let record = census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&6)
        .unwrap();
    let DeliveryEvidence::Known { source, .. } = &mut record.delivery else {
        panic!()
    };
    source.epoch.push('2');
    let p = proof::prove(
        engine::artifact(&i.documents, &i.context).unwrap(),
        &census,
        i.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap();
    let executable = p.artifact.executable();
    assert!(
        executable.find("addflag \"$route\"").unwrap()
            < executable.find("#@ system.role_safety").unwrap()
    );
    p
}
#[test]
fn verified_route_tags_held_mail_without_changing_placement_read_status_or_copies() {
    let statuses = [
        "known",
        "known",
        "unavailable",
        "ambiguous",
        "unsupported",
        "incompatible",
    ];
    let held = evidence_matrix(true);
    let tags = [
        "held_tagged",
        "no_match",
        "unknown",
        "unknown",
        "unknown",
        "unknown",
    ];
    for (n, d) in held.decisions.iter().enumerate() {
        assert_eq!(d.delivery_status, statuses[n], "{n}");
        assert_eq!(d.delivery_tags[0].status, tags[n], "{n}");
        assert!(d.review, "{n}");
        assert_eq!(d.destination, Destination::Inbox, "{n}");
        assert_eq!(d.current_folders, ["INBOX".to_owned()].into(), "{n}");
        assert_eq!(d.matched_rule.as_deref(), Some("system.role_safety"), "{n}");
        assert!(!d.mark_read, "{n}");
        assert!(d.copies.is_empty(), "{n}");
        assert!(
            d.copy_eligibility.iter().all(|c| c.status == "unknown"),
            "{n}"
        );
        let expected: &[&str] = if n == 0 { &["$route"] } else { &[] };
        assert_eq!(d.keyword_additions, expected, "{n}");
    }
    assert_eq!(
        held.decisions[0].observed_flags,
        Some(["unrelated".to_owned()].into())
    );
    assert_eq!(held.counts.copies, 0);
    assert_eq!(held.counts.filed, 0);
    let eligible = evidence_matrix(false);
    let tags = [
        "match", "no_match", "unknown", "unknown", "unknown", "unknown",
    ];
    for (n, d) in eligible.decisions.iter().enumerate() {
        assert_eq!(d.delivery_status, statuses[n], "{n}");
        assert_eq!(d.delivery_tags[0].status, tags[n], "{n}");
        assert_eq!(d.review, n >= 2, "{n}");
        if n >= 2 {
            assert_eq!(d.destination, Destination::Inbox);
            assert!(!d.mark_read && d.copies.is_empty() && d.keyword_additions.is_empty());
        }
    }
    assert_eq!(eligible.decisions[0].keyword_additions, ["$route"]);
    assert!(eligible.decisions[0].mark_read);
    assert_eq!(eligible.decisions[0].copies.len(), 1);
    assert!(eligible.decisions[1].mark_read);
    assert!(eligible.decisions[1].keyword_additions.is_empty());
    assert_ne!(eligible.decisions[0].destination, Destination::Inbox);
    for header_only in [
        EvidenceSource::SyntheticDeliveredTo,
        EvidenceSource::SyntheticOriginalTo,
    ] {
        let mut i = input();
        i.context.recipient_evidence = header_only;
        i.messages = vec![fixture(
            "header",
            None,
            "unavailable",
            "Invoice",
            "header",
            None,
        )];
        edit(&mut i, |o| {
            o.roles
                .addresses
                .insert("invoices@work.example.test".into(), config::Role::Unknown);
        });
        let p = engine::preview(&i).unwrap();
        assert!(!p.artifact.executable().contains("addflag \"$route\""));
        for d in &p.decisions {
            assert_eq!(d.delivery_tags[0].status, "omitted");
            assert!(d.review && !d.mark_read && d.copies.is_empty());
            assert!(d.keyword_additions.is_empty());
        }
    }
}
