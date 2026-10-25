use super::*;
use crate::{
    census::{AccountId, EncryptedStore, LocalSecret},
    engine,
};

fn fixture() -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let directory = tempfile::tempdir().unwrap();
    let account = AccountId::new("proposal-fixture").unwrap();
    let store = EncryptedStore::new(
        directory.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    let mut input: engine::PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.context.account = account.as_str().into();
    input.messages = vec![
        engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: "order-one".into(), current_folder: "INBOX".into(), headers: "From: robot@shop.example\r\nTo: owner@example.test\r\nSubject: order confirmation\r\nAuto-Submitted: auto-generated\r\nMessage-ID: <one@example.test>\r\n\r\n".into() },
        engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: "order-two".into(), current_folder: "INBOX".into(), headers: "From: robot@shop.example\r\nTo: owner@example.test\r\nSubject: order receipt\r\nAuto-Submitted: auto-generated\r\nMessage-ID: <two@example.test>\r\n\r\n".into() },
        engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: "travel".into(), current_folder: "INBOX".into(), headers: "From: robot@travel.example\r\nSubject: booking confirmed\r\nAuto-Submitted: auto-generated\r\nMessage-ID: <three@example.test>\r\n\r\n".into() },
    ];
    let census = engine::synthetic_census(&input).unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(workspace::Workspace {
        revision: 1,
        input: workspace::WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: input.as_of,
            baseline: proof::Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
    (directory, store, state)
}
fn selection(g: &Generation, indices: &[usize]) -> Selection {
    Selection {
        account: g.account.clone(),
        generation: g.generation.clone(),
        candidates: indices
            .iter()
            .map(|i| Selected {
                id: g.candidates[*i].id.clone(),
                change: g.candidates[*i].change.clone(),
            })
            .collect(),
    }
}
fn encoded(v: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

#[test]
fn deterministic_exact_rejection_survives_encrypted_reopen_and_material_changes() {
    let (_root, store, mut state) = fixture();
    let first = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_eq!(first.candidates.len(), 2);
    let again = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_eq!(encoded(&first), encoded(&again));
    let selected = selection(&first, &[0]);
    let review = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selected,
        Disposition::Reject,
    )
    .unwrap();
    let documents = encoded(&state.workspace);
    confirm(
        &mut state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selected,
        Disposition::Reject,
        &review.receipt,
    )
    .unwrap();
    assert_eq!(encoded(&state.workspace), documents);
    store.save_state(&mut state).unwrap();
    let state = store.load_state().unwrap();
    let reopened = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_eq!(reopened.generation, first.generation);
    assert!(reopened.candidates[0].rejected);
    assert!(!reopened.candidates[1].rejected);
    assert!(review_rejected(&state, store.keys(), &selected));
    let mut changed = state.clone();
    changed
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .subject
        .push_str(" changed");
    let fresh = generate(
        &changed,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_ne!(fresh.candidates[0].id, first.candidates[0].id);
    assert!(!fresh.candidates[0].rejected);
    assert_eq!(fresh.candidates[0].prior_rejections, 1);
    assert_eq!(changed.proposals.history.len(), 1);
}
fn review_rejected(state: &AccountState, keys: &CensusKeys, s: &Selection) -> bool {
    review(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        s,
        Disposition::Accept,
    )
    .is_err()
}

#[test]
fn exact_selected_batch_preserves_all_other_state_and_requires_fresh_proof() {
    let (_root, store, mut state) = fixture();
    let old_proof = workspace::preview(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let old = state.clone();
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let selected = selection(&g, &[1]);
    let reviewed = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selected,
        Disposition::Accept,
    )
    .unwrap();
    assert!(reviewed.acceptance_available);
    assert_eq!(
        reviewed.original_documents.preset_yaml,
        reviewed.resulting_documents.preset_yaml
    );
    confirm(
        &mut state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selected,
        Disposition::Accept,
        &reviewed.receipt,
    )
    .unwrap();
    store.save_state(&mut state).unwrap();
    let state = store.load_state().unwrap();
    let (_, overrides) = config::documents(
        &state
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .preset_yaml,
        &state
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .override_yaml,
    )
    .unwrap();
    assert!(
        overrides
            .additions
            .iter()
            .any(|r| r.id == selected.candidates[0].change.target())
    );
    assert!(
        !overrides
            .additions
            .iter()
            .any(|r| r.id == g.candidates[0].change.target())
    );
    let mut before = encoded(&old);
    let mut after = encoded(&state);
    for v in [&mut before, &mut after] {
        for key in ["revision", "workspace", "proposals"] {
            v.as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(before, after);
    let fresh = workspace::preview(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_ne!(fresh.receipt, old_proof.receipt);
    assert!(
        fresh
            .proof
            .decisions
            .iter()
            .all(|d| d.destination == proof::Destination::Inbox && d.copies.is_empty())
    );
    let mut replay = state.clone();
    assert!(
        confirm(
            &mut replay,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &selected,
            Disposition::Accept,
            &reviewed.receipt
        )
        .is_err()
    );
    assert!(
        workspace::confirm_export(
            &mut replay,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &old_proof.receipt
        )
        .is_err()
    );
}

#[test]
fn stale_tamper_cross_purpose_cross_account_and_empty_selections_fail() {
    let (_root, store, state) = fixture();
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let s = selection(&g, &[0]);
    let r = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &s,
        Disposition::Accept,
    )
    .unwrap();
    let attempt = |state: &AccountState, selected: &Selection, receipt: &str, disposition| {
        confirm(
            &mut state.clone(),
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            selected,
            disposition,
            receipt,
        )
        .is_err()
    };
    assert!(attempt(&state, &s, &r.receipt, Disposition::Reject));
    assert!(attempt(&state, &s, &"0".repeat(64), Disposition::Accept));
    let proof = workspace::preview(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert!(attempt(&state, &s, &proof.receipt, Disposition::Accept));
    for variant in 0..5 {
        let mut s = s.clone();
        match variant {
            0 => s.candidates.clear(),
            1 => s.candidates.push(s.candidates[0].clone()),
            2 => s.candidates[0].id = "0".repeat(64),
            3 => s.account = "other-account".into(),
            _ => {
                if let Change::Addition { rule } = &mut s.candidates[0].change {
                    rule.mark_read = true;
                }
            }
        }
        assert!(attempt(&state, &s, &r.receipt, Disposition::Accept));
    }
    for variant in 0..8 {
        let mut state = state.clone();
        match variant {
            0 => state
                .workspace
                .as_mut()
                .unwrap()
                .input
                .documents
                .preset_yaml
                .push_str("\n# formatting only\n"),
            1 => state.revision += 1,
            2 => state.workspace.as_mut().unwrap().revision += 1,
            3 => state.ledger.source_version.push_str("changed"),
            4 => state.workspace.as_mut().unwrap().input.as_of += 1,
            5 => state.census.last_completed_at = Some(1792915201),
            6 => state
                .workspace
                .as_mut()
                .unwrap()
                .input
                .pins
                .push(proof::Pin {
                    account: g.account.clone(),
                    message_id_hash: "f".repeat(64),
                    expected: proof::Destination::Inbox,
                }),
            _ => state.workspace.as_mut().unwrap().input.baseline = proof::Baseline::Previous,
        }
        assert!(attempt(&state, &s, &r.receipt, Disposition::Accept));
    }
    let account = state.census.account.clone();
    let identity = store.keys().identity_material();
    let rotated = CensusKeys::rotated(&LocalSecret::random(), &account, &identity, 1).unwrap();
    assert!(
        confirm(
            &mut state.clone(),
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            &rotated,
            &s,
            Disposition::Accept,
            &r.receipt
        )
        .is_err()
    );
    let mut restored = CensusKeys::rotated(&LocalSecret::random(), &account, &identity, 1).unwrap();
    restored.recovery_authority("restored-authority");
    assert!(
        confirm(
            &mut state.clone(),
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            &restored,
            &s,
            Disposition::Accept,
            &r.receipt
        )
        .is_err()
    );
}

#[test]
fn gmail_dedup_sent_exclusion_unsupported_evidence_and_conflicts() {
    let (_root, store, mut state) = fixture();
    let folder = state.census.folders["INBOX"].clone();
    state.census.folders.insert("Label".into(), folder);
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::Gmail),
        store.keys(),
    )
    .unwrap();
    assert_eq!(g.candidates[0].observations.len(), 2);
    state
        .census
        .folders
        .get_mut("Label")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .direction = crate::census::Direction::Sent;
    let sent = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::Gmail),
        store.keys(),
    )
    .unwrap();
    assert_eq!(sent.candidates[0].observations.len(), 1);
    let mut conflict = state.clone();
    let candidate = &sent.candidates[0];
    let w = conflict.workspace.as_mut().unwrap();
    let (_, mut overrides) = config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap();
    if let Change::Addition { rule } = &candidate.change {
        let mut owner = rule.clone();
        owner.enabled = false;
        overrides.additions.push(owner);
    }
    w.input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    let conflicted = generate(
        &conflict,
        &crate::rules::workspace::WorkspaceContext::local(Provider::Gmail),
        store.keys(),
    )
    .unwrap();
    assert!(
        !conflicted
            .candidates
            .iter()
            .any(|c| c.change.target() == candidate.change.target())
    );
    let m = state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&3)
        .unwrap();
    m.headers.subject = "unsupported opaque topic".into();
    state.census.folders.remove("Label");
    let unsupported = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::Gmail),
        store.keys(),
    )
    .unwrap();
    assert!(
        unsupported
            .unresolved
            .iter()
            .any(|u| u.sender_claim == "robot@travel.example")
    );
}

