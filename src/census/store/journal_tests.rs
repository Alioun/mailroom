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
    assert_eq!(reopened.schema_version().unwrap(), 18);
}

#[test]
fn census_change_writes_a_new_base() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut state) = seeded(dir.path(), 200);
    journal_change(&mut state, 1);
    store.save_state(&mut state).unwrap();
    assert_eq!(frames(&store), 1);
    state.census.account_login = Some("synthetic-owner@example.test".into());
    store.save_state(&mut state).unwrap();
    assert_eq!(frames(&store), 0);
    journal_change(&mut state, 2);
    store.save_state(&mut state).unwrap();
    assert_eq!(frames(&store), 1);
    drop(store);
    let loaded = open(dir.path()).load_state().unwrap();
    assert_eq!(bytes(&loaded), bytes(&state));
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
        std::thread::sleep(std::time::Duration::from_millis(
            1_500 + round * 337 % 1_000,
        ));
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
