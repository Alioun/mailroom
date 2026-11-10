use super::*;
use crate::acquisition::{AccountPaths, Accounts, Provider, lifecycle};
use std::{
    os::unix::fs::symlink,
    path::PathBuf,
    sync::{Arc, Barrier, atomic::Ordering},
};

const PASSWORD: &str = "invented master password 7f3a";
const CANARY: &[u8] = b"VAULT-CANARY-0d9e4c";

fn root() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("vault");
    (directory, path)
}
fn open(root: &Path) -> Vault {
    Vault::open_with(root, Cost::Test).unwrap()
}
fn secret(value: &[u8]) -> SecretValue {
    SecretValue::new(value.to_vec())
}
fn created(root: &Path) -> Vault {
    let vault = open(root);
    vault
        .create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes()))
        .unwrap();
    vault
}
fn derivations(vault: &Vault) -> usize {
    vault.derivations.load(Ordering::SeqCst)
}
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}
fn replace_private(path: &Path, bytes: &[u8]) {
    let _ = std::fs::remove_file(path);
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn stages(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(STAGE)
        })
        .count()
}
impl Vault {
    fn references(&self) -> Vec<String> {
        match &*self.state() {
            State::Unlocked(open) => open.entries.keys().cloned().collect(),
            _ => panic!("vault not unlocked"),
        }
    }
}

#[test]
fn created_vault_survives_restart_and_opens_only_with_its_password() {
    let (_directory, root) = root();
    let vault = open(&root);
    assert_eq!(vault.status(), VaultStatus::Absent);
    assert_eq!(
        vault.get("a-password").err(),
        Some(AcquisitionError::VaultLocked)
    );
    assert_eq!(
        vault.put("a-password", &secret(CANARY)).err(),
        Some(AcquisitionError::VaultLocked)
    );
    assert_eq!(
        vault.unlock(&secret(PASSWORD.as_bytes())),
        Err(VaultError::Absent)
    );
    vault
        .create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes()))
        .unwrap();
    assert_eq!(vault.status(), VaultStatus::Unlocked);
    vault.put("a-password", &secret(CANARY)).unwrap();
    vault.put("a-census-key", &secret(&[7; 32])).unwrap();
    let file = root.join("vault.mrv");
    let bytes = std::fs::read(&file).unwrap();
    assert!(!contains(&bytes, CANARY), "plaintext credential on disk");
    assert!(
        !contains(&bytes, b"a-password"),
        "plaintext reference on disk"
    );
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&root), 0o700);
    assert_eq!(
        vault.create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes())),
        Err(VaultError::Exists)
    );
    drop(vault);

    let vault = open(&root);
    assert_eq!(vault.status(), VaultStatus::Locked);
    assert_eq!(
        vault.get("a-password").err(),
        Some(AcquisitionError::VaultLocked)
    );
    assert_eq!(
        vault.unlock(&secret(b"invented wrong password 0000")),
        Err(VaultError::UnlockFailed)
    );
    assert_eq!(vault.status(), VaultStatus::Locked);
    assert_eq!(
        vault.create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes())),
        Err(VaultError::Exists)
    );
    vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
    assert_eq!(vault.get("a-password").unwrap().bytes(), CANARY);
    assert_eq!(vault.get("a-census-key").unwrap().bytes(), &[7; 32]);
    assert_eq!(
        vault.get("b-password").err(),
        Some(AcquisitionError::CredentialMissing)
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "unlock never rewrites"
    );
}

#[test]
fn weak_or_mismatched_master_passwords_are_refused_before_derivation() {
    let (_directory, root) = root();
    let vault = open(&root);
    for (password, confirmation, error) in [
        ("eleven char", "eleven char", VaultError::WeakPassword),
        (
            "twelve chars\u{7}",
            "twelve chars\u{7}",
            VaultError::WeakPassword,
        ),
        (
            "invented password one",
            "invented password two",
            VaultError::Mismatch,
        ),
    ] {
        assert_eq!(
            vault.create(
                &secret(password.as_bytes()),
                &secret(confirmation.as_bytes())
            ),
            Err(error)
        );
    }
    let long = "x".repeat(1025);
    assert_eq!(
        vault.create(&secret(long.as_bytes()), &secret(long.as_bytes())),
        Err(VaultError::WeakPassword)
    );
    assert_eq!(derivations(&vault), 0);
    assert_eq!(vault.status(), VaultStatus::Absent);
    assert!(!root.join("vault.mrv").exists());
    let unicode = "zwölf Zeichen";
    vault
        .create(&secret(unicode.as_bytes()), &secret(unicode.as_bytes()))
        .unwrap();
    assert_eq!(derivations(&vault), 1);
}

