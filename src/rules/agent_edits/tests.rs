use super::*;
use crate::{
    acquisition::Provider,
    census::{AccountId, EncryptedStore, LocalSecret},
    engine,
    rules::proof,
};
use serde_json::{Value, json};

const SUMMARY: &str = "# Census summary\n\nInvented synthetic summary.\n";

fn fixture(account: &str) -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let directory = tempfile::tempdir().unwrap();
    let account = AccountId::new(account).unwrap();
    let store = EncryptedStore::new(
        directory.path().into(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    let mut input: engine::PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    input.context.account = account.as_str().into();
    input.documents.preset_yaml = input.documents.preset_yaml.replacen(
        "  studio.old: sender.studio\n",
        "  studio.old: sender.studio\n  trips.older: trips.old\n  trips.old: topic.travel\n",
        1,
    );
    store
        .save(&engine::synthetic_census(&input).unwrap())
        .unwrap();
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

fn context() -> WorkspaceContext {
    WorkspaceContext::local(Provider::MailboxOrg)
}

fn proposal(edits: Value) -> Proposal {
    serde_json::from_value(json!({
        "version": 1,
        "summary_sha256": data_encoding::HEXLOWER.encode(&Sha256::digest(SUMMARY.as_bytes())),
        "recommendations": [{"title": "File travel", "reason": "Invented reason.", "suggested_change": "Move travel mail."}],
        "rule_edits": edits,
    }))
    .unwrap()
}

fn travel_and_receipts() -> Proposal {
    proposal(json!({
        "patches": [{"id": "trips.older", "folder": "Trips"}],
        "additions": [{"id": "user.agent.receipts", "category": "orders", "stage": "subject", "when": [{"kind": "subject_contains", "value": "receipt"}], "folder": "Receipts"}],
    }))
}

fn overrides(state: &AccountState) -> config::Overrides {
    let w = state.workspace.as_ref().unwrap();
    config::documents(
        &w.input.documents.preset_yaml,
        &w.input.documents.override_yaml,
    )
    .unwrap()
    .1
}

fn refusal(state: &AccountState, store: &EncryptedStore, edits: Value) -> String {
    match review(state, &context(), store.keys(), &proposal(edits), SUMMARY) {
        Ok(_) => panic!("edits were accepted for review"),
        Err(error) => error,
    }
}

#[test]
fn accepted_edits_follow_the_rename_chain_into_the_saved_override() {
    let (_root, store, mut state) = fixture("agent-edit-accept");
    let before = state.clone();
    let reviewed = review(
        &state,
        &context(),
        store.keys(),
        &travel_and_receipts(),
        SUMMARY,
    )
    .unwrap();
    assert_eq!(reviewed.edits[0].requested, "trips.older");
    assert_eq!(reviewed.edits[0].target, "topic.travel");
    assert_eq!(reviewed.edits[1].target, "user.agent.receipts");
    assert_eq!(reviewed.workspace_revision, 2);
    assert_eq!(
        reviewed.original_documents.override_yaml,
        before
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .override_yaml
    );
    assert!(reviewed.document_diff.contains("+  folder: Trips"));
    assert!(
        reviewed
            .resulting_documents
            .override_yaml
            .contains("user.agent.receipts")
    );
    assert_ne!(reviewed.current.receipt, reviewed.resulting.receipt);
    let receipt = reviewed.accept_receipt.clone().unwrap();
    confirm(
        &mut state,
        &context(),
        store.keys(),
        &travel_and_receipts(),
        SUMMARY,
        Disposition::Accept,
        &receipt,
    )
    .unwrap();
    store.save_state(&mut state).unwrap();
    let saved = store.load_state().unwrap();
    assert!(saved.revision > before.revision);
    let w = saved.workspace.as_ref().unwrap();
    assert_eq!(w.revision, 2);
    assert_eq!(
        w.input.documents.preset_yaml,
        before
            .workspace
            .as_ref()
            .unwrap()
            .input
            .documents
            .preset_yaml
    );
    assert_eq!(
        w.input.documents.override_yaml,
        reviewed.resulting_documents.override_yaml
    );
    let o = overrides(&saved);
    let travel = o.patches.iter().find(|p| p.id == "topic.travel").unwrap();
    assert_eq!(travel.folder.as_deref(), Some("Trips"));
    assert!(o.patches.iter().any(|p| p.id == "studio.old"));
    assert!(o.additions.iter().any(|r| r.id == "user.release"));
    assert!(o.additions.iter().any(|r| r.id == "user.agent.receipts"));
    assert_eq!(o.forwards.len(), 3);
    assert_eq!(o.ledger.len(), 2);
    assert!(saved.proposals.agent_rejections.is_empty());
    let replay = confirm(
        &mut state,
        &context(),
        store.keys(),
        &travel_and_receipts(),
        SUMMARY,
        Disposition::Accept,
        &receipt,
    );
    assert!(replay.is_err(), "an accepted review cannot apply twice");
}

#[test]
fn catalog_rules_count_as_preset_rules() {
    let (_root, store, state) = fixture("agent-edit-catalog");
    let p = proposal(json!({"patches": [{"id": "sender.core.orders", "folder": "Shop"}]}));
    let reviewed = review(&state, &context(), store.keys(), &p, SUMMARY).unwrap();
    assert_eq!(reviewed.edits[0].target, "sender.core.orders");
}

#[test]
fn invalid_edits_are_refused_before_review() {
    let (_root, store, mut state) = fixture("agent-edit-invalid");
    let rule = |id: &str| json!({"id": id, "category": "orders", "stage": "subject", "when": [{"kind": "subject_contains", "value": "x"}]});
    let cases = [
        (
            json!({"patches": [{"id": "topic.nothing", "folder": "X"}]}),
            "targets no existing preset rule",
        ),
        (
            json!({"patches": [{"id": "user.release", "folder": "X"}]}),
            "targets no existing preset rule",
        ),
        (
            json!({"patches": [{"id": "guard.login", "enabled": false}]}),
            "guard rules",
        ),
        (
            json!({"patches": [{"id": "topic.travel"}]}),
            "changes no value",
        ),
        (
            json!({"patches": [{"id": "Bad ID", "folder": "X"}]}),
            "Stable IDs",
        ),
        (
            json!({"patches": [{"id": "topic.travel", "folder": "a/b"}]}),
            "Folder names",
        ),
        (
            json!({"additions": [rule("agent.rule")]}),
            "user.* filing rule",
        ),
        (
            json!({"additions": [{"id": "user.guard", "category": "money", "stage": "guard", "retain": true, "when": [{"kind": "subject_contains", "value": "x"}]}]}),
            "user.* filing rule",
        ),
        (
            json!({"additions": [{"id": "user.domain", "category": "orders", "stage": "from", "when": [{"kind": "from_domain", "value": "UPPER.example"}]}]}),
            "Invalid rule domain",
        ),
        (
            json!({"additions": [{"id": "user.empty", "category": "orders", "stage": "subject", "when": []}]}),
            "1..16",
        ),
    ];
    for (edits, expected) in cases {
        let error = refusal(&state, &store, edits.clone());
        assert!(error.contains(expected), "{edits}: {error}");
    }
    for edits in [
        json!({"patches": [], "additions": []}),
        json!({"additions": (0..21).map(|i| rule(&format!("user.r{i}"))).collect::<Vec<_>>()}),
    ] {
        assert_eq!(
            refusal(&state, &store, edits),
            "invalid_or_changed_proposal"
        );
    }
    for edits in [
        json!({"patches": [], "forwards": []}),
        json!({"patches": [{"id": "topic.travel", "folder": "X", "retain": true}]}),
        json!({"additions": [{"id": "user.x", "category": "orders", "stage": "subject", "when": [{"kind": "subject_contains", "value": "x"}], "forward": "x"}]}),
    ] {
        let document = json!({"version": 1, "summary_sha256": "0".repeat(64), "recommendations": [], "rule_edits": edits});
        assert!(
            serde_json::from_value::<Proposal>(document).is_err(),
            "closed schema"
        );
    }
    let mut text_only: Value = serde_json::to_value(travel_and_receipts()).unwrap();
    text_only.as_object_mut().unwrap().remove("rule_edits");
    let text_only: Proposal = serde_json::from_value(text_only).unwrap();
    assert!(review(&state, &context(), store.keys(), &text_only, SUMMARY).is_err());
    assert!(
        review(
            &state,
            &context(),
            store.keys(),
            &travel_and_receipts(),
            "# Census summary\nchanged\n"
        )
        .is_err()
    );
    state.workspace = None;
    assert!(
        refusal(
            &state,
            &store,
            json!({"patches": [{"id": "topic.travel", "folder": "X"}]})
        )
        .contains("Save local rules")
    );
}

#[test]
fn rename_cycles_are_refused() {
    let (_root, store, mut state) = fixture("agent-edit-cycle");
    let w = state.workspace.as_mut().unwrap();
    w.input.documents.preset_yaml = w.input.documents.preset_yaml.replacen(
        "  trips.old: topic.travel\n",
        "  trips.old: trips.older\n",
        1,
    );
    let error = refusal(
        &state,
        &store,
        json!({"patches": [{"id": "trips.older", "folder": "X"}]}),
    );
    assert_eq!(error, "Cyclic rule rename chain");
}

#[test]
fn each_rule_is_claimed_once_across_owner_and_agent_edits() {
    let (_root, store, state) = fixture("agent-edit-claims");
    let rule = |id: &str| json!({"id": id, "category": "orders", "stage": "subject", "when": [{"kind": "subject_contains", "value": "x"}]});
    for edits in [
        json!({"patches": [{"id": "sender.studio", "folder": "Other"}]}),
        json!({"patches": [{"id": "trips.old", "folder": "A"}, {"id": "topic.travel", "enabled": false}]}),
        json!({"patches": [{"id": "trips.older", "folder": "A"}, {"id": "trips.old", "folder": "B"}]}),
        json!({"additions": [rule("user.release")]}),
        json!({"additions": [rule("user.new"), rule("user.new")]}),
    ] {
        let error = refusal(&state, &store, edits.clone());
        assert!(
            error.contains("already has an edit") || error.contains("reuses an existing rule ID"),
            "{edits}: {error}"
        );
    }
}

#[test]
fn stale_altered_and_cross_purpose_receipts_are_refused() {
    let (_root, store, state) = fixture("agent-edit-receipts");
    let p = travel_and_receipts();
    let reviewed = review(&state, &context(), store.keys(), &p, SUMMARY).unwrap();
    let accept = reviewed.accept_receipt.unwrap();
    let reject = reviewed.reject_receipt.unwrap();
    assert_ne!(accept, reject);
    let attempt = |state: &AccountState, p: &Proposal, d: Disposition, receipt: &str| {
        let mut copy = state.clone();
        let result = confirm(&mut copy, &context(), store.keys(), p, SUMMARY, d, receipt);
        assert!(result.is_err(), "{d:?} with a refused receipt");
        assert_eq!(
            serde_json::to_value(&copy.workspace).unwrap(),
            serde_json::to_value(&state.workspace).unwrap()
        );
        assert!(copy.proposals.agent_rejections.is_empty());
    };
    let mut altered = accept.clone().into_bytes();
    altered[0] = if altered[0] == b'0' { b'1' } else { b'0' };
    attempt(
        &state,
        &p,
        Disposition::Accept,
        std::str::from_utf8(&altered).unwrap(),
    );
    attempt(&state, &p, Disposition::Accept, &reject);
    attempt(&state, &p, Disposition::Reject, &accept);
    let mut reworded = serde_json::to_value(&p).unwrap();
    reworded["recommendations"][0]["reason"] = "Changed after review.".into();
    attempt(
        &state,
        &serde_json::from_value(reworded).unwrap(),
        Disposition::Accept,
        &accept,
    );
    let mut redirected = serde_json::to_value(&p).unwrap();
    redirected["rule_edits"]["patches"][0]["folder"] = "Elsewhere".into();
    attempt(
        &state,
        &serde_json::from_value(redirected).unwrap(),
        Disposition::Accept,
        &accept,
    );
    let mut edited = state.clone();
    let w = edited.workspace.as_mut().unwrap();
    w.input.documents.override_yaml = w
        .input
        .documents
        .override_yaml
        .replace("folder: Releases", "folder: Builds");
    w.revision += 1;
    attempt(&edited, &p, Disposition::Accept, &accept);
    let mut rewritten = state.clone();
    rewritten.revision += 1;
    attempt(&rewritten, &p, Disposition::Accept, &accept);
    let (_other_root, other_store, other) = fixture("agent-edit-other");
    let mut copy = other.clone();
    assert!(
        confirm(
            &mut copy,
            &context(),
            other_store.keys(),
            &p,
            SUMMARY,
            Disposition::Accept,
            &accept
        )
        .is_err(),
        "another account"
    );
}

#[test]
fn rejected_edits_persist_and_are_never_imported_again() {
    let (_root, store, mut state) = fixture("agent-edit-reject");
    let before = state
        .workspace
        .as_ref()
        .unwrap()
        .input
        .documents
        .override_yaml
        .clone();
    let p = travel_and_receipts();
    let reviewed = review(&state, &context(), store.keys(), &p, SUMMARY).unwrap();
    confirm(
        &mut state,
        &context(),
        store.keys(),
        &p,
        SUMMARY,
        Disposition::Reject,
        &reviewed.reject_receipt.unwrap(),
    )
    .unwrap();
    store.save_state(&mut state).unwrap();
    let saved = store.load_state().unwrap();
    let w = saved.workspace.as_ref().unwrap();
    assert_eq!(w.revision, 1);
    assert_eq!(w.input.documents.override_yaml, before);
    let rejected = &saved.proposals.agent_rejections;
    assert_eq!(rejected.len(), 2);
    assert_eq!(rejected[0].target, "topic.travel");
    assert_eq!(rejected[1].target, "user.agent.receipts");
    assert!(saved.proposals.history.is_empty());
    let repeats = [
        travel_and_receipts(),
        proposal(json!({"patches": [{"id": "topic.travel", "folder": "Trips"}]})),
        proposal(json!({"patches": [{"id": "trips.old", "folder": "Trips"}]})),
        proposal(
            json!({"additions": [{"id": "user.agent.receipts", "category": "orders", "stage": "subject", "when": [{"kind": "subject_contains", "value": "receipt"}], "folder": "Receipts"}]}),
        ),
    ];
    for repeat in repeats {
        let error = review(&saved, &context(), store.keys(), &repeat, SUMMARY)
            .err()
            .unwrap();
        assert_eq!(error, "Agent proposal repeats a rejected rule edit");
    }
    let changed = proposal(json!({"patches": [{"id": "topic.travel", "folder": "Journeys"}]}));
    assert!(review(&saved, &context(), store.keys(), &changed, SUMMARY).is_ok());
    let organisational =
        crate::rules::proposals::generate(&saved, &context(), store.keys()).unwrap();
    assert!(
        organisational.candidates.iter().all(|c| !c.rejected),
        "agent rejections do not reject organisational candidates"
    );
}

#[test]
fn full_rejection_history_refuses_new_rejections_and_keeps_old_ones() {
    let (_root, store, mut state) = fixture("agent-edit-bound");
    state.proposals.agent_rejections = (0..REJECTION_LIMIT - 1)
        .map(|i| Rejection {
            edit: identity(&i).unwrap(),
            target: "topic.money".into(),
            summary: "0".repeat(64),
            as_of: 0,
        })
        .collect();
    let two = review(
        &state,
        &context(),
        store.keys(),
        &travel_and_receipts(),
        SUMMARY,
    )
    .unwrap();
    assert!(two.reject_receipt.is_none(), "511 + 2 exceeds the limit");
    assert!(two.accept_receipt.is_some());

    let trips = proposal(json!({"patches": [{"id": "topic.travel", "folder": "Trips"}]}));
    let last = review(&state, &context(), store.keys(), &trips, SUMMARY).unwrap();
    confirm(
        &mut state,
        &context(),
        store.keys(),
        &trips,
        SUMMARY,
        Disposition::Reject,
        &last.reject_receipt.unwrap(),
    )
    .unwrap();
    store.save_state(&mut state).unwrap();
    let mut state = store.load_state().unwrap();
    assert_eq!(state.proposals.agent_rejections.len(), REJECTION_LIMIT);

    for repeat in [
        trips.clone(),
        proposal(json!({"patches": [{"id": "trips.old", "folder": "Trips"}]})),
    ] {
        let error = review(&state, &context(), store.keys(), &repeat, SUMMARY)
            .err()
            .unwrap();
        assert_eq!(error, "Agent proposal repeats a rejected rule edit");
    }

    let journeys = proposal(json!({"patches": [{"id": "topic.travel", "folder": "Journeys"}]}));
    let full = review(&state, &context(), store.keys(), &journeys, SUMMARY).unwrap();
    assert!(full.reject_receipt.is_none());
    assert_eq!(full.rejection_history.recorded, REJECTION_LIMIT);
    assert_eq!(full.rejection_history.limit, REJECTION_LIMIT);
    let before = serde_json::to_vec(&state).unwrap();
    let error = confirm(
        &mut state,
        &context(),
        store.keys(),
        &journeys,
        SUMMARY,
        Disposition::Reject,
        full.accept_receipt.as_ref().unwrap(),
    )
    .err()
    .unwrap();
    assert_eq!(
        error,
        "The rejection history is full. It holds 512 of 512 rejected edits and has no room for 1 more. Rejections are never removed, so rejected edits are never proposed again. Nothing was changed"
    );
    assert_eq!(
        serde_json::to_vec(&state).unwrap(),
        before,
        "nothing changed"
    );

    confirm(
        &mut state,
        &context(),
        store.keys(),
        &journeys,
        SUMMARY,
        Disposition::Accept,
        &full.accept_receipt.unwrap(),
    )
    .unwrap();
    assert!(
        overrides(&state)
            .patches
            .iter()
            .any(|p| p.folder.as_deref() == Some("Journeys"))
    );
    assert_eq!(state.proposals.agent_rejections.len(), REJECTION_LIMIT);

    state
        .proposals
        .agent_rejections
        .push(state.proposals.agent_rejections[0].clone());
    assert!(
        state.proposals.validate().is_err(),
        "duplicate or 513th rejection"
    );
}
