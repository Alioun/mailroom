use super::*;

const OX: &str = "require [\"fileinto\",\"imap4flags\"];\r\n## Flag: |UniqueId:0|Rulename: Verein\r\nif header :contains \"list-id\" \"verein.example.test\"\r\n{\r\n    fileinto \"Verein\";\r\n    stop;\r\n}\r\n## Flag: |UniqueId:4|Rulename: Gelesen\r\nif address :is \"from\" \"noreply@example.test\"\r\n{\r\n    addflag \"\\\\Seen\";\r\n}\r\n";
const MAILROOM: &str = "# Local proof artifact. Provider deployment remains locked.\nrequire [\"fileinto\", \"relational\"];\n\n#@ news\nif header :contains :comparator \"i;ascii-casemap\" \"list-id\" \"news\" {\n  fileinto \"News\";\n  stop;\n}\n\n#@ system.unmatched\nif true {\n  keep;\n  stop;\n}\n";

fn composed(foreign: &str, placement: &Placement) -> Result<Composed, Refusal> {
    let script = parse_foreign(foreign, "Open-Xchange")?;
    script.analyze("Open-Xchange")?;
    compose(foreign, &script, placement, MAILROOM, "Open-Xchange")
}
fn include() -> Placement {
    Placement::Include {
        script: "mailroom".into(),
    }
}
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}
fn refused(foreign: &str) -> Refusal {
    match composed(foreign, &Placement::Block {}) {
        Err(r) => r,
        Ok(_) => panic!("composition must refuse {foreign}"),
    }
}

