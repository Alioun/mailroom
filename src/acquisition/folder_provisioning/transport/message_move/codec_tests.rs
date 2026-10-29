use super::*;
use crate::backlog::imap_copy::{ModSeq, Readback, VersionedMetadata};

fn atom(value: &str) -> Token {
    Token::Atom(value.into())
}
fn mailbox() -> MailboxIdentity {
    MailboxIdentity {
        name: ExactName::from_wire("INBOX").unwrap(),
        uidvalidity: UidValidity::new(101).unwrap(),
    }
}
fn response(status: &str, code: Option<&str>, tagged: bool) -> Vec<Token> {
    let mut row = vec![atom(if tagged { "A1" } else { "*" }), atom(status)];
    if let Some(code) = code {
        row.push(atom(code));
    }
    row
}
fn fetch() -> Vec<Token> {
    vec![
        atom("*"),
        atom("2"),
        atom("FETCH"),
        Token::Open,
        atom("UID"),
        atom("8"),
        atom("FLAGS"),
        Token::Open,
        atom("\\Seen"),
        atom("\\Recent"),
        atom("$owner"),
        Token::Close,
        atom("INTERNALDATE"),
        Token::String(b"24-Oct-2026 12:00:00 +0000".to_vec()),
        atom("RFC822.SIZE"),
        atom("100"),
        atom("BODY[HEADER.FIELDS"),
        Token::Open,
        atom("MESSAGE-ID"),
        Token::Close,
        atom("]"),
        Token::String(b"Message-ID: <codec@example.test>\r\n\r\n".to_vec()),
        Token::Close,
    ]
}
fn versioned(mut row: Vec<Token>, value: &str) -> Vec<Token> {
    row.splice(
        4..4,
        [atom("MODSEQ"), Token::Open, atom(value), Token::Close],
    );
    row
}
fn select(codes: &[&str], completion: Option<&str>) -> Result<(), Error> {
    let identity = mailbox();
    let mut step = Connection::select(&identity, 0)?;
    for code in codes {
        (step.handler)(&response("OK", Some(code), false), false)?;
    }
    (step.handler)(&response("OK", completion, true), true)
}

#[test]
fn move_metadata_accepts_valid_optional_modseq_without_losing_identity_or_flags() {
    let (uid, old) = parse_metadata(&fetch()).unwrap();
    assert_eq!(uid.get(), 8);
    assert_eq!(old.flags, ["\\Seen".into(), "$owner".into()].into());
    for value in ["1", "9007199254740993", "18446744073709551615"] {
        let row = versioned(fetch(), value);
        assert_eq!(parse_metadata(&row).unwrap(), (uid, old.clone()));
        let (parsed_uid, metadata, modseq) = metadata::parse(&row).unwrap();
        assert_eq!(parsed_uid, uid);
        assert_eq!(metadata, old);
        assert_eq!(modseq.unwrap().get().to_string(), value);
        assert_eq!(serde_json::to_value(modseq.unwrap()).unwrap(), value);
    }
    let (_, _, missing) = metadata::parse(&fetch()).unwrap();
    assert!(missing.is_none());
}

#[test]
fn exact_fetch_versions_construct_a_persistent_core_readback() {
    let row = versioned(fetch(), "9007199254740993");
    let (uid, metadata, modseq) = metadata::parse(&row).unwrap();
    let persistent = Readback::new(
        mailbox(),
        metadata::sequence("9007199254740993").unwrap(),
        [(
            uid,
            VersionedMetadata {
                metadata: metadata.clone(),
                modseq: modseq.unwrap(),
            },
        )]
        .into(),
    )
    .unwrap();
    assert_eq!(
        persistent.messages()[&uid].modseq.get(),
        9_007_199_254_740_993
    );
    assert!(
        Readback::new(
            mailbox(),
            ModSeq::new(1).unwrap(),
            [(
                uid,
                VersionedMetadata {
                    metadata,
                    modseq: modseq.unwrap()
                }
            )]
            .into()
        )
        .is_err()
    );
}

