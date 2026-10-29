use super::*;
use crate::acquisition::folder_provisioning::transport::wire::Transcript;
use crate::backlog::protocol::tests::{Stage, run};

const BASE: u64 = 9_007_199_254_740_993;
fn intent() -> FlagIntent {
    let account = AccountId::new("store-reply").unwrap();
    let run = run(&account, Stage::StoreUncertain);
    run.batches[0]
        .as_copy()
        .unwrap()
        .deletions()
        .next()
        .unwrap()
        .intent()
        .clone()
}
fn uid(value: u32) -> Uid {
    Uid::new(value).unwrap()
}
fn mailbox() -> MailboxIdentity {
    intent().before().identity().clone()
}
fn fetch(uid: u32, version: Option<u64>) -> Vec<u8> {
    let header = "Message-ID: <conditional@example.test>\r\n\r\n";
    let version = version
        .map(|v| format!(" MODSEQ ({v})"))
        .unwrap_or_default();
    format!("* 2 FETCH (UID {uid} FLAGS (\\Seen \\Recent $owner) INTERNALDATE \"24-Oct-2026 12:00:00 +0000\" RFC822.SIZE 100 BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header}{version})\r\n", header.len()).into_bytes()
}
fn feed_store(input: &[u8], intent: &FlagIntent) -> Result<StoreResult, Error> {
    let mut input = Transcript::new(input);
    let mut parser = StoreReply::new(intent);
    while !input.exhausted() {
        let row = input.row()?;
        let tagged = row.first().and_then(Token::atom) == Some("F1");
        if let Some(result) = parser.observe(&row, tagged)? {
            return Ok(result);
        }
    }
    Err(Error::Eof)
}
fn readback(transcript: &[u8], uids: BTreeSet<Uid>) -> Result<Readback, Error> {
    readback_in(mailbox(), transcript, uids)
}
fn readback_in(
    identity: MailboxIdentity,
    transcript: &[u8],
    uids: BTreeSet<Uid>,
) -> Result<Readback, Error> {
    let highest = RefCell::new(None);
    let found = RefCell::new(BTreeMap::new());
    let mut input = Transcript::new(transcript);
    let mut commands = steps(&identity, &uids, &highest, &found, 0)?;
    assert_eq!(
        commands[0].command.as_slice(),
        format!("SELECT \"{}\" (CONDSTORE)", identity.name.wire).as_bytes()
    );
    if !uids.is_empty() {
        let command = std::str::from_utf8(&commands[1].command).unwrap();
        assert!(command.starts_with("UID FETCH "));
        assert!(command.ends_with(" MODSEQ)"));
    }
    for (index, command) in commands.iter_mut().enumerate() {
        let expected = format!("F{}", index + 1);
        loop {
            let row = input.row()?;
            let tagged = row.first().and_then(Token::atom) == Some(&expected);
            (command.handler)(&row, tagged)?;
            if tagged {
                break;
            }
        }
    }
    drop(commands);
    assert!(input.exhausted());
    Readback::new(
        identity,
        highest.into_inner().ok_or(Error::Unsupported)?,
        found.into_inner(),
    )
    .map_err(|_| Error::Unstable)
}
fn selected(highest: &str) -> String {
    format!(
        "* 2 EXISTS\r\n* OK [UIDVALIDITY 101] stable\r\n* OK [{highest}] version\r\nF1 OK [READ-WRITE] selected\r\n"
    )
}

fn after_store(intent: &FlagIntent, changes: &[(u32, u64, &[&str])]) -> Readback {
    let mut messages = intent.before().messages().clone();
    for (uid, version, flags) in changes {
        let message = messages.get_mut(&self::uid(*uid)).unwrap();
        message.modseq = ModSeq::new(*version).unwrap();
        message
            .metadata
            .flags
            .extend(flags.iter().map(|f| (*f).into()));
    }
    Readback::new(
        intent.before().identity().clone(),
        ModSeq::new(BASE + 100).unwrap(),
        messages,
    )
    .unwrap()
}

