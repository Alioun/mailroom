use super::*;
use crate::census::scale;

const SECRET: u8 = 41;

fn open(dir: &std::path::Path) -> EncryptedStore {
    let account = AccountId::new("journal-frame-fixture").unwrap();
    EncryptedStore::new(
        dir.into(),
        CensusKeys::derive(
            &LocalSecret::from_bytes(vec![SECRET; 32]).unwrap(),
            &account,
        ),
    )
    .unwrap()
}

fn seeded(dir: &std::path::Path, messages: u32) -> (EncryptedStore, AccountState) {
    let store = open(dir);
    let mut census = Census::new(AccountId::new("journal-frame-fixture").unwrap());
    scale::inflate(
        &mut census,
        scale::Spec {
            messages,
            ..scale::PERSONAL
        },
    );
    store.save(&census).unwrap();
    let state = store.load_state().unwrap();
    (store, state)
}

fn bytes(state: &AccountState) -> Vec<u8> {
    serde_json::to_vec(state).unwrap()
}

fn frames(store: &EncryptedStore) -> u64 {
    store
        .persisted
        .borrow()
        .as_ref()
        .unwrap()
        .tail
        .as_ref()
        .unwrap()
        .next_sequence
}

fn journal_change(state: &mut AccountState, value: u32) {
    state
        .synthetic_uid_next
        .insert("journal-change".into(), value);
}

#[test]
fn journal_saves_append_small_frames_and_reopen_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 2_000);
    let base = fs::metadata(store.path()).unwrap().len();
    assert!(base > 4_000_000, "fixture census is {base} bytes");
    for value in 1..=12 {
        journal_change(&mut state, value);
        store.save_state(&mut state).unwrap();
    }
    let grown = fs::metadata(store.path()).unwrap().len() - base;
    assert!(grown < 12 * 512, "twelve journal saves wrote {grown} bytes");
    assert_eq!(frames(&store), 12);
    drop(store);
    let reopened = open(dir.path());
    let loaded = reopened.load_state().unwrap();
    assert_eq!(bytes(&loaded), bytes(&state));
    assert_eq!(reopened.schema_version().unwrap(), crate::census::SCHEMA);
}

#[test]
fn census_changes_append_frames_that_carry_only_the_changed_messages() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 2_000);
    let base = std::fs::metadata(store.path()).unwrap().len();
    state.census.account_login = Some("synthetic-owner@example.test".into());
    store.save_state(&mut state).unwrap();
    assert_eq!(frames(&store), 1);
    let folder = state.census.folders.keys().next().unwrap().clone();
    let next = *state.census.folders[&folder]
        .messages
        .keys()
        .last()
        .unwrap()
        + 1;
    let added = scale::record(9_999_999, next, false, false);
    {
        let census = &mut *state.census;
        let f = census.folders.get_mut(&folder).unwrap();
        f.live_uids.insert(next);
        f.messages.insert(next, added);
        let first = *f.messages.keys().next().unwrap();
        f.messages
            .get_mut(&first)
            .unwrap()
            .flags
            .insert("\\Flagged".into());
        let removed = *f.messages.keys().nth(1).unwrap();
        f.messages.remove(&removed);
        f.live_uids.remove(&removed);
        census
            .folders
            .remove(&census.folders.keys().last().unwrap().clone());
    }
    let before = std::fs::metadata(store.path()).unwrap().len();
    store.save_state(&mut state).unwrap();
    assert_eq!(frames(&store), 2, "a census change appends a frame");
    let frame = std::fs::metadata(store.path()).unwrap().len() - before;
    assert!(
        frame < base / 20,
        "frame of {frame} B carries the change, not the census"
    );
    let mut census = state.census.clone().into_inner();
    census.folders.get_mut(&folder).unwrap().state = crate::census::ScanState::Running;
    store.save(&census).unwrap();
    assert_eq!(
        frames(&store),
        3,
        "Census-only saves append without rereading"
    );
    drop(store);
    let loaded = open(dir.path()).load_state().unwrap();
    state.census = census.into();
    state.revision += 1;
    let mut expected = state.clone();
    normalize_interrupted(&mut expected);
    assert_eq!(bytes(&loaded), bytes(&expected));
}

