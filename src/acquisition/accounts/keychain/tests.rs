use super::*;
use crate::acquisition::{AccountPaths, Accounts, Provider};
use rand::{RngCore, rngs::OsRng};
use std::{fs, sync::Arc};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainCreate(
        path: *const c_char,
        password_length: u32,
        password: *const c_void,
        prompt_user: u8,
        initial_access: *const c_void,
        keychain: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainDelete(keychain: *mut c_void) -> i32;
}

/// A keychain file created in a private temporary directory for one test and deleted when
/// it ends. It never joins the search list, and its random unlock password stays in memory.
pub(crate) struct Throwaway {
    handle: *mut c_void,
    pub path: PathBuf,
    pub dir: tempfile::TempDir,
}
unsafe impl Send for Throwaway {}
impl Throwaway {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("throwaway.keychain-db");
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let mut password = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut password[..]);
        let mut handle = std::ptr::null_mut();
        let status = unsafe {
            SecKeychainCreate(
                c_path.as_ptr(),
                32,
                password.as_ptr().cast(),
                0,
                std::ptr::null(),
                &mut handle,
            )
        };
        assert_eq!(status, 0, "throwaway keychain");
        Self { handle, path, dir }
    }
    pub fn keychain(&self) -> Keychain {
        explicit(&self.path).unwrap()
    }
}
impl Drop for Throwaway {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(SecKeychainDelete(self.handle), 0);
            core_foundation::base::CFRelease(self.handle as CFTypeRef);
        }
        assert!(!self.path.exists());
    }
}

pub(crate) fn dummy() -> Zeroizing<Vec<u8>> {
    let mut random = [0u8; 12];
    OsRng.fill_bytes(&mut random);
    Zeroizing::new(
        format!(
            "nk-dummy-secret-{}",
            data_encoding::HEXLOWER.encode(&random)
        )
        .into_bytes(),
    )
}

fn application(path: &str) -> CFType {
    let path = CString::new(path).unwrap();
    let mut application = std::ptr::null_mut();
    assert_eq!(
        unsafe { SecTrustedApplicationCreateFromPath(path.as_ptr(), &mut application) },
        0
    );
    unsafe { CFType::wrap_under_create_rule(application as CFTypeRef) }
}
/// An ad-hoc signed binary other than this one, trusted the way an earlier build would be.
fn other() -> CFType {
    application(crate::native::tests::fake_helper().to_str().unwrap())
}
fn own() -> CFType {
    let (application, _) = own_application().unwrap();
    unsafe { CFType::wrap_under_get_rule(application.0 as CFTypeRef) }
}
fn trusting(applications: &[CFType]) -> CFArray<CFType> {
    CFArray::from_CFTypes(applications)
}

/// No file under the test directory, keychain included, holds the dummy or its base64.
fn assert_no_plaintext(root: &Path, dummy: &[u8]) {
    let encoded = data_encoding::BASE64.encode(dummy);
    for path in walk(root) {
        let bytes = fs::read(path).unwrap();
        for needle in [dummy, encoded.as_bytes()] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "plaintext in a file"
            );
        }
    }
}
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path))
        } else {
            files.push(path)
        }
    }
    files
}

#[test]
fn this_binary_is_trusted_by_its_kernel_cdhash() {
    let (_, requirement) = own_application().unwrap();
    assert_eq!(
        requirement,
        format!("cdhash H\"{}\"", crate::native::own_cdhash().unwrap())
    );
}

#[test]
fn pinned_keys_round_trip_without_overwrite_and_delete_idempotently() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    assert_eq!(keychain.create("acct-census-key", &value), Status::Ok);
    assert_eq!(keychain.status("acct-census-key"), Status::Ok);
    assert_eq!(
        keychain.read("acct-census-key").unwrap().as_slice(),
        value.as_slice()
    );
    assert_eq!(keychain.create("acct-census-key", &dummy()), Status::Exists);
    assert_eq!(
        keychain.read("acct-census-key").unwrap().as_slice(),
        value.as_slice()
    );
    assert_eq!(keychain.remove("acct-census-key"), Status::Ok);
    assert_eq!(
        keychain.read("acct-census-key").err(),
        Some(Status::Missing)
    );
    assert_eq!(keychain.remove("acct-census-key"), Status::Missing);
    assert_no_plaintext(throwaway.dir.path(), &value);
}

