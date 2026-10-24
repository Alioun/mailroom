use super::*;
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
                    format!(
                        "* LIST (\\All) \"/\" \"[Gmail]/Alle Nachrichten\"\r\n* LIST (\\Junk) \"/\" \"Indesirable\"\r\n* LIST (\\Sent) \"/\" \"Gesendet\"\r\n* LIST () \"/\" \"Archiv/Gr&APwA3w-e\"\r\n{tag} OK list\r\n"
                    )
                } else if command.starts_with("EXAMINE ") {
                    format!(
                        "* 2 EXISTS\r\n* FLAGS (\\Seen)\r\n* OK [UIDVALIDITY 7] stable\r\n{tag} OK [READ-ONLY] examined\r\n"
                    )
                } else if command == "UID SEARCH ALL" {
                    format!("* SEARCH 1 2\r\n{tag} OK search\r\n")
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
                            response.push_str(&format!("* {uid} FETCH (UID {uid} RFC822.SIZE 4096 INTERNALDATE \"24-Oct-2026 10:00:00 +0000\" FLAGS (\\Seen) X-GM-MSGID {} X-GM-THRID 200 X-GM-LABELS (\\Sent \"Arbeit/&ANw-bersicht\"))\r\n", 100 + uid));
                        }
                    }
                    response.push_str(&format!("{tag} OK fetched\r\n"));
                    response
                } else {
                    panic!("Unexpected synthetic wire command");
                };
                // Small TLS records force literal and response framing across reads.
                for chunk in response.as_bytes().chunks(17) {
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
    let (validity, uids) = source.inventory(&folders[0]).unwrap();
    assert_eq!(validity, 7);
    assert_eq!(uids, BTreeSet::from([1, 2]));
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
    assert_eq!(saved.folders["[Gmail]/Alle Nachrichten"].messages.len(), 2);
    assert!(
        saved.folders["[Gmail]/Alle Nachrichten"]
            .messages
            .values()
            .all(|m| m.headers.direction == Direction::Sent)
    );
}