#[test]
fn parsed_store_acknowledgment_owns_only_exact_unchanged_readback_members() {
    let account = AccountId::new("store-reply").unwrap();
    let intent = intent();
    let text = format!(
        "* 2 FETCH (UID 4 MODSEQ ({}))\r\n* 3 FETCH (UID 8 MODSEQ ({}))\r\nF1 OK complete\r\n",
        BASE + 20,
        BASE + 21
    );
    let result = feed_store(text.as_bytes(), &intent).unwrap();
    for (changes, expected) in [
        (
            vec![
                (4, BASE + 20, &["\\Deleted"][..]),
                (8, BASE + 21, &["\\Deleted"][..]),
            ],
            vec![4, 8],
        ),
        (
            vec![
                (4, BASE + 22, &["\\Deleted"][..]),
                (8, BASE + 21, &["\\Deleted"][..]),
            ],
            vec![8],
        ),
        (
            vec![
                (4, BASE + 20, &["\\Deleted", "$later-owner"][..]),
                (8, BASE + 21, &["\\Deleted"][..]),
            ],
            vec![8],
        ),
        (
            vec![(4, BASE + 20, &[][..]), (8, BASE + 21, &["\\Deleted"][..])],
            vec![8],
        ),
    ] {
        let mut run = run(&account, Stage::StoreUncertain);
        let evidence = run.batches[0].as_copy_mut().unwrap();
        evidence.record_store_result(0, result.clone()).unwrap();
        evidence
            .observe_store(0, after_store(&intent, &changes))
            .unwrap();
        let owned = evidence.deletions().next().unwrap().flag().verified();
        assert_eq!(
            owned.keys().copied().collect::<BTreeSet<_>>(),
            expected.into_iter().map(uid).collect()
        );
    }
}

#[test]
fn partial_no_and_modified_never_own_the_conflicted_member() {
    let account = AccountId::new("store-reply").unwrap();
    let mut run = run(&account, Stage::StoreUncertain);
    let evidence = run.batches[0].as_copy_mut().unwrap();
    let intent = intent();
    let text = format!(
        "* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 NO [MODIFIED 8] conflict\r\n",
        BASE + 20
    );
    evidence
        .record_store_result(0, feed_store(text.as_bytes(), &intent).unwrap())
        .unwrap();
    let observed = after_store(
        &intent,
        &[
            (4, BASE + 20, &["\\Deleted"]),
            (8, BASE + 21, &["\\Deleted"]),
        ],
    );
    evidence.observe_store(0, observed.clone()).unwrap();
    let deletion = evidence.deletions().next().unwrap();
    assert_eq!(
        deletion
            .flag()
            .verified()
            .keys()
            .copied()
            .collect::<BTreeSet<_>>(),
        [uid(4)].into()
    );
    let owned = Readback::new(
        observed.identity().clone(),
        ModSeq::new(BASE + 100).unwrap(),
        deletion.flag().verified(),
    )
    .unwrap();
    let compensation = deletion.pending_clear(owned).unwrap();
    assert_eq!(
        compensation.command(),
        format!(
            "UID STORE 4 (UNCHANGEDSINCE {}) -FLAGS.SILENT (\\Deleted)",
            BASE + 20
        )
    );
}

