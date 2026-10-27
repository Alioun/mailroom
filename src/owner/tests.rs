use super::*;
use crate::acquisition::{
    Provider,
    keychain::tests::{Throwaway, dummy},
};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn malformed_options_are_refused_before_any_backend() {
    for bad in [
        &["keychain", "status", "a-password", "--keychain"][..],
        &[
            "keychain",
            "status",
            "a-password",
            "--keychain",
            "/tmp/a",
            "--keychain",
            "/tmp/b",
        ],
        &[
            "keychain",
            "status",
            "a-password",
            "--keychain",
            "/tmp/a",
            "--keychain",
        ],
        &["keychain", "status", "a-password", "--keychain", ""],
        &["keychain", "status", "a-password", "--keychain", "--config"],
        &["keychain", "refs", "--config"],
        &["keychain", "status", "a-password", "--keychain=/tmp/a"],
        &["keychain", "status", "a-password", "-k", "/tmp/a"],
    ] {
        assert_eq!(parse(&args(bad)), None, "{bad:?}");
        assert_eq!(run(&args(bad)), 1, "{bad:?}");
    }
    let good = args(&["keychain", "status", "a-password", "--keychain", "/tmp/a"]);
    let parsed = parse(&good).unwrap();
    assert_eq!(parsed.keychain, Some(PathBuf::from("/tmp/a")));
    assert_eq!(parsed.positional, vec!["keychain", "status", "a-password"]);
}

#[test]
fn native_check_takes_no_keychain_option() {
    assert!(crate::native::disable_core_dumps());
    assert_eq!(run(&args(&["native-check", "--keychain", "/tmp/a"])), 1);
}

struct Account {
    _throwaway: Throwaway,
    keychain: Arc<Keychain>,
    paths: AccountPaths,
    account: crate::acquisition::Account,
}
fn account() -> Account {
    let throwaway = Throwaway::new();
    let root = throwaway.dir.path().join("accounts");
    let paths = AccountPaths {
        config: root.join("config"),
        data: root.join("data"),
    };
    let keychain = Arc::new(throwaway.keychain());
    let repo = Accounts::new(paths.clone(), keychain.clone());
    let password = SecretValue::new(dummy().to_vec());
    let account = repo
        .create(Provider::MailboxOrg, "login@example.test".into(), &password)
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    store.save_state(&mut state).unwrap();
    Account {
        _throwaway: throwaway,
        keychain,
        paths,
        account,
    }
}

#[test]
fn a_planted_key_fails_against_the_accounts_authenticated_state() {
    let a = account();
    let reference = a.account.local_secret_ref_for_test().to_owned();
    let genuine = a.keychain.read(&reference).unwrap();
    assert_eq!(
        check_store(&a.keychain, &a.paths, &reference, &genuine),
        StoreCheck::Verified
    );
    assert_eq!(
        check_store(&a.keychain, &a.paths, &reference, &dummy()),
        StoreCheck::Mismatch
    );
    assert_eq!(
        check_store(&a.keychain, &a.paths, "other-census-key", &genuine),
        StoreCheck::NoAccount
    );

    assert_eq!(a.keychain.remove(&reference), Status::Ok);
    assert_eq!(
        a.keychain.create(&reference, &dummy()),
        Status::Ok,
        "a same-user process plants a look-alike"
    );
    let verify = |value: &[u8]| {
        check_store(&a.keychain, &a.paths, &reference, value) == StoreCheck::Verified
    };
    assert!(
        !verify(&a.keychain.read(&reference).unwrap()),
        "check-store reports the planted key"
    );
    assert!(
        Accounts::new(a.paths.clone(), a.keychain.clone())
            .store(&a.account)
            .and_then(|s| s.load_state().map_err(Into::into))
            .is_err(),
        "the account refuses to open under the planted key"
    );
}

#[test]
fn a_planted_staged_copy_is_never_promoted() {
    let a = account();
    let reference = a.account.local_secret_ref_for_test().to_owned();
    let verify = |value: &[u8]| {
        check_store(&a.keychain, &a.paths, &reference, value) == StoreCheck::Verified
    };
    let genuine = a.keychain.read(&reference).unwrap();
    assert_eq!(a.keychain.remove(&reference), Status::Ok);
    assert_eq!(
        a.keychain.create_staged_for_test(&reference, &dummy()),
        Status::Ok
    );
    assert_eq!(
        a.keychain.retrust(&reference, &verify),
        Err(Status::Unverified)
    );
    assert_eq!(
        a.keychain.status(&reference),
        Status::Missing,
        "nothing was published"
    );
    assert_eq!(a.keychain.remove_staged_for_test(&reference), Status::Ok);
    assert_eq!(
        a.keychain.create_staged_for_test(&reference, &genuine),
        Status::Ok
    );
    assert_eq!(
        a.keychain.retrust(&reference, &verify),
        Ok(Status::Ok),
        "the genuine staged copy recovers"
    );
    assert_eq!(
        a.keychain.read(&reference).unwrap().as_slice(),
        genuine.as_slice()
    );
}

#[test]
fn a_planted_legacy_key_is_never_migrated() {
    let a = account();
    let reference = a.account.local_secret_ref_for_test().to_owned();
    let verify = |value: &[u8]| {
        check_store(&a.keychain, &a.paths, &reference, value) == StoreCheck::Verified
    };
    let genuine = a.keychain.read(&reference).unwrap();
    assert_eq!(a.keychain.remove(&reference), Status::Ok);
    assert_eq!(
        a.keychain.create_legacy_for_test(&reference, &dummy()),
        Status::Ok
    );
    assert_eq!(
        a.keychain.migrate(&reference, &verify),
        Err(Migration::Protected(Status::Unverified))
    );
    assert_eq!(a.keychain.status(&reference), Status::Missing);
    assert_eq!(a.keychain.remove_legacy_for_test(&reference), Status::Ok);
    assert_eq!(
        a.keychain.create_legacy_for_test(&reference, &genuine),
        Status::Ok
    );
    assert_eq!(a.keychain.migrate(&reference, &verify), Ok(Status::Ok));
}
