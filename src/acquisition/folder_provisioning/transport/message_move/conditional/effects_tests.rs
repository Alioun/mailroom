use super::*;
use crate::backlog::imap_copy::Resolution;

fn permitted(source: &InstalledSource) -> BacklogPermit<'_> {
    source
        .permit(
            &source.account,
            Provider::MailboxOrg,
            &source.login,
            &source.authority,
        )
        .unwrap()
}
impl Script {
    fn conditional(&mut self) -> Conditional<'_, Self> {
        static CAPABILITIES: std::sync::LazyLock<BTreeSet<String>> =
            std::sync::LazyLock::new(capabilities);
        Conditional {
            io: self,
            capabilities: &CAPABILITIES,
        }
    }
}

fn initial() -> CopyEvidence {
    let (copied, _) = prepared();
    let identity = copied.intent().source().clone();
    let before = readback_in(
        identity.clone(),
        &observation(&identity, &[(4, BASE), (8, BASE)]),
        [uid(4), uid(8)].into(),
    )
    .unwrap();
    CopyEvidence::new(copied.intent().clone(), before).unwrap()
}
fn copying(
    source: &InstalledSource,
    evidence: &CopyEvidence,
    response: Result<Vec<u8>, Error>,
) -> Script {
    let mut replies = selection(evidence.intent().destination(), BASE + 10, 1);
    replies.extend(selection(evidence.intent().source(), BASE, 2));
    replies.extend(fetch(4, Some(BASE)));
    replies.extend(fetch(8, Some(BASE)));
    replies.extend_from_slice(b"F3 OK fetched\r\n");
    Script { source: source.identity(), events: Rc::new(RefCell::new(vec![])), flights: [
        Flight { event: "copy-readbacks", commands: vec![
            "SELECT \"Privat/News\" (CONDSTORE)".into(),
            "SELECT \"INBOX\" (CONDSTORE)".into(),
            "UID FETCH 4,8 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into(),
        ], response: Ok(replies) },
        Flight { event: "copy-select", commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into()], response: Ok(selection(evidence.intent().source(), BASE, 1)) },
        Flight { event: "copy", commands: vec!["UID COPY 4,8 \"Privat/News\"".into()], response },
    ].into() }
}
fn paired(
    evidence: &CopyEvidence,
    requested: &[u32],
    present: &[(u32, u64, bool)],
    highest: u64,
) -> Flight {
    let mut replies = selection(evidence.intent().source(), highest, 1);
    for (uid, version, deleted) in present {
        let text = String::from_utf8(fetch(*uid, Some(*version))).unwrap();
        replies.extend(if *deleted {
            text.replace(
                "FLAGS (\\Seen \\Recent $owner)",
                "FLAGS (\\Seen \\Deleted $owner)",
            )
            .into_bytes()
        } else {
            text.into_bytes()
        });
    }
    replies.extend_from_slice(b"F2 OK fetched\r\n");
    replies.extend(selection(evidence.intent().destination(), BASE + 10, 3));
    replies.extend(fetch(60, Some(BASE + 10)));
    replies.extend(fetch(64, Some(BASE + 10)));
    replies.extend_from_slice(b"F4 OK fetched\r\n");
    Flight { event: "observe-pair", commands: vec![
        "SELECT \"INBOX\" (CONDSTORE)".into(),
        format!("UID FETCH {} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)", requested.iter().map(u32::to_string).collect::<Vec<_>>().join(",")),
        "SELECT \"Privat/News\" (CONDSTORE)".into(),
        "UID FETCH 60,64 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into(),
    ], response: Ok(replies) }
}
fn marked() -> CopyEvidence {
    let (mut evidence, prepared) = prepared();
    let intent = prepared.intent().clone();
    let index = evidence.begin_delete(prepared).unwrap();
    evidence
        .record_store_result(index, feed_store(&acknowledged(), &intent).unwrap())
        .unwrap();
    evidence
        .observe_store(
            index,
            after_store(&intent, &[(4, BASE + 20, &["\\Deleted"])]),
        )
        .unwrap();
    evidence
}
fn expunging(
    source: &InstalledSource,
    evidence: &CopyEvidence,
    response: Result<Vec<u8>, Error>,
) -> Script {
    Script {
        source: source.identity(),
        events: Rc::new(RefCell::new(vec![])),
        flights: [
            paired(evidence, &[4], &[(4, BASE + 20, true)], BASE + 20),
            Flight {
                event: "expunge-select",
                commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into()],
                response: Ok(selection(evidence.intent().source(), BASE + 20, 1)),
            },
            Flight {
                event: "expunge",
                commands: vec!["UID EXPUNGE 4".into()],
                response,
            },
        ]
        .into(),
    }
}

