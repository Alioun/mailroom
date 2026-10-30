use super::*;

pub(crate) fn fill(state: &mut AccountState, count: usize) {
    let original = state.sieve_transactions.history[0].clone();
    let artifact = state
        .deployment_artifacts
        .get(&original.id)
        .unwrap()
        .artifact
        .clone();
    state.sieve_transactions.history.clear();
    state.deployment_artifacts = Default::default();
    for index in 1..=count {
        let mut transaction = original.clone();
        transaction.id = format!("{index:064x}");
        state
            .deployment_artifacts
            .insert(&transaction, artifact.clone())
            .unwrap();
        state.sieve_transactions.history.push(transaction);
    }
}

fn fixture(count: usize) -> (tempfile::TempDir, EncryptedStore, AccountState) {
    let (root, store, mut saved) =
        crate::rules::landing::tests::fixture_for("capacity-account", "", Placement::Block {});
    crate::rules::landing::tests::seed_deployment(&mut saved);
    fill(&mut saved, count);
    store.save_state(&mut saved).unwrap();
    (root, store, saved)
}

fn draft(saved: &AccountState) -> Preview {
    let mut transaction = saved.sieve_transactions.history[0].clone();
    transaction.id = format!("{:064x}", saved.sieve_transactions.history.len() + 1);
    transaction.status = Status::Running;
    transaction.steps.clear();
    Preview {
        receipt: transaction.receipt.clone(),
        deployment_artifact: Some(
            saved
                .deployment_artifacts
                .get(&saved.sieve_transactions.history[0].id)
                .unwrap()
                .artifact
                .clone(),
        ),
        effects: effects(&transaction.before, &transaction.after, &transaction),
        transaction,
        proof: serde_json::json!({}),
        precedence: None,
        limitations: LIMITATIONS,
    }
}

#[test]
fn fifth_and_sixty_fourth_intents_persist_without_pruning_prior_transactions_or_artifacts() {
    for count in [4, 63] {
        let (root, store, mut saved) = fixture(count);
        let previous = saved.sieve_transactions.history.clone();
        let preview = draft(&saved);
        reserve_history(&saved, &preview.transaction).unwrap();
        persist_confirmed_intent(&store, &mut saved, preview).unwrap();
        let keys = crate::census::CensusKeys::derive(
            &crate::census::LocalSecret::from_bytes(vec![7; 32]).unwrap(),
            &saved.census.account,
        );
        drop(store);
        let reopened = EncryptedStore::new(root.path().into(), keys).unwrap();
        let loaded = reopened.load_state().unwrap();
        assert_eq!(loaded.sieve_transactions.history.len(), count + 1);
        assert_eq!(
            &loaded.sieve_transactions.history[..count],
            previous.as_slice()
        );
        for transaction in &loaded.sieve_transactions.history {
            assert!(loaded.deployment_artifacts.get(&transaction.id).is_some());
        }
        assert_eq!(
            loaded.sieve_transactions.history[count].status,
            Status::Stopped
        );
        assert!(loaded.sieve_transactions.history[count].steps.is_empty());
        if let Some(directory) = std::env::var_os("MAILROOM_CAPACITY_FIXTURE_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            serde_json::to_writer(
                std::fs::File::create(directory.join(format!("{}.json", count + 1))).unwrap(),
                &serde_json::json!({"count":count+1,"journal":loaded.sieve_transactions}),
            )
            .unwrap();
        }
        println!("Verified retained operation count {}", count + 1);
    }
}

#[test]
fn sixty_fifth_intent_refuses_without_changing_state_or_ciphertext() {
    let (_root, store, mut saved) = fixture(model::HISTORY);
    let before = serde_json::to_vec(&saved).unwrap();
    let encrypted = std::fs::read(store.path()).unwrap();
    let preview = draft(&saved);
    assert!(persist_confirmed_intent(&store, &mut saved, preview).is_err());
    assert_eq!(serde_json::to_vec(&saved).unwrap(), before);
    assert_eq!(std::fs::read(store.path()).unwrap(), encrypted);
}