#[test]
fn malformed_files_are_refused_before_any_key_derivation_and_left_unchanged() {
    let (_directory, root) = root();
    let file = root.join("vault.mrv");
    let original = {
        let vault = created(&root);
        vault.put("a-password", &secret(CANARY)).unwrap();
        std::fs::read(&file).unwrap()
    };
    let mut cases: Vec<(Vec<u8>, Problem)> = (0..original.len())
        .map(|n| (original[..n].to_vec(), Problem::Damaged))
        .collect();
    cases.push(([original.as_slice(), b"x"].concat(), Problem::Damaged));
    let mut version = original.clone();
    version[9] = 2;
    cases.push((version, Problem::UnsupportedVersion));
    let mut magic = original.clone();
    magic[0] ^= 1;
    cases.push((magic, Problem::Damaged));
    for generation in [0, MAX_GENERATION + 1] {
        let mut bytes = original.clone();
        bytes[26..34].copy_from_slice(&generation.to_be_bytes());
        cases.push((bytes, Problem::Damaged));
    }
    let mut length = original.clone();
    length[46..50].copy_from_slice(&u32::MAX.to_be_bytes());
    cases.push((length, Problem::Damaged));
    let mut oversized = original[..HEADER].to_vec();
    oversized[46..50].copy_from_slice(&((2 << 20) as u32).to_be_bytes());
    oversized.resize(HEADER + (2 << 20), 0);
    cases.push((oversized, Problem::Damaged));
    for (bytes, problem) in cases {
        replace_private(&file, &bytes);
        let vault = open(&root);
        assert_eq!(vault.status(), VaultStatus::Unavailable);
        assert_eq!(vault.problem(), Some(problem));
        assert_eq!(
            vault.unlock(&secret(PASSWORD.as_bytes())),
            Err(VaultError::Unavailable)
        );
        assert_eq!(
            vault.create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes())),
            Err(VaultError::Unavailable)
        );
        assert_eq!(
            vault.get("a-password").err(),
            Some(AcquisitionError::VaultLocked)
        );
        assert_eq!(derivations(&vault), 0, "derived a key for a malformed file");
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
}

#[test]
fn every_flipped_bit_fails_closed() {
    let (_directory, root) = root();
    let file = root.join("vault.mrv");
    let original = {
        let vault = created(&root);
        vault.put("a-password", &secret(CANARY)).unwrap();
        std::fs::read(&file).unwrap()
    };
    for position in 0..original.len() {
        for bit in [0x01, 0x80] {
            let mut bytes = original.clone();
            bytes[position] ^= bit;
            replace_private(&file, &bytes);
            let vault = open(&root);
            match vault.unlock(&secret(PASSWORD.as_bytes())) {
                Err(VaultError::UnlockFailed) => assert_eq!(vault.status(), VaultStatus::Locked),
                Err(VaultError::Unavailable) => {
                    assert_eq!(vault.status(), VaultStatus::Unavailable)
                }
                other => panic!("byte {position} bit {bit:#x}: {other:?}"),
            }
            assert_eq!(
                vault.get("a-password").err(),
                Some(AcquisitionError::VaultLocked)
            );
        }
    }
    replace_private(&file, &original);
    open(&root).unlock(&secret(PASSWORD.as_bytes())).unwrap();
}

