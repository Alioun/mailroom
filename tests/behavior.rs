use mailroom::engine::{self, Destination, PreviewInput};
use serde_json::{Value, json};

fn sample() -> Value {
    serde_json::from_str(include_str!("../examples/sample.json")).unwrap()
}
fn run(value: Value) -> Result<engine::Preview, String> {
    let input: PreviewInput = serde_json::from_value(value).map_err(|e| e.to_string())?;
    engine::preview(&input)
}
fn folder(name: &str) -> Destination {
    Destination::Folder { name: name.into() }
}

#[test]
fn overrides_beat_bulk_bulk_beats_topic_and_guards_beat_overrides() {
    let result = run(sample()).unwrap();
    assert_eq!(result.decisions[0].destination, folder("Studio"));
    assert_eq!(result.decisions[0].matched_rule.as_deref(), Some("studio"));
    assert!(result.decisions[0].reason.contains("override"));
    assert_eq!(result.decisions[1].destination, folder("Reading"));
    assert_eq!(result.decisions[2].destination, folder("Receipts"));
    assert_eq!(result.decisions[3].destination, Destination::Inbox);
    assert!(result.decisions[3].matched_rule.is_none());
    assert_eq!(result.decisions[4].destination, folder("Studio"));
    let mut value = sample();
    value["messages"][5]["headers"] =
        json!("From: updates@studio.example.com\nSubject: Invoice\nX-Spam-Flag: YES");
    let result = run(value).unwrap();
    assert_eq!(result.decisions[5].destination, Destination::Inbox);
    assert_eq!(
        result.decisions[5].matched_rule.as_deref(),
        Some("spam-guard")
    );
}

#[test]
fn one_folder_and_copy_counts_are_independent() {
    let mut value = sample();
    let mut copy = value["preset"]["copies"][0].clone();
    copy["id"] = json!("second-copy");
    copy["recipient"] = json!("second@example.org");
    value["preset"]["copies"].as_array_mut().unwrap().push(copy);
    let result = run(value).unwrap();
    assert_eq!(result.counts.messages, 6);
    assert_eq!(result.counts.filed, 4);
    assert_eq!(result.counts.inbox, 2);
    assert_eq!(result.counts.copies, 8);
    assert_eq!(result.counts.copied_messages, 4);
    assert_eq!(result.decisions[5].copies.len(), 2);
    assert_eq!(
        result.counts.messages,
        result.counts.filed + result.counts.inbox
    );
    assert!(
        result.sieve.find("redirect :copy").unwrap() < result.sieve.find("fileinto \"").unwrap()
    );
    assert!(!result.sieve.contains(":create"));
    assert!(result.sieve.contains("stop;"));
}

#[test]
fn disabling_and_additions_change_behavior_without_losing_trace() {
    let mut value = sample();
    value["overrides"]["patches"][0]["enabled"] = json!(false);
    let result = run(value).unwrap();
    assert_eq!(result.decisions[0].destination, folder("Reading"));
    assert_eq!(result.decisions[4].destination, folder("Releases"));
    assert_eq!(result.decisions[4].reason, "added by override");
    assert!(result.trace.iter().any(|s| s.contains("studio: disabled")));
}

#[test]
fn from_precedes_subject_and_domain_matches_subdomains_only() {
    let mut value = sample();
    value["overrides"]["patches"] = json!([]);
    value["messages"] = json!([
        {"id":"a","headers":"From: member@sub.studio.example.com\nSubject: Invoice"},
        {"id":"b","headers":"From: member@notstudio.example.com\nSubject: Invoice"}
    ]);
    let result = run(value).unwrap();
    assert_eq!(result.decisions[0].destination, folder("Projects"));
    assert_eq!(result.decisions[1].destination, folder("Receipts"));
}

#[test]
fn folded_ascii_and_one_click_match_with_ascii_casemap() {
    let mut value = sample();
    value["preset"]["rules"][1]["conditions"] = json!([{"kind":"one_click"}]);
    value["messages"] = json!([{"id":"folded","headers":"From: A@EXAMPLE.COM\r\nSubject: Monthly\r\n INVOICE\r\nList-Unsubscribe-Post: list-unsubscribe=one-click\r\n"}]);
    let result = run(value).unwrap();
    assert_eq!(result.decisions[0].destination, folder("Reading"));
    assert_eq!(result.counts.copies, 1);
}

#[test]
fn invalid_models_fail_instead_of_weakening_rules() {
    for (path, replacement) in [
        ("/overrides/major", json!(2)),
        ("/overrides/patches/0/id", json!("missing")),
        ("/preset/rules/1/id", json!("studio")),
        ("/preset/rules/1/conditions", json!([])),
        ("/preset/rules/1/conditions/0/kind", json!("body_contains")),
        ("/preset/rules/1/stage", json!("subject")),
        ("/messages/1/id", json!("sample-1")),
        ("/preset/copies/0/recipient", json!("person@real-mail.com")),
        (
            "/preset/rules/0/destination",
            json!({"kind":"folder","name":"Inbox"}),
        ),
    ] {
        let mut value = sample();
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(run(value).is_err(), "accepted invalid {path}");
    }
    let mut value = sample();
    let duplicate = value["overrides"]["patches"][0].clone();
    value["overrides"]["patches"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(run(value).is_err());
    let mut value = sample();
    value["preset"]["rules"][0]["unexpected"] = json!(true);
    assert!(run(value).is_err());
}

#[test]
fn unsupported_header_semantics_are_errors() {
    for headers in [
        "From: a@example.org\nSubject: =?UTF-8?Q?invoice?=",
        "From: Alice <a@example.org>",
        "From: a@example.org\nSubject: Grüße",
        "From: a@example.org\nFrom: b@example.org",
        "From: a@example.org\n\nA body",
        "From: a@example.org\nBroken header",
        "Subject: no sender",
        "From: a@example.org\nTo: real@company.com",
    ] {
        let mut value = sample();
        value["messages"][0]["headers"] = json!(headers);
        assert!(run(value).is_err(), "accepted unsupported headers");
    }
}
