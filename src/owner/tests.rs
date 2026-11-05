use super::*;
use crate::acquisition::{
    Provider,
    keychain::tests::{Throwaway, dummy},
};
use crate::confirmation::{Backend, Boundary, CHANGED_AFTER};

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

/// Gives every request the same reply, as the helper or its launcher would.
struct Reply(Result<Vec<u8>, String>);
impl Backend for Reply {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn exchange(&mut self, _: &[u8], _: &[u8; 32]) -> Result<Vec<u8>, String> {
        self.0.clone()
    }
}
fn check(reply: Result<Vec<u8>, String>) -> (&'static str, bool) {
    native_outcome(Boundary::with_backend(Box::new(Reply(reply))))
}

#[test]
fn native_check_approves_and_cancels_as_before() {
    assert_eq!(native_outcome(Boundary::synthetic()), ("approved", true));
    assert_eq!(
        check(Err(crate::confirmation::CANCELLED.into())),
        ("cancelled", true)
    );
    assert_eq!(native_outcome(Boundary::native()), ("unavailable", false));
}

#[test]
fn native_check_names_each_denial_with_a_fixed_code_and_fails() {
    use crate::native::LaunchError;
    let launch = |e: LaunchError| Err(e.to_string());
    let forged = format!(
        r#"{{"request_id":"{0}","request_digest":"{0}","outcome":"approved","authentication":"{0}"}}"#,
        "0".repeat(64)
    );
    for (reply, code) in [
        (launch(LaunchError::Failed), "denied_failed"),
        (launch(LaunchError::Timeout), "denied_timeout"),
        (launch(LaunchError::Channel), "denied_channel"),
        (launch(LaunchError::Oversized), "denied_oversized"),
        (launch(LaunchError::UntrustedFile), "denied_launch"),
        (launch(LaunchError::Spawn), "denied_launch"),
        (launch(LaunchError::IdentityMismatch), "denied_launch"),
        (launch(LaunchError::UnsupportedPin), "denied_launch"),
        (Err(CHANGED_AFTER.into()), "denied_expired"),
        (Ok(b"{\"approved\":true}".to_vec()), "denied_malformed"),
        (Ok(forged.into_bytes()), "denied_unauthenticated"),
        (Ok(vec![b' '; 4097]), "denied_oversized"),
    ] {
        let (line, success) = check(reply);
        assert_eq!(line, code);
        assert!(!success, "{code}");
    }
}

#[test]
fn native_check_reports_any_other_error_as_denied_other_without_its_text() {
    let secret = String::from_utf8(dummy().to_vec()).unwrap();
    for error in [
        secret.clone(),
        format!("{}: {secret}", crate::native::LaunchError::Timeout),
        format!("{secret} {}", crate::confirmation::MALFORMED),
        String::new(),
    ] {
        assert_eq!(check(Err(error)), ("denied_other", false));
    }
}

struct Account {
    _throwaway: Throwaway,
    keychain: Arc<Keychain>,
    paths: AccountPaths,
    account: crate::acquisition::Account,
}
impl Account {
    fn id(&self) -> &str {
        self.account.id.as_str()
    }
    fn key(&self) -> String {
        self.account.key_references()[0].to_owned()
    }
    fn identity(&self) -> String {
        format!("{}-identity", self.id())
    }
    fn settle(&self) -> Result<&'static str, &'static str> {
        settle(&self.keychain, &self.paths, self.id(), false)
    }
    fn adopt(&self) -> Result<&'static str, &'static str> {
        settle(&self.keychain, &self.paths, self.id(), true)
    }
    fn verify(&self) -> StoreCheck {
        verify(&self.paths, &self.reloaded(), self.keychain.clone())
    }
    fn reloaded(&self) -> crate::acquisition::Account {
        Accounts::new(self.paths.clone(), Arc::new(Refusing))
            .load(&self.account.id)
            .unwrap()
    }
    fn opens(&self) -> bool {
        Accounts::new(self.paths.clone(), self.keychain.clone())
            .store(&self.reloaded())
            .and_then(|s| s.load_state().map_err(Into::into))
            .is_ok()
    }
    fn read(&self, reference: &str) -> Zeroizing<Vec<u8>> {
        self.keychain.read(reference).unwrap()
    }
    /// Moves a current key somewhere an earlier build or an interruption could leave it.
    fn leave(&self, reference: &str, place: &str, value: &[u8]) {
        if self.keychain.status(reference) != Status::Missing {
            assert_eq!(self.keychain.remove(reference), Status::Ok);
        }
        let created = match place {
            "current" => self.keychain.create(reference, value),
            "staged" => self.keychain.create_staged_for_test(reference, value),
            "legacy" => self.keychain.create_legacy_for_test(reference, value),
            _ => self.keychain.create_earlier_for_test(reference, value),
        };
        assert_eq!(created, Status::Ok);
    }
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
    drop(store);
    Account {
        _throwaway: throwaway,
        keychain,
        paths,
        account,
    }
}
/// An account rotated once, so it has a generation key and a separate identity key.
fn rotated() -> Account {
    let mut a = account();
    let repo = Accounts::new(a.paths.clone(), a.keychain.clone());
    repo.rotate(&a.account.id, &a.paths.data.join("effects"))
        .unwrap();
    a.account = a.reloaded();
    assert_eq!(a.key(), format!("{}-key-g1", a.id()));
    assert_eq!(a.verify(), StoreCheck::Verified);
    a
}
/// 128 bytes, the length of identity material, carrying a findable dummy.
fn planted_identity() -> Zeroizing<Vec<u8>> {
    let mut value = dummy();
    value.resize(128, b'x');
    value
}