#[test]
fn unsafe_files_and_roots_are_refused_without_repair() {
    let (directory, root) = root();
    let file = root.join("vault.mrv");
    drop(created(&root));
    let elsewhere = directory.path().canonicalize().unwrap().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();

    std::fs::rename(&file, elsewhere.join("vault.mrv")).unwrap();
    symlink(elsewhere.join("vault.mrv"), &file).unwrap();
    let vault = open(&root);
    assert_eq!(vault.problem(), Some(Problem::UnsafeFile));
    assert_eq!(
        vault.unlock(&secret(PASSWORD.as_bytes())),
        Err(VaultError::Unavailable)
    );
    drop(vault);
    assert!(
        std::fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::rename(elsewhere.join("vault.mrv"), &file).unwrap();

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(open(&root).problem(), Some(Problem::UnsafeFile));
    assert_eq!(mode(&file), 0o644);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

    std::fs::hard_link(&file, elsewhere.join("second-link")).unwrap();
    assert_eq!(open(&root).problem(), Some(Problem::UnsafeFile));
    assert_eq!(std::fs::metadata(&file).unwrap().nlink(), 2);
    std::fs::remove_file(elsewhere.join("second-link")).unwrap();
    assert_eq!(open(&root).status(), VaultStatus::Locked);

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        Vault::open_with(&root, Cost::Test).err(),
        Some(VaultError::Unsafe)
    );
    assert_eq!(mode(&root), 0o755);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

    let parent = directory.path().canonicalize().unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(
        Vault::open_with(&root, Cost::Test).err(),
        Some(VaultError::Unsafe)
    );
    assert_eq!(mode(&parent), 0o770);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();

    let other = parent.join("other");
    drop(open(&other));
    std::fs::remove_file(other.join("vault.lock")).unwrap();
    symlink(elsewhere.join("lock-target"), other.join("vault.lock")).unwrap();
    assert_eq!(
        Vault::open_with(&other, Cost::Test).err(),
        Some(VaultError::Unsafe)
    );
    assert!(!elsewhere.join("lock-target").exists());

    let aliased = parent.join("aliased");
    symlink(&root, &aliased).unwrap();
    assert_eq!(
        Vault::open_with(&aliased, Cost::Test).err(),
        Some(VaultError::Unsafe)
    );
}

#[test]
fn failed_commits_keep_the_previous_generation_and_uncertain_ones_lock() {
    let (_directory, root) = root();
    let vault = created(&root);
    vault.put("a-key", &secret(b"one")).unwrap();
    let before = std::fs::read(root.join("vault.mrv")).unwrap();

    lifecycle::inject_failure(Some("vault_staged"));
    assert_eq!(
        vault.put("b-key", &secret(b"two")).err(),
        Some(AcquisitionError::Storage)
    );
    lifecycle::inject_failure(None);
    assert_eq!(vault.status(), VaultStatus::Unlocked);
    assert_eq!(
        vault.get("b-key").err(),
        Some(AcquisitionError::CredentialMissing)
    );
    assert_eq!(std::fs::read(root.join("vault.mrv")).unwrap(), before);
    assert_eq!(stages(&root), 0);

    lifecycle::inject_failure(Some("vault_renamed"));
    assert_eq!(
        vault.put("b-key", &secret(b"two")).err(),
        Some(AcquisitionError::Storage)
    );
    lifecycle::inject_failure(None);
    assert_eq!(vault.status(), VaultStatus::Locked);
    assert_eq!(
        vault.get("a-key").err(),
        Some(AcquisitionError::VaultLocked)
    );
    drop(vault);

    let vault = open(&root);
    vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
    assert_eq!(vault.get("b-key").unwrap().bytes(), b"two");
    assert_eq!(vault.get("a-key").unwrap().bytes(), b"one");
}

#[test]
fn every_commit_writes_a_fresh_nonce_and_the_next_generation() {
    let (_directory, root) = root();
    let vault = created(&root);
    let mut nonces = std::collections::BTreeSet::new();
    for n in 0..1000u32 {
        vault.put("a-key", &secret(&n.to_be_bytes())).unwrap();
        let bytes = std::fs::read(root.join("vault.mrv")).unwrap();
        assert!(nonces.insert(bytes[34..46].to_vec()), "nonce repeated");
        assert_eq!(parse(&bytes).unwrap().generation, u64::from(n) + 2);
    }
}

#[test]
fn entry_bounds_and_references_are_enforced() {
    let (_directory, root) = root();
    let vault = created(&root);
    for reference in ["", "Upper", "a/b", "a.b", &"a".repeat(129)] {
        assert_eq!(
            vault.put(reference, &secret(b"v")).err(),
            Some(AcquisitionError::InvalidInput)
        );
    }
    assert_eq!(
        vault.put("a-key", &secret(&[1; MAX_VALUE + 1])).err(),
        Some(AcquisitionError::InvalidInput)
    );
    vault
        .put(&"a".repeat(128), &secret(&[1; MAX_VALUE]))
        .unwrap();
    let full: Entries = (0..MAX_ENTRIES)
        .map(|n| (format!("r{n:05}"), Zeroizing::new(vec![0; 8])))
        .collect();
    let sealed = seal(&[9; 32], &[1; 16], 2, &full).unwrap();
    assert_eq!(
        decode(&unseal(&[9; 32], &sealed).unwrap()).unwrap().len(),
        MAX_ENTRIES
    );
    let mut over = full.clone();
    over.insert("s".into(), Zeroizing::new(vec![]));
    assert_eq!(
        seal(&[9; 32], &[1; 16], 2, &over).err(),
        Some(AcquisitionError::Storage)
    );
    for plaintext in [
        &[0, 1, 1, b'a', 0, 0, 9][..],
        &[0, 2, 1, b'b', 0, 0, 1, b'a', 0, 0][..],
        &[0, 2, 1, b'a', 0, 0, 1, b'a', 0, 0][..],
        &[0, 1, 1, b'A', 0, 0][..],
        &[0, 1, 1, b'a', 0x10, 0x01][..],
    ] {
        assert!(decode(plaintext).is_err());
    }
}

