use super::*;
use crate::{
    engine::{self, PreviewInput},
    rules::{
        config::{self, Overrides, Patch},
        program::{self, CompiledArtifact, Engine},
        proof::{self, Baseline, Destination, Proof},
    },
    sender_data::Package,
};
use serde::{Deserialize, Serialize};

const EMPTY: &str = "domain,country,priority,settings_url,source,checked\n";
fn catalog(notes: &str) -> Catalog {
    let money =
        format!("{EMPTY}example.com,DE,1,https://example.com/settings,contrib,2026-10-01\n");
    let orders = format!("{EMPTY}example.net,AT,2,,contrib,2026-10-01\n");
    let contracts = format!("{EMPTY}example.org,CH,2,,contrib,2026-10-01\n");
    Catalog::parse(Package {
        release: "schema: 1\nversion: 1.0.0\nreleased: 2026-10-29\nlicense: CC0-1.0\n",
        license: "SPDX-License-Identifier: CC0-1.0",
        sources: notes,
        categories: [&money, &contracts, &orders, EMPTY, EMPTY, EMPTY],
        services: "domain,source,checked\n",
    })
    .unwrap()
}
fn input() -> PreviewInput {
    serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap()
}
fn documents() -> (Preset, Overrides) {
    let input = input();
    config::documents(&input.documents.preset_yaml, &input.documents.override_yaml).unwrap()
}
fn prove(
    prepared: &Prepared,
    overrides: &Overrides,
    sender: &str,
    recipient: &str,
    extra: &str,
) -> Proof {
    let mut input = input();
    input.messages.truncate(1);
    input.messages[0].headers = format!(
        "From: {sender}\r\nTo: {recipient}\r\nDelivered-To: {recipient}\r\nSubject: Account activity\r\nMessage-ID: <core-fixture@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n{extra}\r\n"
    );
    let census = engine::synthetic_census(&input).unwrap();
    proof::prove(
        program::compile_prepared(prepared, overrides, &input.context).unwrap(),
        &census,
        input.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap()
}
fn folder(value: &str) -> Destination {
    Destination::Folder { name: value.into() }
}
fn patch() -> Patch {
    Patch {
        id: "sender.core.money".into(),
        enabled: None,
        folder: None,
        when: None,
        extend: vec![],
        accounts: None,
        mark_read: None,
    }
}

#[test]
fn whole_core_lists_are_present_without_census_selection_and_match_subdomains() {
    let (preset, overrides) = documents();
    let data = catalog("Synthetic public-domain fixtures only");
    let prepared = prepare(&preset, &data).unwrap();
    let artifact = program::compile_prepared(&prepared, &overrides, &input().context).unwrap();
    for domain in ["example.com", "example.net", "example.org"] {
        assert!(artifact.executable().contains(&format!("\"{domain}\"")));
    }
    assert!(
        artifact
            .trace()
            .iter()
            .any(|line| line.contains(&data.binding()))
    );
    for (sender, recipient, expected) in [
        (
            "notice@example.com",
            "team@work.example.test",
            folder("Business/Money"),
        ),
        (
            "notice@sub.example.com",
            "team@work.example.test",
            folder("Business/Money"),
        ),
        (
            "notice@example.net",
            "owner@home.example.test",
            folder("Personal/Orders"),
        ),
        (
            "notice@example.org",
            "team@work.example.test",
            folder("Business/Contracts"),
        ),
        (
            "notice@badexample.com",
            "team@work.example.test",
            Destination::Inbox,
        ),
    ] {
        let result = prove(&prepared, &overrides, sender, recipient, "");
        assert_eq!(result.decisions[0].destination, expected);
        assert_eq!(result.artifact.identity(), artifact.identity());
    }
}

#[test]
fn extensions_are_sender_alternatives_while_bulk_and_guards_keep_precedence() {
    let (preset, mut overrides) = documents();
    let prepared = prepare(&preset, &catalog("Synthetic fixtures")).unwrap();
    let mut change = patch();
    change.folder = Some("Reviewed".into());
    change.extend = vec![
        Condition::FromDomain {
            value: "extra.example.test".into(),
        },
        Condition::FromAddress {
            value: "only@else.example.test".into(),
        },
    ];
    overrides.patches.push(change);
    for sender in [
        "notice@example.com",
        "notice@extra.example.test",
        "notice@sub.extra.example.test",
        "only@else.example.test",
    ] {
        let result = prove(&prepared, &overrides, sender, "team@work.example.test", "");
        assert_eq!(result.decisions[0].destination, folder("Business/Reviewed"));
        let bulk = prove(
            &prepared,
            &overrides,
            sender,
            "team@work.example.test",
            "List-Id: <fixture.example.test>\r\n",
        );
        assert_eq!(
            bulk.decisions[0].destination,
            folder("Business/Newsletters")
        );
    }
    let unrelated = prove(
        &prepared,
        &overrides,
        "other@else.example.test",
        "team@work.example.test",
        "",
    );
    assert_eq!(unrelated.decisions[0].destination, Destination::Inbox);
    let mut guarded = preset.clone();
    guarded.rules.insert(
        0,
        Rule {
            id: "guard.core-fixture".into(),
            category: Category::Money,
            stage: Stage::Guard,
            when: vec![Condition::FromDomain {
                value: "example.com".into(),
            }],
            folder: None,
            enabled: true,
            accounts: BTreeSet::new(),
            retain: true,
            mark_read: false,
        },
    );
    let guard = prove(
        &prepare(&guarded, &catalog("Synthetic fixtures")).unwrap(),
        &overrides,
        "notice@example.com",
        "team@work.example.test",
        "",
    );
    assert_eq!(guard.decisions[0].destination, Destination::Inbox);
}

#[test]
fn category_patches_keep_disable_scope_replacement_and_recipient_safety() {
    let (preset, mut overrides) = documents();
    let prepared = prepare(&preset, &catalog("Synthetic fixtures")).unwrap();
    let mut change = patch();
    change.enabled = Some(false);
    overrides.patches.push(change.clone());
    assert_eq!(
        prove(
            &prepared,
            &overrides,
            "notice@example.com",
            "team@work.example.test",
            ""
        )
        .decisions[0]
            .destination,
        Destination::Inbox
    );
    change.enabled = Some(true);
    change.accounts = Some(BTreeSet::from(["different-account".into()]));
    *overrides.patches.last_mut().unwrap() = change.clone();
    assert_eq!(
        prove(
            &prepared,
            &overrides,
            "notice@example.com",
            "team@work.example.test",
            ""
        )
        .decisions[0]
            .destination,
        Destination::Inbox
    );
    change.accounts = None;
    change.when = Some(vec![
        Condition::FromAddress {
            value: "chosen@else.example.test".into(),
        },
        Condition::NonBulk,
    ]);
    *overrides.patches.last_mut().unwrap() = change;
    assert_eq!(
        prove(
            &prepared,
            &overrides,
            "notice@example.com",
            "team@work.example.test",
            ""
        )
        .decisions[0]
            .destination,
        Destination::Inbox
    );
    assert_eq!(
        prove(
            &prepared,
            &overrides,
            "chosen@else.example.test",
            "team@work.example.test",
            ""
        )
        .decisions[0]
            .destination,
        folder("Business/Money")
    );
    overrides.patches.pop();
    let held = prove(
        &prepared,
        &overrides,
        "notice@example.com",
        "unknown@work.example.test",
        "",
    );
    assert_eq!(held.decisions[0].destination, Destination::Inbox);
    assert!(held.decisions[0].review);
    assert!(held.decisions[0].copies.is_empty());
    assert!(!held.decisions[0].mark_read);
}

#[test]
fn release_provenance_major_pin_collisions_and_opt_out_are_bound() {
    let (mut preset, overrides) = documents();
    let first = prepare(&preset, &catalog("Original fixture provenance")).unwrap();
    let changed = prepare(&preset, &catalog("Changed fixture provenance")).unwrap();
    let first = program::compile_prepared(&first, &overrides, &input().context).unwrap();
    let changed = program::compile_prepared(&changed, &overrides, &input().context).unwrap();
    assert_eq!(first.executable(), changed.executable());
    assert_ne!(first.identity(), changed.identity());
    preset.core_senders.as_mut().unwrap().major = 2;
    assert!(prepare(&preset, &catalog("Synthetic fixtures")).is_err());
    preset.core_senders.as_mut().unwrap().major = 1;
    let mut colliding = preset.rules[0].clone();
    colliding.id = "sender.core.money".into();
    preset.rules.push(colliding);
    assert!(
        program::compile_prepared(
            &prepare(&preset, &catalog("Synthetic fixtures")).unwrap(),
            &overrides,
            &input().context
        )
        .is_err()
    );
    preset.rules.pop();
    preset.core_senders = None;
    let opted_out = load(&preset).unwrap();
    assert!(opted_out.version.is_none());
    assert_eq!(opted_out.preset.rules.len(), preset.rules.len());
}

#[test]
fn empty_seed_has_no_new_routing_effects_and_sender_sets_survive_saved_artifacts() {
    let input = input();
    let result = engine::preview(&input).unwrap();
    assert_eq!(result.counts.filed, 10);
    assert_eq!(result.counts.inbox, 4);
    assert_eq!(result.counts.copies, 3);
    assert!(
        result
            .artifact
            .effective()
            .iter()
            .any(|r| r.id == "sender.core.money")
    );
    let (preset, overrides) = documents();
    let prepared = prepare(&preset, &catalog("Synthetic fixtures")).unwrap();
    let artifact = program::compile_prepared(&prepared, &overrides, &input.context).unwrap();
    #[derive(Serialize, Deserialize)]
    struct Saved {
        #[serde(with = "program::persisted")]
        artifact: CompiledArtifact,
    }
    let saved = serde_json::to_vec(&Saved {
        artifact: artifact.clone(),
    })
    .unwrap();
    let restored: Saved = serde_json::from_slice(&saved).unwrap();
    assert_eq!(restored.artifact.identity(), artifact.identity());
    assert_eq!(restored.artifact.executable(), artifact.executable());
    let mut context = input.context;
    context.engine = Engine::GmailBacklog;
    let gmail = program::compile_prepared(&prepared, &overrides, &context).unwrap();
    assert!(!gmail.executable().contains("redirect"));
    assert!(gmail.executable().contains("\"example.org\""));
}

#[test]
fn malformed_sender_sets_fail_and_existing_conditions_remain_conjunctive() {
    let (mut preset, mut overrides) = documents();
    let mut rule = preset.rules[0].clone();
    rule.id = "user.sender-set".into();
    rule.stage = Stage::From;
    rule.retain = false;
    rule.when = vec![Condition::SenderSet {
        addresses: BTreeSet::new(),
        domains: BTreeSet::from(["INVALID DOMAIN".into()]),
    }];
    overrides.additions.push(rule.clone());
    assert!(program::compile(&preset, &overrides, &input().context).is_err());
    overrides.additions.pop();
    rule.when = vec![Condition::FromAddress {
        value: "one@example.com".into(),
    }];
    overrides.additions.push(rule);
    let mut extension = patch();
    extension.id = "user.sender-set".into();
    extension.extend = vec![Condition::FromAddress {
        value: "two@example.com".into(),
    }];
    overrides.patches.push(extension);
    preset.core_senders = None;
    let prepared = load(&preset).unwrap();
    for sender in ["one@example.com", "two@example.com"] {
        assert_eq!(
            prove(&prepared, &overrides, sender, "team@work.example.test", "").decisions[0]
                .destination,
            Destination::Inbox
        );
    }
}
