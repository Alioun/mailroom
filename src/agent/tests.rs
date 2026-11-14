use super::*;
use serde_json::{Value, json};
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt};

const SUMMARY: &str = "# Census summary\n\nInvented export. 12 newsletter messages.\n";

#[test]
fn agent_packaged_skill_has_valid_yaml_and_usable_identity() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Metadata {
        name: String,
        description: String,
    }
    let skill = include_str!("../../agent/skills/mailroom-assistant/SKILL.md");
    let yaml = skill
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---\n")
        .unwrap()
        .0;
    let metadata: Metadata = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(metadata.name, "mailroom-assistant");
    assert!(!metadata.description.trim().is_empty());
    assert!(metadata.description.len() < 1024);
}

fn fixture() -> (tempfile::TempDir, files::Exchange) {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("exchange");
    let exchange = files::Exchange::initialize(&path).unwrap();
    fs::write(path.join("reviewed/census-summary.md"), SUMMARY).unwrap();
    fs::set_permissions(
        path.join("reviewed/census-summary.md"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    (parent, exchange)
}

fn proposal() -> Proposal {
    Proposal {
        version: 1,
        summary_sha256: digest(SUMMARY.as_bytes()),
        recommendations: vec![Recommendation {
            title: "Review newsletter filing".into(),
            reason: "The invented export reports 12 messages. Read history is unknown.".into(),
            suggested_change: "Review a newsletter category. Preserve unknown recipients in Inbox."
                .into(),
        }],
        rule_edits: None,
    }
}

#[test]
fn agent_proposals_survive_reopen_and_exact_retries_without_overwriting() {
    let (parent, exchange) = fixture();
    let path = parent.path().join("exchange");
    let id = exchange.submit(proposal()).unwrap();
    let file = path.join(format!("proposals/{id}.json"));
    let original = fs::read(&file).unwrap();
    let reopened = files::Exchange::open(&path).unwrap();
    assert_eq!(reopened.summary().unwrap(), SUMMARY);
    assert_eq!(reopened.submit(proposal()).unwrap(), id);
    assert_eq!(fs::read_dir(path.join("proposals")).unwrap().count(), 1);
    assert_eq!(fs::read(&file).unwrap(), original);
    fs::write(&file, b"interrupted partial proposal").unwrap();
    assert!(reopened.submit(proposal()).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"interrupted partial proposal");
    assert!(files::Exchange::initialize(&path).is_ok());
    fs::write(path.join("AGENTS.md"), b"owner edit").unwrap();
    assert!(files::Exchange::initialize(&path).is_err());
    assert_eq!(fs::read(path.join("AGENTS.md")).unwrap(), b"owner edit");
}

#[test]
fn init_updates_unchanged_earlier_packaged_files_and_refuses_user_changes() {
    const AGENTS: &[u8] = include_bytes!("../../agent/AGENTS.md");
    const SKILL: &[u8] = include_bytes!("../../agent/skills/mailroom-assistant/SKILL.md");
    const OLD_AGENTS: &[u8] = include_bytes!("../../agent/superseded/021643c/AGENTS.md");
    const OLD_SKILL: &[u8] =
        include_bytes!("../../agent/superseded/021643c/skills/mailroom-assistant/SKILL.md");
    let (parent, _) = fixture();
    let path = parent.path().join("exchange");
    let agents = path.join("AGENTS.md");
    let skill = path.join("skills/mailroom-assistant/SKILL.md");
    fs::write(&agents, OLD_AGENTS).unwrap();
    fs::write(&skill, OLD_SKILL).unwrap();
    fs::write(path.join(".AGENTS.md.update"), b"interrupted update").unwrap();
    assert!(files::Exchange::initialize(&path).is_ok());
    assert_eq!(fs::read(&agents).unwrap(), AGENTS);
    assert_eq!(fs::read(&skill).unwrap(), SKILL);
    assert_eq!(
        fs::metadata(&agents).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert!(!path.join(".AGENTS.md.update").exists());
    assert!(
        files::Exchange::initialize(&path).is_ok(),
        "repeat is a no-op"
    );
    assert_eq!(fs::read(&agents).unwrap(), AGENTS);

    let mut edited = OLD_SKILL.to_vec();
    edited.extend(b"\nMy own note.\n");
    fs::write(&agents, OLD_AGENTS).unwrap();
    fs::write(&skill, &edited).unwrap();
    assert!(files::Exchange::initialize(&path).is_err());
    assert_eq!(fs::read(&skill).unwrap(), edited, "user change kept");
    assert_eq!(fs::read(&agents).unwrap(), AGENTS, "unchanged file updated");
    let mut trimmed = OLD_AGENTS.to_vec();
    trimmed.pop();
    fs::write(&agents, &trimmed).unwrap();
    assert!(files::Exchange::initialize(&path).is_err());
    assert_eq!(fs::read(&agents).unwrap(), trimmed);
}

#[test]
fn agent_changed_summaries_and_invalid_proposals_never_stage_files() {
    let (parent, exchange) = fixture();
    for alter in [
        |p: &mut Proposal| p.version = 2,
        |p: &mut Proposal| p.summary_sha256 = "0".repeat(64),
        |p: &mut Proposal| p.recommendations.clear(),
        |p: &mut Proposal| p.recommendations[0].title = " ".into(),
        |p: &mut Proposal| p.recommendations[0].reason = "x\0y".into(),
        |p: &mut Proposal| p.recommendations[0].suggested_change = "x".repeat(2001),
    ] {
        let mut invalid = proposal();
        alter(&mut invalid);
        assert!(exchange.submit(invalid).is_err());
    }
    fs::write(
        parent.path().join("exchange/reviewed/census-summary.md"),
        format!("{SUMMARY}changed"),
    )
    .unwrap();
    assert!(exchange.submit(proposal()).is_err());
    assert_eq!(
        fs::read_dir(parent.path().join("exchange/proposals"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn agent_linked_nonregular_and_public_files_are_refused() {
    use std::os::unix::fs::symlink;
    let (parent, _) = fixture();
    let path = parent.path().join("exchange");
    let summary = path.join("reviewed/census-summary.md");
    let target = parent.path().join("outside.txt");
    fs::rename(&summary, &target).unwrap();
    symlink(&target, &summary).unwrap();
    let exchange = files::Exchange::open(&path).unwrap();
    assert!(exchange.summary().is_err());
    fs::remove_file(&summary).unwrap();
    fs::hard_link(&target, &summary).unwrap();
    assert!(exchange.summary().is_err());
    fs::remove_file(&summary).unwrap();
    fs::rename(&target, &summary).unwrap();
    fs::set_permissions(&summary, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(exchange.summary().is_err());
    fs::set_permissions(&summary, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&summary).unwrap();
    fs::create_dir(&summary).unwrap();
    assert!(exchange.summary().is_err());
    symlink(&path, parent.path().join("linked")).unwrap();
    assert!(files::Exchange::open(&parent.path().join("linked")).is_err());
    fs::rename(
        path.join("proposals"),
        parent.path().join("outside-proposals"),
    )
    .unwrap();
    symlink(
        parent.path().join("outside-proposals"),
        path.join("proposals"),
    )
    .unwrap();
    assert!(files::Exchange::open(&path).is_err());
}

fn transcript(exchange: files::Exchange, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = Vec::new();
    protocol::serve(exchange, Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn handshake() -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"invented-client","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ]
}

#[test]
fn agent_stdio_requires_initialization_and_reports_only_staged_counts() {
    let (parent, exchange) = fixture();
    assert!(!exchange.was_read());
    let mut requests = vec![
        json!({"jsonrpc":"2.0","id":0,"method":"tools/call","params":{"name":"read_reviewed_summary","arguments":{}}}),
    ];
    requests.extend(handshake());
    requests.extend([
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_reviewed_summary","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"submit_proposal","arguments":proposal()}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"delete_account","arguments":{}}}),
    ]);
    let responses = transcript(exchange, &requests);
    assert!(responses[0]["error"].is_object());
    assert_eq!(responses[1]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(responses[2]["result"]["tools"].as_array().unwrap().len(), 2);
    let disclosed: Value = serde_json::from_str(
        responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(disclosed["summary"], SUMMARY);
    let staged: Value = serde_json::from_str(
        responses[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(staged["status"], "staged_for_local_review");
    assert_eq!(staged["recommendations"], 1);
    assert_eq!(staged["rule_edits"], 0);
    assert_eq!(staged.as_object().unwrap().len(), 4);
    assert_eq!(responses[5]["result"]["isError"], true);
    assert!(
        files::Exchange::open(&parent.path().join("exchange"))
            .unwrap()
            .was_read()
    );
    assert_eq!(
        fs::read_dir(parent.path().join("exchange/proposals"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn agent_malformed_requests_do_not_echo_inputs_or_read_arbitrary_paths() {
    let (_, exchange) = fixture();
    let mut requests = handshake();
    requests.extend([
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_reviewed_summary","arguments":{"path":"DUMMY_PRIVATE_PATH"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"submit_proposal","arguments":{"password":"DUMMY_SECRET"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"file:///DUMMY_PRIVATE_PATH"}}),
    ]);
    let responses = transcript(exchange, &requests);
    let output = serde_json::to_string(&responses).unwrap();
    assert!(!output.contains("DUMMY_"));
    assert_eq!(responses[1]["result"]["isError"], true);
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(responses[3]["error"]["code"], -32601);
}

#[test]
fn agent_oversized_and_malformed_frames_are_bounded() {
    let (_, exchange) = fixture();
    let mut output = Vec::new();
    protocol::serve(exchange, Cursor::new(b"not JSON\n{}\n"), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("-32700"));
    assert!(output.contains("-32600"));
    let (_, exchange) = fixture();
    assert_eq!(
        protocol::serve(
            exchange,
            Cursor::new(vec![b'x'; REQUEST_LIMIT + 1]),
            Vec::new()
        ),
        Err("agent_request_too_large")
    );
}

#[test]
fn agent_rule_edits_stage_only_through_the_closed_schema() {
    let (parent, exchange) = fixture();
    let mut edited = serde_json::to_value(proposal()).unwrap();
    edited["rule_edits"] = json!({"patches":[{"id":"topic.travel","folder":"Trips"}],"additions":[{"id":"user.agent.receipts","category":"orders","stage":"subject","when":[{"kind":"subject_contains","value":"receipt"}]}]});
    let mut smuggled = edited.clone();
    smuggled["rule_edits"]["forwards"] = json!([]);
    let mut empty = edited.clone();
    empty["rule_edits"] = json!({});
    let mut requests = handshake();
    requests.extend([
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"submit_proposal","arguments":smuggled}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"submit_proposal","arguments":empty}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"submit_proposal","arguments":edited}}),
    ]);
    let responses = transcript(exchange, &requests);
    let schema = &responses[1]["result"]["tools"][1]["inputSchema"];
    assert!(schema["properties"]["rule_edits"].is_object());
    assert!(schema.to_string().contains("\"additions\""));
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(responses[3]["result"]["isError"], true);
    let staged: Value = serde_json::from_str(
        responses[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(staged["rule_edits"], 2);
    assert_eq!(
        fs::read_dir(parent.path().join("exchange/proposals"))
            .unwrap()
            .count(),
        1
    );
}