#[test]
fn synthetic_engine_demonstrates_filing_and_bulk_priority_separately_from_held_proof() {
    let (_root, store, state) = fixture();
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let selected = selection(&g, &[0]);
    let reviewed = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selected,
        Disposition::Accept,
    )
    .unwrap();
    let mut input: engine::PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.documents = reviewed.resulting_documents;
    input.context.account = g.account.clone();
    let (_, mut overrides) =
        config::documents(&input.documents.preset_yaml, &input.documents.override_yaml).unwrap();
    overrides
        .roles
        .addresses
        .insert("owner@example.test".into(), config::Role::Personal);
    input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    input.messages = vec![
        engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: "ordinary".into(), current_folder: "INBOX".into(), headers: "From: robot@shop.example\r\nDelivered-To: owner@example.test\r\nSubject: a generic message\r\n\r\n".into() },
        engine::Fixture { delivery: Default::default(), observed_flags: Some(Default::default()), id: "bulk".into(), current_folder: "INBOX".into(), headers: "From: robot@shop.example\r\nDelivered-To: owner@example.test\r\nList-Id: shop.example\r\nSubject: a generic message\r\n\r\n".into() },
    ];
    input.pins.clear();
    let proof = engine::preview(&input).unwrap();
    assert!(
        proof
            .decisions
            .iter()
            .any(|d| d.destination.label() == "Personal/Orders")
    );
    assert!(
        proof
            .decisions
            .iter()
            .any(|d| d.destination.label() == "Personal/Newsletters")
    );
    assert!(
        proof
            .decisions
            .iter()
            .all(|d| !d.mark_read && d.copies.is_empty())
    );
}

