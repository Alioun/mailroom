use super::*;

#[test]
fn list_quoted_escapes_and_literal_backslashes_have_the_same_exact_identity() {
    for raw in [
        b"* LIST () \"/\" \"Personal/&ANw- &- \\\"quotes\\\" \\\\backslash\"\r\n".as_slice(),
        b"* LIST () \"/\" {37}\r\nPersonal/&ANw- &- \"quotes\" \\backslash\r\n".as_slice(),
    ] {
        let (_, Response::MailboxData(MailboxDatum::List { name, .. })) =
            imap_proto::parser::parse_response(raw).unwrap()
        else {
            panic!("Expected LIST");
        };
        let wire = list_wire_name(raw, &name).unwrap();
        assert_eq!(wire, "Personal/&ANw- &- \"quotes\" \\backslash");
        assert_eq!(
            decode_name(&wire).unwrap(),
            "Personal/Ü & \"quotes\" \\backslash"
        );
    }
}
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    thread,
};

struct FakeTls {
    address: std::net::SocketAddr,
    certificate: Vec<u8>,
    commands: Arc<Mutex<Vec<String>>>,
    worker: thread::JoinHandle<()>,
}
impl FakeTls {
    fn start(auth_failure: bool) -> Self {
        Self::start_with_limit_probe(auth_failure, false)
    }
    fn start_with_limit_probe(auth_failure: bool, excessive: bool) -> Self {
        Self::scenario(auth_failure, excessive, "normal", true, None)
    }
    fn scenario(
        auth_failure: bool,
        excessive: bool,
        mode: &str,
        fragmented: bool,
        wire_log: Option<std::path::PathBuf>,
    ) -> Self {
        let mode = mode.to_owned();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = cert.cert.der().to_vec();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.cert.der().clone()], key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        let worker = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut io = BufReader::new(StreamOwned::new(
                ServerConnection::new(Arc::new(config)).unwrap(),
                tcp,
            ));
            if io
                .get_mut()
                .write_all(b"* OK synthetic IMAP\r\n")
                .and_then(|_| io.get_mut().flush())
                .is_err()
            {
                return;
            }
            loop {
                let mut line = String::new();
                if !matches!(io.read_line(&mut line), Ok(n) if n > 0) {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                captured
                    .lock()
                    .unwrap()
                    .push(if command.starts_with("LOGIN ") {
                        "LOGIN [REDACTED]".to_owned()
                    } else {
                        command.to_owned()
                    });
                if let Some(path) = &wire_log {
                    use std::io::Write;
                    let mut log = std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(path)
                        .unwrap();
                    writeln!(log, "{}", captured.lock().unwrap().last().unwrap()).unwrap();
                }
                let response = if command.starts_with("LOGIN ") {
                    if auth_failure {
                        format!("{tag} NO synthetic-password-reflected-by-server\r\n")
                    } else {
                        format!("{tag} OK login\r\n")
                    }
                } else if command == "CAPABILITY" {
                    if excessive {
                        let _ = io.get_mut().write_all(&vec![b'x'; RESPONSE_LIMIT + 1]);
                        let _ = io.get_mut().flush();
                        return;
                    }
                    format!(
                        "* CAPABILITY IMAP4rev1 X-GM-EXT-1 SPECIAL-USE\r\n{tag} OK capability\r\n"
                    )
                } else if command == "LIST \"\" \"*\" RETURN (SPECIAL-USE)" {
                    if mode == "partial_list" {
                        format!(
                            "* LIST (\\Junk) \"/\" \"New incomplete role\"\r\n{tag} NO incomplete listing\r\n"
                        )
                    } else if mode == "changed" {
                        format!(
                            "* LIST () \".\" \"INBOX\"\r\n* LIST () NIL \"Flat\"\r\n{tag} OK list\r\n"
                        )
                    } else {
                        let junk = if mode == "absent_spam" {
                            String::new()
                        } else {
                            "* LIST (\\Junk) \"/\" \"Indesirable\"\r\n".into()
                        };
                        let second = if mode == "ambiguous" {
                            "* LIST (\\Junk) NIL \"Other Junk\"\r\n"
                        } else {
                            ""
                        };
                        let all = if mode == "multiple_all" {
                            "* LIST (\\All) NIL \"Other All\"\r\n"
                        } else {
                            ""
                        };
                        format!(
                            "* LIST (\\All) \"/\" \"[Gmail]/Alle Nachrichten\"\r\n{junk}{second}{all}* LIST (\\Sent) \"/\" \"Gesendet\"\r\n* LIST () \"/\" \"Archiv/Gr&APwA3w-e\"\r\n* LIST (\\Archive) \"/\" \"Opaque mailbox\"\r\n* LIST (\\Noselect) \".\" \"Namespace\"\r\n* LIST (\\HasNoChildren) NIL \"Flat\"\r\n{tag} OK list\r\n"
                        )
                    }
                } else if command.starts_with("STATUS ") {
                    let mailbox = command
                        .strip_prefix("STATUS ")
                        .unwrap()
                        .strip_suffix(" (MESSAGES)")
                        .unwrap();
                    match mode.as_str() {
                        "denied" => format!("{tag} NO unavailable secret text suppressed\r\n"),
                        "missing" => {
                            format!("* STATUS {mailbox} (UNSEEN 0)\r\n{tag} OK status\r\n")
                        }
                        "malformed" => {
                            format!("* STATUS {mailbox} (MESSAGES invalid)\r\n{tag} OK status\r\n")
                        }
                        "duplicate_count" => format!(
                            "* STATUS {mailbox} (MESSAGES 0 MESSAGES 1)\r\n{tag} OK status\r\n"
                        ),
                        "wrong_mailbox" => {
                            format!("* STATUS \"unexpected\" (MESSAGES 0)\r\n{tag} OK status\r\n")
                        }
                        "zero" => format!("* STATUS {mailbox} (MESSAGES 0)\r\n{tag} OK status\r\n"),
                        _ => format!("* STATUS {mailbox} (MESSAGES 3)\r\n{tag} OK status\r\n"),
                    }
                } else if command.starts_with("EXAMINE ") {
                    let validity = if mode == "reset" { 8 } else { 7 };
                    format!(
                        "* 2 EXISTS\r\n* FLAGS (\\Seen)\r\n* OK [UIDVALIDITY {validity}] stable\r\n{tag} OK [READ-ONLY] examined\r\n"
                    )
                } else if command == "UID SEARCH ALL" {
                    format!("* SEARCH 1 2\r\n{tag} OK search\r\n")
                } else if command.starts_with("UID FETCH ") && mode == "fail_metadata" {
                    format!("{tag} NO incomplete selected read\r\n")
                } else if command.starts_with("UID FETCH ")
                    && !command.contains("BODY.PEEK")
                    && let Some((lines, size, pause)) = match mode.as_str() {
                        "slow_metadata" => Some((40, 1000, 50)),
                        "stalled_metadata" => Some((5, 1000, 3000)),
                        "trickled_metadata" => Some((40, 50, 300)),
                        _ => None,
                    }
                {
                    // Untagged filler, paced like a slow, stalled or
                    // trickling link, before the real rows.
                    for _ in 0..lines {
                        let line = format!("* OK {}\r\n", "x".repeat(size));
                        if io
                            .get_mut()
                            .write_all(line.as_bytes())
                            .and_then(|_| io.get_mut().flush())
                            .is_err()
                        {
                            return;
                        }
                        thread::sleep(Duration::from_millis(pause));
                    }
                    format!(
                        "* 1 FETCH (UID 1 RFC822.SIZE 4096 INTERNALDATE \"24-Oct-2026 10:00:00 +0000\" FLAGS (\\Seen))\r\n{tag} OK fetched\r\n"
                    )
                } else if command.starts_with("UID FETCH ") {
                    let uids = command.split_whitespace().nth(2).unwrap();
                    let mut response = String::new();
                    for value in uids.split(',') {
                        let uid: u32 = value.parse().unwrap();
                        if command.contains("BODY.PEEK") {
                            let header = format!(
                                "From: sender@example.test\r\nTo: owner@example.test\r\nSubject: Literal {uid}\r\nX-Evidence: A999 OK inside literal\r\n\r\n"
                            );
                            response.push_str(&format!("* {uid} FETCH (UID {uid} BODY[HEADER.FIELDS (FROM SUBJECT)]<0> {{{}}}\r\n{header})\r\n", header.len()));
                        } else {
                            if command.contains("X-GM-MSGID") {
                                response.push_str(&format!("* {uid} FETCH (UID {uid} RFC822.SIZE 4096 INTERNALDATE \"24-Oct-2026 10:00:00 +0000\" FLAGS (\\Seen) X-GM-MSGID {} X-GM-THRID 200 X-GM-LABELS (\\Sent \"Arbeit/&ANw-bersicht\"))\r\n", 100 + uid));
                            } else {
                                response.push_str(&format!("* {uid} FETCH (UID {uid} RFC822.SIZE 4096 INTERNALDATE \"24-Oct-2026 10:00:00 +0000\" FLAGS (\\Seen))\r\n"));
                            }
                        }
                    }
                    response.push_str(&format!("{tag} OK fetched\r\n"));
                    response
                } else {
                    panic!("Unexpected synthetic wire command");
                };
                // Small TLS records force literal and response framing across reads.
                for chunk in response
                    .as_bytes()
                    .chunks(if fragmented { 17 } else { usize::MAX })
                {
                    if io
                        .get_mut()
                        .write_all(chunk)
                        .and_then(|_| io.get_mut().flush())
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        Self {
            address,
            certificate,
            commands,
            worker,
        }
    }
    fn connector(&self) -> native_tls::TlsConnector {
        native_tls::TlsConnector::builder()
            .add_root_certificate(native_tls::Certificate::from_der(&self.certificate).unwrap())
            .build()
            .unwrap()
    }
    fn connect(
        &self,
        host: &str,
        connector: native_tls::TlsConnector,
        cancel: Cancellation,
    ) -> Result<ReadOnlyImap, AcquisitionError> {
        ReadOnlyImap::open(
            TcpStream::connect(self.address).unwrap(),
            host,
            connector,
            "owner@example.test",
            &SecretValue::new(b"synthetic-password".to_vec()),
            cancel,
            true,
        )
    }
}

#[test]
fn tls_wire_is_read_only_and_parses_fragmented_literals_unicode_and_gmail_metadata() {
    let fake = FakeTls::start(false);
    let mut source = fake
        .connect("localhost", fake.connector(), Cancellation::default())
        .unwrap();
    let folders = source.folders().unwrap();
    assert!(folders[0].all);
    assert!(folders[1].excluded);
    assert!(folders[2].sent);
    assert_eq!(folders[3].name, "Archiv/Grüße");
    assert!(folders[3].roles.is_empty());
    assert!(folders[4].roles.contains("archive"));
    let (listing, changed) = source.inventory(&folders[0], Some(5)).unwrap();
    assert!(changed.is_none(), "no CHANGEDSINCE without CONDSTORE");
    assert_eq!(listing.validity, 7);
    assert_eq!(listing.uids, BTreeSet::from([1, 2]));
    let metadata = source.metadata(&[1, 2]).unwrap();
    assert_eq!(metadata[&1].gmail_id.as_deref(), Some("101"));
    assert!(metadata[&1].labels.contains("Arbeit/Übersicht"));
    let headers = source.headers(&[1, 2]).unwrap();
    let parsed = record(&metadata[&1], &folders[0], &headers[&1]).unwrap();
    assert_eq!(parsed.headers.subject, "Literal 1");
    assert_eq!(parsed.headers.direction, Direction::Sent);
    drop(source);
    fake.worker.join().unwrap();
    let commands = fake.commands.lock().unwrap();
    assert_eq!(commands.len(), 7);
    assert_eq!(
        &commands[..6],
        &[
            "LOGIN [REDACTED]",
            "CAPABILITY",
            "LIST \"\" \"*\" RETURN (SPECIAL-USE)",
            "EXAMINE \"[Gmail]/Alle Nachrichten\"",
            "UID SEARCH ALL",
            "UID FETCH 1,2 (UID RFC822.SIZE INTERNALDATE FLAGS X-GM-MSGID X-GM-THRID X-GM-LABELS)"
        ]
    );
    assert_eq!(
        commands[6],
        format!(
            "UID FETCH 1,2 (UID BODY.PEEK[HEADER.FIELDS ({})]<0.65537>)",
            HEADER_FIELDS.join(" ").to_ascii_uppercase()
        )
    );
    for denied in [
        "SELECT ", "CLOSE", "EXPUNGE", "STORE", "MOVE", "COPY", "APPEND", "CREATE", "DELETE",
        "BODY[]", "RFC822 ",
    ] {
        assert!(commands.iter().all(|c| !c.contains(denied)));
    }
}

#[test]
fn tls_rejects_untrusted_certificates_and_wrong_hostnames_before_authentication() {
    for wrong_host in [false, true] {
        let fake = FakeTls::start(false);
        let connector = if wrong_host {
            fake.connector()
        } else {
            native_tls::TlsConnector::new().unwrap()
        };
        assert!(matches!(
            fake.connect(
                if wrong_host {
                    "other.example.test"
                } else {
                    "localhost"
                },
                connector,
                Cancellation::default()
            ),
            Err(AcquisitionError::Tls)
        ));
        fake.worker.join().unwrap();
        assert!(fake.commands.lock().unwrap().is_empty());
    }
}

#[test]
fn authentication_failure_is_redacted_and_cancellation_stops_further_commands() {
    let fake = FakeTls::start(true);
    let error = fake
        .connect("localhost", fake.connector(), Cancellation::default())
        .err()
        .unwrap();
    assert_eq!(error, AcquisitionError::Authentication);
    assert!(!format!("{error:?} {error}").contains("synthetic-password"));
    fake.worker.join().unwrap();
    let fake = FakeTls::start(false);
    let cancel = Cancellation::default();
    let mut source = fake
        .connect("localhost", fake.connector(), cancel.clone())
        .unwrap();
    cancel.cancel();
    assert!(matches!(source.folders(), Err(AcquisitionError::Cancelled)));
    drop(source);
    fake.worker.join().unwrap();
    assert_eq!(fake.commands.lock().unwrap().len(), 2);
}

#[test]
fn malformed_names_and_partial_metadata_are_rejected() {
    for name in [
        "Broken&!!!-",
        "Broken&AA-",
        "Broken&2AA-",
        "Broken&unterminated",
    ] {
        assert!(decode_name(name).is_err());
    }
    assert_eq!(decode_name("A&-B").unwrap(), "A&B");
    assert_eq!(
        parse_count(b"* STATUS Junk (MESSAGES invalid)\r\n", "Junk"),
        CountObservation::unknown(UnknownCount::Malformed)
    );
    assert!(validate_uids(&[1, 2], [2].into_iter()).is_err());
}

#[test]
fn excessive_tls_response_is_bounded_and_does_not_reach_discovery() {
    let fake = FakeTls::start_with_limit_probe(false, true);
    assert!(matches!(
        fake.connect("localhost", fake.connector(), Cancellation::default()),
        Err(AcquisitionError::Protocol)
    ));
    fake.worker.join().unwrap();
    assert_eq!(
        &*fake.commands.lock().unwrap(),
        &["LOGIN [REDACTED]", "CAPABILITY"]
    );
}

#[test]
fn actual_tls_acquisition_checkpoints_the_provider_inventory() {
    use crate::census::{AccountId, Census, CensusKeys, EncryptedStore, LocalSecret};
    let fake = FakeTls::start(false);
    let cancel = Cancellation::default();
    let mut source = fake
        .connect("localhost", fake.connector(), cancel.clone())
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let account = AccountId::new("fake-tls-account").unwrap();
    let store = EncryptedStore::new(
        dir.path().to_owned(),
        CensusKeys::derive(&LocalSecret::random(), &account),
    )
    .unwrap();
    let mut census = Census::new(account);
    store.save(&census).unwrap();
    crate::acquisition::scan::run(
        &mut source,
        Provider::Gmail,
        &mut census,
        &store,
        &cancel,
        |_| {},
    )
    .unwrap();
    drop(source);
    fake.worker.join().unwrap();
    let saved = store.load().unwrap();
    assert_eq!(saved.folders.len(), 1);
    assert!(saved.folder_roles["Opaque mailbox"].contains("archive"));
    assert_eq!(saved.folders["[Gmail]/Alle Nachrichten"].messages.len(), 2);
    assert!(
        saved.folders["[Gmail]/Alle Nachrichten"]
            .messages
            .values()
            .all(|m| m.headers.direction == Direction::Sent)
    );
    let evidence = saved.acquisition.view(i64::MAX);
    assert!(evidence.current);
    assert_eq!(evidence.spam.messages(), Some(3));
    let snapshot = evidence.last_successful.unwrap();
    assert!(snapshot.imap_capabilities.contains("SPECIAL-USE"));
    assert_eq!(snapshot.folders[3].delimiter.as_deref(), Some("/"));
    assert_eq!(snapshot.folders[3].name, "Archiv/Grüße");
    assert_eq!(snapshot.folders.last().unwrap().delimiter, None);
    assert!(
        snapshot
            .folders
            .last()
            .unwrap()
            .attributes
            .contains("\\HasNoChildren")
    );
    assert!(saved.winddown_evidence.complete_since.is_none());
    assert!(saved.winddown_evidence.deliveries.is_empty());
    assert_read_only(&fake.commands.lock().unwrap());
}

fn assert_read_only(commands: &[String]) {
    let mut selected = false;
    for command in commands {
        assert!(
            command == "LOGIN [REDACTED]"
                || command == "CAPABILITY"
                || command == "LIST \"\" \"*\" RETURN (SPECIAL-USE)"
                || command.starts_with("STATUS ")
                || command.starts_with("EXAMINE ")
                || command == "UID SEARCH ALL"
                || command.starts_with("UID FETCH ")
        );
        if command.starts_with("EXAMINE ") {
            selected = true;
            assert!(
                !command.contains("Indesirable")
                    && !command.contains("Junk")
                    && !command.contains("Namespace")
            );
        }
        if command.starts_with("UID FETCH ") {
            assert!(selected);
        }
        assert!(!command.contains("synthetic-password"));
        for denied in [
            "STORE", "MOVE", "COPY", "APPEND", "CREATE", "DELETE", "EXPUNGE", "CLOSE", "SELECT ",
            "BODY[]",
        ] {
            assert!(!command.contains(denied));
        }
    }
}

pub(crate) fn install_provider_fixture(repo: &mut super::super::Accounts, root: &std::path::Path) {
    let root = root.to_owned();
    repo.imap_factory = Some(Arc::new(move |permit, login, password, cancel| {
        let mode =
            std::fs::read_to_string(root.join("provider-mode")).unwrap_or_else(|_| "normal".into());
        if mode == "connect_failure" {
            return Err(AcquisitionError::Tls);
        }
        let fake = FakeTls::scenario(
            false,
            false,
            mode.trim(),
            true,
            Some(root.join("provider-wire.log")),
        );
        ReadOnlyImap::open(
            TcpStream::connect(fake.address).unwrap(),
            "localhost",
            fake.connector(),
            login,
            password,
            cancel,
            permit.provider() == Provider::Gmail,
        )
    }));
}

#[test]
fn provider_evidence_reopen_worker() {
    use sha2::{Digest, Sha256};
    let Some(root) = std::env::var_os("MAILROOM_PROVIDER_EVIDENCE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let repo = crate::acquisition::persistent_test_accounts(root);
    let id = crate::census::AccountId::new(
        &std::env::var("MAILROOM_PROVIDER_EVIDENCE_ACCOUNT").unwrap(),
    )
    .unwrap();
    let account = repo.load(&id).unwrap();
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    let mut unrelated = serde_json::to_value(&saved).unwrap();
    unrelated.as_object_mut().unwrap().remove("census");
    unrelated.as_object_mut().unwrap().remove("revision");
    let hash =
        data_encoding::HEXLOWER.encode(&Sha256::digest(serde_json::to_vec(&unrelated).unwrap()));
    std::fs::write(
        root.join("provider-reopen.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": store.schema_version().unwrap(), "evidence": saved.census.acquisition,
            "other_state_sha256":hash, "revision":saved.revision,
            "history_unknown": saved.census.winddown_evidence.complete_since.is_none(),
            "delivery_unknown":saved.census.winddown_evidence.deliveries.is_empty(),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn optional_tls_counts_distinguish_zero_absent_denied_malformed_and_ambiguous() {
    use crate::census::{
        AccountId, Census, CensusKeys, EncryptedStore, LocalSecret, acquisition::UnknownCount,
    };
    for (mode, reason) in [
        ("zero", None),
        ("absent_spam", Some(UnknownCount::NoJunkRole)),
        ("missing", Some(UnknownCount::Missing)),
        ("denied", Some(UnknownCount::Denied)),
        ("malformed", Some(UnknownCount::Unavailable)),
        ("duplicate_count", Some(UnknownCount::Ambiguous)),
        ("wrong_mailbox", Some(UnknownCount::Ambiguous)),
        ("ambiguous", Some(UnknownCount::Ambiguous)),
    ] {
        let fake = FakeTls::scenario(false, false, mode, false, None);
        let cancel = Cancellation::default();
        let mut source = fake
            .connect("localhost", fake.connector(), cancel.clone())
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let account = AccountId::new("optional-tls-count").unwrap();
        let secret = LocalSecret::random();
        let store = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &account),
        )
        .unwrap();
        let mut census = Census::new(account.clone());
        store.save(&census).unwrap();
        crate::acquisition::scan::run(
            &mut source,
            Provider::Gmail,
            &mut census,
            &store,
            &cancel,
            |_| {},
        )
        .unwrap();
        drop(source);
        fake.worker.join().unwrap();
        drop(store);
        let reopened = EncryptedStore::new(
            directory.path().into(),
            CensusKeys::derive(&secret, &account),
        )
        .unwrap();
        assert_eq!(reopened.schema_version().unwrap(), crate::census::SCHEMA);
        let census = reopened.load().unwrap();
        let observation = census.acquisition.view(i64::MAX).spam;
        match reason {
            None => assert_eq!(observation.messages(), Some(0), "{mode}"),
            Some(reason) => assert_eq!(observation, CountObservation::unknown(reason), "{mode}"),
        }
        assert_eq!(
            census.acquisition_state,
            crate::census::ScanState::Complete,
            "{mode}"
        );
        let commands = fake.commands.lock().unwrap();
        assert_read_only(&commands);
        assert!(
            commands.iter().all(
                |c| !c.contains("STATUS \"Namespace\"") && !c.contains("EXAMINE \"Namespace\"")
            )
        );
    }
}

#[test]
fn installed_tls_scan_reopen_failures_retry_topology_reset_and_cancellation_preserve_state() {
    use crate::census::{ScanState, acquisition::UnknownCount};
    let root = tempfile::tempdir().unwrap();
    let mut repo = crate::acquisition::persistent_test_accounts(root.path());
    install_provider_fixture(&mut repo, root.path());
    let account = repo
        .create(
            Provider::Gmail,
            "invented@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    state.winddown.dismissed.insert("retained-decision".into());
    state
        .synthetic_uid_next
        .insert("stable-identity".into(), 31);
    store.save_state(&mut state).unwrap();
    let mut unrelated = serde_json::to_value(&state).unwrap();
    unrelated.as_object_mut().unwrap().remove("census");
    unrelated.as_object_mut().unwrap().remove("revision");
    drop(store);
    repo.scan(&account.id, Cancellation::default(), |_| {})
        .unwrap();
    let store = repo.store(&account).unwrap();
    let good = store.load_state().unwrap();
    let good_snapshot = good.census.acquisition.last_successful.clone();
    let version = good
        .census
        .report(store.keys(), i64::from(i32::MAX), true)
        .unwrap()
        .version;
    drop(store);
    for mode in [
        "partial_list",
        "fail_metadata",
        "connect_failure",
        "multiple_all",
    ] {
        std::fs::write(root.path().join("provider-mode"), mode).unwrap();
        assert!(
            repo.scan(&account.id, Cancellation::default(), |_| {})
                .is_err()
        );
        let store = repo.store(&account).unwrap();
        let saved = store.load_state().unwrap();
        assert_eq!(saved.census.acquisition_state, ScanState::Failed);
        assert_eq!(saved.census.acquisition.last_successful, good_snapshot);
        assert_eq!(saved.census.folder_roles, good.census.folder_roles);
        assert!(!saved.census.acquisition.view(i64::MAX).current);
        assert_eq!(
            saved.census.acquisition.view(i64::MAX).spam,
            CountObservation::unknown(UnknownCount::OlderSnapshot)
        );
        assert_ne!(
            saved
                .census
                .report(store.keys(), i64::from(i32::MAX), true)
                .unwrap()
                .version,
            version
        );
    }
    std::fs::write(root.path().join("provider-mode"), "reset").unwrap();
    repo.scan(&account.id, Cancellation::default(), |_| {})
        .unwrap();
    let saved = repo.store(&account).unwrap().load().unwrap();
    assert!(saved.folders.values().all(|f| f.uidvalidity == 8));
    let prior = saved.acquisition.last_successful;
    let cancel = Cancellation::default();
    assert_eq!(
        repo.scan(&account.id, cancel.clone(), |_| cancel.cancel()),
        Err(AcquisitionError::Cancelled)
    );
    let saved = repo.store(&account).unwrap().load().unwrap();
    assert_eq!(saved.acquisition.last_successful, prior);
    assert_eq!(saved.acquisition_state, ScanState::Cancelled);
    std::fs::write(root.path().join("provider-mode"), "changed").unwrap();
    repo.scan(&account.id, Cancellation::default(), |_| {})
        .unwrap();
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert!(!saved.census.folder_roles.contains_key("Opaque mailbox"));
    assert!(
        !saved
            .census
            .folder_roles
            .values()
            .any(|r| r.contains("all") || r.contains("junk"))
    );
    assert_eq!(
        saved
            .census
            .folders
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["INBOX".into(), "Flat".into()])
    );
    let mut after = serde_json::to_value(&saved).unwrap();
    after.as_object_mut().unwrap().remove("census");
    after.as_object_mut().unwrap().remove("revision");
    assert_eq!(after, unrelated);
    let wire = std::fs::read_to_string(root.path().join("provider-wire.log")).unwrap();
    assert_read_only(&wire.lines().map(String::from).collect::<Vec<_>>());
    assert_eq!(
        wire.lines()
            .filter(|c| *c == "LIST \"\" \"*\" RETURN (SPECIAL-USE)")
            .count(),
        13
    );
}

/// UID sets name exactly the given UIDs: runs of three or more become
/// ranges, and long sets split below the command-line bound.
#[test]
fn uid_sets_name_exactly_the_given_uids() {
    assert_eq!(uid_sets([4, 8, 19]), ["4,8,19"]);
    assert_eq!(uid_sets([1, 2]), ["1,2"]);
    assert_eq!(uid_sets([3, 1, 2, 2, 7, 9, 8, 10]), ["1:3,7:10"]);
    let sparse: Vec<u32> = (0..5_000).map(|n| 1_000_000 + 2 * n).collect();
    let sets = uid_sets(sparse.iter().copied());
    assert!(sets.len() > 1 && sets.iter().all(|s| s.len() <= SET_LENGTH));
    let existing: BTreeSet<u32> = (1..=2_000_000).collect();
    let named: Vec<u32> = sets
        .iter()
        .flat_map(|s| crate::acquisition::fixture_peer::uid_set(s, &existing).unwrap())
        .collect();
    assert_eq!(named, sparse);
}

/// A response may be slow as long as its bytes keep coming at a healthy
/// average; one that stalls or trickles fails within the pace's idle time.
#[test]
fn slow_responses_complete_and_stalled_or_trickling_ones_fail_promptly() {
    for (mode, completes) in [
        ("slow_metadata", true),
        ("stalled_metadata", false),
        ("trickled_metadata", false),
    ] {
        let fake = FakeTls::scenario(false, false, mode, false, None);
        let mut source = fake
            .connect("localhost", fake.connector(), Cancellation::default())
            .unwrap();
        source.gmail = false;
        source.pace(Duration::from_millis(500), 8 * 1024);
        let folders = source.folders().unwrap();
        source.inventory(&folders[0], None).unwrap();
        let started = Instant::now();
        let result = source.metadata(&[1]);
        assert_eq!(result.is_ok(), completes, "{mode}");
        if !completes {
            assert_eq!(result.err(), Some(AcquisitionError::Protocol));
            assert!(started.elapsed() < Duration::from_millis(1500), "{mode}");
        }
        drop(source);
        fake.worker.join().unwrap();
    }
}
