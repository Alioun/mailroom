use super::*;
use crate::backlog::imap_move::{MoveIntent, Purpose};
use std::{collections::VecDeque, rc::Rc};

#[path = "compensation_tests.rs"]
mod compensation;
#[path = "effects_tests.rs"]
mod effects;

struct Flight {
    event: &'static str,
    commands: Vec<String>,
    response: Result<Vec<u8>, Error>,
}
struct Script {
    source: String,
    flights: VecDeque<Flight>,
    events: Rc<RefCell<Vec<&'static str>>>,
}
impl Commands for Script {
    fn source(&self) -> &str {
        &self.source
    }
    fn flight(&mut self, steps: Vec<Step<'_>>) -> Result<Vec<Result<(), Error>>, Error> {
        let expected = self.flights.pop_front().expect("unexpected command flight");
        assert_eq!(
            steps
                .iter()
                .map(|s| std::str::from_utf8(&s.command).unwrap())
                .collect::<Vec<_>>(),
            expected.commands
        );
        self.events.borrow_mut().push(expected.event);
        let bytes = expected.response?;
        let mut input = Transcript::new(&bytes);
        let mut results = vec![Ok(()); steps.iter().map(|s| s.unit + 1).max().unwrap_or(0)];
        for (index, mut step) in steps.into_iter().enumerate() {
            let tag = format!("F{}", index + 1);
            loop {
                let row = input.row()?;
                let first = row.first().and_then(Token::atom).ok_or(Error::Protocol)?;
                let tagged = first == tag;
                if !tagged && first != "*" {
                    return Err(Error::Protocol);
                }
                if row.get(1).and_then(Token::atom) == Some("BYE") {
                    return Err(Error::Bye);
                }
                if results[step.unit].is_ok() {
                    results[step.unit] = (step.handler)(&row, tagged);
                }
                if tagged {
                    break;
                }
            }
        }
        assert!(input.exhausted());
        Ok(results)
    }
}

fn source() -> InstalledSource {
    InstalledSource {
        account: AccountId::new("conditional-script").unwrap(),
        login: "dummy@example.test".into(),
        address: "127.0.0.1:1".parse().unwrap(),
        tls_name: "fixture.example.test".into(),
        root: b"dummy-not-a-certificate".into(),
        epoch: "conditional-script/v1".into(),
        authority: "fixture-message-copy/v1".into(),
        command_timeout: Duration::from_secs(1),
        operation_timeout: Duration::from_secs(1),
    }
}
fn selection(identity: &MailboxIdentity, highest: u64, tag: usize) -> Vec<u8> {
    format!("* OK [UIDVALIDITY {}] stable\r\n* OK [HIGHESTMODSEQ {highest}] persistent\r\nF{tag} OK [READ-WRITE] selected\r\n", identity.uidvalidity.get()).into_bytes()
}
fn observation(identity: &MailboxIdentity, members: &[(u32, u64)]) -> Vec<u8> {
    let highest = members.iter().map(|(_, v)| *v).max().unwrap_or(BASE);
    let mut bytes = selection(identity, highest, 1);
    for (uid, version) in members {
        bytes.extend(fetch(*uid, Some(*version)));
    }
    bytes.extend_from_slice(b"F2 OK fetched\r\n");
    bytes
}
fn prepared() -> (CopyEvidence, PreparedDelete) {
    let source = mailbox();
    let destination = MailboxIdentity {
        name: crate::census::folder_provisioning::ExactName::from_wire("Privat/News").unwrap(),
        uidvalidity: UidValidity::new(202).unwrap(),
    };
    let before = readback_in(
        source.clone(),
        &observation(&source, &[(4, BASE), (8, BASE)]),
        [uid(4), uid(8)].into(),
    )
    .unwrap();
    let copied = readback_in(
        destination.clone(),
        &observation(&destination, &[(60, BASE + 10), (64, BASE + 10)]),
        [uid(60), uid(64)].into(),
    )
    .unwrap();
    let intent = MoveIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        source,
        destination,
        before
            .messages()
            .iter()
            .map(|(uid, m)| (*uid, m.metadata.clone()))
            .collect(),
    )
    .unwrap();
    let mut evidence = CopyEvidence::new(intent, before.clone()).unwrap();
    evidence.begin_copy(&before).unwrap();
    evidence
        .record_mapping(
            CopyUidMapping::parse("[COPYUID 202 4,8 60,64]", evidence.intent()).unwrap(),
        )
        .unwrap();
    evidence.record_copy_completion(Completion::Lost).unwrap();
    evidence
        .observe_copy(before.clone(), copied.clone())
        .unwrap();
    let prepared = evidence
        .prepare_deletions(&before, &copied)
        .unwrap()
        .remove(0);
    (evidence, prepared)
}
fn capabilities() -> BTreeSet<String> {
    ["UIDPLUS".into(), "CONDSTORE".into()].into()
}
fn script(
    source: &InstalledSource,
    prepared: &PreparedDelete,
    result: Result<Vec<u8>, Error>,
) -> Script {
    let before = prepared.intent().before();
    let destination = prepared.destination();
    let mut observations = observation(before.identity(), &[(4, BASE), (8, BASE)]);
    let destination_bytes = String::from_utf8(observation(
        destination.identity(),
        &[(60, BASE + 10), (64, BASE + 10)],
    ))
    .unwrap()
    .replace("F1 ", "F3 ")
    .replace("F2 ", "F4 ");
    observations.extend(destination_bytes.as_bytes());
    Script {
        source: source.identity(),
        flights: [
            Flight { event: "readbacks", commands: vec![
                "SELECT \"INBOX\" (CONDSTORE)".into(),
                "UID FETCH 4,8 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into(),
                "SELECT \"Privat/News\" (CONDSTORE)".into(),
                "UID FETCH 60,64 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)".into(),
            ], response: Ok(observations) },
            Flight { event: "select", commands: vec!["SELECT \"INBOX\" (CONDSTORE)".into()], response: Ok(selection(before.identity(), BASE, 1)) },
            Flight { event: "store", commands: vec![format!("UID STORE 4,8 (UNCHANGEDSINCE {BASE}) +FLAGS.SILENT (\\Deleted)")], response: result },
        ].into(),
        events: Rc::new(RefCell::new(vec![])),
    }
}
fn acknowledged() -> Vec<u8> {
    format!(
        "* 2 FETCH (UID 4 MODSEQ ({}))\r\nF1 NO [MODIFIED 8] partial\r\n",
        BASE + 20
    )
    .into_bytes()
}