#[test]
fn delete_owned_removes_only_the_checked_value() {
    let (_directory, root) = root();
    let vault = created(&root);
    vault.put("a-key", &secret(CANARY)).unwrap();
    assert!(!vault.delete_owned("a-key", &|v| v == b"other").unwrap());
    assert_eq!(vault.get("a-key").unwrap().bytes(), CANARY);
    assert!(vault.delete_owned("a-key", &|v| v == CANARY).unwrap());
    assert!(!vault.delete_owned("a-key", &|_| true).unwrap());
    vault.delete("a-key").unwrap();
    drop(vault);
    let vault = open(&root);
    assert_eq!(
        vault.delete_owned("a-key", &|_| true).err(),
        Some(AcquisitionError::VaultLocked)
    );
    vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
    assert_eq!(
        vault.get("a-key").err(),
        Some(AcquisitionError::CredentialMissing)
    );
}

#[test]
fn concurrent_unlocks_derive_one_key() {
    let (_directory, root) = root();
    drop(created(&root));
    let vault = Arc::new(open(&root));
    let barrier = Arc::new(Barrier::new(8));
    let results: Vec<_> = (0..8)
        .map(|_| {
            let (vault, barrier) = (vault.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                vault.unlock(&secret(PASSWORD.as_bytes()))
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert_eq!(derivations(&vault), 1);
    assert!(
        results
            .iter()
            .all(|r| matches!(r, Ok(()) | Err(VaultError::Busy)))
    );
    assert_eq!(vault.status(), VaultStatus::Unlocked);
}

#[test]
fn a_file_replaced_by_another_writer_locks_the_vault() {
    let (_directory, root) = root();
    let vault = created(&root);
    vault.put("a-key", &secret(b"one")).unwrap();
    let file = root.join("vault.mrv");
    let copy = root.join("copy");
    replace_private(&copy, &std::fs::read(&file).unwrap());
    std::fs::rename(&copy, &file).unwrap();
    assert_eq!(
        vault.put("b-key", &secret(b"two")).err(),
        Some(AcquisitionError::VaultLocked)
    );
    assert_eq!(vault.status(), VaultStatus::Locked);
    vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
    assert_eq!(
        vault.get("b-key").err(),
        Some(AcquisitionError::CredentialMissing)
    );
    assert_eq!(vault.get("a-key").unwrap().bytes(), b"one");
}

#[test]
fn standard_cost_vault_round_trips_and_differs_from_test_cost() {
    let (_directory, root) = root();
    let vault = Vault::open(&root).unwrap();
    vault
        .create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes()))
        .unwrap();
    vault.put("a-key", &secret(CANARY)).unwrap();
    drop(vault);
    let bytes = std::fs::read(root.join("vault.mrv")).unwrap();
    assert_eq!(&bytes[..10], b"MRVAULT1\x00\x01");
    let cheap = open(&root);
    assert_eq!(
        cheap.unlock(&secret(PASSWORD.as_bytes())),
        Err(VaultError::UnlockFailed)
    );
    drop(cheap);
    let vault = Vault::open(&root).unwrap();
    vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
    assert_eq!(vault.get("a-key").unwrap().bytes(), CANARY);
}

#[test]
fn interrupted_account_creation_recovers_exactly_the_named_references() {
    for boundary in ["create_key", "create_password", "create_data", "metadata"] {
        let (directory, root) = root();
        let vault = Arc::new(created(&root));
        let data = directory.path().canonicalize().unwrap().join("data");
        let repo = Accounts::new(
            AccountPaths {
                config: root.clone(),
                data: data.clone(),
            },
            vault.clone(),
        );
        lifecycle::inject_failure(Some(boundary));
        let result = repo.create_reviewed(
            Provider::Gmail,
            "owner@example.test".into(),
            &secret(CANARY),
            |_| Ok(()),
        );
        lifecycle::inject_failure(None);
        assert!(result.is_err(), "{boundary}");
        let pending = repo.recovery_status().unwrap();
        assert_eq!(pending.len(), 1, "{boundary}");
        let id = pending[0].account.clone();
        assert!(!vault.references().is_empty(), "{boundary}");
        repo.recover(&id, &data).unwrap();
        assert!(repo.recovery_status().unwrap().is_empty());
        if boundary == "metadata" {
            let account = repo.load(&id).unwrap();
            repo.store(&account).unwrap().load_state().unwrap();
            assert_eq!(
                vault.references(),
                vec![
                    format!("{}-census-key", id.as_str()),
                    format!("{}-password", id.as_str())
                ]
            );
        } else {
            assert!(repo.list().unwrap().is_empty(), "{boundary}");
            assert!(vault.references().is_empty(), "{boundary}");
        }
        let created = repo.list().unwrap().len();
        if created == 0 {
            repo.create_reviewed(
                Provider::Gmail,
                "owner@example.test".into(),
                &secret(CANARY),
                |_| Ok(()),
            )
            .unwrap();
        }
        assert_eq!(
            repo.create_reviewed(
                Provider::Gmail,
                "OWNER@example.test".into(),
                &secret(CANARY),
                |_| { Ok(()) }
            )
            .err(),
            Some(AcquisitionError::InvalidInput)
        );
        let bytes = std::fs::read(root.join("vault.mrv")).unwrap();
        assert!(!contains(&bytes, CANARY));
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                assert!(
                    !contains(&std::fs::read(&path).unwrap(), CANARY),
                    "{path:?}"
                );
            }
        }
    }
}