#[test]
fn uncommitted_tails_roll_back_and_the_next_save_replaces_them() {
    for damage in ["partial", "zeros", "flipped_last_byte"] {
        let dir = tempfile::tempdir().unwrap();
        let (store, mut state) = seeded(dir.path(), 100);
        journal_change(&mut state, 1);
        store.save_state(&mut state).unwrap();
        let committed = bytes(&state);
        let before = fs::read(store.path()).unwrap();
        journal_change(&mut state, 2);
        store.save_state(&mut state).unwrap();
        let after = fs::read(store.path()).unwrap();
        drop(store);
        let damaged = match damage {
            "partial" => after[..after.len() - 5].to_vec(),
            "zeros" => [&before[..], &[0; 100][..]].concat(),
            _ => {
                let mut v = after.clone();
                *v.last_mut().unwrap() ^= 1;
                v
            }
        };
        fs::write(open_path(dir.path()), &damaged).unwrap();
        let store = open(dir.path());
        let mut loaded = store.load_state().unwrap();
        assert_eq!(bytes(&loaded), committed, "{damage}");
        journal_change(&mut loaded, 3);
        store.save_state(&mut loaded).unwrap();
        drop(store);
        assert_eq!(
            bytes(&open(dir.path()).load_state().unwrap()),
            bytes(&loaded),
            "{damage}"
        );
    }
}

fn open_path(dir: &std::path::Path) -> PathBuf {
    let store = open(dir);
    store.path()
}

#[test]
fn damaged_or_reordered_committed_frames_never_load() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 100);
    let base = fs::metadata(store.path()).unwrap().len() as usize;
    let mut ends = vec![];
    for value in 1..=3 {
        journal_change(&mut state, value);
        store.save_state(&mut state).unwrap();
        ends.push(fs::metadata(store.path()).unwrap().len() as usize);
    }
    let path = store.path();
    drop(store);
    let original = fs::read(&path).unwrap();
    let mut flipped = original.clone();
    flipped[base + 20] ^= 1;
    let mut swapped = original[..base].to_vec();
    swapped.extend_from_slice(&original[ends[0]..ends[1]]);
    swapped.extend_from_slice(&original[base..ends[0]]);
    swapped.extend_from_slice(&original[ends[1]..]);
    for (name, damaged) in [("flipped", flipped), ("swapped", swapped)] {
        fs::write(&path, &damaged).unwrap();
        assert_eq!(
            open(dir.path()).load_state().err(),
            Some(CensusError::Authentication),
            "{name}"
        );
    }
    fs::write(&path, &original).unwrap();
    assert_eq!(
        bytes(&open(dir.path()).load_state().unwrap()),
        bytes(&state)
    );
}

#[test]
fn frames_from_an_earlier_base_are_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 100);
    let base = fs::metadata(store.path()).unwrap().len() as usize;
    journal_change(&mut state, 1);
    store.save_state(&mut state).unwrap();
    let old = fs::read(store.path()).unwrap();
    state.census.account_login = Some("synthetic-owner@example.test".into());
    store.save_state(&mut state).unwrap();
    let rewritten = bytes(&state);
    let path = store.path();
    drop(store);
    let mut spliced = fs::read(&path).unwrap();
    spliced.extend_from_slice(&old[base..]);
    fs::write(&path, &spliced).unwrap();
    assert_eq!(bytes(&open(dir.path()).load_state().unwrap()), rewritten);
}

#[test]
fn version_one_files_stay_readable_and_the_first_save_writes_version_two() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 100);
    journal_change(&mut state, 1);
    store.save_state(&mut state).unwrap();
    store
        .stage_v1(18, &serde_json::to_string(&state).unwrap())
        .unwrap()
        .persist(store.path())
        .unwrap();
    drop(store);
    let store = open(dir.path());
    assert!(fs::read(store.path()).unwrap().starts_with(MAGIC));
    let mut loaded = store.load_state().unwrap();
    assert_eq!(bytes(&loaded), bytes(&state));
    journal_change(&mut loaded, 2);
    store.save_state(&mut loaded).unwrap();
    assert!(fs::read(store.path()).unwrap().starts_with(log::MAGIC_V2));
    drop(store);
    assert_eq!(
        bytes(&open(dir.path()).load_state().unwrap()),
        bytes(&loaded)
    );
}