#[test]
fn parsed_store_acknowledgment_and_observation_survive_independent_encrypted_reopen() {
    use crate::backlog::protocol::{Status, outcome};
    use crate::census::{Census, CensusKeys, EncryptedStore, LocalSecret};

    let account = AccountId::new("store-reply").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret = LocalSecret::random();
    let intent = intent();
    let text = format!(
        "* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 NO [MODIFIED 8] conflict\r\n",
        BASE + 20
    );
    let result = feed_store(text.as_bytes(), &intent).unwrap();
    let store = EncryptedStore::new(
        directory.path().into(),
        CensusKeys::derive(&secret, &account),
    )
    .unwrap();
    store.save(&Census::new(account.clone())).unwrap();
    let mut state = store.load_state().unwrap();
    state
        .protocol_backlog
        .runs
        .push(run(&account, Stage::StoreUncertain));
    store.save_state(&mut state).unwrap();
    drop(store);

    for phase in 0..3 {
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &account),
        )
        .unwrap();
        let mut state = store.load_state().unwrap();
        let run = &mut state.protocol_backlog.runs[0];
        let evidence = run.batches[0].as_copy_mut().unwrap();
        if phase == 0 {
            assert!(
                evidence
                    .deletions()
                    .next()
                    .unwrap()
                    .flag()
                    .result()
                    .is_none()
            );
            evidence.record_store_result(0, result.clone()).unwrap();
        } else if phase == 1 {
            assert_eq!(
                evidence.deletions().next().unwrap().flag().result(),
                Some(&result)
            );
            assert!(
                evidence
                    .deletions()
                    .next()
                    .unwrap()
                    .flag()
                    .verified()
                    .is_empty()
            );
            evidence
                .observe_store(0, after_store(&intent, &[(4, BASE + 20, &["\\Deleted"])]))
                .unwrap();
        } else {
            assert_eq!(
                evidence
                    .deletions()
                    .next()
                    .unwrap()
                    .flag()
                    .verified()
                    .keys()
                    .copied()
                    .collect::<BTreeSet<_>>(),
                [uid(4)].into()
            );
            assert_eq!(evidence.deletions().count(), 1);
            assert!(evidence.record_store_result(0, result.clone()).is_err());
            assert_ne!(run.status, Status::Complete);
            let ciphertext = std::fs::read(store.path()).unwrap();
            let expected = serde_json::to_vec(&state.protocol_backlog).unwrap();
            assert_eq!(
                serde_json::to_vec(&store.load_state().unwrap().protocol_backlog).unwrap(),
                expected
            );
            assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
            assert!(
                !ciphertext
                    .windows(b"copy-journal-4".len())
                    .any(|part| part == b"copy-journal-4")
            );
            break;
        }
        run.status = outcome(run, false);
        state.protocol_backlog.validate(&account).unwrap();
        store.save_state(&mut state).unwrap();
    }
}

#[test]
fn invalid_sequence_numbers_wrong_tags_and_replayed_completions_fail() {
    let intent = intent();
    for sequence in ["0", "x", "4294967296"] {
        let text = format!(
            "* {sequence} FETCH (UID 4 MODSEQ ({}))\r\nF1 OK complete\r\n",
            BASE + 20
        );
        assert!(
            feed_store(text.as_bytes(), &intent).is_err(),
            "invalid sequence {sequence}"
        );
    }
    assert!(feed_store(b"F2 OK complete\r\n", &intent).is_err());
    let mut input = Transcript::new(b"F1 OK complete\r\nF1 OK replayed\r\n");
    let mut reply = StoreReply::new(&intent);
    assert!(
        reply
            .observe(&input.row().unwrap(), true)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reply.observe(&input.row().unwrap(), true),
        Err(Error::Protocol)
    );
}

#[test]
fn persistent_select_and_literal_fetch_use_the_production_byte_parser() {
    let mut transcript = selected(&format!("HIGHESTMODSEQ {}", BASE + 1)).into_bytes();
    transcript.extend(fetch(4, Some(BASE)));
    transcript.extend_from_slice(b"F2 OK fetched\r\n");
    let read = readback(&transcript, [uid(4)].into()).unwrap();
    assert_eq!(read.messages()[&uid(4)].modseq.get(), BASE);
    assert_eq!(
        read.messages()[&uid(4)].metadata.flags,
        ["\\Seen".into(), "$owner".into()].into()
    );
    assert_eq!(
        serde_json::to_value(read).unwrap()["highest_modseq"],
        (BASE + 1).to_string()
    );
}