fn worker(root: &Path, mode: &str, pause: Option<&str>) -> std::process::Child {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "acquisition::accounts::vault::tests::vault_process_worker",
            "--nocapture",
        ])
        .env("MAILROOM_VAULT_WORKER_ROOT", root)
        .env("MAILROOM_VAULT_WORKER_MODE", mode)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(boundary) = pause {
        command
            .env("MAILROOM_LIFECYCLE_BOUNDARY", boundary)
            .env("MAILROOM_DURABLE_BROWSER_FIXTURE", root.parent().unwrap());
    }
    command.spawn().unwrap()
}
fn probe(root: &Path) -> String {
    let result = root.parent().unwrap().join("probe-result");
    let _ = std::fs::remove_file(&result);
    assert!(worker(root, "probe", None).wait().unwrap().success());
    std::fs::read_to_string(result).unwrap()
}

#[test]
fn vault_process_worker() {
    let Some(root) = std::env::var_os("MAILROOM_VAULT_WORKER_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    match std::env::var("MAILROOM_VAULT_WORKER_MODE")
        .unwrap()
        .as_str()
    {
        "probe" => {
            let outcome = match Vault::open_with(&root, Cost::Test) {
                Ok(vault) => format!("{:?}", vault.status()),
                Err(error) => error.code().to_owned(),
            };
            std::fs::write(root.parent().unwrap().join("probe-result"), outcome).unwrap();
        }
        "commit" => {
            let vault = open(&root);
            vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
            vault.put("b-key", &secret(b"two")).unwrap();
        }
        other => panic!("unknown worker mode {other}"),
    }
}

#[test]
fn one_process_owns_the_vault_for_its_whole_lifetime() {
    let (_directory, root) = root();
    let vault = open(&root);
    assert_eq!(
        Vault::open_with(&root, Cost::Test).err(),
        Some(VaultError::InUse)
    );
    assert_eq!(probe(&root), "in_use");
    vault
        .create(&secret(PASSWORD.as_bytes()), &secret(PASSWORD.as_bytes()))
        .unwrap();
    assert_eq!(probe(&root), "in_use");
    drop(vault);
    assert_eq!(probe(&root), "Locked");
}

#[test]
fn a_killed_writer_leaves_the_old_or_the_new_generation() {
    for (boundary, survives) in [("vault_staged", false), ("vault_renamed", true)] {
        let (_directory, root) = root();
        {
            let vault = created(&root);
            vault.put("a-key", &secret(b"one")).unwrap();
        }
        let mut child = worker(&root, "commit", Some(boundary));
        let reached = root.parent().unwrap().join(format!("reached-{boundary}"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !reached.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker never reached {boundary}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let vault = open(&root);
        assert_eq!(stages(&root), 0, "{boundary}");
        vault.unlock(&secret(PASSWORD.as_bytes())).unwrap();
        assert_eq!(vault.get("a-key").unwrap().bytes(), b"one");
        assert_eq!(vault.get("b-key").is_ok(), survives, "{boundary}");
    }
}
