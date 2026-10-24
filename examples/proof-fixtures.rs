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
        "Synthetic artifacts generated. Twelve variants, 200 independent messages. Artifact {}.",
        proof.artifact.identity()
    );
}