#[test]
fn passwords_trust_no_application_and_every_read_needs_a_person() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    assert_eq!(keychain.create("acct-password", &value), Status::Ok);
    assert_eq!(keychain.status("acct-password"), Status::Ok);
    for _ in 0..2 {
        assert_eq!(keychain.read("acct-password").err(), Some(Status::Denied));
    }
    assert_no_plaintext(throwaway.dir.path(), &value);
}

/// "Always Allow" or a Keychain Access edit adds an application. Keychain would then hand
/// the password over silently; the read boundary refuses it instead.
#[test]
fn a_granted_or_edited_access_list_is_refused_at_read() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    assert_eq!(
        keychain.add(SERVICE, "acct-password", &trusting(&[own()]), &value),
        Status::Ok
    );
    let item = keychain.item(SERVICE, "acct-password").unwrap();
    assert_eq!(
        keychain.data(&item).unwrap().as_slice(),
        value.as_slice(),
        "Keychain itself reads silently"
    );
    assert_eq!(keychain.status("acct-password"), Status::Changed);
    assert_eq!(keychain.read("acct-password").err(), Some(Status::Changed));
    assert_eq!(
        keychain.add(SERVICE, "b-password", &trusting(&[other()]), &value),
        Status::Ok
    );
    assert_eq!(keychain.read("b-password").err(), Some(Status::Changed));
    assert_eq!(
        keychain.add(
            SERVICE,
            "acct-census-key",
            &trusting(&[own(), other()]),
            &value
        ),
        Status::Ok
    );
    assert_eq!(
        keychain.read("acct-census-key").err(),
        Some(Status::Changed)
    );
    assert_eq!(
        keychain.add(SERVICE, "acct-key-g2", &trusting(&[]), &value),
        Status::Ok
    );
    assert_eq!(keychain.read("acct-key-g2").err(), Some(Status::Changed));
}

#[test]
fn a_key_trusting_another_binary_is_reported_and_never_read_silently() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    assert_eq!(
        keychain.add(SERVICE, "acct-census-key", &trusting(&[other()]), &value),
        Status::Ok
    );
    assert_eq!(keychain.status("acct-census-key"), Status::OtherBinary);
    assert_eq!(
        keychain.read("acct-census-key").err(),
        Some(Status::OtherBinary)
    );
    assert_eq!(
        keychain.gather("acct-census-key").err(),
        Some(Status::Denied),
        "reading an earlier build's key needs a person"
    );
    assert_eq!(
        keychain.status("acct-census-key"),
        Status::OtherBinary,
        "the original stays"
    );
    assert_eq!(keychain.status("acct-census-key-retrust"), Status::Missing);
}

fn gathered(keychain: &Keychain, reference: &str) -> (String, Zeroizing<Vec<u8>>) {
    let key = keychain.gather(reference).unwrap();
    let origin = format!("{:?}", key.origin);
    keychain.settle(&key).unwrap();
    (origin, key.value)
}