pub(crate) fn seed(state: &mut AccountState, keys: &CensusKeys) {
    let (_root, _store, mut example) = fixture();
    example.census.account = state.census.account.clone();
    state.census = example.census;
    state.workspace = example.workspace;
    let g = generate(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
    )
    .unwrap();
    let rejected = selection(&g, &[0]);
    let r = review(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        &rejected,
        Disposition::Reject,
    )
    .unwrap();
    confirm(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        &rejected,
        Disposition::Reject,
        &r.receipt,
    )
    .unwrap();
    let g = generate(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
    )
    .unwrap();
    let accepted = selection(&g, &[1]);
    let r = review(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        &accepted,
        Disposition::Accept,
    )
    .unwrap();
    confirm(
        state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        keys,
        &accepted,
        Disposition::Accept,
        &r.receipt,
    )
    .unwrap();
    assert_eq!(state.proposals.history.len(), 2);
}

#[test]
fn known_preset_uses_nonbulk_patch_and_owner_patch_is_preserved() {
    let (_root, store, mut state) = fixture();
    let w = state.workspace.as_mut().unwrap();
    let (mut preset, _) = config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap();
    preset.rules.push(Rule {
        id: "sender.disabled".into(),
        category: Category::Orders,
        stage: Stage::From,
        when: vec![Condition::FromAddress {
            value: "robot@shop.example".into(),
        }],
        folder: None,
        enabled: false,
        accounts: BTreeSet::new(),
        retain: false,
        mark_read: false,
    });
    w.input.documents.preset_yaml = serde_yaml_ng::to_string(&preset).unwrap();
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let Change::Patch { patch } = &g.candidates[0].change else {
        panic!("Known ID requires patch")
    };
    assert_eq!(patch.id, "sender.disabled");
    assert!(matches!(patch.extend.as_slice(), [Condition::NonBulk]));
    let r = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &selection(&g, &[0]),
        Disposition::Accept,
    )
    .unwrap();
    assert!(
        r.resulting_documents
            .override_yaml
            .contains("sender.disabled")
    );
    let mut changed = state.clone();
    let w = changed.workspace.as_mut().unwrap();
    let (_, mut overrides) = config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap();
    overrides.patches.push(Patch {
        id: "sender.disabled".into(),
        enabled: Some(false),
        folder: None,
        when: None,
        extend: vec![],
        accounts: None,
        mark_read: None,
    });
    w.input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    assert!(
        !generate(
            &changed,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys()
        )
        .unwrap()
        .candidates
        .iter()
        .any(|c| matches!(c.change, Change::Patch { .. }))
    );
}