#[test]
fn a_planted_key_fails_against_the_accounts_authenticated_state() {
    let a = account();
    let reference = a.key();
    let genuine = a.read(&reference);
    let with = |value: &[u8]| {
        verify(
            &a.paths,
            &a.account,
            Arc::new(Values(vec![(
                reference.clone(),
                Zeroizing::new(value.to_vec()),
            )])),
        )
    };
    assert_eq!(with(&genuine), StoreCheck::Verified);
    assert_eq!(with(&dummy()), StoreCheck::Mismatch);

    a.leave(&reference, "current", &dummy());
    assert_eq!(
        a.verify(),
        StoreCheck::Mismatch,
        "check-store reports the planted key"
    );
    assert!(
        !a.opens(),
        "the account refuses to open under the planted key"
    );
}

#[test]
fn a_planted_identity_fails_against_its_commitment_and_is_never_published() {
    let a = rotated();
    let identity = a.identity();
    let genuine = a.read(&identity);
    assert!(a.opens());

    a.leave(&identity, "current", &planted_identity());
    assert_eq!(
        a.verify(),
        StoreCheck::Mismatch,
        "status=verified is refused"
    );
    assert!(
        !a.opens(),
        "the unchanged state opens only with the committed identity"
    );
    assert_eq!(a.settle(), Err("mismatch"));

    a.leave(&identity, "staged", &planted_identity());
    assert_eq!(a.settle(), Err("mismatch"));
    assert_eq!(
        a.keychain.status(&identity),
        Status::Missing,
        "nothing was published"
    );

    assert_eq!(a.keychain.remove_staged_for_test(&identity), Status::Ok);
    a.leave(&identity, "staged", &genuine);
    assert_eq!(
        a.settle(),
        Ok("settled"),
        "the genuine staged copy recovers"
    );
    assert_eq!(a.read(&identity).as_slice(), genuine.as_slice());
    assert!(a.opens());
}

#[test]
fn every_key_of_an_account_moves_as_one_verified_set() {
    let a = rotated();
    let (key, identity) = (a.key(), a.identity());
    let (key_value, identity_value) = (a.read(&key), a.read(&identity));

    a.leave(&key, "legacy", &key_value);
    a.leave(&identity, "staged", &identity_value);
    assert!(!a.opens(), "neither key is where this build reads it");
    assert_eq!(a.settle(), Ok("settled"));
    assert_eq!(a.read(&key).as_slice(), key_value.as_slice());
    assert_eq!(a.read(&identity).as_slice(), identity_value.as_slice());
    assert!(a.opens());
    assert_eq!(a.settle(), Ok("unchanged"), "a rerun converges");

    a.leave(&identity, "staged", &identity_value);
    assert_eq!(
        a.settle(),
        Ok("settled"),
        "a companion left staged is recovered"
    );

    a.leave(&key, "earlier", &key_value);
    a.leave(&identity, "earlier", &identity_value);
    assert_eq!(
        a.settle(),
        Err("denied"),
        "reading an earlier build's keys needs a person"
    );
    for reference in [&key, &identity] {
        assert_eq!(
            a.keychain.status(reference),
            Status::OtherBinary,
            "nothing moved"
        );
    }
}

#[test]
fn a_planted_legacy_or_staged_key_is_never_published() {
    let a = account();
    let reference = a.key();
    let genuine = a.read(&reference);
    a.leave(&reference, "legacy", &dummy());
    assert_eq!(a.settle(), Err("mismatch"));
    assert_eq!(a.keychain.status(&reference), Status::Missing);
    assert_eq!(a.keychain.remove_legacy_for_test(&reference), Status::Ok);

    a.leave(&reference, "staged", &dummy());
    assert_eq!(a.settle(), Err("mismatch"));
    assert_eq!(a.keychain.status(&reference), Status::Missing);
    assert_eq!(a.keychain.remove_staged_for_test(&reference), Status::Ok);

    a.leave(&reference, "legacy", &genuine);
    assert_eq!(a.settle(), Ok("settled"));
    assert_eq!(a.verify(), StoreCheck::Verified);
}

#[test]
fn an_identity_saved_before_commitments_needs_the_owner_to_adopt_it() {
    let a = rotated();
    let metadata = a.paths.config.join(format!("{}.json", a.id()));
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
    assert!(
        saved
            .as_object_mut()
            .unwrap()
            .remove("identity_commitment")
            .is_some()
    );
    std::fs::write(&metadata, serde_json::to_vec(&saved).unwrap()).unwrap();

    assert!(!a.opens(), "an uncommitted identity does not open");
    assert_eq!(a.verify(), StoreCheck::Uncommitted);
    assert_eq!(a.settle(), Err("uncommitted"));

    let key = a.key();
    let genuine = a.read(&key);
    a.leave(&key, "current", &dummy());
    assert_eq!(
        a.adopt(),
        Err("mismatch"),
        "adoption still needs the genuine encryption key"
    );
    assert!(a.reloaded().uncommitted());

    a.leave(&key, "current", &genuine);
    assert_eq!(a.adopt(), Ok("unchanged"));
    assert!(!a.reloaded().uncommitted());
    assert!(a.opens());
    assert_eq!(a.verify(), StoreCheck::Verified);
    assert_eq!(
        a.adopt(),
        Err("refused"),
        "a committed identity is never adopted again"
    );
}