#[test]
fn every_place_a_key_can_be_left_settles_into_one_current_item() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    assert_eq!(
        keychain.create_staged_for_test("acct-identity", &value),
        Status::Ok
    );
    let (origin, found) = gathered(&keychain, "acct-identity");
    assert_eq!(
        (origin.as_str(), found.as_slice()),
        ("Staged", value.as_slice())
    );
    assert_eq!(
        keychain.read("acct-identity").unwrap().as_slice(),
        value.as_slice(),
        "an interrupted move finishes from its staged copy"
    );
    assert_eq!(
        keychain.status_as("acct-identity-retrust", Class::PinnedSelf),
        Status::Missing
    );
    assert!(
        !keychain.gather("acct-identity").unwrap().moves(),
        "a rerun converges"
    );

    assert_eq!(
        keychain.create_staged_for_test("acct-identity", &value),
        Status::Ok
    );
    assert_eq!(gathered(&keychain, "acct-identity").0, "CurrentAndStaged");
    assert_eq!(
        keychain.status_as("acct-identity-retrust", Class::PinnedSelf),
        Status::Missing,
        "an equal leftover copy is removed"
    );
    assert_eq!(
        keychain.create_staged_for_test("acct-identity", &dummy()),
        Status::Ok
    );
    assert_eq!(
        keychain.gather("acct-identity").err(),
        Some(Status::Changed)
    );

    legacy(&keychain, "acct-key-g1", &value, &[own()]);
    assert_eq!(gathered(&keychain, "acct-key-g1").0, "Legacy");
    assert_eq!(
        keychain.read("acct-key-g1").unwrap().as_slice(),
        value.as_slice()
    );
    assert_eq!(
        keychain.legacy_references().unwrap(),
        vec!["acct-key-g1".to_owned()],
        "the legacy original stays until it is retired"
    );
    assert_eq!(keychain.gather("acct-key-g2").err(), Some(Status::Missing));
    assert_eq!(keychain.gather("acct-password").err(), Some(Status::Failed));
    assert_no_plaintext(throwaway.dir.path(), &value);
}

fn legacy(keychain: &Keychain, reference: &str, value: &[u8], trusted: &[CFType]) {
    assert_eq!(
        keychain.add(LEGACY_SERVICE, reference, &trusting(trusted), value),
        Status::Ok
    );
}

#[test]
fn migration_moves_passwords_only_and_retirement_needs_an_equal_protected_value() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    let value = dummy();
    legacy(&keychain, "a-census-key", &value, &[own()]);
    assert_eq!(
        keychain.legacy_references().unwrap(),
        vec!["a-census-key".to_owned()]
    );
    assert_eq!(
        keychain.migrate("a-census-key"),
        Err(Migration::Refused),
        "keys move only with their whole account"
    );
    assert_eq!(keychain.status("a-census-key"), Status::Missing);
    gathered(&keychain, "a-census-key");
    assert_eq!(keychain.retire_legacy("a-census-key"), Ok(()));
    assert!(keychain.legacy_references().unwrap().is_empty());
    assert_eq!(
        keychain.read("a-census-key").unwrap().as_slice(),
        value.as_slice()
    );

    assert_eq!(keychain.create("b-census-key", &dummy()), Status::Ok);
    legacy(&keychain, "b-census-key", &value, &[own()]);
    assert_eq!(
        keychain.retire_legacy("b-census-key"),
        Err(Migration::Mismatch)
    );
    assert_eq!(
        keychain.legacy_references().unwrap(),
        vec!["b-census-key".to_owned()],
        "the original stays"
    );

    legacy(&keychain, "c-census-key", &value, &[other()]);
    assert_eq!(keychain.gather("c-census-key").err(), Some(Status::Denied));
    assert_eq!(keychain.status("c-census-key"), Status::Missing);

    legacy(&keychain, "d-password", &value, &[own()]);
    assert_eq!(
        keychain.migrate("d-password"),
        Err(Migration::Protected(Status::Denied))
    );
    assert_eq!(keychain.status("d-password"), Status::Ok);
    assert_eq!(
        keychain.retire_legacy("d-password"),
        Err(Migration::Protected(Status::Denied))
    );
    assert!(
        keychain
            .legacy_references()
            .unwrap()
            .contains(&"d-password".to_owned())
    );

    assert_eq!(
        keychain.add(SERVICE, "e-census-key", &trusting(&[other()]), &value),
        Status::Ok
    );
    legacy(&keychain, "e-census-key", &value, &[own()]);
    assert_eq!(keychain.gather("e-census-key").err(), Some(Status::Denied));
    assert_eq!(
        keychain.retire_legacy("e-census-key"),
        Err(Migration::Protected(Status::OtherBinary))
    );
    assert_eq!(
        keychain.migrate("f-password"),
        Err(Migration::Legacy(Status::Missing))
    );
    assert_no_plaintext(throwaway.dir.path(), &value);
}