#[test]
fn store_orchestration_revalidates_then_persists_before_each_effect_boundary() {
    let source = source();
    let permit = source
        .permit(
            &source.account,
            Provider::MailboxOrg,
            &source.login,
            &source.authority,
        )
        .unwrap();
    let (mut evidence, prepared) = prepared();
    let mut io = script(&source, &prepared, Ok(acknowledged()));
    let events = io.events.clone();
    let mut snapshots = vec![];
    let index = Conditional {
        io: &mut io,
        capabilities: &capabilities(),
    }
    .store(&permit, &mut evidence, prepared, |e| {
        let flag = e.deletions().next().unwrap().flag();
        events.borrow_mut().push(if flag.result().is_some() {
            "result"
        } else {
            "intent"
        });
        snapshots.push(serde_json::to_vec(e).unwrap());
        Ok(())
    })
    .unwrap();
    assert_eq!(index, 0);
    assert_eq!(
        *events.borrow(),
        ["readbacks", "select", "intent", "store", "result"]
    );
    assert!(io.flights.is_empty());
    assert_eq!(snapshots.len(), 2);
    let intent: CopyEvidence = serde_json::from_slice(&snapshots[0]).unwrap();
    assert!(intent.deletions().next().unwrap().flag().result().is_none());
    assert_eq!(
        serde_json::from_slice::<CopyEvidence>(&snapshots[1]).unwrap(),
        evidence
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
}

#[test]
fn parsed_post_store_readback_certifies_only_the_acknowledged_member() {
    let source = source();
    let permit = source
        .permit(
            &source.account,
            Provider::MailboxOrg,
            &source.login,
            &source.authority,
        )
        .unwrap();
    let (mut evidence, prepared) = prepared();
    let mut io = script(&source, &prepared, Ok(acknowledged()));
    Conditional {
        io: &mut io,
        capabilities: &capabilities(),
    }
    .store(&permit, &mut evidence, prepared, |_| Ok(()))
    .unwrap();
    let identity = evidence.intent().source().clone();
    let observed = String::from_utf8(observation(&identity, &[(4, BASE + 20), (8, BASE)]))
        .unwrap()
        .replacen(
            "FLAGS (\\Seen \\Recent $owner)",
            "FLAGS (\\Seen \\Deleted $owner)",
            1,
        );
    evidence
        .observe_store(
            0,
            readback_in(identity, observed.as_bytes(), [uid(4), uid(8)].into()).unwrap(),
        )
        .unwrap();
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
}

#[test]
fn authority_and_capability_failures_precede_any_command_or_journal_intent() {
    for variant in 0..5 {
        let mut source = source();
        if variant == 0 {
            source.authority = "fixture-message-move/v1".into();
        }
        let permit = source
            .permit(
                &source.account,
                Provider::MailboxOrg,
                &source.login,
                &source.authority,
            )
            .unwrap();
        let (mut evidence, prepared) = prepared();
        let mut io = script(&source, &prepared, Ok(acknowledged()));
        let mut capabilities = capabilities();
        match variant {
            1 => io.source = "different-installed-source".into(),
            2 => {
                capabilities.insert("MOVE".into());
            }
            3 => {
                capabilities.remove("UIDPLUS");
            }
            4 => {
                capabilities.remove("CONDSTORE");
            }
            _ => {}
        }
        let result = Conditional {
            io: &mut io,
            capabilities: &capabilities,
        }
        .store(&permit, &mut evidence, prepared, |_| {
            panic!("must not persist intent")
        });
        assert_eq!(result, Err(Error::Unsupported));
        assert!(io.events.borrow().is_empty());
        assert_eq!(evidence.deletions().count(), 0);
    }
}

#[test]
fn changed_source_destination_or_nonpersistent_reselection_never_sends_store() {
    for variant in 0..5 {
        let source = source();
        let permit = source
            .permit(
                &source.account,
                Provider::MailboxOrg,
                &source.login,
                &source.authority,
            )
            .unwrap();
        let (mut evidence, prepared) = prepared();
        let mut io = script(&source, &prepared, Ok(acknowledged()));
        let flight = if variant >= 2 {
            &mut io.flights[1]
        } else {
            &mut io.flights[0]
        };
        let text = String::from_utf8(flight.response.as_ref().unwrap().clone()).unwrap();
        flight.response = Ok(match variant {
            0 => text.replacen(
                "FLAGS (\\Seen \\Recent $owner)",
                "FLAGS (\\Seen $changed)",
                1,
            ),
            1 => text.replace(
                &format!("MODSEQ ({})", BASE + 10),
                &format!("MODSEQ ({})", BASE + 9),
            ),
            2 => text.replace(&format!("HIGHESTMODSEQ {BASE}"), "NOMODSEQ"),
            3 => text.replace(
                &format!("HIGHESTMODSEQ {BASE}"),
                &format!("HIGHESTMODSEQ {}", BASE - 1),
            ),
            _ => text.replace("UIDVALIDITY 101", "UIDVALIDITY 102"),
        }
        .into_bytes());
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .store(&permit, &mut evidence, prepared, |_| panic!(
                "guard must precede intent"
            ))
            .is_err()
        );
        assert!(!io.events.borrow().contains(&"store"));
        assert_eq!(evidence.deletions().count(), 0);
    }
}

