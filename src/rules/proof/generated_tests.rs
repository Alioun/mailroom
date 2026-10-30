use super::*;
use crate::engine::{self, Fixture, FixtureDelivery, PreviewInput};

fn fixture() -> PreviewInput {
    let mut input: PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.documents.preset_yaml = "id: generated\nmajor: 1\nrules:\n  - id: guard.code\n    category: orders\n    stage: guard\n    retain: true\n    when: [{kind: subject_contains, value: login code}]\n".into();
    input.documents.override_yaml = r#"preset: generated
major: 1
roles:
  domains:
    home.example.test: personal
    work.example.test: business
    signup.example.test: personal
  signup_domains: [signup.example.test]
ledger:
  - address: service@home.example.test
    status: active
    category: orders
    folder: Service
additions:
  - id: user.vendor
    category: orders
    stage: from
    when: [{kind: from_domain, value: vendor.example.test}]
    folder: Vendor
"#
    .into();
    input.pins.clear();
    input.messages = [
        ("ledger", "service@home.example.test", true, "routine"),
        ("new", "new@signup.example.test", true, "routine"),
        ("bulk", "owner@home.example.test", true, "routine"),
        ("unmatched", "owner@home.example.test", false, "routine"),
        ("guard", "service@home.example.test", true, "login code"),
        ("vendor", "owner@home.example.test", false, "routine"),
        ("business", "team@work.example.test", true, "routine"),
        ("unknown", "owner@unknown.example.test", true, "routine"),
        ("mixed", "owner@home.example.test", true, "routine"),
        ("old", "service@home.example.test", false, "routine"),
        ("earliest", "service@home.example.test", false, "routine"),
        ("future", "service@home.example.test", false, "routine"),
        ("invalid", "service@home.example.test", false, "routine"),
    ]
    .into_iter()
    .map(|(id, recipient, bulk, subject)| {
        let date = match id {
            "old" => "Mon, 01 Sep 2025 00:00:00 +0000",
            "earliest" => "Wed, 01 Oct 2025 00:00:00 +0000",
            "future" => "Sun, 01 Nov 2026 00:00:00 +0000",
            "invalid" => "not-a-date",
            _ => "Sat, 24 Oct 2026 00:00:00 +0000",
        };
        Fixture {
            id: id.into(),
            headers: format!(
                "From: {}\r\nTo: {recipient}\r\nDelivered-To: {recipient}\r\nSubject: {subject}\r\nMessage-ID: <{id}@generated.example.test>\r\nDate: {date}\r\n{}{}\r\n",
                if id == "vendor" { "sender@vendor.example.test" } else { "sender@example.test" },
                if bulk { "List-Id: <news.example.test>\r\n" } else { "" },
                if id == "mixed" { "Delivered-To: team@work.example.test\r\n" } else { "" },
            ),
            current_folder: "INBOX".into(),
            delivery: FixtureDelivery::Unavailable,
            observed_flags: Some(BTreeSet::new()),
        }
    })
    .collect();
    input
}

fn report(input: &PreviewInput) -> Proof {
    prove(
        engine::artifact(&input.documents, &input.context).unwrap(),
        &engine::synthetic_census(input).unwrap(),
        input.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap()
}

#[test]
fn generated_filing_rows_count_unique_safe_messages_and_actual_precedence_in_the_window() {
    let proof = report(&fixture());
    assert_eq!(proof.generated_rules.len(), 4);
    let ledger = &proof.generated_rules[0];
    assert!(ledger.id.starts_with("system.ledger."));
    assert!(!ledger.id.contains("service@"));
    assert_eq!((ledger.matched, ledger.candidates), (2, 3));
    assert_eq!(ledger.monthly["2025-10"], 1);
    assert_eq!(ledger.monthly["2026-10"], 1);
    assert_eq!(ledger.monthly.values().sum::<usize>(), 2);
    assert_eq!(
        proof.generated_rules[1..]
            .iter()
            .map(|r| (r.id.as_str(), r.matched, r.candidates))
            .collect::<Vec<_>>(),
        [
            ("system.new_addresses", 1, 1),
            ("system.bulk", 2, 5),
            ("system.unmatched", 1, 8),
        ]
    );
    assert_eq!(proof.counts.review, 2);
    assert_eq!(proof.counts.messages, 13);
    assert_eq!(proof.rules.len(), 2);
    assert!(proof.rules.iter().all(|r| r.matched == 1));
    assert!(proof.rules.iter().all(|r| !r.id.starts_with("system.")));
    assert!(proof.generated_rules.iter().all(|r| r.monthly.len() == 13));
    assert_eq!(proof.gate, "warnings");
    println!(
        "MAILROOM_GENERATED_RULE_PROOF {}",
        serde_json::to_string(&proof).unwrap()
    );
}

#[test]
fn a_shadowed_generated_fallback_does_not_create_authored_rule_warnings() {
    let mut input = fixture();
    let before = report(&input);
    input.messages.retain(|m| m.id != "unmatched");
    let after = report(&input);
    let fallback = after.generated_rules.last().unwrap();
    assert_eq!(fallback.id, "system.unmatched");
    assert_eq!((fallback.matched, fallback.candidates), (0, 7));
    assert!(fallback.zero_match && fallback.fully_shadowed);
    assert_eq!(before.warnings, after.warnings);
    assert_eq!(before.gate, after.gate);
    assert!(after.rules.iter().all(|r| !r.zero_match));
    println!(
        "MAILROOM_SHADOWED_GENERATED_RULE_PROOF {}",
        serde_json::to_string(&after).unwrap()
    );
}

#[test]
fn unknown_authority_omits_generated_rows_without_inventing_zero_or_eligibility() {
    let mut input = fixture();
    input.context.recipient_evidence = super::super::EvidenceSource::Unknown;
    let proof = report(&input);
    assert!(proof.generated_rules.is_empty());
    assert_eq!(proof.counts.review, 13);
    assert_eq!(proof.counts.filed, 0);
    assert_eq!(proof.counts.copies, 0);
    assert!(
        proof
            .decisions
            .iter()
            .all(|d| d.destination == Destination::Inbox)
    );
    assert!(
        proof
            .artifact
            .omissions()
            .iter()
            .any(|o| o.rule == "system.bulk")
    );
    println!(
        "MAILROOM_OMITTED_GENERATED_RULE_PROOF {}",
        serde_json::to_string(&proof).unwrap()
    );
}