#[test]
fn nonpersistent_or_stale_selection_never_returns_a_conditional_readback() {
    for version in [
        "NOMODSEQ",
        "HIGHESTMODSEQ 0",
        "HIGHESTMODSEQ 18446744073709551616",
    ] {
        let mut transcript = selected(version).into_bytes();
        transcript.extend(fetch(4, Some(BASE)));
        transcript.extend_from_slice(b"F2 OK fetched\r\n");
        assert!(readback(&transcript, [uid(4)].into()).is_err(), "{version}");
    }
    let mut transcript = selected("HIGHESTMODSEQ 1").into_bytes();
    transcript.extend(fetch(4, Some(2)));
    transcript.extend_from_slice(b"F2 OK fetched\r\n");
    assert_eq!(readback(&transcript, [uid(4)].into()), Err(Error::Unstable));
}

#[test]
fn missing_or_duplicate_fetch_versions_and_incomplete_selection_fail() {
    let mut transcript = selected(&format!("HIGHESTMODSEQ {}", BASE + 1)).into_bytes();
    transcript.extend(fetch(4, None));
    transcript.extend_from_slice(b"F2 OK fetched\r\n");
    assert_eq!(
        readback(&transcript, [uid(4)].into()),
        Err(Error::Unsupported)
    );
    let mut transcript = selected(&format!("HIGHESTMODSEQ {}", BASE + 1)).into_bytes();
    transcript.extend(fetch(4, Some(BASE)));
    transcript.extend(fetch(4, Some(BASE)));
    transcript.extend_from_slice(b"F2 OK fetched\r\n");
    assert_eq!(readback(&transcript, [uid(4)].into()), Err(Error::Unstable));
    for start in [
        "* OK [UIDVALIDITY 102] changed\r\nF1 OK [READ-WRITE] selected\r\n",
        "* OK [UIDVALIDITY 101] stable\r\nF1 OK selected\r\n",
        "* OK [UIDVALIDITY 101] stable\r\nF1 OK [READ-ONLY] selected\r\n",
    ] {
        assert!(readback(start.as_bytes(), BTreeSet::new()).is_err());
    }
}

#[test]
fn empty_persistent_mailbox_and_unrelated_fetch_have_distinct_meaning() {
    let transcript = selected("HIGHESTMODSEQ 1");
    assert!(
        readback(transcript.as_bytes(), BTreeSet::new())
            .unwrap()
            .messages()
            .is_empty()
    );
    let mut transcript = selected(&format!("HIGHESTMODSEQ {}", BASE + 1)).into_bytes();
    transcript.extend(fetch(99, None));
    transcript.extend_from_slice(b"F2 OK fetched\r\n");
    assert!(
        readback(&transcript, [uid(4)].into())
            .unwrap()
            .messages()
            .is_empty()
    );
}

#[test]
fn tagged_store_and_modified_subset_bind_exact_returned_versions() {
    let intent = intent();
    for status in ["OK", "NO"] {
        let text = format!(
            "* 2 FETCH (MODSEQ ({}) UID 4 FLAGS (\\Seen \\Deleted $owner))\r\nF1 {status} [MODIFIED 8] conditional\r\n",
            BASE + 20
        );
        let result = feed_store(text.as_bytes(), &intent).unwrap();
        let StoreResult::Tagged {
            modified, updated, ..
        } = result
        else {
            panic!("expected tagged result")
        };
        assert_eq!(modified.unwrap(), [uid(8)].into());
        assert_eq!(updated[&uid(4)].get(), BASE + 20);
    }
}