#[test]
fn copy_mapping_and_completion_are_independent_durable_callbacks() {
    let source = source();
    let permit = permitted(&source);
    let mut evidence = initial();
    let mut io = copying(
        &source,
        &evidence,
        Ok(b"F1 OK [COPYUID 202 4,8 60,64] copied\r\n".to_vec()),
    );
    let events = io.events.clone();
    let mut saved = vec![];
    io.conditional()
        .copy(&permit, &mut evidence, |e| {
            events
                .borrow_mut()
                .push(if e.copy().completion().is_some() {
                    "completion"
                } else if e.copy().mapping().is_some() {
                    "mapping"
                } else {
                    "intent"
                });
            saved.push(serde_json::to_vec(e).unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        *events.borrow(),
        [
            "copy-readbacks",
            "copy-select",
            "intent",
            "copy",
            "mapping",
            "completion"
        ]
    );
    assert_eq!(saved.len(), 3);
    for bytes in &saved {
        serde_json::from_slice::<CopyEvidence>(bytes).unwrap();
    }
    assert!(
        evidence
            .resolutions()
            .values()
            .all(|r| *r == Resolution::Uncertain)
    );
    io.flights.push_back(paired(
        &evidence,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    io.conditional()
        .observe_copy(&permit, &mut evidence, |_| Ok(()))
        .unwrap();
    assert_eq!(
        evidence.resolutions(),
        [
            (
                uid(4),
                Resolution::CopyOnly {
                    destination: uid(60)
                }
            ),
            (
                uid(8),
                Resolution::CopyOnly {
                    destination: uid(64)
                }
            )
        ]
        .into()
    );
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .copy(&permit, &mut evidence, |_| panic!("no retry"))
        .is_err()
    );
    assert!(io.flights.is_empty());
}

#[test]
fn lost_copy_without_mapping_never_guesses_or_retransmits() {
    let source = source();
    let permit = permitted(&source);
    let mut evidence = initial();
    let mut io = copying(&source, &evidence, Err(Error::Eof));
    let mut saves = 0;
    assert_eq!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .copy(&permit, &mut evidence, |_| {
            saves += 1;
            Ok(())
        }),
        Err(Error::Eof)
    );
    assert_eq!(saves, 2);
    assert_eq!(evidence.copy().completion(), Some(&Completion::Lost));
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .observe_copy(&permit, &mut evidence, |_| panic!("no invented evidence"))
        .is_err()
    );
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .copy(&permit, &mut evidence, |_| panic!("no retry"))
        .is_err()
    );
    assert!(io.flights.is_empty());
}

#[test]
fn untagged_mapping_survives_a_lost_tagged_copy_reply_for_read_only_reconciliation() {
    let source = source();
    let permit = permitted(&source);
    let mut evidence = initial();
    let mut io = copying(
        &source,
        &evidence,
        Ok(b"* OK [COPYUID 202 4,8 60,64] mapped\r\n".to_vec()),
    );
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .copy(&permit, &mut evidence, |_| Ok(()))
        .is_err()
    );
    assert!(evidence.copy().mapping().is_some());
    assert_eq!(evidence.copy().completion(), Some(&Completion::Lost));
    io.flights.push_back(paired(
        &evidence,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    io.conditional()
        .observe_copy(&permit, &mut evidence, |_| Ok(()))
        .unwrap();
    assert!(
        evidence
            .resolutions()
            .values()
            .all(|r| matches!(r, Resolution::CopyOnly { .. }))
    );
    assert_eq!(
        io.events.borrow().iter().filter(|e| **e == "copy").count(),
        1
    );
}

#[test]
fn copy_rejects_wrong_uidvalidity_membership_and_contradictory_mapping() {
    for response in [
        b"F1 OK [COPYUID 203 4,8 60,64] wrong\r\n".as_slice(),
        b"F1 OK [COPYUID 202 4,9 60,64] wrong\r\n",
        b"* OK [COPYUID 202 4,8 60,64] mapped\r\nF1 OK [COPYUID 202 4,8 61,65] conflict\r\n",
        b"F1 OK [UIDNOTSTICKY] unsafe\r\n",
    ] {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = initial();
        let mut io = copying(&source, &evidence, Ok(response.to_vec()));
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .copy(&permit, &mut evidence, |_| Ok(()))
            .is_err()
        );
        assert!(
            evidence
                .resolutions()
                .values()
                .all(|r| *r == Resolution::Conflict)
        );
        assert_eq!(evidence.copy().completion(), Some(&Completion::Lost));
    }
}