#[test]
fn capacity_reserves_effect_records_before_an_intent_that_alone_would_fit() {
    let (_root, store, mut saved) = fixture(1);
    let mut found = false;
    'fixtures: for width in [10_000, 12_000, 14_000, 16_000, 18_000, 20_000, 22_000] {
        let foreign = format!("# {}\n", "x".repeat(width));
        let (_source, _store, mut padded) = crate::rules::landing::tests::fixture_for(
            "capacity-account",
            &foreign,
            Placement::Block {},
        );
        padded
            .deployment_artifacts
            .insert(
                &padded.sieve_transactions.history[0],
                padded.exports[0].artifact.clone(),
            )
            .unwrap();
        for count in 1..model::HISTORY {
            fill(&mut padded, count);
            if padded
                .sieve_transactions
                .validate(&padded.census.account)
                .is_err()
            {
                break;
            }
            let preview = draft(&padded);
            let mut intent_only = padded.sieve_transactions.clone();
            intent_only.history.push(preview.transaction.clone());
            if intent_only.validate(&padded.census.account).is_err() {
                break;
            }
            if reserve_history(&padded, &preview.transaction).is_ok() {
                continue;
            }
            saved.sieve_transactions = padded.sieve_transactions.clone();
            saved.deployment_artifacts = padded.deployment_artifacts.clone();
            store.save_state(&mut saved).unwrap();
            let before = serde_json::to_vec(&saved).unwrap();
            let encrypted = std::fs::read(store.path()).unwrap();
            assert!(persist_confirmed_intent(&store, &mut saved, preview).is_err());
            assert_eq!(serde_json::to_vec(&saved).unwrap(), before);
            assert_eq!(std::fs::read(store.path()).unwrap(), encrypted);
            println!(
                "Verified byte-budget refusal with {} retained operations",
                count
            );
            found = true;
            break 'fixtures;
        }
    }
    assert!(
        found,
        "The fixture must reach the byte budget before the count limit"
    );
}

#[test]
fn compensation_links_survive_a_history_larger_than_four() {
    let (_root, store, mut saved) = fixture(5);
    let original = saved.sieve_transactions.history[0].clone();
    let mut compensation = original.clone();
    compensation.id = format!("{:064x}", 6);
    compensation.compensates = Some(original.id.clone());
    compensation.before = original.after.clone();
    compensation.after = original.before.clone();
    let composition = compensation.composition.as_mut().unwrap();
    composition.purpose = Purpose::Compensation;
    compensation.steps = vec![Step {
        effect: Effect::Upload,
        before: compensation.before.clone(),
        after: compensation.after.clone(),
        outcome: Outcome::Observed,
        acknowledgment: true,
        failure: None,
    }];
    saved.sieve_transactions.history.push(compensation.clone());
    store.save_state(&mut saved).unwrap();
    let loaded = store.load_state().unwrap();
    assert_eq!(loaded.sieve_transactions.history[5], compensation);
    assert_eq!(
        loaded.sieve_transactions.history[5].compensates.as_ref(),
        Some(&original.id)
    );
}

#[test]
fn two_step_include_projection_preserves_the_host_until_the_owned_upload_is_staged() {
    let (_root, _store, mut saved) = crate::rules::landing::tests::fixture_for(
        "include-capacity",
        "",
        Placement::Include {
            script: "mailroom".into(),
        },
    );
    let transaction = saved.sieve_transactions.history.pop().unwrap();
    let actions = effects(&transaction.before, &transaction.after, &transaction);
    assert_eq!(actions, vec![Effect::UploadOwned, Effect::Upload]);
    let intermediate = effect_image(&transaction.before, &transaction, actions[0]).unwrap();
    assert!(intermediate.validate());
    assert_eq!(
        intermediate.body(&transaction.target),
        transaction.before.body(&transaction.target)
    );
    assert_eq!(
        intermediate.body("mailroom"),
        transaction.after.body("mailroom")
    );
    assert_eq!(
        effect_image(&intermediate, &transaction, actions[1]).unwrap(),
        transaction.after
    );
    reserve_history(&saved, &transaction).unwrap();
    assert!(saved.sieve_transactions.history.is_empty());
}