#[test]
fn failed_intent_or_result_persistence_never_retries_transmission() {
    for fail_at in 1..=2 {
        let source = source();
        let permit = source
            .permit(
                &source.account,
                Provider::MailboxOrg,
                &source.login,
                &source.authority,
            )
            .unwrap();
        let (mut evidence, prepared) = prepared();
        let mut io = script(&source, &prepared, Ok(acknowledged()));
        let mut saves = 0;
        assert_eq!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .store(&permit, &mut evidence, prepared.clone(), |_| {
                saves += 1;
                if saves == fail_at {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            }),
            Err(Error::Cancelled)
        );
        assert_eq!(saves, fail_at);
        assert_eq!(
            io.events.borrow().iter().filter(|e| **e == "store").count(),
            fail_at - 1
        );
        assert_eq!(evidence.deletions().count(), 1);
        let last = evidence.deletions().next().unwrap().flag();
        assert_eq!(last.result().is_some(), fail_at == 2);
        assert!(last.verified().is_empty());
        assert!(evidence.begin_delete(prepared).is_err());
    }
}

#[test]
fn lost_or_malformed_reply_retains_uncertainty_without_flag_ownership() {
    for (response, expected) in [
        (Err(Error::Eof), StoreResult::Lost),
        (
            Ok(b"F1 OK [OVERQUOTA] unknown\r\n".to_vec()),
            StoreResult::Rejected,
        ),
        (
            Ok(format!("* 2 FETCH (UID 4 MODSEQ ({}))\r\n", BASE + 20).into_bytes()),
            StoreResult::Lost,
        ),
    ] {
        let source = source();
        let permit = source
            .permit(
                &source.account,
                Provider::MailboxOrg,
                &source.login,
                &source.authority,
            )
            .unwrap();
        let (mut evidence, prepared) = prepared();
        let mut io = script(&source, &prepared, response);
        let mut saved = vec![];
        assert!(
            Conditional {
                io: &mut io,
                capabilities: &capabilities()
            }
            .store(&permit, &mut evidence, prepared, |e| {
                saved.push(e.clone());
                Ok(())
            })
            .is_err()
        );
        assert_eq!(saved.len(), 2);
        let flag = evidence.deletions().next().unwrap().flag();
        assert_eq!(flag.result(), Some(&expected));
        assert!(flag.verified().is_empty());
        assert_eq!(
            io.events.borrow().iter().filter(|e| **e == "store").count(),
            1
        );
    }
}