#[test]
fn explicit_keychains_never_open_the_login_or_system_keychains() {
    let throwaway = Throwaway::new();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for path in [
        home.join("Library/Keychains/login.keychain-db"),
        PathBuf::from("/Library/Keychains/System.keychain"),
        throwaway.dir.path().join("absent.keychain-db"),
        PathBuf::from("relative.keychain-db"),
    ] {
        assert!(explicit(&path).is_err(), "{}", path.display());
    }
    let link = throwaway.dir.path().join("link.keychain-db");
    std::os::unix::fs::symlink(home.join("Library/Keychains/login.keychain-db"), &link).unwrap();
    assert!(explicit(&link).is_err(), "a symlink to the login keychain");
}

fn chmod(arguments: &[&str], path: &Path) {
    let status = std::process::Command::new("/bin/chmod")
        .args(arguments)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn explicit_keychains_refuse_second_links_and_granting_acls() {
    let throwaway = Throwaway::new();
    let me = std::process::Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .unwrap();
    let me = String::from_utf8(me.stdout).unwrap();
    let me = me.trim();
    assert!(explicit(&throwaway.path).is_ok());

    let alias = throwaway.dir.path().join("alias.keychain-db");
    fs::hard_link(&throwaway.path, &alias).unwrap();
    assert!(
        explicit(&throwaway.path).is_err(),
        "a file with a second link"
    );
    assert!(explicit(&alias).is_err(), "the second link itself");
    fs::remove_file(&alias).unwrap();

    let directory = throwaway.dir.path();
    chmod(&["+a", &format!("user:{me} allow add_file")], directory);
    assert!(
        explicit(&throwaway.path).is_err(),
        "an ACL granting writes above the file"
    );
    chmod(&["-N"], directory);
    chmod(&["+a", &format!("user:{me} allow write")], &throwaway.path);
    assert!(
        explicit(&throwaway.path).is_err(),
        "an ACL granting writes on the file"
    );
    chmod(&["-N"], &throwaway.path);
    chmod(&["+a", "everyone deny delete"], directory);
    assert!(
        explicit(&throwaway.path).is_ok(),
        "a deny entry grants nothing"
    );
    chmod(&["-N"], directory);
}

#[test]
fn production_backend_refuses_this_unhardened_binary() {
    assert!(installed().is_err());
    let os = crate::acquisition::OsCredentials;
    assert_eq!(
        os.get("acct-census-key").err(),
        Some(AcquisitionError::Credentials)
    );
    assert_eq!(
        os.put("acct-census-key", &SecretValue::new(dummy().to_vec()))
            .err(),
        Some(AcquisitionError::Credentials)
    );
    assert_eq!(
        os.delete("acct-census-key").err(),
        Some(AcquisitionError::Credentials)
    );
}

#[test]
fn account_lifecycle_runs_on_the_keychain_and_never_reads_the_password_silently() {
    let throwaway = Throwaway::new();
    let root = throwaway.dir.path().join("accounts");
    let repo = Accounts::new(
        AccountPaths {
            config: root.join("config"),
            data: root.join("data"),
        },
        Arc::new(throwaway.keychain()),
    );
    let password = SecretValue::new(dummy().to_vec());
    let account = repo
        .create(Provider::MailboxOrg, "login@example.test".into(), &password)
        .unwrap();
    repo.store(&account).unwrap();
    let denied = repo.password(&account).err().unwrap();
    assert_eq!(denied, AcquisitionError::Credentials);
    let text = format!("{denied} {denied:?}");
    assert!(!text.contains("nk-dummy-secret"));
    let keychain = throwaway.keychain();
    assert_eq!(Class::of(&account.password_ref), Class::EveryAccess);
    assert_eq!(keychain.status(&account.password_ref), Status::Ok);
    assert_eq!(Class::of(&account.local_secret_ref), Class::PinnedSelf);
    assert_eq!(keychain.status(&account.local_secret_ref), Status::Ok);
    assert_no_plaintext(throwaway.dir.path(), password.bytes());
}

#[test]
fn reference_classes() {
    for (reference, class) in [
        ("a-census-key", Class::PinnedSelf),
        ("a-key-g3", Class::PinnedSelf),
        ("a-identity", Class::PinnedSelf),
        ("a-password", Class::EveryAccess),
        ("a-password-0a1b", Class::EveryAccess),
        ("a-key-g", Class::EveryAccess),
        ("a-census-key-retrust", Class::EveryAccess),
        ("unknown", Class::EveryAccess),
    ] {
        assert_eq!(Class::of(reference), class, "{reference}");
    }
    for bad in ["", "a/b", "a b", "ä", &"a".repeat(129)] {
        assert!(!valid_reference(bad));
    }
}

const READ: [&str; 6] = [
    "ACLAuthorizationDecrypt",
    "ACLAuthorizationDerive",
    "ACLAuthorizationExportClear",
    "ACLAuthorizationExportWrapped",
    "ACLAuthorizationMAC",
    "ACLAuthorizationSign",
];
fn entry(authorizations: &[&str], trusted: Option<&[&str]>, selector: u16) -> Entry {
    Entry {
        authorizations: authorizations.iter().map(|a| a.to_string()).collect(),
        trusted: trusted.map(|t| t.iter().map(|r| r.to_string()).collect()),
        selector,
    }
}
fn created(trusted: &[&str]) -> Vec<Entry> {
    vec![
        entry(&READ, Some(trusted), 0),
        entry(&["ACLAuthorizationEncrypt"], None, 0),
        entry(&["ACLAuthorizationChangeACL"], Some(&[]), 0),
    ]
}

#[test]
fn created_items_have_exactly_the_entries_the_policy_expects() {
    let throwaway = Throwaway::new();
    let keychain = throwaway.keychain();
    assert_eq!(keychain.create("acct-census-key", &dummy()), Status::Ok);
    assert_eq!(keychain.create("acct-password", &dummy()), Status::Ok);
    let own = format!("cdhash H\"{}\"", crate::native::own_cdhash().unwrap());
    let mut key = Keychain::entries(&keychain.item(SERVICE, "acct-census-key").unwrap()).unwrap();
    let mut password =
        Keychain::entries(&keychain.item(SERVICE, "acct-password").unwrap()).unwrap();
    key.sort_by_key(|e| e.authorizations.clone());
    password.sort_by_key(|e| e.authorizations.clone());
    let mut expected_key = created(&[&own]);
    let mut expected_password = created(&[]);
    expected_key.sort_by_key(|e| e.authorizations.clone());
    expected_password.sort_by_key(|e| e.authorizations.clone());
    assert_eq!(key, expected_key);
    assert_eq!(password, expected_password);
}

#[test]
fn the_policy_refuses_every_edited_shape() {
    let own = "cdhash H\"0000000000000000000000000000000000000000\"";
    assert_eq!(classify(&created(&[])), Observed::EveryAccess);
    assert_eq!(classify(&created(&[own])), Observed::Pinned(own.into()));
    let mut edits: Vec<Vec<Entry>> = Vec::new();
    for selector in [1u16, 0x0010, 0xffff] {
        let mut e = created(&[]);
        e[0].selector = selector;
        edits.push(e);
        let mut e = created(&[]);
        e[2].selector = selector;
        edits.push(e);
    }
    let mut e = created(&[]);
    e[0].trusted = None;
    edits.push(e);
    let mut e = created(&[]);
    e[2].trusted = Some(vec![own.into()]);
    edits.push(e);
    let mut e = created(&[own]);
    e.push(entry(
        &["ACLAuthorizationDecrypt"],
        Some(&["cdhash H\"1111111111111111111111111111111111111111\""]),
        0,
    ));
    edits.push(e);
    let mut e = created(&[]);
    e.push(entry(&["ACLAuthorizationPartitionID"], None, 0));
    edits.push(e);
    edits.push(created(&[own, "anchor apple"]));
    edits.push(vec![entry(&["ACLAuthorizationEncrypt"], None, 0)]);
    edits.push(vec![]);
    for (index, edit) in edits.iter().enumerate() {
        assert_eq!(classify(edit), Observed::Changed, "edit {index}");
    }
}
