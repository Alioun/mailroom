use super::*;
use crate::{
    census::{
        AccountState,
        delivery::{DeliveryEvidence, Source},
    },
    rules::{config, proof::Baseline, workspace},
};
use std::path::Path;

pub(crate) fn install(
    root: &Path,
) -> (
    Accounts,
    Account,
    crate::acquisition::folder_provisioning::tests::Fixture,
    crate::acquisition::sieve_transaction::tests::Fixture,
) {
    let (mut repo, account, folder) = crate::acquisition::folder_provisioning::tests::install(root);
    let sieve =
        crate::acquisition::sieve_transaction::tests::Fixture::start(root, &account.id, false);
    sieve.install(&mut repo, &account.id);
    let epoch = std::fs::read_to_string(root.join("envelope-authority"))
        .unwrap_or_else(|_| "delivery-fixture-v1".into());
    if !epoch.is_empty() {
        repo.install_synthetic_envelope(&account, &epoch);
    }
    if !root.join("envelope-seeded").exists() {
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        let context = repo.workspace_context(&account, &store).unwrap();
        let mut sample: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
        sample.context = context.engine(&account.id).unwrap();
        sample.messages.clear();
        for (i, (status, recipient)) in [
            ("known", "route@business.test"),
            ("known", "other@business.test"),
            ("unavailable", ""),
            ("unsupported", ""),
            ("ambiguous", ""),
            ("known", "route@unknown.test"),
        ]
        .iter()
        .enumerate()
        {
            let delivery = if *status == "known" {
                serde_json::json!({"status":status,"recipient":recipient})
            } else {
                serde_json::json!({"status":status})
            };
            sample.messages.push(serde_json::from_value(serde_json::json!({"id":format!("arrival-{i}"),"headers":format!("From: shop@sender.test\r\nTo: unrelated@personal.test\r\nDelivered-To: route@business.test\r\nX-Original-To: forged@personal.test\r\nSubject: Envelope {status} {i}\r\nDate: 24 Oct 2026 12:00:00 +0000\r\nMessage-ID: <delivery-{i}@example.test>\r\n\r\n"),"current_folder":"INBOX","delivery":delivery,"observed_flags":["\\Seen","\\Flagged","\\Answered","unrelated","$ROUTE"]})).unwrap());
        }
        sample.documents.override_yaml.push_str("\n");
        let (preset, mut overrides) = config::documents(
            &sample.documents.preset_yaml,
            &sample.documents.override_yaml,
        )
        .unwrap();
        overrides
            .roles
            .domains
            .insert("business.test".into(), config::Role::Business);
        overrides
            .roles
            .domains
            .insert("personal.test".into(), config::Role::Personal);
        overrides.additions.push(serde_json::from_value(serde_json::json!({"id":"user.envelope-filing","category":"orders","stage":"subject","when":[{"kind":"subject_contains","value":"Envelope"}],"folder":"Delivery"})).unwrap());
        sample.documents.preset_yaml = serde_yaml_ng::to_string(&preset).unwrap();
        sample.documents.override_yaml = serde_yaml_ng::to_string(&overrides)
            .unwrap()
            .replace("delivery_tags: []\n", "");
        saved.census = crate::engine::synthetic_census(&sample).unwrap().into();
        saved.workspace.as_mut().unwrap().input.documents = sample.documents;
        saved.workspace.as_mut().unwrap().input.pins.clear();
        saved.workspace.as_mut().unwrap().input.baseline = Baseline::Current;
        store.save_state(&mut saved).unwrap();
        std::fs::write(root.join("envelope-seeded"), b"synthetic-only").unwrap();
    }
    (repo, account, folder, sieve)
}

pub(crate) fn tag_yaml(account: &str) -> String {
    format!(
        "\ndelivery_tags:\n  - id: user.route\n    enabled: true\n    accounts: [{account}]\n    envelope_to: route@business.test\n    add_keyword: $route\n"
    )
}
fn digest(value: &impl serde::Serialize) -> String {
    crate::unsubscribe::digest(&serde_json::to_vec(&serde_json::to_value(value).unwrap()).unwrap())
}
fn preserved(saved: &AccountState) -> String {
    let mut value = serde_json::to_value(saved).unwrap();
    for key in [
        "workspace",
        "exports",
        "revision",
        "sieve",
        "sieve_transactions",
        "sieve_placement",
        "folder_provisioning",
    ] {
        value.as_object_mut().unwrap().remove(key);
    }
    digest(&value)
}
fn save_preserved(saved: &AccountState) -> String {
    let mut value = serde_json::to_value(saved).unwrap();
    for key in ["workspace", "exports", "revision"] {
        value.as_object_mut().unwrap().remove(key);
    }
    digest(&value)
}