#[test]
fn failing_pins_block_acceptance_stale_pins_warn_and_history_limit_never_prunes() {
    let (_root, store, mut state) = fixture();
    let hash = state.census.folders["INBOX"].messages[&1]
        .headers
        .message_id_hash
        .clone()
        .unwrap();
    state
        .workspace
        .as_mut()
        .unwrap()
        .input
        .pins
        .push(proof::Pin {
            account: state.census.account.as_str().into(),
            message_id_hash: hash,
            expected: proof::Destination::Folder {
                name: "Personal/Orders".into(),
            },
        });
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let s = selection(&g, &[0]);
    let r = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &s,
        Disposition::Accept,
    )
    .unwrap();
    assert!(!r.acceptance_available);
    assert!(
        confirm(
            &mut state.clone(),
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &s,
            Disposition::Accept,
            &r.receipt
        )
        .is_err()
    );
    assert!(
        review(
            &state,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &s,
            Disposition::Reject
        )
        .unwrap()
        .acceptance_available
    );
    state.workspace.as_mut().unwrap().input.pins[0].message_id_hash = "f".repeat(64);
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert!(
        review(
            &state,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &selection(&g, &[0]),
            Disposition::Accept
        )
        .unwrap()
        .acceptance_available
    );
    for i in 0..DECISION_LIMIT {
        state.proposals.history.push(Decision {
            candidate: format!("{i:064x}"),
            generation: "f".repeat(64),
            disposition: Disposition::Reject,
            target: "user.historical".into(),
            as_of: 1792915200,
        });
    }
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let before = encoded(&state.proposals);
    assert!(
        review(
            &state,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &selection(&g, &[0]),
            Disposition::Reject
        )
        .is_err()
    );
    assert_eq!(encoded(&state.proposals), before);
}