#[test]
fn frames_compact_into_a_new_base_once_they_outgrow_their_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 10);
    let mut compactions = 0;
    for round in 0..16u32 {
        for i in 0..2_000u32 {
            state
                .synthetic_uid_next
                .insert(format!("journal-{round}-{i:04}-{}", "x".repeat(40)), i);
        }
        let before = frames(&store);
        store.save_state(&mut state).unwrap();
        if frames(&store) < before {
            compactions += 1;
        }
    }
    assert!(compactions > 0);
    drop(store);
    assert_eq!(
        bytes(&open(dir.path()).load_state().unwrap()),
        bytes(&state)
    );
}

#[test]
fn a_replaced_file_is_read_again_before_the_revision_check() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 10);
    let older = fs::read(store.path()).unwrap();
    journal_change(&mut state, 1);
    store.save_state(&mut state).unwrap();
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, &older).unwrap();
    fs::rename(&replacement, store.path()).unwrap();
    journal_change(&mut state, 2);
    assert_eq!(store.save_state(&mut state), Err(CensusError::Storage));
    assert_eq!(fs::read(store.path()).unwrap(), older);
}

#[test]
fn synthetic_journal_kill_worker() {
    let Some(dir) = std::env::var_os("MAILROOM_JOURNAL_KILL_TEST") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let store = open(&dir);
    let mut state = store.load_state().unwrap();
    let progress = dir.join("progress");
    loop {
        let next = state.revision + 1;
        journal_change(&mut state, next as u32);
        if next % 5 == 0 {
            state.census.account_login = Some(next.to_string());
        }
        store.save_state(&mut state).unwrap();
        let staged = dir.join("progress.next");
        fs::write(&staged, next.to_string()).unwrap();
        fs::rename(&staged, &progress).unwrap();
    }
}

/// Kills a process that saves frames and new bases in a loop at arbitrary
/// points. Every reopen must show the last reported save or the one after it,
/// with both its journal and its census exactly as that save wrote them.
#[test]
fn actual_kill_during_frame_and_base_writes_keeps_a_committed_state() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 3_000);
    for i in 0..20_000u32 {
        state
            .synthetic_uid_next
            .insert(format!("padding-{i:05}"), i);
    }
    store.save_state(&mut state).unwrap();
    drop(store);
    let mut rewrites_seen = 0;
    for round in 0..10u64 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "census::store::journal_tests::synthetic_journal_kill_worker",
            ])
            .env("MAILROOM_JOURNAL_KILL_TEST", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let progress = dir.path().join("progress");
        let before = fs::read_to_string(&progress).ok();
        let started = std::time::Instant::now();
        while fs::read_to_string(&progress).ok() == before {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(120),
                "worker never saved"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(200 + round * 337 % 1_000));
        child.kill().unwrap();
        child.wait().unwrap();
        let reported: u64 = fs::read_to_string(dir.path().join("progress"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let stages = fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".stage-")
            })
            .count();
        let store = open(dir.path());
        let state = store.load_state().unwrap();
        let torn = store
            .persisted
            .borrow()
            .as_ref()
            .map(|p| p.file.len - p.tail.as_ref().map_or(p.file.len, |t| t.valid_end));
        eprintln!(
            "journal.kill round={round} reported={reported} loaded={} torn_tail_bytes={torn:?} interrupted_stages={stages}",
            state.revision
        );
        assert!(
            state.revision == reported || state.revision == reported + 1,
            "round {round}: reported {reported}, loaded {}",
            state.revision
        );
        if state.revision > 1 {
            assert_eq!(
                state.synthetic_uid_next["journal-change"] as u64, state.revision,
                "round {round}"
            );
            let census = state.revision / 5 * 5;
            assert_eq!(
                state.census.account_login,
                (census > 1).then(|| census.to_string()),
                "round {round}"
            );
            rewrites_seen += u64::from(census > 0);
        }
    }
    assert!(rewrites_seen > 0);
}

#[test]
fn torn_append_worker() {
    let Some(dir) = std::env::var_os("MAILROOM_TORN_APPEND_TEST") else {
        return;
    };
    let store = open(std::path::Path::new(&dir));
    let mut state = store.load_state().unwrap();
    journal_change(&mut state, 7_777);
    let _ = store.save_state(&mut state);
    unreachable!("the save kills this process halfway through its frame");
}