#[test]
fn malformed_fetch_versions_and_duplicate_fields_never_become_observations() {
    for text in [
        "",
        "0",
        "-1",
        "+1",
        "1.5",
        "18446744073709551616",
        "123456789012345678901",
    ] {
        assert!(
            metadata::parse(&versioned(fetch(), text)).is_err(),
            "{text}"
        );
    }
    let duplicate = versioned(versioned(fetch(), "900"), "900");
    assert!(metadata::parse(&duplicate).is_err());
    let valid = versioned(fetch(), "900");
    for mutation in 0..7 {
        let mut changed = valid.clone();
        match mutation {
            0 => changed[5] = atom("900"),
            1 => changed[6] = Token::String(b"900".to_vec()),
            2 => {
                changed.insert(7, atom("901"));
            }
            3 => changed[7] = Token::Open,
            4 => changed[0] = atom("A1"),
            5 => changed[1] = atom("0"),
            6 => changed[2] = atom("VANISHED"),
            _ => unreachable!(),
        }
        assert!(metadata::parse(&changed).is_err(), "mutation {mutation}");
    }
    for key in [
        "UID",
        "FLAGS",
        "RFC822.SIZE",
        "INTERNALDATE",
        "BODY[HEADER.FIELDS",
    ] {
        let mut changed = valid.clone();
        let at = changed.iter().position(|t| t.atom() == Some(key)).unwrap();
        changed[at] = atom("UNKNOWN");
        assert!(metadata::parse(&changed).is_err(), "{key}");
    }
}

#[test]
fn modseq_can_appear_after_all_other_fetch_attributes() {
    let mut row = fetch();
    row.pop();
    row.extend([
        atom("MODSEQ"),
        Token::Open,
        atom("18446744073709551615"),
        Token::Close,
        Token::Close,
    ]);
    assert_eq!(metadata::parse(&row).unwrap().2.unwrap().get(), u64::MAX);
}

#[test]
fn selected_move_mailboxes_validate_optional_persistence_without_requiring_it() {
    for code in [
        None,
        Some("[HIGHESTMODSEQ 1]"),
        Some("[HIGHESTMODSEQ 9007199254740993]"),
        Some("[HIGHESTMODSEQ 18446744073709551615]"),
        Some("[NOMODSEQ]"),
    ] {
        let mut codes = vec!["[UIDVALIDITY 101]"];
        if let Some(code) = code {
            codes.push(code);
        }
        assert_eq!(select(&codes, Some("[READ-WRITE]")), Ok(()), "{code:?}");
    }
}

#[test]
fn malformed_or_contradictory_select_versions_fail_the_real_step() {
    for bad in [
        "[HIGHESTMODSEQ 0]",
        "[HIGHESTMODSEQ -1]",
        "[HIGHESTMODSEQ +1]",
        "[HIGHESTMODSEQ 1.5]",
        "[HIGHESTMODSEQ 18446744073709551616]",
        "[HIGHESTMODSEQ]",
        "[HIGHESTMODSEQ 1",
        "[UIDVALIDITY 0]",
        "[UIDVALIDITY +101]",
        "[UIDVALIDITY 4294967296]",
    ] {
        assert!(select(&[bad], Some("[READ-WRITE]")).is_err(), "{bad}");
    }
    for codes in [
        vec![
            "[UIDVALIDITY 101]",
            "[HIGHESTMODSEQ 1]",
            "[HIGHESTMODSEQ 1]",
        ],
        vec!["[UIDVALIDITY 101]", "[HIGHESTMODSEQ 1]", "[NOMODSEQ]"],
        vec!["[UIDVALIDITY 101]", "[NOMODSEQ]", "[HIGHESTMODSEQ 1]"],
        vec!["[UIDVALIDITY 101]", "[NOMODSEQ]", "[NOMODSEQ]"],
        vec!["[UIDVALIDITY 101]", "[UIDVALIDITY 101]"],
    ] {
        assert_eq!(select(&codes, Some("[READ-WRITE]")), Err(Error::Protocol));
    }
}

#[test]
fn select_never_takes_write_authority_from_untagged_ok_or_wrong_identity() {
    assert_eq!(
        select(&["[UIDVALIDITY 102]"], Some("[READ-WRITE]")),
        Err(Error::Unstable)
    );
    assert_eq!(select(&[], Some("[READ-WRITE]")), Err(Error::Unstable));
    assert_eq!(select(&["[UIDVALIDITY 101]"], None), Err(Error::Unstable));
    assert_eq!(
        select(&["[UIDVALIDITY 101]", "[READ-WRITE]"], None),
        Err(Error::Protocol)
    );
    for code in ["[READ-ONLY]", "[UIDNOTSTICKY]"] {
        assert_eq!(
            select(&["[UIDVALIDITY 101]", code], Some("[READ-WRITE]")),
            Err(Error::Unsupported)
        );
    }
}

#[test]
fn select_status_failures_never_become_successful_selection() {
    for (status, error) in [("NO", Error::No), ("BAD", Error::Bad), ("BYE", Error::Bye)] {
        for tagged in [false, true] {
            let identity = mailbox();
            let mut step = Connection::select(&identity, 0).unwrap();
            assert_eq!(
                (step.handler)(&response(status, None, tagged), tagged),
                Err(error)
            );
        }
    }
}