#[test]
fn untagged_updates_or_ok_without_versions_never_establish_ownership() {
    let intent = intent();
    assert_eq!(
        feed_store(
            format!("* 2 FETCH (UID 4 MODSEQ ({}))\r\n", BASE + 20).as_bytes(),
            &intent
        ),
        Err(Error::Eof)
    );
    let result = feed_store(b"F1 OK complete\r\n", &intent).unwrap();
    let account = AccountId::new("store-reply").unwrap();
    let mut run = run(&account, Stage::StoreUncertain);
    let evidence = run.batches[0].as_copy_mut().unwrap();
    evidence.record_store_result(0, result).unwrap();
    let observed = Readback::new(
        intent.before().identity().clone(),
        ModSeq::new(BASE + 100).unwrap(),
        intent
            .before()
            .messages()
            .iter()
            .map(|(uid, value)| {
                let mut value = value.clone();
                value.metadata.flags.insert("\\Deleted".into());
                value.modseq = ModSeq::new(BASE + 20).unwrap();
                (*uid, value)
            })
            .collect(),
    )
    .unwrap();
    evidence.observe_store(0, observed).unwrap();
    assert!(
        evidence
            .deletions()
            .next()
            .unwrap()
            .flag()
            .verified()
            .is_empty()
    );
}

#[test]
fn malformed_foreign_duplicate_or_contradictory_store_evidence_is_rejected() {
    let intent = intent();
    for text in [
        format!("* 2 FETCH (UID 4 MODSEQ (0))\r\nF1 OK done\r\n"),
        format!("* 2 FETCH (UID 4 MODSEQ (18446744073709551616))\r\nF1 OK done\r\n"),
        format!(
            "* 2 FETCH (UID 4 MODSEQ ({}))\r\n* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 OK done\r\n",
            BASE + 20,
            BASE + 20
        ),
        format!(
            "* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 OK [MODIFIED 4] conflict\r\n",
            BASE + 20
        ),
        format!("F1 OK [MODIFIED 99] foreign\r\n"),
        format!(
            "* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 BAD invalid\r\n",
            BASE + 20
        ),
        format!("* OK [MODIFIED 8] asynchronous\r\nF1 OK done\r\n"),
        format!("* 2 FETCH (UID 4 FLAGS (\\Deleted))\r\nF1 OK done\r\n"),
        format!(
            "* 2 FETCH (UID 4 MODSEQ ({} {}) )\r\nF1 OK done\r\n",
            BASE + 20,
            BASE + 21
        ),
        format!("F1 OK [OVERQUOTA] done\r\n"),
    ] {
        assert!(
            feed_store(text.as_bytes(), &intent).is_err(),
            "rejected fixture"
        );
    }
}

#[test]
fn unrelated_store_updates_are_ignored_and_uid_ranges_remain_exact() {
    let intent = intent();
    let result = feed_store(
        format!(
            "* 2 FETCH (UID 99 MODSEQ ({}))\r\nF1 NO [MODIFIED 8,4] unchanged\r\n",
            BASE + 20
        )
        .as_bytes(),
        &intent,
    )
    .unwrap();
    let StoreResult::Tagged {
        updated, modified, ..
    } = result
    else {
        panic!("expected tagged result")
    };
    assert!(updated.is_empty());
    assert_eq!(modified.unwrap(), [uid(4), uid(8)].into());
    assert!(feed_store(b"F1 OK [MODIFIED 4:8] unexpected-members\r\n", &intent).is_err());
}

#[test]
fn malformed_framing_and_literal_bounds_fail_before_protocol_evidence() {
    for bytes in [
        b"* 2 FETCH (UID 4 MODSEQ (1)\r\n".as_slice(),
        b"* OK [MODIFIED 4\r\n",
        b"* 2 FETCH (UID 4 BODY[] {2049}\r\n",
        b"* 2 FETCH (UID 4 BODY[] {9999999}\r\n",
        b"* 2 FETCH (UID 4 FLAGS (\\Seen))\n",
        b"* 2 FETCH (UID 4 MODSEQ (1))\r",
    ] {
        assert!(Transcript::new(bytes).row().is_err());
    }
    let mut input = Transcript::new(b"* OK [MODIFIED 4] private free text\r\nF1 OK done\r\n");
    let first = input.row().unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(first[2].atom(), Some("[MODIFIED 4]"));
    assert_eq!(input.row().unwrap().len(), 2);
    assert!(input.exhausted());
}

#[path = "workflow_tests.rs"]
mod workflow;