/// SIGKILL lands deterministically after half of a frame reached the file.
/// The reopened store shows the save before it, and the next save replaces
/// the torn bytes.
#[test]
fn sigkill_halfway_through_a_frame_rolls_back_to_the_previous_save() {
    use std::os::unix::process::ExitStatusExt;
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 300);
    journal_change(&mut state, 1);
    store.save_state(&mut state).unwrap();
    drop(store);
    let committed = fs::read(open_path(dir.path())).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "census::store::journal_tests::torn_append_worker",
        ])
        .env("MAILROOM_TORN_APPEND_TEST", dir.path())
        .env("MAILROOM_KILL_AT", "frame_half")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let torn = fs::read(open_path(dir.path())).unwrap();
    assert!(
        torn.len() > committed.len(),
        "half a frame reached the file"
    );
    assert_eq!(&torn[..committed.len()], &committed[..]);
    let store = open(dir.path());
    let mut reopened = store.load_state().unwrap();
    assert_eq!(bytes(&reopened), bytes(&state));
    journal_change(&mut reopened, 2);
    store.save_state(&mut reopened).unwrap();
    drop(store);
    assert_eq!(
        bytes(&open(dir.path()).load_state().unwrap()),
        bytes(&reopened)
    );
}

/// A read served from the decoded-file cache, kept current by this
/// process's own saves, equals decoding the file again, across journal
/// frames, census frames, a torn tail and new bases.
#[test]
fn cached_reads_equal_fresh_decoding_after_every_kind_of_save() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 400);
    let path = store.path();
    let check = |store: &EncryptedStore, label: &str| {
        assert!(super::decoded::holds(&path), "{label}: cached");
        let cached = store.load_state().unwrap();
        super::decoded::forget(&path);
        let fresh = store.load_state().unwrap();
        assert_eq!(bytes(&cached), bytes(&fresh), "{label}");
        assert!(
            super::decoded::holds(&path),
            "{label}: decoding refilled the cache"
        );
    };
    for round in 0..40u32 {
        journal_change(&mut state, round);
        if round % 3 == 0 {
            let census = &mut *state.census;
            let folder = census.folders.values_mut().next().unwrap();
            let uid = folder.messages.keys().last().unwrap() + 1;
            folder.live_uids.insert(uid);
            folder
                .messages
                .insert(uid, scale::record(500_000 + round, uid, false, false));
        }
        if round == 20 {
            state.census.account_login = Some("x".repeat(400_000));
        }
        store.save_state(&mut state).unwrap();
        check(&store, &format!("round {round}"));
    }
    drop(store);
    let mut torn = fs::read(&path).unwrap();
    torn.extend_from_slice(&[7; 40]);
    fs::write(&path, &torn).unwrap();
    let store = open(dir.path());
    store.load_state().unwrap();
    check(&store, "torn tail");
    let mut state = store.load_state().unwrap();
    journal_change(&mut state, 999);
    store.save_state(&mut state).unwrap();
    check(&store, "after truncating the torn tail");
}

/// Schema 20 adds each folder's HIGHESTMODSEQ. A schema-19 file, frames
/// included, stays readable and its next save writes a schema-20 base;
/// older schemas never carry a mod-sequence, in the base or after frames.
#[test]
fn schema_twenty_adds_folder_modseqs_and_earlier_files_stay_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (store, state) = seeded(dir.path(), 300);
    let snapshot = serde_json::to_string(&state).unwrap();
    for version in [2, 18, 19] {
        store
            .stage_snapshot(version, &snapshot)
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut loaded = store.load_state().unwrap();
        assert!(bytes(&loaded) == bytes(&state), "schema {version}");
        journal_change(&mut loaded, version);
        store.save_state(&mut loaded).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA, "schema {version}");
    }
    let mut marked = state.clone();
    marked
        .census
        .folders
        .values_mut()
        .next()
        .unwrap()
        .highestmodseq = Some(7);
    for version in [2, 18, 19] {
        store
            .stage_snapshot(version, &serde_json::to_string(&marked).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "schema {version} base"
        );
    }
    // A schema-19 base that a frame gives a mod-sequence.
    for (modseq, readable) in [(None, true), (Some(9), false)] {
        store
            .stage_snapshot(19, &snapshot)
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut loaded = store.load_state().unwrap();
        let file = std::fs::read(store.path()).unwrap();
        let cipher = Aes256Gcm::new_from_slice(store.keys().encryption.as_ref()).unwrap();
        let decoded = log::decode(&cipher, store.keys().account.as_str(), &file).unwrap();
        store.persisted.borrow_mut().as_mut().unwrap().tail = Some(Tail {
            base_tag: decoded.base_tag,
            base_len: decoded.base_len,
            valid_end: decoded.valid_end,
            next_sequence: 0,
        });
        let folder = loaded.census.folders.keys().next().unwrap().clone();
        loaded
            .census
            .folders
            .get_mut(&folder)
            .unwrap()
            .highestmodseq = modseq;
        journal_change(&mut loaded, 20);
        store.save_state(&mut loaded).unwrap();
        assert!(
            std::fs::metadata(store.path()).unwrap().len() > decoded.valid_end,
            "a frame on the schema-19 base"
        );
        super::decoded::forget(&store.path());
        let reopened = store.load_state();
        assert_eq!(reopened.is_ok(), readable, "{modseq:?}");
        if !readable {
            assert!(matches!(reopened, Err(CensusError::UnsupportedVersion)));
        }
    }
    drop(state);
}