#[test]
fn ox_rules_keep_names_order_and_exact_bytes_under_both_placements() {
    let script = parse(OX).unwrap();
    let rules = script.analyze("Open-Xchange").unwrap();
    assert_eq!(
        rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["Verein", "Gelesen"]
    );
    assert_eq!(
        rules.iter().map(|r| r.may_stop).collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(
        rules[0].dispositions,
        [ForeignDest::Folder {
            name: "Verein".into()
        }]
    );
    let block = composed(OX, &Placement::Block {}).unwrap();
    let host = text(&block.host);
    assert!(host.starts_with("require [\"fileinto\",\"imap4flags\"];\r\n\n# mailroom:require-begin v1\nrequire [\"relational\"];\n# mailroom:require-end v1\n## Flag: |UniqueId:0|Rulename: Verein"));
    assert!(host.contains("## Flag: |UniqueId:5|Rulename: Mailroom news\n#@ news\n"));
    assert!(host.contains("## Flag: |UniqueId:6|Rulename: Mailroom system.unmatched\n"));
    assert!(host.ends_with("  stop;\n}\n# mailroom:block-end v1\n"));
    assert!(host.find("Gelesen").unwrap() < host.find("mailroom:block-begin").unwrap());
    assert!(block.owned.is_none());
    let (foreign, found) = strip(&block.host, "Open-Xchange").unwrap();
    assert_eq!(foreign.as_bytes(), OX.as_bytes());
    assert_eq!(
        found,
        Found {
            require: true,
            block: true,
            include: None
        }
    );
    let inc = composed(OX, &include()).unwrap();
    let host = text(&inc.host);
    assert!(host.contains("\n# mailroom:require-begin v1\nrequire [\"include\"];\n"));
    assert!(host.ends_with(
        "}\r\n\n# mailroom:include-begin v1\ninclude :personal \"mailroom\";\n# mailroom:include-end v1\n"
    ));
    let (name, owned) = inc.owned.unwrap();
    assert_eq!(name, "mailroom");
    assert_eq!(owned, format!("{OWNED}{MAILROOM}").into_bytes());
    let (foreign, found) = strip(&inc.host, "Open-Xchange").unwrap();
    assert_eq!(foreign, OX);
    assert_eq!(found.include.as_deref(), Some("mailroom"));
    assert!(!found.block);
}

#[test]
fn redeploy_switch_and_removal_touch_only_mailroom_segments() {
    for foreign in [
        OX,
        "# no requirements\nif header :is \"subject\" \"x\" { fileinto \"X\"; stop; }",
        "require \"fileinto\"; if true { addflag \"a\"; }\n/* trailing */",
        "",
    ] {
        let first = composed(foreign, &Placement::Block {}).unwrap();
        let (stripped, _) = strip(&first.host, "Open-Xchange").unwrap();
        assert_eq!(stripped, foreign);
        let again = composed(&stripped, &Placement::Block {}).unwrap();
        assert_eq!(again.host, first.host, "re-deploy converges");
        let switched = composed(&stripped, &include()).unwrap();
        let (removed, found) = strip(&switched.host, "Open-Xchange").unwrap();
        assert_eq!(removed.as_bytes(), foreign.as_bytes());
        assert!(found.include.is_some() && !found.block);
        let (none, nothing) = strip(foreign.as_bytes(), "Open-Xchange").unwrap();
        assert_eq!(none, foreign);
        assert_eq!(nothing, Found::default());
    }
}

#[test]
fn require_merging_adds_only_missing_extensions_in_a_marked_segment() {
    let all = "require [\"fileinto\", \"relational\", \"include\"];\nif true { addflag \"x\"; }\n";
    for placement in [Placement::Block {}, include()] {
        let host = composed(all, &placement).unwrap().host;
        assert!(!text(&host).contains("require-begin"), "nothing missing");
        assert!(text(&host).starts_with(all));
    }
    let midline = "require \"fileinto\"; if true { addflag \"x\"; }\n";
    let host = composed(midline, &Placement::Block {}).unwrap().host;
    assert!(text(&host).starts_with("require \"fileinto\";\n# mailroom:require-begin v1\nrequire [\"relational\"];\n# mailroom:require-end v1\n if true"));
    let plain = "if header :is \"subject\" \"x\" { fileinto \"X\"; stop; }\n";
    let host = composed(plain, &Placement::Block {}).unwrap().host;
    assert!(
        text(&host).starts_with(
            "\n# mailroom:require-begin v1\nrequire [\"fileinto\", \"relational\"];\n"
        )
    );
    assert_eq!(
        parse(plain).unwrap().analyze("s").unwrap()[0].name,
        "Rule 1"
    );
}

#[test]
fn existing_stop_and_unsafe_foreign_flow_refuse_instead_of_rewriting() {
    assert!(matches!(
        refused("stop;\n"),
        Refusal::MailroomUnreachable { line: 1, .. }
    ));
    assert!(matches!(
        refused("if true { fileinto \"A\"; stop; }\n"),
        Refusal::MailroomUnreachable { .. }
    ));
    assert!(matches!(
        refused("if exists \"x\" { stop; } else { keep; stop; }\n"),
        Refusal::MailroomUnreachable { .. }
    ));
    for unsafe_rule in [
        "keep;\n",
        "if header :is \"subject\" \"a\" { fileinto \"A\"; }\n",
        "if exists \"x\" { discard; } elsif true { stop; }\n",
        "if true { redirect \"a@example.test\"; }\n",
        "require \"include\"; include \"other\";\n",
        "require \"vnd.dovecot.pipe\"; pipe \"x\";\n",
        "foreverypart { addflag \"x\"; }\n",
    ] {
        assert!(
            matches!(refused(unsafe_rule), Refusal::UnsafeRule { .. }),
            "{unsafe_rule}"
        );
    }
    for safe in [
        "require \"copy\"; if true { fileinto :copy \"Archive\"; redirect :copy \"a@example.test\"; }\n",
        "if header :is \"subject\" \"a\" { fileinto \"A\"; stop; } elsif true { addflag \"b\"; }\n",
        "if size :over 1M { discard; stop; }\nvacation :days 3 \"away\";\n",
        "if exists \"x\" { keep; if false { stop; } stop; }\nif true { set \"a\" \"b\"; }\n",
    ] {
        assert!(composed(safe, &Placement::Block {}).is_ok(), "{safe}");
    }
}

#[test]
fn unparseable_or_damaged_foreign_content_blocks_with_its_location() {
    for (bad, line) in [
        ("if true {\n fileinto \"A\";\n", 3),
        ("if true { fileinto \"A; }\n", 1),
        ("if true { stop; }\nrequire \"x\";\n", 2),
        ("\n\nelsif true { stop; }\n", 3),
        ("if true { stop; } /* open\n", 1),
        ("fileinto text:\nunterminated\n", 1),
        ("if @ { }", 1),
    ] {
        assert!(
            matches!(refused(bad), Refusal::Unparseable { line: l, .. } if l == line),
            "{bad}"
        );
    }
    assert!(matches!(
        strip(b"keep;\xff", "Open-Xchange"),
        Err(Refusal::Unparseable { .. })
    ));
    let good = composed(OX, &Placement::Block {}).unwrap().host;
    let good = text(&good);
    for damaged in [
        format!(
            "{good}{}",
            &good[good.find("\n# mailroom:block-begin").unwrap()..]
        ),
        good.replace("# mailroom:block-end v1\n", ""),
        good.replace("# mailroom:block-end v1", "# mailroom:block-end v2"),
        format!("{OX}# mailroom:something\n"),
        format!(
            "{OX}if header :is \"x\" \"\n# mailroom:block-begin v1\n# mailroom:block-end v1\n\" {{ stop; }}\n"
        ),
    ] {
        assert!(
            matches!(
                strip(damaged.as_bytes(), "Open-Xchange"),
                Err(Refusal::MarkerDamaged { .. })
            ),
            "{damaged}"
        );
    }
    let multiline =
        "require \"vacation\";\nvacation text:\n..dot-stuffed\n# not a comment\n.\n;\n/* note */\n";
    assert!(composed(multiline, &Placement::Block {}).is_ok());
}

#[test]
fn foreign_rules_win_by_order_and_unmodeled_tests_stay_undetermined() {
    let headers = crate::census::parse_headers(
        b"From: noreply@example.test\r\nTo: owner@example.test\r\nSubject: hello\r\nList-Id: <verein.example.test>\r\n\r\n",
        crate::census::Direction::Received,
    )
    .unwrap();
    let view = MessageView {
        headers: &headers,
        envelope: None,
    };
    let outcome = |source: &str| {
        let script = parse(source).unwrap();
        for rule in &script.rules {
            let mut dest = None;
            match run(&rule.commands, &view, &mut dest) {
                Run::Continue => continue,
                Run::Stop(d) => return format!("{} stop {d:?}", rule.name),
                Run::Unknown => return format!("{} unknown", rule.name),
            }
        }
        "mailroom".into()
    };
    assert_eq!(
        outcome(OX),
        "Verein stop Folder { name: \"Verein\" }",
        "existing filing wins before Mailroom"
    );
    assert_eq!(
        outcome("if address :domain \"from\" \"example.test\" { addflag \"x\"; stop; }"),
        "Rule 1 stop Inbox"
    );
    assert_eq!(
        outcome("if header :is \"subject\" \"other\" { keep; stop; }"),
        "mailroom"
    );
    assert_eq!(
        outcome("if header :contains \"x-spam-flag\" \"yes\" { fileinto \"Spam\"; stop; }"),
        "Rule 1 unknown"
    );
    assert_eq!(
        outcome("if size :over 10K { discard; stop; }"),
        "Rule 1 unknown"
    );
    assert_eq!(
        outcome("if header :regex \"subject\" \"h.*\" { addflag \"x\"; }"),
        "mailroom",
        "an undetermined branch that cannot stop does not affect precedence"
    );
    assert_eq!(
        outcome(
            "if anyof (header :is \"subject\" \"HELLO\", size :over 1) { fileinto \"INBOX\"; stop; }"
        ),
        "Rule 1 stop Inbox"
    );
    assert_eq!(
        outcome("if envelope :all :is \"to\" \"owner@example.test\" { stop; }"),
        "Rule 1 unknown",
        "missing envelope evidence is unknown"
    );
}

#[test]
fn placement_choice_is_strict_and_has_no_default() {
    let block: Placement = serde_json::from_str(r#"{"kind":"block"}"#).unwrap();
    assert_eq!(block, Placement::Block {});
    assert_eq!(
        serde_json::to_string(&block).unwrap(),
        r#"{"kind":"block"}"#
    );
    for bad in [
        r#"{"kind":"block","extra":true}"#,
        r#"{"kind":"include"}"#,
        r#"{"kind":"include","script":"x","extra":1}"#,
        r#"{}"#,
    ] {
        assert!(serde_json::from_str::<Placement>(bad).is_err(), "{bad}");
    }
    assert!(!include().valid_for("mailroom"));
    assert!(include().valid_for("Open-Xchange"));
}
