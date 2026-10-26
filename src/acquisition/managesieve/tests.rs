use super::*;
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crate::acquisition::fixture_peer::Peer;
pub(crate) const PRIVATE_SCRIPT: &[u8] =
    b"# invented private bytes\r\n# OK\r\n# {17} \\\" \xc3\xa4\r\nkeep;\r\n\xff";
pub(crate) struct Fake {
    pub(in crate::acquisition) source: InstalledSource,
    pub commands: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    tls_hello: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
fn send(io: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    for fragment in bytes.chunks(if bytes.len() > 65536 { 4096 } else { 3 }) {
        io.write_all(fragment).inspect_err(|error| {
            eprintln!("sieve.fixture.write.io_kind={:?}", error.kind());
        })?;
        io.flush().inspect_err(|error| {
            eprintln!("sieve.fixture.flush.io_kind={:?}", error.kind());
        })?;
    }
    Ok(())
}
impl Fake {
    pub(crate) fn start(mode: &str, mode_file: Option<std::path::PathBuf>) -> Self {
        let mode = mode.to_owned();
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        if mode == "expired" {
            params.not_before = rcgen::date_time_ymd(2010, 1, 1);
            params.not_after = rcgen::date_time_ymd(2011, 1, 1);
        }
        let cert = params.self_signed(&key).unwrap();
        let root = cert.der().to_vec();
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
                )
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let commands = Arc::new(Mutex::new(vec![]));
        let log = commands.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let tls_hello = Arc::new(AtomicBool::new(false));
        let observed_tls_hello = tls_hello.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (tcp, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if pending(&e) => {
                        thread::sleep(POLL);
                        continue;
                    }
                    Err(_) => return,
                };
                let mode = mode_file
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_else(|| mode.clone());
                let mut tcp = Peer::new(tcp, stopping.clone()).unwrap();
                let greeting = if mode == "no_starttls" {
                    b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n".as_slice()
                } else {
                    b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"pre-tls-only\"\r\n\"VERSION\" \"1.0\"\r\n\"STARTTLS\"\r\nOK\r\n".as_slice()
                };
                if mode == "drip" {
                    for b in greeting {
                        if tcp.write_all(&[*b]).is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(15));
                    }
                    continue;
                }
                if send(&mut tcp, greeting).is_err() {
                    continue;
                }
                let mut plain = BufReader::new(tcp);
                let mut line = String::new();
                if plain.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                assert!(line == "STARTTLS\r\n");
                log.lock().unwrap().push("STARTTLS".into());
                if mode == "deny_starttls" {
                    let _ = send(plain.get_mut(), b"NO (TRYLATER) \"denied\"\r\n");
                    continue;
                }
                if send(plain.get_mut(), b"OK\r\n").is_err() {
                    continue;
                }
                let mut tcp = plain.into_inner();
                if mode == "stall_tls" {
                    let mut header = [0; 5];
                    if tcp.read_exact(&mut header).is_ok() && header[0] == 22 {
                        observed_tls_hello.store(true, Ordering::Relaxed);
                    }
                    Peer::until_closed(&mut tcp);
                    continue;
                }
                let mut io = BufReader::new(StreamOwned::new(
                    ServerConnection::new(config.clone()).unwrap(),
                    tcp,
                ));
                let post = b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"post-tls-only\"\r\n\"SASL\" \"PLAIN\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n";
                let post = if mode == "unsupported_sasl" {
                    b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\n\"SASL\" \"EXTERNAL\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n".as_slice()
                } else {
                    post.as_slice()
                };
                let post = if mode == "post_starttls" {
                    b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\n\"SASL\" \"PLAIN\"\r\n\"VERSION\" \"1.0\"\r\n\"STARTTLS\"\r\nOK\r\n".as_slice()
                } else {
                    post
                };
                if send(io.get_mut(), post).is_err() {
                    continue;
                }
                let mut gets = 0;
                let mut lists = 0;
                let mut cap_reads = 0;
                loop {
                    let mut line = String::new();
                    match io.read_line(&mut line) {
                        Ok(n) if n > 0 => {}
                        Ok(_) => break,
                        Err(error) => {
                            eprintln!("sieve.fixture.read.io_kind={:?}", error.kind());
                            break;
                        }
                    }
                    let kind = line.split_ascii_whitespace().next().unwrap().to_owned();
                    assert!(matches!(
                        kind.as_str(),
                        "AUTHENTICATE" | "CAPABILITY" | "LISTSCRIPTS" | "GETSCRIPT" | "LOGOUT"
                    ));
                    log.lock().unwrap().push(if kind == "AUTHENTICATE" {
                        "AUTHENTICATE [REDACTED]".into()
                    } else {
                        kind.clone()
                    });
                    if let Some(path) = &mode_file {
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path.with_file_name("sieve-wire.log"))
                            .unwrap();
                        writeln!(
                            file,
                            "{}",
                            if kind == "AUTHENTICATE" {
                                "AUTHENTICATE [REDACTED]"
                            } else {
                                &kind
                            }
                        )
                        .unwrap();
                    }
                    let response: Vec<u8> = match kind.as_str() {
                        "AUTHENTICATE" => {
                            assert!(line.starts_with("AUTHENTICATE \"PLAIN\" \""));
                            if mode == "auth_denied" {
                                b"NO \"invented-secret-reflected-but-never-retained\"\r\n".to_vec()
                            } else if mode == "stall_auth" {
                                Peer::until_closed(&mut io);
                                break;
                            } else {
                                b"OK\r\n".to_vec()
                            }
                        }
                        "CAPABILITY" => {
                            cap_reads += 1;
                            if mode == "many_caps" {
                                (0..70)
                                    .map(|n| format!("\"X-{n}\" \"{}\"\r\n", "x".repeat(1000)))
                                    .collect::<String>()
                                    .into_bytes()
                            } else if mode == "cap_count" {
                                (0..257)
                                    .map(|n| format!("\"X-{n}\" \"x\"\r\n"))
                                    .collect::<String>()
                                    .into_bytes()
                            } else if mode == "excess_caps" {
                                b"X".repeat(RESPONSE + 1)
                            } else if mode == "duplicate_caps" {
                                b"\"SIEVE\" \"x\"\r\n\"sieve\" \"y\"\r\nOK\r\n".to_vec()
                            } else if mode == "legacy" {
                                b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\nOK\r\n"
                                    .to_vec()
                            } else {
                                format!("\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"{}\"\r\n\"SASL\" \"PLAIN\"\r\n\"VERSION\" \"1.0\"\r\n\"X-UNKNOWN\" \"ignored\"\r\nOK (X-UNKNOWN (ONE TWO)) \"localized diagnostic\"\r\n", if mode == "changed" || (mode == "unstable_caps" && cap_reads == 2) { "fileinto" } else { "fileinto include imap4flags" }).into_bytes()
                            }
                        }
                        "LISTSCRIPTS" => {
                            lists += 1;
                            match mode.as_str() {
                            "line_separator" => "\"bad\u{2028}name\"\r\nOK\r\n".as_bytes().to_vec(),
                            "paragraph_separator" => "\"bad\u{2029}name\"\r\nOK\r\n".as_bytes().to_vec(),
                            "bom_name" => "\"\u{feff}bad\"\r\nOK\r\n".as_bytes().to_vec(),
                            "unassigned_name" => "\"bad\u{0378}name\"\r\nOK\r\n".as_bytes().to_vec(),
                            "literal_names" => b"{12}\r\nOpen-Xchange ACTIVE\r\n{3}\r\n\xc3\xa4x\r\nOK\r\n".to_vec(),
                            "zero_body" | "max_body" => b"\"Open-Xchange\" ACTIVE\r\nOK\r\n".to_vec(),
                            "partial_bodies" | "two_bodies" | "attempt_limit" => b"\"other\" ACTIVE\r\n\"Open-Xchange\"\r\nOK\r\n".to_vec(),
                            "bad_escape" => b"\"bad\\q\"\r\nOK\r\n".to_vec(),
                            "bad_crlf" => b"\"bad\"\nOK\r\n".to_vec(),
                            "control_name" => b"\"bad\x01name\"\r\nOK\r\n".to_vec(),
                            "too_many_names" => (0..257).map(|n| format!("\"name{n}\"\r\n")).collect::<String>().into_bytes(),
                            "empty" => b"OK\r\n".to_vec(), "partial_list" => b"\"Open-Xchange\" ACTIVE\r\nNO (QUOTA/MAXSIZE) \"incomplete\"\r\n".to_vec(),
                            "two_active" => b"\"one\" ACTIVE\r\n\"two\" ACTIVE\r\nOK\r\n".to_vec(),
                            "duplicate_names" => b"\"one\"\r\n\"one\"\r\nOK\r\n".to_vec(),
                            "bye" => b"BYE (REFERRAL \"sieve://unreachable.invalid\") \"bye\"\r\n".to_vec(),
                            "changed" => b"\"new\"\r\nOK\r\n".to_vec(),
                            "unstable_names" if lists == 2 => b"OK\r\n".to_vec(),
                            _ => "\"Open-Xchange\" ACTIVE\r\n\"foreign\\\"\\\\ä\"\r\nOK\r\n".as_bytes().to_vec() }
                        }
                        "GETSCRIPT" => {
                            let n: usize = line
                                .trim_end()
                                .strip_prefix("GETSCRIPT {")
                                .unwrap()
                                .strip_suffix("+}")
                                .unwrap()
                                .parse()
                                .unwrap();
                            let mut name = vec![0; n + 2];
                            if io.read_exact(&mut name).is_err() {
                                break;
                            }
                            assert_eq!(&name[n..], b"\r\n");
                            gets += 1;
                            match mode.as_str() {
                                "missing" => b"NO (NONEXISTENT) \"missing\"\r\n".to_vec(),
                                "denied_get" => b"NO \"denied\"\r\n".to_vec(),
                                "partial_bodies" if gets == 2 => {
                                    b"NO (NONEXISTENT) \"missing second body\"\r\n".to_vec()
                                }
                                "long_quoted_wire" => {
                                    [b"\"".as_slice(), &b"\\\"".repeat(600), b"\"\r\nOK\r\n"]
                                        .concat()
                                }
                                "bad_quoted_utf8" => b"\"\xff\"\r\nOK\r\n".to_vec(),
                                "error_literal" => {
                                    let text = b"OK\r\n{1}\r\nx";
                                    [
                                        format!("NO (QUOTA/MAXSIZE) {{{}}}\r\n", text.len())
                                            .as_bytes(),
                                        text,
                                        b"\r\n",
                                    ]
                                    .concat()
                                }
                                "valid_text" => {
                                    let bytes = b"keep;\r\n# <img src='https://remote.invalid/x'> <script>throw 1</script>\r\n";
                                    [
                                        format!("{{{}}}\r\n", bytes.len()).as_bytes(),
                                        bytes,
                                        b"\r\nOK\r\n",
                                    ]
                                    .concat()
                                }
                                "leading_zero" => b"{01}\r\nx\r\nOK\r\n".to_vec(),
                                "malformed_code" => b"NO (QUOTA (ONE) \"bad\"\r\n".to_vec(),
                                "zero_body" => b"{0}\r\n\r\nOK\r\n".to_vec(),
                                "max_body" | "attempt_limit" => [
                                    format!("{{{}}}\r\n", SCRIPT).as_bytes(),
                                    &vec![b'x'; SCRIPT],
                                    b"\r\nOK\r\n",
                                ]
                                .concat(),
                                "overflow" => b"{4294967296}\r\n".to_vec(),
                                "oversize" => b"{1048577}\r\n".to_vec(),
                                "eof" => {
                                    let _ = send(io.get_mut(), b"{100}\r\npartial");
                                    break;
                                }
                                "stall_read" => {
                                    Peer::until_closed(&mut io);
                                    break;
                                }
                                _ => {
                                    let bytes = if mode == "unstable" && gets == 2 {
                                        b"changed".as_slice()
                                    } else {
                                        PRIVATE_SCRIPT
                                    };
                                    [
                                        format!("{{{}}}\r\n", bytes.len()).as_bytes(),
                                        bytes,
                                        b"\r\nOK\r\n",
                                    ]
                                    .concat()
                                }
                            }
                        }
                        "LOGOUT" => {
                            let _ = send(io.get_mut(), b"OK \"bye\"\r\n");
                            break;
                        }
                        _ => unreachable!(),
                    };
                    if send(io.get_mut(), &response).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            source: InstalledSource {
                address,
                tls_name: "localhost".into(),
                root,
                epoch: "invented-installation-v1".into(),
                command_timeout: crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT,
                attempt_timeout: Duration::from_secs(10),
            },
            commands,
            stop,
            tls_hello,
            worker: Some(worker),
        }
    }
}
#[test]
fn actual_starttls_plain_explicit_authenticated_capability_and_exact_literal() {
    let fake = Fake::start("normal", None);
    let (mut session, caps) = Session::connect(
        fake.source.permit().unwrap(),
        "invented@example.test",
        &SecretValue::new(b"invented".to_vec()),
        Cancellation::default(),
    )
    .unwrap();
    assert_eq!(caps["SIEVE"], "fileinto include imap4flags");
    let names = session.list().unwrap();
    assert_eq!(names.len(), 2);
    assert!(names[0].active);
    assert!(session.get("Open-Xchange").unwrap() == PRIVATE_SCRIPT);
    session.logout().unwrap();
    assert_eq!(
        *fake.commands.lock().unwrap(),
        [
            "STARTTLS",
            "AUTHENTICATE [REDACTED]",
            "CAPABILITY",
            "LISTSCRIPTS",
            "GETSCRIPT",
            "LOGOUT"
        ]
    );
}
#[test]
fn tls_identity_chain_expiry_and_starttls_fail_before_authentication() {
    for mode in [
        "wrong_name",
        "untrusted",
        "expired",
        "no_starttls",
        "deny_starttls",
        "post_starttls",
        "unsupported_sasl",
    ] {
        let mut fake = Fake::start(mode, None);
        if mode == "wrong_name" {
            fake.source.tls_name = "wrong.invalid".into();
        }
        if mode == "untrusted" {
            fake.source.root = rcgen::generate_simple_self_signed(vec!["localhost".into()])
                .unwrap()
                .cert
                .der()
                .to_vec();
        }
        assert!(
            Session::connect(
                fake.source.permit().unwrap(),
                "invented@example.test",
                &SecretValue::new(b"invented".to_vec()),
                Cancellation::default()
            )
            .is_err(),
            "{mode}"
        );
        assert!(
            !fake
                .commands
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("AUTHENTICATE")),
            "{mode}"
        );
    }
}
#[test]
fn absolute_drip_deadlines_and_cancellation_before_connect_tls_auth_read() {
    for mode in ["drip", "stall_tls", "stall_auth", "stall_read", "before"] {
        let mut fake = Fake::start(mode, None);
        if mode != "stall_read" {
            fake.source.command_timeout = Duration::from_millis(100);
        }
        let cancel = Cancellation::default();
        if mode == "before" {
            cancel.cancel();
        }
        let mut start = Instant::now();
        let result = Session::connect(
            fake.source.permit().unwrap(),
            "invented@example.test",
            &SecretValue::new(b"invented".to_vec()),
            cancel,
        );
        if mode == "stall_read" {
            let (mut s, _) = result.unwrap();
            assert!(s.list().is_ok());
            s.set_command_timeout(Duration::from_millis(100));
            start = Instant::now();
            assert_eq!(s.get("Open-Xchange"), Err(Error::Deadline));
        } else {
            assert!(result.is_err(), "{mode}");
        }
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(1), "{mode} {elapsed:?}");
    }
}