/// Schema 21 adds Gmail `label_provisioning` and `closed` on backlog runs.
/// Files of schemas 2 to 20, frames included, stay readable with neither and
/// their next save writes a schema-21 base; an older file carrying label
/// provisioning, in its base or through a frame, is refused.
#[test]
fn schema_twenty_one_adds_label_provisioning_and_earlier_files_stay_readable() {
    use crate::census::folder_provisioning::{
        Context, FolderJournal, FolderPlan, GMAIL_PROFILE, Inventory, Namespaces, Operation, Status,
    };
    let dir = tempfile::tempdir().unwrap();
    let (store, state) = seeded(dir.path(), 50);
    let snapshot = serde_json::to_string(&state).unwrap();
    assert!(
        !snapshot.contains("label_provisioning"),
        "omitted while empty"
    );
    for version in [2, 18, 19, 20] {
        store
            .stage_snapshot(version, &snapshot)
            .unwrap()
            .persist(store.path())
            .unwrap();
        let mut loaded = store.load_state().unwrap();
        assert!(bytes(&loaded) == bytes(&state), "schema {version}");
        journal_change(&mut loaded, version);
        store.save_state(&mut loaded).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA, "schema {version}");
    }
    let digest = "a".repeat(64);
    let before = Inventory {
        capabilities: ["IMAP4REV1", "NAMESPACE"].map(String::from).into(),
        namespaces: Namespaces {
            personal: vec![crate::census::folder_provisioning::Namespace {
                prefix: String::new(),
                delimiter: Some("/".into()),
            }],
            other: vec![],
            shared: vec![],
        },
        mailboxes: vec![],
    };
    let labels = FolderJournal {
        history: vec![Operation {
            id: digest.clone(),
            context: Context {
                account: state.census.account.clone(),
                source: digest.clone(),
                authority: digest.clone(),
                profile: GMAIL_PROFILE.into(),
            },
            artifact: digest.clone(),
            proof_binding: digest.clone(),
            receipt: digest.clone(),
            before,
            plan: FolderPlan {
                logical_leaves: Default::default(),
                entries: vec![],
            },
            status: Status::Complete,
            steps: vec![],
        }],
    };
    let mut marked = state.clone();
    marked.label_provisioning = labels.clone();
    for version in [2, 18, 19, 20] {
        store
            .stage_snapshot(version, &serde_json::to_string(&marked).unwrap())
            .unwrap()
            .persist(store.path())
            .unwrap();
        assert!(
            matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
            "schema {version} base"
        );
    }
    store
        .stage_snapshot(20, &snapshot)
        .unwrap()
        .persist(store.path())
        .unwrap();
    let mut loaded = store.load_state().unwrap();
    let file = std::fs::read(store.path()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(store.keys().encryption.as_ref()).unwrap();
    let decoded = log::decode(&cipher, store.keys().account.as_str(), &file).unwrap();
    store.persisted.borrow_mut().as_mut().unwrap().tail = Some(Tail {
        base_tag: decoded.base_tag,
        base_len: decoded.base_len,
        valid_end: decoded.valid_end,
        next_sequence: 0,
    });
    loaded.label_provisioning = labels;
    store.save_state(&mut loaded).unwrap();
    assert!(
        std::fs::metadata(store.path()).unwrap().len() > decoded.valid_end,
        "a frame on the schema-20 base"
    );
    super::decoded::forget(&store.path());
    assert!(
        matches!(store.load_state(), Err(CensusError::UnsupportedVersion)),
        "a frame never gives a schema-20 base label provisioning"
    );
}
