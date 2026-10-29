use super::*;

fn source(authority: &str) -> InstalledSource {
    InstalledSource {
        account: AccountId::new("profile-fixture").unwrap(),
        login: "owner@example.test".into(),
        address: "127.0.0.1:9".parse().unwrap(),
        tls_name: "fixture.example.test".into(),
        root: vec![1, 2, 3],
        epoch: "profile-v1".into(),
        authority: authority.into(),
        command_timeout: Duration::from_secs(1),
        operation_timeout: Duration::from_secs(5),
    }
}
fn binding(source: &InstalledSource) -> RuntimeBinding {
    RuntimeBinding {
        account: source.account.clone(),
        provider: Provider::MailboxOrg,
        login: source.login.clone(),
        source: source.identity(),
        credential_key_recovery: "credential-key-recovery".into(),
        workspace: "workspace".into(),
        topology: "topology".into(),
    }
}

#[test]
fn source_profiles_keep_move_identity_bytes_and_separate_copy_authority() {
    use sha2::{Digest, Sha256};
    let moved = source("fixture-message-move/v1");
    let legacy = serde_json::to_vec(&(
        "loopback-imap-move-uidplus/v1",
        &moved.account,
        &moved.login,
        moved.address,
        &moved.tls_name,
        &moved.root,
        &moved.epoch,
        &moved.authority,
    ))
    .unwrap();
    assert_eq!(
        moved.identity(),
        data_encoding::HEXLOWER.encode(&Sha256::digest(legacy))
    );
    assert_eq!(moved.profile().unwrap().method(), MoveMethod::UidMove);
    let copied = source("fixture-message-copy/v1");
    assert_ne!(copied.identity(), moved.identity());
    assert_eq!(
        copied.profile().unwrap().method(),
        MoveMethod::CopyUidExpunge
    );
    for source in [&moved, &copied] {
        assert!(source.runtime_permit(&binding(source)).is_ok());
        let mut changed = binding(source);
        changed.source = "wrong-source".into();
        assert!(source.runtime_permit(&changed).is_err());
        let mut changed = binding(source);
        changed.provider = Provider::Gmail;
        assert!(source.runtime_permit(&changed).is_err());
    }
    for authority in [
        "",
        "production",
        "fixture-message-copy/v2",
        "fixture-message-move/v2",
    ] {
        let source = source(authority);
        assert!(source.profile().is_err());
        assert!(source.runtime_permit(&binding(&source)).is_err());
    }
}

#[test]
fn capabilities_never_silently_switch_the_installed_filing_method() {
    for (names, moved, copied) in [
        (vec!["UIDPLUS", "MOVE"], true, false),
        (vec!["UIDPLUS", "CONDSTORE"], false, true),
        (vec!["UIDPLUS", "CONDSTORE", "MOVE"], true, false),
        (vec!["CONDSTORE"], false, false),
        (vec!["UIDPLUS"], false, false),
        (vec!["MOVE"], false, false),
    ] {
        let capabilities = names.into_iter().map(str::to_owned).collect();
        assert_eq!(
            Profile::Move.validate_capabilities(&capabilities).is_ok(),
            moved
        );
        assert_eq!(
            Profile::Copy.validate_capabilities(&capabilities).is_ok(),
            copied
        );
    }
}
