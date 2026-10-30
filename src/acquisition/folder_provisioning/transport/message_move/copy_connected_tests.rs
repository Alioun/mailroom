use super::*;
use crate::{
    backlog::{Options, Scope, protocol::Status},
    engine::Documents,
    rules::{proof::Baseline, workspace},
};

#[test]
fn saved_rule_confirmation_files_exact_members_over_verified_tls_and_reopens_both_journals() {
    let root = EvidenceRoot::new();
    let (mut repo, account) = world(root.path());
    let fake = Fake::start(root.path(), &account);
    let mut source = fake.source.clone();
    source.authority = "fixture-message-copy/v1".into();
    source.command_timeout = crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT;
    source.operation_timeout = Duration::from_secs(120);
    repo.backlog_source = Some(Arc::new(source));
    repo.install_synthetic_workspace(&account, "copy-tls-workspace/v1");
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let (_, independent) = external(&repo, &account.id, root.path());
    saved.census = independent.census.clone();
    saved.workspace = Some(workspace::Workspace {
        revision: 1,
        input: workspace::WorkspaceInput {
            documents: Documents {
                preset_yaml: "id: copy-tls\nmajor: 1\nrules: []\n".into(),
                override_yaml: "preset: copy-tls\nmajor: 1\nroles:\n  domains: {home.example.test: personal}\nadditions:\n  - id: user.news\n    category: newsletters\n    stage: from\n    when: [{kind: from_address, value: updates@studio.example.com}]\n    folder: News\n".into(),
            },
            pins: vec![],
            as_of: 1793260800,
            baseline: Baseline::Current,
        },
    });
    let context = repo.workspace_context(&account, &store).unwrap();
    workspace::validate(&saved.workspace.as_ref().unwrap().input, &saved, &context).unwrap();
    store.save_state(&mut saved).unwrap();
    let ciphertext = std::fs::read(store.path()).unwrap();
    let options = Options {
        scope: Scope::Inbox,
        ..Options::default()
    };
    let preview = repo
        .backlog_preview(
            &account,
            &store,
            &saved,
            options.clone(),
            Cancellation::default(),
        )
        .unwrap();
    assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
    assert_eq!(preview.operation.plan.filing.changes.len(), 3);
    assert_eq!(preview.operation.batches.len(), 1);
    let evidence = preview.operation.batches[0].as_copy().unwrap();
    assert_eq!(
        evidence
            .intent()
            .messages()
            .keys()
            .map(|uid| uid.get())
            .collect::<Vec<_>>(),
        vec![4, 8, 19]
    );
    assert!(!repo.backlog_installed(Some(&account.id)));
    let (_, before) = external(&repo, &account.id, root.path());
    assert!(events(root.path()).is_empty());
    repo.backlog_confirm_copy(
        &account,
        &store,
        &mut saved,
        options,
        &preview.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    let reopened = store.load_state().unwrap();
    assert_eq!(reopened.protocol_backlog.runs[0].status, Status::Complete);
    assert_eq!(reopened.protocol_backlog.runs[0].receipt, preview.receipt);
    assert!(reopened.protocol_backlog.runs[0].plan == preview.operation.plan);
    let (_, after) = external(&repo, &account.id, root.path());
    assert_eq!(after.census.folders["INBOX"].live_uids, [99].into());
    assert_eq!(
        after.census.folders["Personal/News"].live_uids,
        [60, 64, 68].into()
    );
    assert_eq!(
        serde_json::to_value(&before.census.folders["INBOX"].messages[&99]).unwrap(),
        serde_json::to_value(&after.census.folders["INBOX"].messages[&99]).unwrap()
    );
    for (original, copied) in [(4, 60), (8, 64), (19, 68)] {
        let mut expected = before.census.folders["INBOX"].messages[&original].clone();
        expected.uid = copied;
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::to_value(&after.census.folders["Personal/News"].messages[&copied]).unwrap()
        );
    }
    for event in ["UID COPY", "UID STORE", "UID EXPUNGE"] {
        assert_eq!(
            events(root.path())
                .iter()
                .filter(|command| *command == event)
                .count(),
            1
        );
    }
    assert!(!repo.backlog_installed(Some(&account.id)));
}
