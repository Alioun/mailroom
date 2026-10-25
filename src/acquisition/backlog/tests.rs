use super::*;

#[test]
fn actual_saved_workflow_validates_phases_and_retains_journal_on_overflow() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("workspace-authority"),
        "journal-fixture-v1",
    )
    .unwrap();
    let (repo, account, _fake) = fixture_install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let preview = repo
        .backlog_preview(
            &account,
            &store,
            &saved,
            Options::default(),
            Cancellation::default(),
        )
        .unwrap();
    repo.backlog_confirm(
        &account,
        &store,
        &mut saved,
        Options::default(),
        &preview.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.protocol_backlog.runs[0].status, Status::Complete);
    let id = &reopened.protocol_backlog.runs[0].id;
    let reverse = repo
        .backlog_undo_preview(&account, &store, &reopened, id, Cancellation::default())
        .unwrap();
    repo.backlog_undo(
        &account,
        &store,
        &mut saved,
        id,
        &reverse.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    let original = std::fs::read(store.path()).unwrap();
    let canonical = serde_json::to_value(&saved.protocol_backlog).unwrap();
    let decoded: model::Journal = serde_json::from_value(canonical.clone()).unwrap();
    decoded.validate(&account.id).unwrap();
    for mutation in [
        "phase",
        "account",
        "duplicate",
        "reverse",
        "binding",
        "conflicts",
        "digest",
        "method",
        "semantics",
        "reverse_empty",
        "uncertain",
        "stop_phase",
    ] {
        let mut invalid = canonical.clone();
        match mutation {
            "phase" => invalid["runs"][0]["status"] = "prepared".into(),
            "account" => {
                invalid["runs"][0]["plan"]["binding"]["account"] = "different-account".into()
            }
            "duplicate" => {
                let run = invalid["runs"][0].clone();
                invalid["runs"].as_array_mut().unwrap().push(run);
            }
            "reverse" => invalid["runs"][0]["reverse"][0]["forward"] = 999.into(),
            "binding" => invalid["runs"][0]["plan"]["binding"]["authority"] = "forged".into(),
            "digest" => invalid["runs"][0]["id"] = "z".repeat(64).into(),
            "method" => {
                invalid["runs"][0]["plan"]["filing"]["changes"][0]["operation"]["method"] =
                    "copy_uid_expunge".into()
            }
            "semantics" => invalid["runs"][0]["plan"]["filing"]["semantics"] = "legacy".into(),
            "reverse_empty" => invalid["runs"][0]["reverse"] = serde_json::json!([]),
            "uncertain" => invalid["runs"][0]["status"] = "uncertain".into(),
            "stop_phase" => {
                invalid["runs"][0]["status"] = "undo_running".into();
                invalid["runs"][0]["stop_requested"] = true.into();
            }
            _ => invalid["runs"][0]["undo_conflicts"] = 9999.into(),
        }
        let invalid: model::Journal = serde_json::from_value(invalid).unwrap();
        assert!(invalid.validate(&account.id).is_err(), "{mutation}");
    }
    saved.protocol_backlog.runs = vec![saved.protocol_backlog.runs[0].clone(); model::HISTORY + 1];
    assert!(store.save_state(&mut saved).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), original);
}
