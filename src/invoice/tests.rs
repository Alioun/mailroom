use super::*;
use crate::{census::EncryptedStore, rules::proof::Baseline};
use crate::{
    census::LocalSecret,
    engine::{self, PreviewInput},
    rules::workspace::{Workspace, WorkspaceInput},
};
fn fixture() -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let input: PreviewInput =
        serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
    let census = engine::synthetic_census(&input).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let keys = CensusKeys::derive(&LocalSecret::random(), &census.account);
    let store = EncryptedStore::new(dir.path().join("journal"), keys).unwrap();
    store.save(&census).unwrap();
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents: input.documents,
            pins: vec![],
            as_of: input.as_of,
            baseline: Baseline::Current,
        },
    });
    initialize_synthetic(&mut state);
    store.save_state(&mut state).unwrap();
    (dir, store, state)
}
#[test]
fn report_keeps_predicted_copy_unknown_handoff_roles_and_mime_separate() {
    let (_dir, store, mut state) = fixture();
    let report = report(&state, Provider::MailboxOrg, true, store.keys()).unwrap();
    assert!(
        report
            .rows
            .iter()
            .any(|r| !r.predicted_targets.is_empty() && r.handoff == "real_handoff_unknown")
    );
    assert!(
        report
            .rows
            .iter()
            .any(|r| r.role == "unresolved" && !r.send_available)
    );
    assert!(
        report
            .rows
            .iter()
            .all(|r| r.accounting_acceptance == "unknown")
    );
    let real = super::report(&state, Provider::MailboxOrg, false, store.keys()).unwrap();
    assert_eq!(real.eligible_business, 0);
    assert!(
        real.rows
            .iter()
            .all(|r| r.evidence == ContentEvidence::Unknown && !r.send_available)
    );
    let selection = "INBOX/1/1";
    let prior = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        false,
    )
    .unwrap();
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&1)
        .unwrap()
        .headers
        .message_id_hash = Some("f".repeat(64));
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            selection,
            "accounting",
            true,
            false
        )
        .is_err()
    );
    let changed = super::report(&state, Provider::MailboxOrg, true, store.keys()).unwrap();
    assert_ne!(changed.version, prior.plan.report_version);
    assert_eq!(
        changed
            .rows
            .iter()
            .find(|r| r.id == selection)
            .unwrap()
            .evidence,
        ContentEvidence::Unknown
    );
}
#[test]
fn exact_send_hmac_rejects_state_content_targets_sender_and_other_account() {
    let (_dir, store, mut state) = fixture();
    let selection = "INBOX/1/1";
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        false,
    )
    .unwrap();
    let receipt = p.receipt.clone();
    let action = p.plan.confirmation_action.clone();
    state
        .invoices
        .sources
        .get_mut(selection)
        .unwrap()
        .original
        .push_str("edited");
    let fresh = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        selection,
        "accounting",
        true,
        false,
    )
    .unwrap();
    assert!(
        SyntheticConfirmation
            .confirm(&mut state, fresh, &receipt, &action)
            .is_err()
    );
    let other_keys = CensusKeys::derive(&LocalSecret::random(), &state.census.account);
    let other = preview(
        &state,
        Provider::MailboxOrg,
        &other_keys,
        selection,
        "accounting",
        true,
        false,
    )
    .unwrap();
    assert!(
        SyntheticConfirmation
            .confirm(&mut state, other, &receipt, &action)
            .is_err()
    );
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            "INBOX/1/6",
            "accounting",
            true,
            false
        )
        .is_err()
    );
    assert!(
        preview(
            &state,
            Provider::MailboxOrg,
            store.keys(),
            selection,
            "accounting",
            false,
            false
        )
        .is_err()
    );
}
#[test]
fn durable_effect_reconciles_exact_bytes_and_missing_ack_never_retries() {
    let (dir, store, mut state) = fixture();
    let p = preview(
        &state,
        Provider::MailboxOrg,
        store.keys(),
        "INBOX/1/1",
        "accounting",
        true,
        false,
    )
    .unwrap();
    let receipt = p.receipt.clone();
    let action = p.plan.confirmation_action.clone();
    let exact = p.plan.clone();
    let id = SyntheticConfirmation
        .confirm(&mut state, p, &receipt, &action)
        .unwrap();
    store.save_state(&mut state).unwrap();
    let mut state = store.load_state().unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Uncertain);
    let mut transport = SyntheticTransport::open(dir.path(), &store, &state).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Uncertain);
    assert!(transport.received().is_empty());
    transport.submit(&id, &exact).unwrap();
    drop(transport);
    let transport = SyntheticTransport::open(dir.path(), &store, &state).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    observe(&mut state, &transport, &id).unwrap();
    assert_eq!(state.invoices.sends[0].status, Status::Completed);
    assert_eq!(transport.received().len(), 1);
    assert_eq!(transport.received()[&id], exact);
    store.save_state(&mut state).unwrap();
    let bytes = std::fs::read(store.path()).unwrap();
    assert!(
        !bytes
            .windows(b"SYNTHETIC-INVOICE-PRIVATE-BYTES".len())
            .any(|w| w == b"SYNTHETIC-INVOICE-PRIVATE-BYTES")
    );
}
