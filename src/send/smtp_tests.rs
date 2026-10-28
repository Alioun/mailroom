use super::*;
use crate::acquisition::{AcquisitionError, SecretValue};
use std::{
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
use zeroize::Zeroizing;

pub(crate) const DUMMY: &str = "dummy-SMTP-secret-7Qx";
const MESSAGE: &[u8] = b"From: books@work.example.test\r\nTo: intake@accounting.example.net\r\nSubject: Fwd\r\n\r\n.leading dot line\r\nbody\r\n.\r\nend\r\n";

pub(crate) struct Dummy(pub AtomicUsize);
impl CredentialBackend for Dummy {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        assert_eq!(reference, "account-password-ref");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(SecretValue::new(DUMMY.as_bytes().to_vec()))
    }
    fn put(&self, _: &str, _: &SecretValue) -> Result<(), AcquisitionError> {
        unreachable!()
    }
    fn delete(&self, _: &str) -> Result<(), AcquisitionError> {
        unreachable!()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Accept,
    Reject,
    DropAfterData,
    Slow,
    NoStartTls,
    InjectAfterStartTls,
    AuthFailEcho,
    /// Echoes the credentials in a continuation line, then closes.
    AuthEchoTruncated,
    /// Echoes the credentials, then sends a line with another code.
    AuthEchoMalformed,
    /// Accepts TCP for implicit TLS and never answers the handshake.
    StallTls,
    RcptReject,
}

/// What the fake server saw. AUTH is checked and then kept only as a yes/no.
#[derive(Default, Debug)]
pub(crate) struct Seen {
    pub commands: Vec<String>,
    pub credentials_matched: bool,
    pub plain_text_auth: bool,
    pub message: Option<Vec<u8>>,
}

pub(crate) struct Fake {
    pub address: SocketAddr,
    pub identity: crate::fake_tls::Identity,
    pub seen: Arc<Mutex<Seen>>,
    worker: thread::JoinHandle<()>,
}
impl Fake {
    pub(crate) fn start(mode: Mode, implicit: bool) -> Self {
        let identity = crate::fake_tls::identity("localhost");
        let server = identity.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let record = seen.clone();
        let worker = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let tcp = loop {
                match listener.accept() {
                    Ok((tcp, _)) => break tcp,
                    Err(_) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => return,
                }
            };
            tcp.set_nonblocking(false).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            if mode == Mode::StallTls {
                thread::sleep(Duration::from_secs(4));
                return;
            }
            let mut plain: Option<BufReader<TcpStream>> = None;
            let mut tls = None;
            if implicit {
                tls = Some(BufReader::new(server.accept(tcp)));
            } else {
                plain = Some(BufReader::new(tcp));
            }
            macro_rules! write {
                ($bytes:expr) => {{
                    let bytes: &[u8] = $bytes;
                    let ok = match (&mut plain, &mut tls) {
                        (Some(p), _) => p.get_mut().write_all(bytes).is_ok(),
                        (_, Some(t)) => t
                            .get_mut()
                            .write_all(bytes)
                            .and_then(|_| t.get_mut().flush())
                            .is_ok(),
                        _ => false,
                    };
                    if !ok {
                        return;
                    }
                }};
            }
            write!(b"220 fake ESMTP\r\n");
            let mut in_data = false;
            let mut data = vec![];
            loop {
                let mut line = vec![];
                let read = match (&mut plain, &mut tls) {
                    (Some(p), _) => p.read_until(b'\n', &mut line),
                    (_, Some(t)) => t.read_until(b'\n', &mut line),
                    _ => return,
                };
                if !matches!(read, Ok(n) if n > 0) {
                    return;
                }
                if in_data {
                    if line == b".\r\n" {
                        in_data = false;
                        record.lock().unwrap().message = Some(std::mem::take(&mut data));
                        match mode {
                            Mode::DropAfterData => return,
                            Mode::Reject => write!(b"554 5.7.1 rejected\r\n"),
                            Mode::Slow => {
                                thread::sleep(Duration::from_millis(1500));
                                write!(b"250 2.0.0 queued late\r\n");
                            }
                            _ => write!(b"250 2.0.0 Ok queued as FAKE1\r\n"),
                        }
                    } else {
                        data.extend_from_slice(line.strip_prefix(b".").unwrap_or(&line));
                    }
                    continue;
                }
                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                let upper = text.to_ascii_uppercase();
                if let Some(encoded) = text.strip_prefix("AUTH PLAIN ") {
                    let decoded = data_encoding::BASE64.decode(encoded.as_bytes()).unwrap();
                    let mut seen = record.lock().unwrap();
                    seen.commands.push("AUTH PLAIN [checked]".into());
                    seen.plain_text_auth |= tls.is_none();
                    seen.credentials_matched = decoded
                        == [b"\0books@work.example.test\0".as_slice(), DUMMY.as_bytes()].concat();
                    drop(seen);
                    if mode == Mode::AuthFailEcho {
                        let echo = format!("535-5.7.8 bad {DUMMY}\r\n535 5.7.8 {encoded}\r\n");
                        write!(echo.as_bytes());
                    } else if mode == Mode::AuthEchoTruncated {
                        let echo = format!("535-5.7.8 {encoded}\r\n535-5.7.8 {DUMMY}");
                        write!(echo.as_bytes());
                        return;
                    } else if mode == Mode::AuthEchoMalformed {
                        let echo = format!("535-5.7.8 {encoded}\r\n421 {DUMMY}\r\n");
                        write!(echo.as_bytes());
                    } else {
                        write!(b"235 2.7.0 ok\r\n");
                    }
                    continue;
                }
                record.lock().unwrap().commands.push(text.clone());
                if upper.starts_with("EHLO") {
                    if tls.is_none() && mode != Mode::NoStartTls {
                        write!(b"250-fake\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n");
                    } else {
                        write!(b"250-fake\r\n250-8BITMIME\r\n250 AUTH PLAIN LOGIN\r\n");
                    }
                } else if upper == "STARTTLS" {
                    if mode == Mode::InjectAfterStartTls {
                        write!(b"220 go\r\n250 injected\r\n");
                    } else {
                        write!(b"220 go\r\n");
                    }
                    let tcp = plain.take().unwrap().into_inner();
                    tls = Some(BufReader::new(server.accept(tcp)));
                } else if upper.starts_with("MAIL FROM:") {
                    write!(b"250 ok\r\n");
                } else if upper.starts_with("RCPT TO:") {
                    if mode == Mode::RcptReject {
                        write!(b"550 5.1.1 no such user\r\n");
                    } else {
                        write!(b"250 ok\r\n");
                    }
                } else if upper == "DATA" {
                    in_data = true;
                    write!(b"354 go\r\n");
                } else if upper == "QUIT" {
                    write!(b"221 bye\r\n");
                    return;
                } else {
                    write!(b"502 unknown\r\n");
                }
            }
        });
        Self {
            address,
            identity,
            seen,
            worker,
        }
    }
    pub(crate) fn server(&self, implicit: bool) -> Server {
        Server {
            address: self.address,
            tls_name: "localhost".into(),
            security: if implicit {
                Security::ImplicitTls
            } else {
                Security::StartTls
            },
            tls: self.identity.connector(),
        }
    }
    pub(crate) fn finish(self) -> Seen {
        let _ = self.worker.join();
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

pub(crate) fn submit_to(
    fake: &Fake,
    implicit: bool,
    backend: &Dummy,
    timeouts: Timeouts,
) -> Outcome {
    submit(
        &fake.server(implicit),
        &Login {
            user: "books@work.example.test",
            backend,
            reference: "account-password-ref",
        },
        "books@work.example.test",
        "intake@accounting.example.net",
        MESSAGE,
        timeouts,
    )
}

fn quick() -> Timeouts {
    Timeouts {
        command: Duration::from_secs(5),
        final_reply: Duration::from_millis(500),
    }
}

/// Runs every scenario and returns each outcome with what the server saw.
fn scenarios() -> Vec<(String, Outcome, Seen, usize)> {
    let mut results = vec![];
    for (mode, implicit) in [
        (Mode::Accept, true),
        (Mode::Accept, false),
        (Mode::Reject, true),
        (Mode::DropAfterData, false),
        (Mode::Slow, true),
        (Mode::NoStartTls, false),
        (Mode::InjectAfterStartTls, false),
        (Mode::AuthFailEcho, true),
        (Mode::AuthEchoTruncated, true),
        (Mode::AuthEchoMalformed, false),
        (Mode::RcptReject, false),
    ] {
        let fake = Fake::start(mode, implicit);
        let backend = Dummy(AtomicUsize::new(0));
        let outcome = submit_to(&fake, implicit, &backend, quick());
        let seen = fake.finish();
        results.push((
            format!("{mode:?}/{implicit}"),
            outcome,
            seen,
            backend.0.load(Ordering::SeqCst),
        ));
    }
    results
}

#[test]
fn submissions_record_exact_outcomes_and_never_retry() {
    for (name, outcome, seen, reads) in scenarios() {
        let messages = seen.message.iter().count();
        assert!(messages <= 1, "{name}: at most one submission");
        assert!(!seen.plain_text_auth, "{name}: AUTH only over TLS");
        match name.as_str() {
            "Accept/true" | "Accept/false" => {
                assert_eq!(
                    outcome,
                    Outcome::Accepted {
                        code: 250,
                        enhanced: Some("2.0.0".into())
                    },
                    "{name}"
                );
                assert_eq!(
                    seen.message.as_deref(),
                    Some(MESSAGE),
                    "{name}: exact bytes after dot-unstuffing"
                );
                assert!(seen.credentials_matched && reads == 1, "{name}");
                assert!(
                    seen.commands
                        .contains(&"MAIL FROM:<books@work.example.test>".to_owned())
                );
                assert!(
                    seen.commands
                        .contains(&"RCPT TO:<intake@accounting.example.net>".to_owned())
                );
                assert_eq!(
                    seen.commands
                        .iter()
                        .filter(|c| c.starts_with("RCPT"))
                        .count(),
                    1
                );
            }
            "Reject/true" => assert_eq!(
                outcome,
                Outcome::Rejected {
                    code: 554,
                    enhanced: Some("5.7.1".into())
                }
            ),
            "DropAfterData/false" => {
                assert_eq!(
                    outcome,
                    Outcome::Uncertain {
                        stage: Stage::Message
                    }
                );
                assert_eq!(
                    seen.message.as_deref(),
                    Some(MESSAGE),
                    "the server may have kept it"
                );
            }
            "Slow/true" => assert_eq!(
                outcome,
                Outcome::Uncertain {
                    stage: Stage::Message
                }
            ),
            "NoStartTls/false" => {
                assert_eq!(
                    outcome,
                    Outcome::NotSent {
                        stage: Stage::StartTls,
                        code: None
                    }
                );
                assert_eq!(reads, 0, "no password read without TLS");
                assert!(
                    seen.commands
                        .iter()
                        .all(|c| !c.starts_with("AUTH") && !c.starts_with("MAIL"))
                );
            }
            "InjectAfterStartTls/false" => {
                assert_eq!(
                    outcome,
                    Outcome::NotSent {
                        stage: Stage::StartTls,
                        code: None
                    }
                );
                assert_eq!(reads, 0);
            }
            "AuthEchoTruncated/true" | "AuthEchoMalformed/false" => {
                assert_eq!(
                    outcome,
                    Outcome::NotSent {
                        stage: Stage::Auth,
                        code: None
                    },
                    "{name}: an echoed, unfinished or inconsistent reply ends the session"
                );
                assert!(seen.commands.iter().all(|c| !c.starts_with("MAIL")));
            }
            "AuthFailEcho/true" => {
                assert_eq!(
                    outcome,
                    Outcome::NotSent {
                        stage: Stage::Auth,
                        code: Some(535)
                    }
                );
                assert!(seen.commands.iter().all(|c| !c.starts_with("MAIL")));
            }
            "RcptReject/false" => {
                assert_eq!(
                    outcome,
                    Outcome::NotSent {
                        stage: Stage::Rcpt,
                        code: Some(550)
                    }
                );
                assert!(seen.message.is_none() && seen.commands.iter().all(|c| c != "DATA"));
            }
            _ => unreachable!(),
        }
        let recorded = format!("{outcome:?}{}", serde_json::to_string(&outcome).unwrap());
        assert!(!recorded.contains(DUMMY), "{name}");
    }
}

#[test]
fn slow_final_reply_within_its_deadline_is_accepted() {
    let fake = Fake::start(Mode::Slow, false);
    let outcome = submit_to(
        &fake,
        false,
        &Dummy(AtomicUsize::new(0)),
        Timeouts {
            command: Duration::from_secs(5),
            final_reply: Duration::from_secs(5),
        },
    );
    assert_eq!(
        outcome,
        Outcome::Accepted {
            code: 250,
            enhanced: Some("2.0.0".into())
        }
    );
    fake.finish();
}

#[test]
fn untrusted_certificate_or_wrong_name_is_refused_before_any_command() {
    for (implicit, wrong_name) in [(true, false), (false, false), (true, true)] {
        let fake = Fake::start(Mode::Accept, implicit);
        let mut server = fake.server(implicit);
        if wrong_name {
            server.tls_name = "other.example".into();
        } else {
            server.tls = crate::fake_tls::identity("localhost").connector();
        }
        let backend = Dummy(AtomicUsize::new(0));
        let outcome = submit(
            &server,
            &Login {
                user: "books@work.example.test",
                backend: &backend,
                reference: "account-password-ref",
            },
            "books@work.example.test",
            "intake@accounting.example.net",
            MESSAGE,
            quick(),
        );
        assert_eq!(
            outcome,
            Outcome::NotSent {
                stage: Stage::Tls,
                code: None
            }
        );
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
        drop(server);
        drop(fake);
    }
}

#[test]
fn malformed_input_is_refused_before_connecting() {
    let backend = Dummy(AtomicUsize::new(0));
    let fake = Fake::start(Mode::Accept, true);
    let server = fake.server(true);
    let login = Login {
        user: "books@work.example.test",
        backend: &backend,
        reference: "account-password-ref",
    };
    for (sender, recipient, message) in [
        (
            "books@work.example.test",
            "a@b.example\r\nRCPT TO:<x@y.example>",
            MESSAGE,
        ),
        (
            "books@work.example.test",
            "intake@accounting.example.net",
            b"bare\nnewline\r\n".as_slice(),
        ),
        (
            "books@work.example.test",
            "intake@accounting.example.net",
            b"no final line end".as_slice(),
        ),
    ] {
        assert_eq!(
            submit(&server, &login, sender, recipient, message, quick()),
            Outcome::NotSent {
                stage: Stage::Input,
                code: None
            }
        );
    }
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
}

/// Child process for `dummy_password_never_reaches_output`: runs every scenario
/// and prints each outcome as an adapter's caller would log it.
#[test]
fn secret_flow_worker() {
    if std::env::var_os("MAILROOM_SMTP_SECRET_WORKER").is_none() {
        return;
    }
    for (name, outcome, seen, reads) in scenarios() {
        println!(
            "{name} {outcome:?} {} reads={reads} commands={:?}",
            serde_json::to_string(&outcome).unwrap(),
            seen.commands
        );
        eprintln!("{name} {outcome:?}");
    }
    println!("worker-complete");
}

#[test]
fn dummy_password_never_reaches_output_argv_or_errors() {
    let exe = std::env::current_exe().unwrap();
    let args = [
        "--exact",
        "send::smtp::tests::secret_flow_worker",
        "--nocapture",
        "--test-threads=1",
    ];
    assert!(args.iter().all(|a| !a.contains(DUMMY)));
    let output = std::process::Command::new(exe)
        .args(args)
        .env("MAILROOM_SMTP_SECRET_WORKER", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("worker-complete"));
    for name in ["AuthFailEcho", "AuthEchoTruncated", "AuthEchoMalformed"] {
        assert!(stdout.contains(name), "failure path {name} ran");
    }
    // The raw password, the exact AUTH PLAIN encoding the fake echoes, and the
    // password's own encoding at each of the three base64 alignments.
    let auth = data_encoding::BASE64
        .encode(&[b"\0books@work.example.test\0".as_slice(), DUMMY.as_bytes()].concat());
    let mut needles = vec![DUMMY.to_owned(), auth];
    for pad in 0..3 {
        let encoded = data_encoding::BASE64.encode(&[&b"xx"[..pad], DUMMY.as_bytes()].concat());
        needles.push(encoded[4.min(encoded.len())..20].to_owned());
    }
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        for needle in &needles {
            assert!(
                !text.contains(needle.as_str()),
                "dummy password form in child output"
            );
        }
    }
}

