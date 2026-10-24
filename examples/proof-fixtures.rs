use mailroom::{
    engine::{self, PreviewInput},
    rules::config,
};
use serde_json::json;
use std::{fs, path::PathBuf};
fn main() {
    let directory = PathBuf::from(std::env::args().nth(1).expect("synthetic output directory"));
    fs::create_dir_all(&directory).unwrap();
    let source: serde_json::Value = serde_json::from_str(include_str!("sample.json")).unwrap();
    let input: PreviewInput = serde_json::from_value(source.clone()).unwrap();
    let proof = engine::preview(&input).unwrap();
    fs::write(
        directory.join("proof.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    let schema = json!({"preset": schemars::schema_for!(config::Preset), "overrides": schemars::schema_for!(config::Overrides)});
    fs::write(
        directory.join("schema.json"),
        serde_json::to_vec_pretty(&schema).unwrap(),
    )
    .unwrap();
    for variant in [
        "ledger-projection",
        "standard",
        "german-read",
        "unknown-trust",
        "gmail-backlog",
        "limited",
        "overlap",
        "aliases",
        "distinct",
        "mute-exact",
        "mute-no-flags",
        "mute-unknown",
        "mute-from",
    ] {
        let case_dir = directory.join(variant);
        fs::create_dir_all(&case_dir).unwrap();
        let mut source = source.clone();
        match variant {
            "ledger-projection" => {
                let input: PreviewInput = serde_json::from_value(source.clone()).unwrap();
                let root = tempfile::tempdir().unwrap();
                let adapter = mailroom::ledger::InstalledLedger::install(root.path().join("private")).unwrap();
                let guard = adapter.lock().unwrap();
                let initial = guard.read().unwrap();
                let mut document = initial.document;
                document.enabled = true;
                for (address, status, invoice) in [("active@home.example.test", "active", false), ("retired@home.example.test", "retired", false), ("retired-invoice@work.example.test", "retired", true)] {
                    document.entries.push(serde_json::from_value(json!({"address":address,"service":"Synthetic service", "folder":"Ledger filing","first_seen":"synthetic historical value","note":"synthetic only","status":status,"category":"orders","invoice":invoice})).unwrap());
                }
                let snapshot = guard.replace(&initial.content_version, &document).unwrap();
                let account = mailroom::census::AccountId::new(&input.context.account).unwrap();
                let store = mailroom::census::EncryptedStore::new(root.path().join("account"), mailroom::census::CensusKeys::derive(&mailroom::census::LocalSecret::random(), &account)).unwrap();
                store.save(&mailroom::engine::synthetic_census(&input).unwrap()).unwrap();
                let mut state = store.load_state().unwrap();
                state.workspace = Some(mailroom::rules::workspace::Workspace {revision:1,input:mailroom::rules::workspace::WorkspaceInput{documents:input.documents,pins:vec![],as_of:input.as_of,baseline:mailroom::rules::proof::Baseline::Current}});
                let w = &mut state.workspace.as_mut().unwrap().input.documents;
                let (_, mut overrides) = config::documents(&w.preset_yaml, &w.override_yaml).unwrap();
                overrides.additions.insert(0, serde_json::from_value(json!({"id":"user.mute.ledger","category":"newsletters","stage":"from","folder":"Quiet","mark_read":true,"when":[{"kind":"list_id_exact","value":"<ledger-muted.example.test>"}]})).unwrap());
                w.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
                mailroom::ledger::reconcile(&mut state,&snapshot,mailroom::acquisition::Provider::MailboxOrg).unwrap();
                source["documents"]["override_yaml"] = state.workspace.unwrap().input.documents.override_yaml.into();
                for (id, recipient, subject, list) in [
                    ("ledger-active","active@home.example.test","Order",""),
                    ("ledger-retired","retired@home.example.test","Order",""),
                    ("ledger-invoice","retired-invoice@work.example.test","Invoice",""),
                    ("ledger-guard","active@home.example.test","Your login code",""),
                    ("ledger-override","active@home.example.test","Release","List-Id: <studio.example.net>\n"),
                    ("ledger-mute","active@home.example.test","Order","List-Id: <ledger-muted.example.test>\n"),
                    ("ledger-mixed","active@home.example.test, team@work.example.test","Order",""),
                    ("ledger-unknown","active@unknown.example.test","Order",""),
                    ("ordinary-classified","other@work.example.test","Invoice",""),
                ] {
                    let sender = if id == "ledger-invoice" { "invoicing@separate.example.net" } else { "billing@shop.example.org" };
                    source["messages"].as_array_mut().unwrap().push(json!({"id":id,"headers":format!("From: {sender}\nDelivered-To: {recipient}\nTo: {recipient}\nSubject: {subject}\n{list}Message-ID: <{id}@example.test>\nDate: Sat, 24 Oct 2026 00:00:00 +0000"),"current_folder":"INBOX"}));
                }
            },
            "german-read" => source["documents"]["override_yaml"] = format!("{}\nlayout:\n  language: de\n  personal_root: Beispiel\n  business_root: Büro\n  new_addresses: Neue Adressen\n  retired_addresses: Alte Adressen\n", source["documents"]["override_yaml"].as_str().unwrap().replace("    folder: Studio", "    folder: Stille\n    mark_read: true")).into(),
            "unknown-trust" => source["context"]["recipient_evidence"] = "unknown".into(),
            "gmail-backlog" => source["context"]["engine"] = "gmail_backlog".into(),
            "overlap" | "aliases" | "distinct" => {
                source["messages"][2]["headers"] = source["messages"][2]["headers"].as_str().unwrap().replace("Delivered-To: team@work.example.test", "Delivered-To: invoices@work.example.test").into();
                if variant == "aliases" || variant == "distinct" {
                    let yaml = source["documents"]["override_yaml"].as_str().unwrap().replace("  accounting: {address: archive@example.net}", "  accounting: {address: archive@example.net}\n  alias: {address: ARCHIVE@example.net}").replace("id: forward.invoice_address\n    target: accounting", "id: forward.invoice_address\n    target: alias");
                    source["documents"]["override_yaml"] = if variant == "distinct" { yaml.replace("ARCHIVE@example.net", "distinct@example.test") } else { yaml }.into();
                }
            },
            "mute-exact" | "mute-no-flags" | "mute-unknown" | "mute-from" => {
                let mut overrides: serde_json::Value = serde_yaml_ng::from_str(source["documents"]["override_yaml"].as_str().unwrap()).unwrap();
                let condition = if variant == "mute-from" { json!({"kind":"from_address", "value":"billing@shop.example.org"}) } else { json!({"kind":"list_id_exact", "value":"<letters.example.net>"}) };
                let conditions = if variant == "mute-from" { vec![condition, json!({"kind":"list_id_absent"})] } else { vec![condition] };
                overrides["additions"].as_array_mut().unwrap().insert(0, json!({"id":"user.mute.fixture", "category":"newsletters", "stage":"from", "folder":"Muted", "mark_read":true, "when":conditions}));
                source["documents"]["override_yaml"] = serde_yaml_ng::to_string(&overrides).unwrap().into();
                let additions = [
                    ("exact-list", "billing@shop.example.org", "team@work.example.test", "<letters.example.net>", "Invoice with list"),
                    ("other-list", "billing@shop.example.org", "team@work.example.test", "<other.example.net>", "Invoice other list"),
                    ("substring-list", "billing@shop.example.org", "team@work.example.test", "prefix<letters.example.net>suffix", "Invoice substring"),
                    ("unknown-role-list", "billing@shop.example.org", "unknown@work.example.test", "<letters.example.net>", "Invoice unknown"),
                    ("mixed-list", "billing@shop.example.org", "team@work.example.test, owner@home.example.test", "<letters.example.net>", "Invoice mixed"),
                    ("personal-list", "news@letters.example.net", "owner@home.example.test", "<letters.example.net>", "Personal mail"),
                    ("case-list", "news@letters.example.net", "owner@home.example.test", "<LETTERS.EXAMPLE.NET>", "ASCII casemap exact semantics"),
                    ("guard-list", "billing@shop.example.org", "team@work.example.test", "<letters.example.net>", "Invoice login code"),
                ];
                for (id, sender, recipient, list, subject) in additions {
                    source["messages"].as_array_mut().unwrap().push(json!({"id":id, "headers":format!("From: {sender}\nDelivered-To: {recipient}\nTo: {recipient}\nSubject: {subject}\nList-Id: {list}\nMessage-ID: <{id}@example.test>\nDate: Sat, 24 Oct 2026 00:00:00 +0000"), "current_folder":"INBOX"}));
                }
                if variant == "mute-no-flags" { source["context"]["capabilities"].as_array_mut().unwrap().retain(|v| v != "imap4flags"); }
                if variant == "mute-unknown" { source["context"]["recipient_evidence"] = "unknown".into(); }
            },
            "limited" => source["context"]["capabilities"] = json!([]), _ => (),
        }
        let input: PreviewInput = serde_json::from_value(source.clone()).unwrap();
        let artifact = engine::artifact(&input.documents, &input.context).unwrap();
        fs::write(case_dir.join("rules.sieve"), artifact.executable()).unwrap();
        let fixtures = source["messages"].as_array().unwrap().clone();
        let mut expected = Vec::new();
        for (i, fixture) in fixtures.iter().enumerate() {
            source["messages"] = json!([fixture]);
            source["pins"] = json!([]);
            source["baseline"] = "current".into();
            let input: PreviewInput = serde_json::from_value(source.clone()).unwrap();
            let proof = engine::preview(&input).unwrap();
            if variant == "ledger-projection" {
                let decision = &proof.decisions[0];
                let expected_folder = match fixture["id"].as_str().unwrap() {
                    "ledger-active" => Some("Personal/Ledger filing"),
                    "ledger-retired" => Some("Personal/Retired addresses"),
                    "ledger-invoice" => Some("Business/Retired addresses"),
                    "ledger-guard" | "ledger-mixed" | "ledger-unknown" => Some("INBOX"),
                    "ledger-override" => Some("Personal/Releases"),
                    "ledger-mute" => Some("Personal/Quiet"),
                    "ordinary-classified" => Some("Business/Money"),
                    _ => None,
                };
                if let Some(folder) = expected_folder {
                    let actual = serde_json::to_value(&decision.destination).unwrap();
                    assert_eq!(
                        actual
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("INBOX"),
                        folder,
                        "explicit ledger behavior"
                    );
                }
                if fixture["id"] == "ledger-mute" {
                    assert!(decision.mark_read);
                }
                if fixture["id"] == "ledger-invoice" {
                    assert_eq!(decision.copies.len(), 1);
                }
                if fixture["id"] == "ledger-mixed" || fixture["id"] == "ledger-unknown" {
                    assert!(decision.copies.is_empty());
                    assert!(decision.review);
                }
            }
            expected.push(serde_json::to_value(&proof.decisions[0]).unwrap());
            fs::write(
                case_dir.join(format!("{i}.eml")),
                format!(
                    "{}\r\n\r\nSynthetic body for isolated engine verification only.\r\n",
                    input.messages[0].headers.replace('\n', "\r\n")
                ),
            )
            .unwrap();
        }
        fs::write(
            case_dir.join("expected.json"),
            serde_json::to_vec_pretty(&expected).unwrap(),
        )
        .unwrap();
    }
    println!(
        "Synthetic artifacts generated, including private-file ledger projection. Artifact {}.",
        proof.artifact.identity()
    );
}
