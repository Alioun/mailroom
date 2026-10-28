use super::*;
use crate::acquisition::Provider;
use crate::{
    census::{AccountId, EncryptedStore, LocalSecret},
    engine::PreviewInput,
    rules::workspace::Workspace,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
fn entry(address: &str) -> Entry {
    serde_json::from_value(serde_json::json!({"address":address,"service":"Synthetic distinct service","folder":"Purchases","first_seen":"owner supplied historical value","status":"active","note":"private synthetic note","owner_extension":{"keep":[1,"two"]}})).unwrap()
}
fn state(root: &std::path::Path, id: &str) -> (EncryptedStore, AccountState) {
    let account = AccountId::new(id).unwrap();
    let store = EncryptedStore::new(
        root.join(id),
        CensusKeys::derive(&LocalSecret::from_bytes(vec![7; 32]).unwrap(), &account),
    )
    .unwrap();
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
    source["context"]["account"] = id.into();
    let input: PreviewInput = serde_json::from_value(source).unwrap();
    let census = crate::engine::synthetic_census(&input).unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: workspace::WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: input.as_of,
            baseline: crate::rules::proof::Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
    (store, state)
}
#[test]
fn private_file_roundtrip_preconditions_unknown_fields_and_unsafe_paths() {
    let root = tempfile::tempdir().unwrap();
    let adapter = InstalledLedger::install(root.path().join("private")).unwrap();
    let lock = adapter.lock().unwrap();
    let initial = lock.read().unwrap();
    assert!(!initial.document.enabled);
    let mut doc = initial.document;
    doc.entries.push(entry("Case+tag@signup.example.test"));
    doc.extra
        .insert("owner_extension".into(), serde_json::json!({"nested":true}));
    let saved = lock.replace(&initial.content_version, &doc).unwrap();
    assert_eq!(
        saved.document.entries[0].address,
        "Case+tag@signup.example.test"
    );
    assert!(adapter.lock().is_err());
    assert_eq!(
        fs::metadata(root.path().join("private"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(root.path().join("private/addresses.yaml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(lock.replace("stale", &doc).is_err());
    doc.entries.push(entry("case+tag@signup.example.test"));
    assert!(lock.replace(&saved.content_version, &doc).is_err());
    doc.entries.pop();
    doc.entries[0].folder = "bad/path".into();
    assert!(lock.replace(&saved.content_version, &doc).is_err());
    let bytes = fs::read(root.path().join("private/addresses.yaml")).unwrap();
    assert!(
        bytes
            .windows(b"owner_extension".len())
            .any(|w| w == b"owner_extension")
    );
    fs::rename(
        root.path().join("private/addresses.yaml"),
        root.path().join("original"),
    )
    .unwrap();
    symlink(
        root.path().join("original"),
        root.path().join("private/addresses.yaml"),
    )
    .unwrap();
    assert!(lock.read().is_err());
    assert!(
        lock.replace(&saved.content_version, &saved.document)
            .is_err()
    );
    drop(lock);
    fs::remove_file(root.path().join("private/addresses.lock")).unwrap();
    symlink(
        root.path().join("original"),
        root.path().join("private/addresses.lock"),
    )
    .unwrap();
    assert!(adapter.lock().is_err());
}
#[test]
fn shared_boundary_coordinates_independent_accounts_and_reinstallations() {
    let root = tempfile::tempdir().unwrap();
    let one = InstalledLedger::install(root.path().join("private")).unwrap();
    let two = InstalledLedger::install(root.path().join("private")).unwrap();
    let guard = one.lock().unwrap();
    assert!(two.lock().is_err());
    let s = guard.read().unwrap();
    let mut d = s.document;
    d.entries.push(entry("first@signup.example.test"));
    let saved = guard.replace(&s.content_version, &d).unwrap();
    drop(guard);
    let guard = two.lock().unwrap();
    assert!(
        guard
            .replace(&s.content_version, &Document::default())
            .is_err()
    );
    assert_eq!(guard.read().unwrap().content_version, saved.content_version);
    drop(guard);
    fs::rename(root.path().join("private"), root.path().join("moved")).unwrap();
    fs::create_dir(root.path().join("private")).unwrap();
    assert!(one.lock().is_err());
}
#[test]
fn exact_review_binds_account_changes_semantics_and_preserves_independent_state() {
    let root = tempfile::tempdir().unwrap();
    let adapter = InstalledLedger::install(root.path().join("private")).unwrap();
    let guard = adapter.lock().unwrap();
    let snapshot = guard.read().unwrap();
    let (store, mut saved) = state(root.path(), "first");
    let (_, other) = state(root.path(), "second");
    reconcile(
        &mut saved,
        &snapshot,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
    )
    .unwrap();
    let before = serde_json::to_value(&saved).unwrap();
    let batch = Batch {
        content_version: snapshot.content_version.clone(),
        revision: saved.revision,
        enabled: true,
        entries: vec![entry("ledger@signup.example.test")],
    };
    let reviewed = review(
        &saved,
        &snapshot,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &batch,
    )
    .unwrap();
    assert_eq!(
        review(
            &saved,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .unwrap()
        .receipt,
        reviewed.receipt
    );
    let mut changed = batch.clone();
    changed.entries[0].note.push_str(" changed");
    assert_ne!(
        review(
            &saved,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &changed
        )
        .unwrap()
        .receipt,
        reviewed.receipt
    );
    assert!(
        review(
            &other,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .is_ok()
    );
    assert_ne!(
        review(
            &other,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .unwrap()
        .receipt,
        reviewed.receipt
    );
    let written = guard
        .replace(&snapshot.content_version, &reviewed.resulting_document)
        .unwrap();
    reconcile(
        &mut saved,
        &written,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
    )
    .unwrap();
    store.save_state(&mut saved).unwrap();
    assert!(
        review(
            &saved,
            &written,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .is_err()
    );
    let once = serde_json::to_vec(&saved).unwrap();
    assert!(
        !reconcile(
            &mut saved,
            &written,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg)
        )
        .unwrap()
    );
    assert_eq!(serde_json::to_vec(&saved).unwrap(), once);
    let after = serde_json::to_value(&saved).unwrap();
    for key in [
        "census",
        "invoices",
        "unsubscribe",
        "backlog",
        "exports",
        "synthetic_effects",
    ] {
        assert_eq!(before[key], after[key]);
    }
    let (_, old) = config::documents(
        before["workspace"]["input"]["documents"]["preset_yaml"]
            .as_str()
            .unwrap(),
        before["workspace"]["input"]["documents"]["override_yaml"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let w = saved.workspace.as_ref().unwrap();
    let (_, new) = config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&old.patches).unwrap(),
        serde_json::to_value(&new.patches).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&old.targets).unwrap(),
        serde_json::to_value(&new.targets).unwrap()
    );
    assert_eq!(old.ledger.len() + 1, new.ledger.len());
    let mut bypass = w.input.clone();
    bypass.documents.override_yaml = bypass
        .documents
        .override_yaml
        .replace("Purchases", "Altered");
    assert!(validate_workspace_edit(&saved, &bypass).is_err());
}
#[test]
fn recovery_uses_newest_owner_file_and_invalidates_proof_without_prior_refresh() {
    let root = tempfile::tempdir().unwrap();
    let adapter = InstalledLedger::install(root.path().join("private")).unwrap();
    let guard = adapter.lock().unwrap();
    let initial = guard.read().unwrap();
    let (store, mut saved) = state(root.path(), "first");
    reconcile(
        &mut saved,
        &initial,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
    )
    .unwrap();
    store.save_state(&mut saved).unwrap();
    let proof = workspace::preview(
        &saved,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let mut d = initial.document;
    d.enabled = true;
    d.entries.push(entry("ledger@signup.example.test"));
    guard.replace(&initial.content_version, &d).unwrap();
    // Simulates interruption before the encrypted projection is saved, then a newer owner edit.
    d.entries[0].note = "newer owner edit".into();
    fs::write(
        root.path().join("private/addresses.yaml"),
        encode(&d).unwrap(),
    )
    .unwrap();
    let mut recovered = store.load_state().unwrap();
    reconcile(
        &mut recovered,
        &guard.read().unwrap(),
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
    )
    .unwrap();
    assert!(
        workspace::confirm_export(
            &mut recovered,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &proof.receipt
        )
        .is_err()
    );
    assert_eq!(
        recovered.ledger.document.as_ref().unwrap().entries[0].note,
        "newer owner edit"
    );
    store.save_state(&mut recovered).unwrap();
    assert_eq!(store.load_state().unwrap().ledger.managed.len(), 1);
}
#[test]
fn conflicting_independent_entries_and_original_approvals_are_not_claimed() {
    let root = tempfile::tempdir().unwrap();
    let (store, saved) = state(root.path(), "first");
    let (_, overrides) = config::documents(
        &saved
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .preset_yaml,
        &saved
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .override_yaml,
    )
    .unwrap();
    let existing = &overrides.ledger[0];
    let mut d = Document::default();
    d.enabled = true;
    d.entries.push(entry(&existing.address));
    let snapshot = Snapshot {
        content_version: "source".into(),
        document: d,
    };
    assert!(
        reconcile(
            &mut saved.clone(),
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg)
        )
        .is_err()
    );
    let mut e = entry("ledger@signup.example.test");
    e.association = Some(Association {
        sender_domains: BTreeSet::from(["example.org".into()]),
        approved_evidence: "original explicit owner evidence".into(),
    });
    let snapshot = Snapshot {
        content_version: "another".into(),
        document: Document {
            entries: vec![e.clone()],
            ..Document::default()
        },
    };
    let mut batch = Batch {
        content_version: snapshot.content_version.clone(),
        revision: saved.revision,
        enabled: false,
        entries: vec![e],
    };
    batch.entries[0]
        .association
        .as_mut()
        .unwrap()
        .sender_domains
        .insert("example.net".into());
    assert!(
        review(
            &saved,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .is_err()
    );
    batch.entries[0] = snapshot.document.entries[0].clone();
    batch.entries[0].first_seen = "changed".into();
    assert!(
        review(
            &saved,
            &snapshot,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &batch
        )
        .is_err()
    );
}
#[test]
fn discovery_is_header_specific_unknown_and_dates_are_not_observation_times() {
    let root = tempfile::tempdir().unwrap();
    let (_, mut saved) = state(root.path(), "first");
    let mut doc = Document::default();
    let mut e = entry("invoices@work.example.test");
    e.association = Some(Association {
        sender_domains: BTreeSet::from(["unrelated.example".into()]),
        approved_evidence: "explicit synthetic association".into(),
    });
    doc.entries.push(e);
    let report = view(
        &saved,
        Snapshot {
            content_version: "source".into(),
            document: doc.clone(),
        },
        false,
    );
    assert!(!report.proposals.is_empty());
    assert!(!report.leaks.is_empty());
    for p in &report.proposals {
        assert_eq!(p.ownership, "unknown");
        assert_eq!(p.role, "unknown");
        assert!(p.service.is_none());
        assert!(p.first_seen_meaning.contains("not first receipt"));
        assert!(
            p.evidence
                .iter()
                .all(|e| e.delivery_authority.starts_with("unknown"))
        );
    }
    assert!(
        report
            .proposals
            .iter()
            .any(|p| p.evidence.iter().any(|e| e.recipient_header == "to"))
    );
    let association = serde_json::to_value(&doc.entries[0].association).unwrap();
    let _ = view(
        &saved,
        Snapshot {
            content_version: "source".into(),
            document: doc.clone(),
        },
        false,
    );
    assert_eq!(
        serde_json::to_value(&doc.entries[0].association).unwrap(),
        association
    );
    saved.census.acquisition_state = ScanState::Cancelled;
    assert_eq!(
        view(
            &saved,
            Snapshot {
                content_version: "source".into(),
                document: doc
            },
            false
        )
        .census_state,
        ScanState::Cancelled
    );
}
#[test]
fn shared_file_process_probe() {
    let Some(root) = std::env::var_os("MAILROOM_LEDGER_LOCK_PROBE") else {
        return;
    };
    assert!(
        InstalledLedger::install(std::path::PathBuf::from(root).join("ledger")).is_err(),
        "Another process must hold the authoritative boundary"
    );
}
#[test]
fn unsupported_yaml_and_symlinked_directory_replacements_leave_owner_bytes_intact() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("private");
    let adapter = InstalledLedger::install(path.clone()).unwrap();
    let guard = adapter.lock().unwrap();
    for invalid in [
        "version: 2\nenabled: false\nentries: []\n",
        "version: 1\nversion: 9\nenabled: false\nentries: []\n",
        "version: 1\nenabled: false\nentries: []\ncustom: !unsupported tagged\n",
    ] {
        fs::write(path.join("addresses.yaml"), invalid).unwrap();
        assert!(guard.read().is_err());
        assert_eq!(
            fs::read_to_string(path.join("addresses.yaml")).unwrap(),
            invalid
        );
    }
    drop(guard);
    fs::rename(&path, root.path().join("moved")).unwrap();
    symlink(root.path().join("moved"), &path).unwrap();
    assert!(adapter.lock().is_err());
}
#[test]
fn ledger_change_blocks_old_sorting_plan_without_starting_effects_or_undo() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut saved) = state(root.path(), "first");
    let mut mailbox =
        crate::backlog::SyntheticMailbox::open(&root.path().join("mailbox"), &store, &saved.census)
            .unwrap();
    let preview = crate::backlog::preview(
        &saved,
        Provider::MailboxOrg,
        store.keys(),
        crate::backlog::Options::default(),
        Some(&mailbox),
    )
    .unwrap();
    let receipt = preview.receipt.clone();
    let id = crate::backlog::start(&mut saved, preview, &receipt).unwrap();
    store.save_state(&mut saved).unwrap();
    let original = serde_json::to_value(&mailbox.snapshot().census).unwrap();
    saved.ledger.source_version = "external-ledger-edit".into();
    assert!(crate::backlog::advance(&store, &mut saved, &mut mailbox, &id).is_err());
    assert!(crate::backlog::resume(&mut saved, &id).is_err());
    assert_eq!(saved.backlog[0].batches, 0);
    assert!(!saved.backlog[0].undo_requested);
    crate::backlog::stop(&mut saved, &id).unwrap();
    assert_eq!(saved.backlog[0].status, crate::backlog::Status::Stopped);
    assert_eq!(
        serde_json::to_value(&mailbox.snapshot().census).unwrap(),
        original
    );
}

#[test]
fn unsafe_parent_is_refused_before_creating_ledger() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let path = root.path().join("private");
    assert!(InstalledLedger::install(path.clone()).is_err());
    assert!(!path.exists());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(InstalledLedger::install(path).is_ok());
}

#[cfg(target_os = "macos")]
#[test]
fn granting_acls_are_refused_on_directory_lock_and_ledger_but_deny_only_is_allowed() {
    fn chmod(path: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("/bin/chmod")
            .args(args)
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success(), "dummy ACL fixture command failed");
    }
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let installed = InstalledLedger::install(directory.clone()).unwrap();
    for path in [
        directory.clone(),
        directory.join("addresses.lock"),
        directory.join("addresses.yaml"),
    ] {
        chmod(&path, &["+a", "everyone allow read"]);
        assert!(
            installed.lock().and_then(|g| g.read()).is_err(),
            "granting ACL accepted"
        );
        assert!(
            InstalledLedger::install(directory.clone()).is_err(),
            "reinstallation accepted granting ACL"
        );
        chmod(&path, &["-N"]);
        assert!(installed.lock().and_then(|g| g.read()).is_ok());
        chmod(&path, &["+a", "everyone deny delete"]);
        assert!(
            installed.lock().and_then(|g| g.read()).is_ok(),
            "deny-only ACL refused"
        );
        chmod(&path, &["-N"]);
    }
    chmod(root.path(), &["+a", "everyone allow read"]);
    assert!(InstalledLedger::install(root.path().join("second")).is_err());
    assert!(!root.path().join("second").exists());
    chmod(root.path(), &["-N"]);
}