#[test]
fn a_stalled_implicit_tls_handshake_ends_at_the_command_deadline() {
    let fake = Fake::start(Mode::StallTls, true);
    let backend = Dummy(AtomicUsize::new(0));
    let timeouts = Timeouts {
        command: Duration::from_millis(300),
        final_reply: Duration::from_millis(300),
    };
    let started = std::time::Instant::now();
    let outcome = submit_to(&fake, true, &backend, timeouts);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the handshake waited {:?}",
        started.elapsed()
    );
    assert_eq!(
        outcome,
        Outcome::NotSent {
            stage: Stage::Tls,
            code: None
        }
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 0, "no password was read");
    drop(fake);
}

/// A reply's lines own erasing storage, so dropping a reply, or any error inside
/// `Session::reply`, wipes every echoed line.
#[test]
fn reply_lines_wipe_themselves() {
    fn erasing(_: &[Zeroizing<String>]) {}
    let reply = Reply {
        code: 535,
        lines: vec![Zeroizing::new(DUMMY.to_owned())],
    };
    erasing(&reply.lines);
}

#[test]
fn invalid_utf8_reply_text_preserves_lossy_decoding_without_growing() {
    let samples = [
        Vec::new(),
        DUMMY.as_bytes().to_vec(),
        [b"\xff".as_slice(), DUMMY.as_bytes(), b"\xfe"].concat(),
        vec![0xff; LINE_LIMIT],
        "Grüße 📨".as_bytes().to_vec(),
        vec![0xe2, 0x82],
        vec![0xf0, 0x90, 0x80, 0x41, 0xff],
    ];
    for bytes in samples {
        let text = reply_text(&bytes);
        assert_eq!(text.as_str(), String::from_utf8_lossy(&bytes).as_ref());
    }
}