#[test]
fn proposal_observation_worker() {
    let Some(root) = std::env::var_os("MAILROOM_PROPOSALS_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let repo = crate::acquisition::persistent_test_accounts(&root);
    let id = AccountId::new(&std::env::var("MAILROOM_PROPOSALS_ACCOUNT").unwrap()).unwrap();
    let account = repo.load(&id).unwrap();
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    let mut unrelated = encoded(&state);
    for key in ["revision", "workspace", "proposals"] {
        unrelated.as_object_mut().unwrap().remove(key);
    }
    std::fs::write(root.join("proposal-observation.json"), serde_json::to_vec(&serde_json::json!({
        "schema": store.schema_version().unwrap(), "unrelated_sha256": identity(&unrelated).unwrap(),
        "decision_count": state.proposals.history.len(), "decisions_sha256": identity(&state.proposals).unwrap(),
    })).unwrap()).unwrap();
}

#[test]
fn bilingual_majorities_and_known_folder_evidence_require_unique_existing_mappings() {
    let (_root, store, mut state) = fixture();
    let messages = &mut state.census.folders.get_mut("INBOX").unwrap().messages;
    messages.get_mut(&1).unwrap().headers.subject = "Invoice".into();
    messages.get_mut(&2).unwrap().headers.subject = "Rechnung".into();
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let Change::Addition { rule } = &g.candidates[0].change else {
        panic!("new rule expected")
    };
    assert_eq!(category_key(rule.category), category_key(Category::Money));
    assert!(g.candidates[0].rationale.contains("2/2"));
    let mut folder = state.census.folders.remove("INBOX").unwrap();
    for m in folder.messages.values_mut() {
        m.headers.subject = "opaque topic".into();
    }
    state
        .census
        .folders
        .insert("Personal/Orders".into(), folder);
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_eq!(g.candidates.len(), 2);
    assert!(
        g.candidates
            .iter()
            .all(|c| c.rationale.contains("filing folder"))
    );
    let folder = state.census.folders.remove("Personal/Orders").unwrap();
    state
        .census
        .folders
        .insert("Unknown catalogue".into(), folder);
    assert!(
        generate(
            &state,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys()
        )
        .unwrap()
        .candidates
        .is_empty()
    );
}

#[test]
fn renamed_owner_patch_is_a_conflict_without_overwrite() {
    let (_root, store, mut state) = fixture();
    let w = state.workspace.as_mut().unwrap();
    let (mut preset, mut overrides) = config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap();
    preset.rules.push(Rule {
        id: "sender.known".into(),
        category: Category::Orders,
        stage: Stage::From,
        when: vec![Condition::FromAddress {
            value: "robot@shop.example".into(),
        }],
        folder: None,
        enabled: false,
        accounts: BTreeSet::new(),
        retain: false,
        mark_read: false,
    });
    preset
        .renamed
        .insert("sender.old".into(), "sender.known".into());
    overrides.patches.push(Patch {
        id: "sender.old".into(),
        enabled: Some(false),
        folder: None,
        when: None,
        extend: vec![],
        accounts: None,
        mark_read: None,
    });
    w.input.documents.preset_yaml = serde_yaml_ng::to_string(&preset).unwrap();
    w.input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    let before = encoded(&state.workspace);
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert!(
        !g.candidates
            .iter()
            .any(|c| c.sender_claim == "robot@shop.example")
    );
    assert_eq!(encoded(&state.workspace), before);
}

#[test]
fn identical_acquisition_refresh_preserves_exact_rejection_but_invalidates_approval() {
    use crate::census::ScanState;
    use crate::census::acquisition::{ADAPTER, Attempt, SessionObservation, Snapshot};
    let (_root, store, mut state) = fixture();
    let account = state.census.account.clone();
    state.census.acquisition.latest = Some(Attempt {
        id: "1".repeat(32),
        adapter: ADAPTER.into(),
        provider: Provider::MailboxOrg,
        started_at: 1,
        finished_at: Some(2),
        state: ScanState::Complete,
        session: Some(SessionObservation {
            observed_at: 1,
            imap_capabilities: BTreeSet::new(),
        }),
        checkpoint: None,
    });
    state.census.acquisition.last_successful = Some(Snapshot {
        version: 1,
        adapter: ADAPTER.into(),
        account,
        provider: Provider::MailboxOrg,
        attempt_id: "1".repeat(32),
        topology_observed_at: 1,
        completed_at: Some(2),
        imap_capabilities: BTreeSet::new(),
        folders: vec![],
    });
    let g = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    let rejected = selection(&g, &[0]);
    let r = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &rejected,
        Disposition::Reject,
    )
    .unwrap();
    confirm(
        &mut state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &rejected,
        Disposition::Reject,
        &r.receipt,
    )
    .unwrap();
    let other = selection(&g, &[1]);
    let r = review(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
        &other,
        Disposition::Accept,
    )
    .unwrap();
    let latest = state.census.acquisition.latest.as_mut().unwrap();
    latest.id = "2".repeat(32);
    latest.started_at = 3;
    latest.finished_at = Some(4);
    latest.session.as_mut().unwrap().observed_at = 3;
    let success = state.census.acquisition.last_successful.as_mut().unwrap();
    success.attempt_id = "2".repeat(32);
    success.topology_observed_at = 3;
    success.completed_at = Some(4);
    let fresh = generate(
        &state,
        &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
        store.keys(),
    )
    .unwrap();
    assert_eq!(fresh.generation, g.generation);
    assert_eq!(fresh.candidates[0].id, g.candidates[0].id);
    assert!(fresh.candidates[0].rejected);
    assert!(
        confirm(
            &mut state,
            &crate::rules::workspace::WorkspaceContext::local(Provider::MailboxOrg),
            store.keys(),
            &other,
            Disposition::Accept,
            &r.receipt
        )
        .is_err()
    );
}
