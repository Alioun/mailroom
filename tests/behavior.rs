use mailroom::{
    engine::{self, PreviewInput},
    rules::{
        Engine, EvidenceSource,
        config::{self, Condition, Role},
        proof::{Baseline, Destination, Pin},
    },
};
use serde_json::json;
fn input() -> PreviewInput {
    serde_json::from_str(include_str!("../examples/sample.json")).unwrap()
}
fn run(input: &PreviewInput) -> mailroom::rules::proof::Proof {
    engine::preview(input).unwrap()
}
fn edit(
    input: &mut PreviewInput,
    change: impl FnOnce(&mut config::Preset, &mut config::Overrides),
) {
    let (mut p, mut o) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml).unwrap();
    change(&mut p, &mut o);
    input.documents.preset_yaml = serde_yaml_ng::to_string(&p).unwrap();
    input.documents.override_yaml = serde_yaml_ng::to_string(&o).unwrap();
}
fn folder(name: &str) -> Destination {
    Destination::Folder { name: name.into() }
}
#[test]
fn settled_order_and_one_destination_with_safe_independent_copies() {
    let p = run(&input());
    assert_eq!(p.counts.messages, 14);
    assert_eq!(p.counts.filed, 10);
    assert_eq!(p.counts.inbox, 4);
    assert_eq!(p.counts.review, 2);
    assert_eq!(p.counts.copies, 3);
    assert_eq!(p.decisions[0].destination, folder("Business/Studio"));
    assert_eq!(p.decisions[1].destination, folder("Business/Newsletters"));
    assert_eq!(p.decisions[2].copies.len(), 1);
    assert_eq!(p.decisions[3].destination, Destination::Inbox);
    assert!(p.decisions[5].review && p.decisions[5].copies.is_empty());
    assert!(p.decisions[6].review && p.decisions[6].copies.is_empty());
    assert_eq!(p.decisions[7].destination, folder("Personal/New addresses"));
    assert!(!p.decisions[7].review);
    assert_eq!(p.decisions[10].destination, Destination::Inbox);
    assert_eq!(p.decisions[10].copies.len(), 1);
    assert_eq!(p.decisions[11].destination, folder("Business/Invoices"));
    assert_eq!(p.decisions[11].copies.len(), 1);
    assert!(p.decisions[12].copies.is_empty());
    assert!(
        p.artifact.executable().find("redirect :copy").unwrap()
            < p.artifact.executable().find("fileinto ").unwrap()
    );
    assert!(!p.artifact.executable().contains(":create"));
}
#[test]
fn full_one_click_pair_and_encoded_unicode_subjects() {
    let p = run(&input());
    assert_eq!(p.decisions[8].destination, folder("Personal/Money"));
    assert_eq!(p.decisions[9].subject, "Grüße und Rechnung");
    assert_eq!(p.decisions[9].destination, folder("Personal/Money"));
    assert_eq!(p.decisions[13].destination, folder("Personal/Newsletters"));
}
#[test]
fn unicode_paths_and_mute_read_action_are_configurable_only() {
    let mut i = input();
    edit(&mut i, |_, o| {
        o.layout.language = config::Language::De;
        o.layout.personal_root = "Beispiel".into();
        o.layout.business_root = "Büro".into();
        o.patches[0].mark_read = Some(true);
        o.patches[0].folder = Some("Stille".into());
    });
    let p = run(&i);
    assert_eq!(p.decisions[0].destination, folder("Büro/Stille"));
    assert!(p.decisions[0].mark_read);
    assert!(!p.decisions[5].mark_read);
    assert_eq!(p.decisions[9].destination, folder("Beispiel/Finanzen"));
}
#[test]
fn unknown_trust_never_falls_back_to_visible_recipients() {
    let mut i = input();
    i.context.recipient_evidence = EvidenceSource::Unknown;
    let p = run(&i);
    assert_eq!(p.counts.filed, 0);
    assert_eq!(p.counts.copies, 0);
    assert_eq!(p.counts.review, 14);
    assert!(
        p.decisions
            .iter()
            .all(|d| !d.mark_read && d.copy_eligibility.iter().all(|e| e.status == "unknown"))
    );
    i.context.recipient_evidence = EvidenceSource::SyntheticOriginalTo;
    assert_eq!(run(&i).counts.review, 14);
}
#[test]
fn approved_domain_is_distinct_from_address_ledger_and_address_override_wins() {
    let mut i = input();
    edit(&mut i, |_, o| {
        o.roles.signup_domains.clear();
        o.roles
            .addresses
            .insert("team@work.example.test".into(), Role::Unknown);
    });
    let p = run(&i);
    assert!(p.decisions[0].review);
    assert!(!p.decisions[7].review);
    assert_eq!(p.decisions[7].destination, Destination::Inbox);
}
#[test]
fn gmail_backlog_keeps_filing_and_never_gets_invoice_copies() {
    let mut i = input();
    i.context.engine = Engine::GmailBacklog;
    let p = run(&i);
    assert_eq!(p.counts.copies, 0);
    assert_eq!(p.counts.filed, 10);
    assert!(!p.artifact.executable().contains("redirect"));
    assert_eq!(p.artifact.omissions().len(), 3);
}
#[test]
fn unsupported_required_predicate_omits_entire_rule() {
    let mut i = input();
    edit(&mut i, |_, o| {
        o.patches[0].extend.push(Condition::Attachment)
    });
    let p = run(&i);
    assert_eq!(p.decisions[0].destination, folder("Business/Newsletters"));
    assert!(
        p.artifact
            .omissions()
            .iter()
            .any(|o| o.rule == "sender.studio")
    );
    assert!(!p.artifact.executable().contains("#@ sender.studio"));
    assert!(
        p.artifact
            .omissions()
            .iter()
            .any(|o| o.rule == "forward.keywords")
    );
}
#[test]
fn stable_renames_pins_scopes_disables_and_effective_origins() {
    let mut i = input();
    let p = run(&i);
    assert!(
        p.artifact
            .trace()
            .iter()
            .any(|t| t.contains("Renamed studio.old"))
    );
    assert!(
        p.artifact
            .effective()
            .iter()
            .any(|r| r.id == "sender.studio" && r.origin == "preset patched by override")
    );
    edit(&mut i, |_, o| {
        o.patches[0].enabled = Some(false);
        o.additions[0].accounts.insert("other-account".into());
    });
    let p = run(&i);
    assert_eq!(p.decisions[4].destination, folder("Business/Newsletters"));
}
#[test]
fn invalid_configuration_fails_without_weakening() {
    for case in 0..9 {
        let mut i = input();
        edit(&mut i, |p, o| match case {
            0 => o.major = 2,
            1 => o.patches[0].id = "typo.preset".into(),
            2 => p.rules[1].id = p.rules[0].id.clone(),
            3 => o.patches[0].when = Some(vec![]),
            4 => o.patches[0].folder = Some("three/levels/deep".into()),
            5 => o.additions[0].id = "topic.typo".into(),
            6 => {
                p.renamed.insert("loop.a".into(), "loop.b".into());
                p.renamed.insert("loop.b".into(), "loop.a".into());
            }
            7 => {
                o.roles
                    .signup_domains
                    .insert("unapproved.example.test".into());
            }
            _ => o.forwards[0].target = "missing".into(),
        });
        assert!(engine::preview(&i).is_err(), "case {case}");
    }
    let mut i = input();
    i.documents.override_yaml.push_str("unknown_field: true\n");
    assert!(engine::preview(&i).is_err());
}
#[test]
fn identity_binds_source_scope_capabilities_and_executable() {
    let a = run(&input()).artifact.identity().to_owned();
    assert_eq!(a, run(&input()).artifact.identity());
    let mut i = input();
    i.context.capabilities.remove("copy");
    assert_ne!(a, run(&i).artifact.identity());
    assert_eq!(run(&i).counts.copies, 0);
    let mut i = input();
    edit(&mut i, |_, o| {
        o.patches[0].accounts = Some(["other-account".into()].into());
    });
    assert_ne!(a, run(&i).artifact.identity());
}
#[test]
fn proof_counts_pins_stale_and_baseline_come_from_displayed_decisions() {
    let mut i = input();
    let p = run(&i);
    assert_eq!(p.months.len(), 13);
    assert_eq!(p.pins[0].status, "pass");
    assert_eq!(p.pins[1].status, "stale");
    assert_eq!(p.gate, "warnings");
    assert_eq!(
        p.counts.copies,
        p.decisions.iter().map(|d| d.copies.len()).sum::<usize>()
    );
    assert!(
        p.rules
            .iter()
            .all(|r| r.matched == r.monthly.values().sum::<usize>())
    );
    assert_eq!(p.counts.inbox, p.unmatched_senders.values().sum::<usize>());
    i.baseline = Baseline::Previous;
    assert!(run(&i).diff.len() < p.diff.len());
    i.pins[0].expected = Destination::Inbox;
    assert_eq!(run(&i).gate, "blocked");
    i.pins.push(Pin {
        account: "other-account".into(),
        message_id_hash: "f".repeat(64),
        expected: Destination::Inbox,
    });
    assert_eq!(run(&i).pins[2].status, "stale");
}
#[test]
fn shadow_and_zero_warnings_do_not_block() {
    let mut i = input();
    edit(&mut i, |p, _| {
        let mut shadow = p.rules[2].clone();
        shadow.id = "topic.shadow".into();
        p.rules.push(shadow);
    });
    let p = run(&i);
    assert!(
        p.rules
            .iter()
            .find(|r| r.id == "topic.shadow")
            .unwrap()
            .fully_shadowed
    );
    assert_eq!(p.gate, "warnings");
}
#[test]
fn normalized_census_input_rejects_bodies_and_uses_existing_parser() {
    let mut i = input();
    i.messages[0].headers.push_str("\n\nNot a header");
    assert!(engine::preview(&i).is_err());
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../examples/sample.json")).unwrap();
    value["allowed"] = json!(true);
    assert!(serde_json::from_value::<PreviewInput>(value).is_err());
}