#[test]
fn reply_parser_handles_echoes_and_refuses_malformed_or_truncated_responses() {
    use std::io::Cursor;
    let encoded = data_encoding::BASE64.encode(format!("\0owner@example.test\0{DUMMY}").as_bytes());
    let accepted = format!("535-{DUMMY}\r\n535 {encoded}\r\n");
    let reply = read_reply(&mut Cursor::new(accepted.as_bytes())).unwrap();
    assert_eq!(reply.code, 535);
    assert_eq!(reply.lines.len(), 2);
    assert_eq!(reply.lines[0].as_str(), DUMMY);
    assert_eq!(reply.lines[1].as_str(), encoded);
    for input in [
        format!("535-{DUMMY}\r\n"),
        format!("535-{encoded}\r\n535 {DUMMY}"),
        format!("535-{encoded}\r\n235 {DUMMY}\r\n"),
        format!("535?{DUMMY}\r\n"),
        format!("535 {}{DUMMY}\r\n", "x".repeat(LINE_LIMIT)),
        format!(
            "{}535 {DUMMY}\r\n",
            format!("535-{encoded}\r\n").repeat(REPLY_LINES)
        ),
    ] {
        let error = read_reply(&mut Cursor::new(input.as_bytes()))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "Malformed SMTP reply");
    }
    let invalid_utf8 = [b"535 ".as_slice(), DUMMY.as_bytes(), b"\xff\r\n"].concat();
    let reply = read_reply(&mut Cursor::new(invalid_utf8)).unwrap();
    assert_eq!(reply.lines[0].as_str(), format!("{DUMMY}\u{fffd}"));
}