#[test]
fn envelope_fixture_observer() {
    let Some(root) = std::env::var_os("MAILROOM_ENVELOPE_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let repo = crate::acquisition::persistent_test_accounts(root);
    let account = repo.list().unwrap().remove(0);
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    let external = repo
        .fixture_sieve_store(&account.id, root)
        .unwrap()
        .load_state()
        .unwrap();
    let target = external
        .synthetic_sieve
        .as_ref()
        .and_then(|p| p.image.body("Open-Xchange"));
    let value = serde_json::json!({"account":account.id,"schema":store.schema_version().unwrap(),"revision":saved.revision,"preserved":preserved(&saved),"save_preserved":save_preserved(&saved),"complete":digest(&saved),"exports":saved.exports.iter().map(|e|serde_json::json!({"identity":e.artifact.identity(),"semantics":e.semantics,"proof_semantics":e.proof_semantics,"bytes":digest(&e.artifact.executable())})).collect::<Vec<_>>(),"provider_target":target.map(|b| &b.sha256),"provider_bytes":target.map(|b| String::from_utf8_lossy(&b.bytes)),"provider_owned":external.synthetic_sieve.as_ref().and_then(|p| p.image.body("mailroom")).map(|b| String::from_utf8_lossy(&b.bytes)),"provider_foreign":external.synthetic_sieve.as_ref().and_then(|p| crate::census::sieve_transaction::foreign(&p.image, "Open-Xchange")).map(|f| digest(&f)),"commands":external.synthetic_sieve.as_ref().map(|p|&p.commands),"folder_commands":external.synthetic_folders.as_ref().map(|p|&p.commands),"independent_effects":crate::acquisition::sieve_transaction::tests::independent_effects(root,&store)});
    std::fs::write(
        root.join("envelope-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}
#[test]
fn saved_envelope_tags_share_proof_and_hold_unknown_without_header_authority() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _folder, _sieve) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let original = preserved(&saved);
    let context = repo.workspace_context(&account, &store).unwrap();
    let before = workspace::preview(&saved, &context, store.keys()).unwrap();
    saved
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml
        .push_str(&tag_yaml(account.id.as_str()));
    let proof = workspace::preview(&saved, &context, store.keys()).unwrap();
    assert_eq!(
        proof.proof.artifact.folders(),
        before.proof.artifact.folders()
    );
    assert_ne!(
        proof.proof.artifact.identity(),
        before.proof.artifact.identity()
    );
    assert_ne!(proof.receipt, before.receipt);
    assert_eq!(proof.proof.decisions[0].keyword_additions, ["$route"]);
    assert_eq!(proof.proof.decisions[0].delivery_tags[0].status, "match");
    assert_eq!(proof.proof.decisions[1].delivery_tags[0].status, "no_match");
    for i in 2..6 {
        assert!(proof.proof.decisions[i].review);
        assert!(proof.proof.decisions[i].copies.is_empty());
        assert!(proof.proof.decisions[i].keyword_additions.is_empty());
    }
    assert_eq!(proof.proof.decisions[5].delivery_tags[0].status, "no_match");
    let mut held = saved.clone();
    let docs = &mut held.workspace.as_mut().unwrap().input.documents;
    let (_, mut overrides) = config::documents(&docs.preset_yaml, &docs.override_yaml).unwrap();
    overrides
        .roles
        .addresses
        .insert("route@business.test".into(), config::Role::Unknown);
    docs.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    let held = workspace::preview(&held, &context, store.keys()).unwrap();
    assert_eq!(
        held.proof.decisions[0].delivery_tags[0].status,
        "held_tagged"
    );
    assert_eq!(held.proof.decisions[0].keyword_additions, ["$route"]);
    assert!(held.proof.decisions[0].review);
    assert!(!held.proof.decisions[0].mark_read);
    assert!(held.proof.decisions[0].copies.is_empty());
    assert_eq!(
        held.proof.decisions[0].destination,
        crate::rules::proof::Destination::Inbox
    );
    assert_eq!(
        held.proof.artifact.folders(),
        proof.proof.artifact.folders()
    );
    assert!(
        proof
            .proof
            .artifact
            .executable()
            .contains("addflag \"$route\"")
    );
    assert!(!proof.proof.artifact.executable().contains("setflag"));
    workspace::confirm_export(&mut saved, &context, store.keys(), &proof.receipt).unwrap();
    store.save_state(&mut saved).unwrap();
    assert_eq!(preserved(&store.load_state().unwrap()), original);
    assert_eq!(store.schema_version().unwrap(), crate::census::SCHEMA);
    let known = saved.census.folders["INBOX"].messages[&1].delivery.clone();
    let DeliveryEvidence::Known {
        source, message, ..
    } = &known
    else {
        panic!()
    };
    for field in [
        "account",
        "provider",
        "identity",
        "profile",
        "epoch",
        "authority",
        "message",
    ] {
        let mut changed: Source = source.clone();
        match field {
            "account" => changed.account.push('x'),
            "provider" => changed.provider.push('x'),
            "identity" => changed.identity.push('x'),
            "profile" => changed.profile.push('x'),
            "epoch" => changed.epoch.push('x'),
            "authority" => changed.authority.push('x'),
            _ => (),
        }
        assert_eq!(
            known
                .resolve(
                    Some(&changed),
                    if field == "message" { "wrong" } else { message }
                )
                .1,
            "incompatible"
        );
    }
}

#[test]
fn delivery_lifecycle_preserves_complete_state_and_invalidates_credential_and_key_authority() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, folder, sieve) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved
        .workspace
        .as_mut()
        .unwrap()
        .input
        .documents
        .override_yaml
        .push_str(&tag_yaml(account.id.as_str()));
    let context = repo.workspace_context(&account, &store).unwrap();
    let proof = workspace::preview(&saved, &context, store.keys()).unwrap();
    workspace::confirm_export(&mut saved, &context, store.keys(), &proof.receipt).unwrap();
    store.save_state(&mut saved).unwrap();
    let original = serde_json::to_value(&saved).unwrap();
    let effects =
        crate::acquisition::sieve_transaction::tests::independent_effects(root.path(), &store);
    let _ledger = crate::ledger::InstalledLedger::install(root.path().join("ledger")).unwrap();
    let ledger = std::fs::read(root.path().join("ledger/addresses.yaml")).unwrap();
    drop(store);
    drop(folder);
    drop(sieve);
    repo.replace_password(
        &account.id,
        &crate::acquisition::SecretValue::new(b"invented-replacement".to_vec()),
    )
    .unwrap();
    let current = repo.load(&account.id).unwrap();
    let store = repo.store(&current).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(digest(&after), digest(&original));
    let p = workspace::preview(
        &after,
        &repo.workspace_context(&current, &store).unwrap(),
        store.keys(),
    )
    .unwrap();
    assert!(
        p.proof
            .decisions
            .iter()
            .all(|d| d.review && d.keyword_additions.is_empty() && d.copies.is_empty())
    );
    assert_eq!(
        crate::acquisition::sieve_transaction::tests::independent_effects(root.path(), &store),
        effects
    );
    drop(store);
    repo.rotate(&account.id, &root.path().join("mailboxes"))
        .unwrap();
    let current = repo.load(&account.id).unwrap();
    let store = repo.store(&current).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(after.revision, original["revision"].as_u64().unwrap() + 1);
    let mut expected = original.clone();
    expected["revision"] = serde_json::json!(after.revision);
    assert_eq!(digest(&after), digest(&expected));
    let p = workspace::preview(
        &after,
        &repo.workspace_context(&current, &store).unwrap(),
        store.keys(),
    )
    .unwrap();
    assert!(
        p.proof
            .decisions
            .iter()
            .all(|d| d.review && d.keyword_additions.is_empty())
    );
    assert_eq!(
        crate::acquisition::sieve_transaction::tests::independent_effects(root.path(), &store),
        effects
    );
    drop(store);
    let preview = repo
        .forget_preview(&account.id, &root.path().join("mailboxes"))
        .unwrap();
    repo.forget(&account.id, &root.path().join("mailboxes"), &preview, true)
        .unwrap();
    assert!(repo.load(&account.id).is_err());
    assert_eq!(
        std::fs::read(root.path().join("ledger/addresses.yaml")).unwrap(),
        ledger
    );
}