#[test]
fn published_json_schemas_follow_the_validated_yaml_model() {
    let preset: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/preset.schema.json")).unwrap();
    let overrides: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/overrides.schema.json")).unwrap();
    assert_eq!(
        preset,
        serde_json::to_value(schemars::schema_for!(config::Preset)).unwrap()
    );
    assert_eq!(
        overrides,
        serde_json::to_value(schemars::schema_for!(config::Overrides)).unwrap()
    );
}

#[test]
fn census_proof_deduplicates_gmail_unions_placement_and_excludes_sent() {
    use mailroom::census::Direction;
    let mut i = input();
    i.context.engine = Engine::GmailBacklog;
    let artifact = engine::artifact(&i.documents, &i.context).unwrap();
    let mut census = engine::synthetic_census(&i).unwrap();
    let mut duplicate = census.folders["INBOX"].clone();
    duplicate.messages.retain(|uid, _| *uid == 3);
    duplicate.live_uids = [3].into();
    duplicate
        .messages
        .get_mut(&3)
        .unwrap()
        .gmail_labels
        .extend(["Existing label".into(), "\\Inbox".into()]);
    census.folders.insert("Archive".into(), duplicate);
    let all_mail = census.folders.remove("INBOX").unwrap();
    census.folders.insert("[Gmail]/All Mail".into(), all_mail);
    let p = mailroom::rules::proof::prove(
        artifact.clone(),
        &census,
        i.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap();
    assert_eq!(p.counts.messages, 14);
    assert!(p.decisions[2].current_folders.contains("Existing label"));
    assert!(p.decisions[2].current_folders.contains("INBOX"));
    census
        .folders
        .get_mut("Archive")
        .unwrap()
        .messages
        .get_mut(&3)
        .unwrap()
        .headers
        .direction = Direction::Sent;
    let p = mailroom::rules::proof::prove(artifact, &census, i.as_of, &[], Baseline::Current, None)
        .unwrap();
    assert_eq!(p.counts.messages, 13);
}

#[test]
fn destination_intents_copy_once_across_overlapping_triggers_and_aliases() {
    let mut i = input();
    i.messages[2].headers = i.messages[2].headers.replace(
        "Delivered-To: team@work.example.test",
        "Delivered-To: invoices@work.example.test",
    );
    edit(&mut i, |_, o| {
        o.targets.insert(
            "alias".into(),
            config::Target {
                address: "ARCHIVE@example.net".into(),
            },
        );
        o.forwards
            .iter_mut()
            .find(|f| f.id == "forward.invoice_address")
            .unwrap()
            .target = "alias".into();
    });
    let p = run(&i);
    assert_eq!(p.decisions[2].copies.len(), 1);
    assert_eq!(p.decisions[2].copies[0].rule, "forward.approved");
    assert_eq!(p.decisions[2].destination, folder("Business/Invoices"));
    edit(&mut i, |_, o| {
        o.targets.insert(
            "distinct".into(),
            config::Target {
                address: "distinct@example.test".into(),
            },
        );
        let mut f = o.forwards[0].clone();
        f.id = "forward.distinct".into();
        f.target = "distinct".into();
        o.forwards.push(f);
    });
    assert_eq!(run(&i).decisions[2].copies.len(), 2);
}

#[test]
fn repeated_original_and_forwarded_message_ids_share_the_destination_guard() {
    let mut i = input();
    let original = i.messages[11].headers.clone();
    i.messages[2].headers = original
        .replace(
            "From: Example Sender <stranger@example.org>",
            "From: Example Sender <billing@shop.example.org>",
        )
        .replace(
            "Delivered-To: invoices@work.example.test",
            "Delivered-To: team@work.example.test",
        );
    i.messages[2].headers = i.messages[2].headers.replace(
        "Date: Sat, 24 Oct 2026 00:00:00 +0000",
        "Date: Fri, 23 Oct 2026 00:00:00 +0000",
    );
    let p = run(&i);
    let hash = p
        .decisions
        .iter()
        .find(|d| d.id.ends_with("/3"))
        .unwrap()
        .message_id_hash
        .clone();
    let repeated: Vec<_> = p
        .decisions
        .iter()
        .filter(|d| d.message_id_hash == hash)
        .collect();
    assert_eq!(repeated.len(), 2);
    assert_eq!(repeated.iter().map(|d| d.copies.len()).sum::<usize>(), 1);
}