#[test]
fn actual_cancellation_interrupts_tls_authentication_and_script_read() {
    for (mode, command) in [
        ("stall_tls", "STARTTLS"),
        ("stall_auth", "AUTHENTICATE [REDACTED]"),
        ("stall_read", "GETSCRIPT"),
    ] {
        let fake = Fake::start(mode, None);
        let cancel = Cancellation::default();
        let cancelling = cancel.clone();
        let log = fake.commands.clone();
        let tls_hello = fake.tls_hello.clone();
        let in_tls = mode == "stall_tls";
        let window = fake.source.attempt_timeout;
        let worker = thread::spawn(move || {
            let end = Instant::now() + window;
            while Instant::now() < end {
                if if in_tls {
                    tls_hello.load(Ordering::Relaxed)
                } else {
                    log.lock().unwrap().iter().any(|c| c == command)
                } {
                    cancelling.cancel();
                    return;
                }
                thread::sleep(POLL);
            }
            panic!("Expected cancellation boundary not reached");
        });
        let result = Session::connect(
            fake.source.permit().unwrap(),
            "invented@example.test",
            &SecretValue::new(b"invented".to_vec()),
            cancel,
        );
        if mode == "stall_read" {
            let (mut session, _) = result.unwrap();
            session.list().unwrap();
            assert_eq!(session.get("Open-Xchange"), Err(Error::Cancelled));
        } else {
            assert!(matches!(result, Err(Error::Cancelled)), "{mode}");
        }
        worker.join().unwrap();
    }
}

#[test]
fn counted_utf8_names_and_empty_or_maximum_exact_bodies_are_successful() {
    for mode in ["literal_names", "zero_body", "max_body"] {
        let fake = Fake::start(mode, None);
        let (mut session, _) = Session::connect(
            fake.source.permit().unwrap(),
            "invented@example.test",
            &SecretValue::new(b"invented".to_vec()),
            Cancellation::default(),
        )
        .unwrap();
        let names = session.list().unwrap();
        if mode == "literal_names" {
            assert!(names.iter().any(|n| n.name == "äx"));
        }
        let bytes = session.get("Open-Xchange").unwrap();
        if mode == "zero_body" {
            assert!(bytes.is_empty());
        }
        if mode == "max_body" {
            assert_eq!(bytes.len(), SCRIPT);
            assert!(bytes.iter().all(|b| *b == b'x'));
        }
        session.logout().unwrap();
    }
}