#[test]
fn every_copy_persistence_failure_stops_without_an_automatic_retry() {
    for fail_at in 1..=3 {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = initial();
        let mut io = copying(
            &source,
            &evidence,
            Ok(b"F1 OK [COPYUID 202 4,8 60,64] copied\r\n".to_vec()),
        );
        let mut saves = 0;
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .copy(&permit, &mut evidence, |_| {
                saves += 1;
                if saves == fail_at {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert!(evidence.copy().attempted());
        assert_eq!(
            io.events.borrow().iter().filter(|e| **e == "copy").count(),
            usize::from(fail_at > 1)
        );
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .copy(&permit, &mut evidence, |_| panic!("no retry"))
            .is_err()
        );
    }
}

#[test]
fn exact_expunge_persists_only_owned_subset_and_requires_final_observation() {
    let source = source();
    let permit = permitted(&source);
    let mut evidence = marked();
    let mut io = expunging(
        &source,
        &evidence,
        Ok(b"* 2 EXPUNGE\r\nF1 OK expunged\r\n".to_vec()),
    );
    let events = io.events.clone();
    let mut saved = vec![];
    io.conditional()
        .expunge(&permit, &mut evidence, 0, |e| {
            let deletion = e.deletions().next().unwrap();
            events
                .borrow_mut()
                .push(if deletion.expunge_completion().is_some() {
                    "completion"
                } else {
                    "intent"
                });
            saved.push(serde_json::to_vec(e).unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        *events.borrow(),
        [
            "observe-pair",
            "expunge-select",
            "intent",
            "expunge",
            "completion"
        ]
    );
    assert_eq!(
        evidence
            .deletions()
            .next()
            .unwrap()
            .expunge_before()
            .unwrap()
            .messages()
            .keys()
            .copied()
            .collect::<BTreeSet<_>>(),
        [uid(4)].into()
    );
    assert!(
        !evidence
            .resolutions()
            .values()
            .any(|r| matches!(r, Resolution::Filed { .. }))
    );
    for bytes in &saved {
        serde_json::from_slice::<CopyEvidence>(bytes).unwrap();
    }
    io.flights
        .push_back(paired(&evidence, &[4], &[], BASE + 30));
    io.conditional()
        .observe_expunge(&permit, &mut evidence, 0, |_| Ok(()))
        .unwrap();
    assert_eq!(
        evidence.resolutions()[&uid(4)],
        Resolution::Filed {
            destination: uid(60)
        }
    );
    assert_eq!(evidence.resolutions()[&uid(8)], Resolution::Conflict);
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .expunge(&permit, &mut evidence, 0, |_| panic!("no retry"))
        .is_err()
    );
    assert!(io.flights.is_empty());
}

#[test]
fn changed_owned_flags_versions_or_destination_never_begin_expunge() {
    for variant in 0..3 {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = marked();
        let mut io = expunging(&source, &evidence, Ok(b"F1 OK done\r\n".to_vec()));
        let response = String::from_utf8(io.flights[0].response.as_ref().unwrap().clone()).unwrap();
        io.flights[0].response = Ok(match variant {
            0 => response.replacen(
                "FLAGS (\\Seen \\Deleted $owner)",
                "FLAGS (\\Seen \\Deleted $changed)",
                1,
            ),
            1 => response.replace(
                &format!("MODSEQ ({})", BASE + 20),
                &format!("MODSEQ ({})", BASE + 19),
            ),
            _ => response.replace(
                &format!("MODSEQ ({})", BASE + 10),
                &format!("MODSEQ ({})", BASE + 9),
            ),
        }
        .into_bytes());
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .expunge(&permit, &mut evidence, 0, |_| panic!(
                "no intent for changed evidence"
            ))
            .is_err()
        );
        assert!(
            evidence
                .deletions()
                .next()
                .unwrap()
                .expunge_before()
                .is_none()
        );
        assert!(!io.events.borrow().contains(&"expunge"));
    }
}

#[test]
fn lost_expunge_reconciles_by_reading_without_repeating_the_effect() {
    let source = source();
    let permit = permitted(&source);
    let mut evidence = marked();
    let mut io = expunging(&source, &evidence, Err(Error::Eof));
    assert_eq!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .expunge(&permit, &mut evidence, 0, |_| Ok(())),
        Err(Error::Eof)
    );
    assert_eq!(
        evidence.deletions().next().unwrap().expunge_completion(),
        Some(&Completion::Lost)
    );
    io.flights
        .push_back(paired(&evidence, &[4], &[], BASE + 30));
    io.conditional()
        .observe_expunge(&permit, &mut evidence, 0, |_| Ok(()))
        .unwrap();
    assert_eq!(
        evidence.resolutions()[&uid(4)],
        Resolution::Filed {
            destination: uid(60)
        }
    );
    assert_eq!(
        io.events
            .borrow()
            .iter()
            .filter(|e| **e == "expunge")
            .count(),
        1
    );
}

#[test]
fn read_only_store_recovery_cannot_invent_a_missing_acknowledgment() {
    let source = source();
    let permit = permitted(&source);
    let (mut evidence, prepared) = prepared();
    let intent = prepared.intent().clone();
    evidence.begin_delete(prepared).unwrap();
    let mut response = observation(
        intent.before().identity(),
        &[(4, BASE + 20), (8, BASE + 20)],
    );
    response = String::from_utf8(response)
        .unwrap()
        .replace(
            "FLAGS (\\Seen \\Recent $owner)",
            "FLAGS (\\Seen \\Deleted $owner)",
        )
        .into_bytes();
    let mut io = Script { source: source.identity(), events: Rc::new(RefCell::new(vec![])), flights: [Flight { event: "read-only-store", commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into(), "UID FETCH 4,8 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into()], response: Ok(response) }].into() };
    let mut saved = vec![];
    io.conditional()
        .observe_store(&permit, &mut evidence, 0, |e| {
            saved.push(e.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(
        evidence.deletions().next().unwrap().flag().result(),
        Some(&StoreResult::Lost)
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
    assert!(
        Conditional {
            io: &mut io,
            capabilities: &capabilities()
        }
        .expunge(&permit, &mut evidence, 0, |_| panic!("no ownership"))
        .is_err()
    );
    assert!(io.flights.is_empty());
}

#[test]
fn changed_initial_copy_observations_fail_without_an_intent_or_copy_command() {
    for variant in 0..2 {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = initial();
        let mut io = copying(&source, &evidence, Ok(b"F1 OK done\r\n".to_vec()));
        let bytes = String::from_utf8(io.flights[0].response.as_ref().unwrap().clone()).unwrap();
        io.flights[0].response = Ok(if variant == 0 {
            bytes.replacen(
                "FLAGS (\\Seen \\Recent $owner)",
                "FLAGS (\\Seen $changed)",
                1,
            )
        } else {
            bytes.replace("UIDVALIDITY 202", "UIDVALIDITY 203")
        }
        .into_bytes());
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .copy(&permit, &mut evidence, |_| panic!(
                "no intent for changed observations"
            ))
            .is_err()
        );
        assert!(!evidence.copy().attempted());
        assert!(!io.events.borrow().contains(&"copy"));
    }
}

#[test]
fn expunge_persistence_failures_stop_without_a_repeated_effect() {
    for fail_at in 1..=2 {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = marked();
        let mut io = expunging(&source, &evidence, Ok(b"F1 OK complete\r\n".to_vec()));
        let mut saves = 0;
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .expunge(&permit, &mut evidence, 0, |_| {
                saves += 1;
                if saves == fail_at {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(saves, fail_at);
        assert_eq!(
            io.events
                .borrow()
                .iter()
                .filter(|e| **e == "expunge")
                .count(),
            fail_at - 1
        );
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .expunge(&permit, &mut evidence, 0, |_| panic!("no retry"))
            .is_err()
        );
        assert!(evidence.deletions().next().unwrap().removed().is_empty());
    }
}

#[test]
fn negative_or_unknown_expunge_completion_does_not_prove_removal() {
    for response in [
        b"* 2 EXPUNGE\r\nF1 NO refused\r\n".as_slice(),
        b"* 2 EXPUNGE\r\nF1 BAD invalid\r\n",
        b"F1 OK [UNKNOWN] untrusted\r\n",
    ] {
        let source = source();
        let permit = permitted(&source);
        let mut evidence = marked();
        let mut io = expunging(&source, &evidence, Ok(response.to_vec()));
        let result = io
            .conditional()
            .expunge(&permit, &mut evidence, 0, |_| Ok(()));
        assert_eq!(result.is_err(), response.starts_with(b"F1 OK [UNKNOWN]"));
        assert!(evidence.deletions().next().unwrap().removed().is_empty());
        io.flights
            .push_back(paired(&evidence, &[4], &[(4, BASE + 20, true)], BASE + 30));
        io.conditional()
            .observe_expunge(&permit, &mut evidence, 0, |_| Ok(()))
            .unwrap();
        assert!(
            !evidence
                .resolutions()
                .values()
                .any(|r| matches!(r, Resolution::Filed { .. }))
        );
    }
}

#[test]
fn restored_mapping_or_expunge_intent_reconciles_without_replaying_commands() {
    let source = source();
    let permit = permitted(&source);
    let mut original = initial();
    let mut io = copying(
        &source,
        &original,
        Ok(b"F1 OK [COPYUID 202 4,8 60,64] complete\r\n".to_vec()),
    );
    let mut checkpoints = vec![];
    io.conditional()
        .copy(&permit, &mut original, |e| {
            checkpoints.push(serde_json::to_vec(e).unwrap());
            Ok(())
        })
        .unwrap();
    let mut restored: CopyEvidence = serde_json::from_slice(&checkpoints[1]).unwrap();
    assert!(restored.copy().completion().is_none());
    io.flights.push_back(paired(
        &restored,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    io.events.borrow_mut().clear();
    let mut saves = 0;
    io.conditional()
        .observe_copy(&permit, &mut restored, |_| {
            saves += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(saves, 2);
    assert_eq!(*io.events.borrow(), ["observe-pair"]);
    assert_eq!(restored.copy().completion(), Some(&Completion::Lost));

    let mut original = marked();
    let mut io = expunging(&source, &original, Ok(b"F1 OK complete\r\n".to_vec()));
    checkpoints.clear();
    io.conditional()
        .expunge(&permit, &mut original, 0, |e| {
            checkpoints.push(serde_json::to_vec(e).unwrap());
            Ok(())
        })
        .unwrap();
    let mut restored: CopyEvidence = serde_json::from_slice(&checkpoints[0]).unwrap();
    assert!(
        restored
            .deletions()
            .next()
            .unwrap()
            .expunge_completion()
            .is_none()
    );
    io.flights
        .push_back(paired(&restored, &[4], &[], BASE + 30));
    io.events.borrow_mut().clear();
    saves = 0;
    io.conditional()
        .observe_expunge(&permit, &mut restored, 0, |_| {
            saves += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(saves, 2);
    assert_eq!(*io.events.borrow(), ["observe-pair"]);
    assert_eq!(
        restored.deletions().next().unwrap().expunge_completion(),
        Some(&Completion::Lost)
    );
    assert_eq!(
        restored.resolutions()[&uid(4)],
        Resolution::Filed {
            destination: uid(60)
        }
    );
}

#[test]
fn parsed_copy_callbacks_reopen_through_the_encrypted_account_journal() {
    use crate::backlog::protocol::{Batch, outcome};
    use crate::census::{Census, CensusKeys, EncryptedStore, LocalSecret};

    let source = source();
    let permit = permitted(&source);
    let directory = tempfile::tempdir().unwrap();
    let secret = LocalSecret::random();
    {
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &source.account),
        )
        .unwrap();
        store.save(&Census::new(source.account.clone())).unwrap();
    }
    let mut evidence = initial();
    let mut io = copying(
        &source,
        &evidence,
        Ok(b"F1 OK [COPYUID 202 4,8 60,64] complete\r\n".to_vec()),
    );
    let mut count = 0;
    let mut persist = |e: &CopyEvidence| {
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &source.account),
        )
        .unwrap();
        let mut state = store.load_state().unwrap();
        let mut record = run(&source.account, Stage::Prepared);
        for change in &mut record.plan.filing.changes {
            let metadata = &e.intent().messages()[&uid(change.source.uid)];
            change.source.message_id_hash = metadata.message_id_hash.clone();
            change.flags = metadata.flags.clone();
        }
        record.batches = vec![Batch::copy(e.clone())];
        record.status = outcome(&record, false);
        state.protocol_backlog.runs = vec![record];
        state.protocol_backlog.validate(&source.account).unwrap();
        store.save_state(&mut state).unwrap();
        drop(store);
        let reopened = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &source.account),
        )
        .unwrap();
        let state = reopened.load_state().unwrap();
        assert_eq!(
            state.protocol_backlog.runs[0].batches[0].as_copy().unwrap(),
            e
        );
        let before = std::fs::read(reopened.path()).unwrap();
        reopened.load_state().unwrap();
        assert_eq!(std::fs::read(reopened.path()).unwrap(), before);
        count += 1;
        Ok(())
    };
    io.conditional()
        .copy(&permit, &mut evidence, &mut persist)
        .unwrap();
    io.flights.push_back(paired(
        &evidence,
        &[4, 8],
        &[(4, BASE, false), (8, BASE, false)],
        BASE,
    ));
    io.conditional()
        .observe_copy(&permit, &mut evidence, &mut persist)
        .unwrap();
    assert_eq!(count, 4);
    assert!(io.flights.is_empty());
}
