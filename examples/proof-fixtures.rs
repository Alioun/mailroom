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
    ] {
        let case_dir = directory.join(variant);
        fs::create_dir_all(&case_dir).unwrap();
        let mut source = source.clone();
        match variant {
            "german-read" => source["documents"]["override_yaml"] = format!("{}\nlayout:\n  language: de\n  personal_root: Beispiel\n  business_root: Büro\n  new_addresses: Neue Adressen\n  retired_addresses: Alte Adressen\n", source["documents"]["override_yaml"].as_str().unwrap().replace("    folder: Studio", "    folder: Stille\n    mark_read: true")).into(),
            "unknown-trust" => source["context"]["recipient_evidence"] = "unknown".into(),
            "gmail-backlog" => source["context"]["engine"] = "gmail_backlog".into(),
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
        "Synthetic artifacts generated. Five variants, 70 independent messages. Artifact {}.",
        proof.artifact.identity()
    );
}
