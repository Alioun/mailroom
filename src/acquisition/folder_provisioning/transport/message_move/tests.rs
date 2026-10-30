use super::*;
use crate::{
    acquisition::{Account, Accounts, fixture_peer::Peer, persistent_test_accounts},
    backlog::imap_move::{MoveIntent, Purpose, Resolution},
    census::{
        AccountState, Direction, EncryptedStore, FolderCensus, MessageRecord, ScanState,
        parse_headers,
    },
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[path = "copy_fixture_tests.rs"]
mod copy_fixture;

const HEADER: &[u8] = b"From: updates@studio.example.com\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\nSubject: Release notes\r\nMessage-ID: <identical@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n";

/// How long the `slow_greeting` fake takes to trickle out its greeting, one
/// byte at a time. It must outlast the longest greeting deadline any fixture
/// source installs, which is the installed source's command timeout, or the
/// greeting completes and the fault case tests nothing.
const SLOW_GREETING: Duration = Duration::from_millis(
    crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT.as_millis() as u64 * 3 / 2,
);

struct EvidenceRoot(Option<tempfile::TempDir>);
impl EvidenceRoot {
    fn new() -> Self {
        Self(Some(
            if let Some(path) = std::env::var_os("MAILROOM_MOVE_TEST_EVIDENCE") {
                std::fs::create_dir_all(&path).unwrap();
                tempfile::Builder::new()
                    .prefix("move-tls-")
                    .tempdir_in(path)
                    .unwrap()
            } else {
                tempfile::tempdir().unwrap()
            },
        ))
    }
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for EvidenceRoot {
    fn drop(&mut self) {
        if std::env::var_os("MAILROOM_MOVE_TEST_EVIDENCE").is_some() {
            let _ = self.0.take().unwrap().keep();
        }
    }
}

pub(crate) struct Fake {
    source: InstalledSource,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    root: std::path::PathBuf,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
        modseqs::persist(&self.root);
    }
}

fn external(repo: &Accounts, id: &AccountId, root: &Path) -> (EncryptedStore, AccountState) {
    let store = repo.fixture_sieve_store(id, root).unwrap();
    let state = store.load_state().unwrap();
    (store, state)
}

fn record(uid: u32) -> MessageRecord {
    MessageRecord {
        delivery: Default::default(),
        uid,
        size: 1200,
        internal_date: "25-Oct-2026 12:00:00 +0000".into(),
        flags_observed: true,
        flags: ["\\Seen".into(), "$unrelated".into()].into(),
        gmail_message_id: None,
        gmail_thread_id: None,
        gmail_labels: BTreeSet::new(),
        headers: parse_headers(HEADER, Direction::Received).unwrap(),
    }
}

fn folder(validity: u32, uids: &[u32]) -> FolderCensus {
    FolderCensus {
        uidvalidity: validity,
        state: ScanState::Complete,
        live_uids: uids.iter().copied().collect(),
        messages: uids.iter().map(|u| (*u, record(*u))).collect(),
        highestmodseq: None,
    }
}

impl Fake {
    pub(crate) fn start(root: &Path, account: &Account) -> Self {
        let repo = persistent_test_accounts(root);
        let (store, mut state) = external(&repo, &account.id, root);
        let initialized = state.synthetic_uid_next.contains_key("Filed/Receipts")
            || state.synthetic_uid_next.contains_key("Personal/Studio")
            || state
                .synthetic_folders
                .as_ref()
                .unwrap()
                .epoch
                .starts_with("move-");
        let p = state.synthetic_folders.as_mut().unwrap();
        let listener = TcpListener::bind(p.address).unwrap();
        listener.set_nonblocking(true).unwrap();
        p.address = listener.local_addr().unwrap();
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![p.root.clone().into()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(p.key.clone())),
                )
                .unwrap(),
        );
        if !initialized {
            p.epoch = format!("move-{}", p.epoch);
            p.inventory.capabilities = ["IMAP4REV1", "NAMESPACE", "SPECIAL-USE", "MOVE", "UIDPLUS"]
                .map(String::from)
                .into();
            p.inventory.mailboxes = ["Filed/Receipts", "INBOX"]
                .into_iter()
                .map(|name| Mailbox {
                    name: ExactName::new(name).unwrap(),
                    delimiter: Some("/".into()),
                    attributes: BTreeSet::new(),
                })
                .collect();
            state.census.folders = [
                ("INBOX".into(), folder(101, &[4, 8, 19, 99])),
                ("Filed/Receipts".into(), folder(202, &[40])),
            ]
            .into();
            state
                .census
                .folders
                .get_mut("INBOX")
                .unwrap()
                .messages
                .get_mut(&99)
                .unwrap()
                .flags
                .insert("\\Deleted".into());
            state.synthetic_uid_next.insert("Filed/Receipts".into(), 60);
            state.synthetic_uid_next.insert("INBOX".into(), 104);
            if let Ok(large) = std::fs::read_to_string(root.join("move-large")) {
                let eligible: u32 = large
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(501);
                let inbox = state.census.folders.get_mut("INBOX").unwrap();
                let last = 200 + 2 * (eligible - 4);
                for uid in (200..=last).step_by(2) {
                    inbox.live_uids.insert(uid);
                    inbox.messages.insert(uid, record(uid));
                }
                state
                    .synthetic_uid_next
                    .insert("INBOX".into(), (last + 2).max(2000));
            }
        }
        if root.join("move-workflow").exists() {
            let destination = "Personal/Studio";
            if let Some(folder) = state.census.folders.remove("Filed/Receipts") {
                state.census.folders.insert(destination.into(), folder);
                let next = state.synthetic_uid_next.remove("Filed/Receipts").unwrap();
                state.synthetic_uid_next.insert(destination.into(), next);
                state
                    .synthetic_folders
                    .as_mut()
                    .unwrap()
                    .inventory
                    .mailboxes = state
                    .census
                    .folders
                    .keys()
                    .map(|n| Mailbox {
                        name: ExactName::new(n).unwrap(),
                        delimiter: Some("/".into()),
                        attributes: BTreeSet::new(),
                    })
                    .collect();
            }
        }
        if root.join("copy-workflow").exists() {
            let capabilities = &mut state
                .synthetic_folders
                .as_mut()
                .unwrap()
                .inventory
                .capabilities;
            capabilities.remove("MOVE");
            capabilities.insert("CONDSTORE".into());
        }
        if root.join("move-condstore").exists() {
            state
                .synthetic_folders
                .as_mut()
                .unwrap()
                .inventory
                .capabilities
                .insert("CONDSTORE".into());
        }
        if let Some(spec) = scale_spec(root) {
            if !state.census.folders.contains_key("Scale/Sent") {
                crate::census::scale::inflate(&mut state.census, spec);
                let p = state.synthetic_folders.as_mut().unwrap();
                for name in state.census.folders.keys() {
                    if p.inventory
                        .mailboxes
                        .iter()
                        .all(|m| m.name.decoded != *name)
                    {
                        p.inventory.mailboxes.push(Mailbox {
                            name: ExactName::new(name).unwrap(),
                            delimiter: Some("/".into()),
                            attributes: if name == "Scale/Sent" {
                                ["\\Sent".to_owned()].into()
                            } else {
                                BTreeSet::new()
                            },
                        });
                    }
                }
                p.inventory
                    .mailboxes
                    .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
                for (name, folder) in &state.census.folders {
                    let next = folder.live_uids.last().map_or(1, |u| u + 1);
                    state.synthetic_uid_next.entry(name.clone()).or_insert(next);
                }
            }
        }
        let p = state.synthetic_folders.as_ref().unwrap();
        let source = InstalledSource {
            account: account.id.clone(),
            login: account.login.clone(),
            address: p.address,
            tls_name: "localhost".into(),
            root: p.root.clone(),
            epoch: p.epoch.clone(),
            authority: "test-bound-authority".into(),
            command_timeout: Duration::from_millis(800),
            operation_timeout: Duration::from_secs(10),
        };
        store.save_state(&mut state).unwrap();
        drop(store);
        modseqs::restore(root, &state);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let id = account.id.clone();
        let path = root.to_owned();
        let worker = crate::acquisition::fixture_peer::serve_sessions(
            listener,
            stopping,
            move |tcp, lock| serve(tcp, config.clone(), &repo, &id, &path, lock),
        );
        Self {
            source,
            stop,
            worker: Some(worker),
            root: root.to_owned(),
        }
    }

    fn connect(&self, account: &Account, cancel: Cancellation) -> Result<Connection, Error> {
        Connection::connect(
            &self.source.permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )?,
            &SecretValue::new(b"invented".to_vec()),
            cancel,
        )
    }
}

fn unquote(s: &str) -> std::io::Result<String> {
    let mut chars = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(std::io::ErrorKind::InvalidData)?
        .chars();
    let mut text = String::new();
    while let Some(c) = chars.next() {
        text.push(if c == '\\' {
            chars.next().ok_or(std::io::ErrorKind::InvalidData)?
        } else {
            c
        });
    }
    ExactName::from_wire(&text)
        .map(|name| name.decoded)
        .map_err(|_| std::io::ErrorKind::InvalidData.into())
}

fn serve(
    tcp: Peer,
    config: Arc<ServerConfig>,
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
    lock: &std::sync::Mutex<()>,
) -> std::io::Result<()> {
    tcp.tcp.set_nodelay(true)?;
    let mut io = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        tcp,
    ));
    let mode = std::fs::read_to_string(root.join("move-mode")).unwrap_or_default();
    let greeting = match mode.as_str() {
        "malformed_greeting" => b"* PREAUTH unsupported\r\n".as_slice(),
        "referral" => b"* OK [REFERRAL imaps://unusable.example.test/] unsupported\r\n".as_slice(),
        _ => b"* OK synthetic MOVE fixture\r\n".as_slice(),
    };
    if mode == "slow_greeting" {
        let pace = SLOW_GREETING / u32::try_from(greeting.len()).unwrap();
        for byte in greeting {
            io.get_mut().write_all(&[*byte])?;
            io.get_mut().flush()?;
            thread::sleep(pace);
        }
    } else {
        io.get_mut().write_all(greeting)?;
    }
    io.get_mut().flush()?;
    let mut selected = None;
    let mut authenticated = false;
    let scaled = scale_spec(root).is_some();
    let mut cache = crate::acquisition::fixture_peer::Cached::default();
    loop {
        let mut line = String::new();
        if io.by_ref().take(16385).read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.len() > 16384 || !line.ends_with("\r\n") {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let (tag, command) = line
            .trim_end_matches("\r\n")
            .split_once(' ')
            .ok_or(std::io::ErrorKind::InvalidData)?;
        let guard = crate::acquisition::fixture_peer::command(lock);
        if root.join("move-command-log").exists() {
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("move-commands.log"))?;
            let logged = if command.starts_with("LOGIN ") {
                "LOGIN [REDACTED]"
            } else {
                command
            };
            writeln!(log, "{logged}")?;
        }
        let (store, mut state) = if scaled {
            let store = repo.fixture_sieve_store(id, root).unwrap();
            let state = cache.load(&store);
            (store, state)
        } else {
            external(repo, id, root)
        };
        let mode = std::fs::read_to_string(root.join("move-mode")).unwrap_or_default();
        let condstore = state
            .synthetic_folders
            .as_ref()
            .unwrap()
            .inventory
            .capabilities
            .contains("CONDSTORE");
        let mut modseq_changed = condstore && modseqs::sync(root, &store, &mut state, false);
        let mut reply = String::new();
        if command == "CAPABILITY" {
            reply = format!(
                "* CAPABILITY {}\r\n",
                state
                    .synthetic_folders
                    .as_ref()
                    .unwrap()
                    .inventory
                    .capabilities
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        } else if command.starts_with("LOGIN ") {
            assert!(!authenticated);
            authenticated = true;
            state
                .synthetic_folders
                .as_mut()
                .unwrap()
                .commands
                .push("LOGIN".into());
            if mode == "authentication_failed" {
                reply = format!("{tag} NO refused\r\n");
            }
        } else {
            assert!(authenticated);
            match command {
                "NAMESPACE" => reply = "* NAMESPACE ((\"\" \"/\")) NIL NIL\r\n".into(),
                "LIST \"\" \"*\"" | "LIST \"\" \"*\" RETURN (SPECIAL-USE)" => {
                    for m in &state
                        .synthetic_folders
                        .as_ref()
                        .unwrap()
                        .inventory
                        .mailboxes
                    {
                        reply.push_str(&format!(
                            "* LIST ({}) \"/\" {}\r\n",
                            m.attributes.iter().cloned().collect::<Vec<_>>().join(" "),
                            String::from_utf8(quoted(m.name.wire.as_bytes()).unwrap()).unwrap()
                        ));
                    }
                }
                _ if command.starts_with("SELECT ") || command.starts_with("EXAMINE ") => {
                    let read_only = command.starts_with("EXAMINE ");
                    let argument = &command[if read_only { 8 } else { 7 }..];
                    let (argument, enable) = match argument.strip_suffix(" (CONDSTORE)") {
                        Some(name) => (name, true),
                        None => (argument, false),
                    };
                    let name = unquote(argument)?;
                    let folder = &state.census.folders[&name];
                    reply = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] instance\r\n{}{tag} OK [READ-WRITE] selected\r\n",
                        folder.live_uids.len(),
                        folder.uidvalidity,
                        if condstore {
                            format!(
                                "* OK [HIGHESTMODSEQ {}] modseq\r\n",
                                modseqs::highest(folder)
                            )
                        } else {
                            String::new()
                        }
                    );
                    if enable && !condstore {
                        reply = format!("{tag} BAD CONDSTORE is not advertised\r\n");
                    }
                    if read_only || mode == "read_only" {
                        reply = reply.replace("READ-WRITE", "READ-ONLY");
                    }
                    selected = Some(name);
                }
                "UID SEARCH ALL" => {
                    let f = &state.census.folders[selected.as_ref().unwrap()];
                    reply = format!(
                        "* SEARCH {}\r\n",
                        f.live_uids
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
                _ if command.starts_with("STATUS ") => {
                    let (name, _) = command[7..].rsplit_once(" (MESSAGES)").unwrap();
                    let name = unquote(name)?;
                    reply = format!(
                        "* STATUS {} (MESSAGES {})\r\n",
                        String::from_utf8(
                            quoted(ExactName::new(&name).unwrap().wire.as_bytes()).unwrap()
                        )
                        .unwrap(),
                        state.census.folders[&name].live_uids.len()
                    );
                }
                "LOGOUT" => {
                    io.get_mut()
                        .write_all(format!("* BYE\r\n{tag} OK logout\r\n").as_bytes())?;
                    io.get_mut().flush()?;
                    return Ok(());
                }
                _ if command.starts_with("UID FETCH ") => {
                    if mode == "oversized_literal" {
                        // Larger than every response limit of both client
                        // sessions, sent rather than announced, and never
                        // finished: the fake holds the connection until the
                        // client hangs up. A fake that hung up after the size
                        // would let any client pass on end of stream, and a
                        // client that read the whole literal would wait out
                        // its idle deadline, so only a refusal at the
                        // client's own limit fails the preview promptly.
                        let size = 64 * 1024 * 1024;
                        io.get_mut().write_all(
                            format!("* 1 FETCH (UID 4 BODY[HEADER] {{{size}}}\r\n").as_bytes(),
                        )?;
                        let chunk = [b'x'; 64 * 1024];
                        for _ in 0..size / chunk.len() {
                            if io
                                .get_mut()
                                .write_all(&chunk)
                                .and_then(|()| io.get_mut().flush())
                                .is_err()
                            {
                                break;
                            }
                        }
                        Peer::until_closed(&mut io);
                        return Ok(());
                    }
                    let set = command[10..].split_once(' ').unwrap().0;
                    let f = &state.census.folders[selected.as_ref().unwrap()];
                    let since = command
                        .rsplit_once(" (CHANGEDSINCE ")
                        .and_then(|(_, n)| n.strip_suffix(')'))
                        .map(|n| n.parse::<u64>().unwrap());
                    let Some(requested) =
                        crate::acquisition::fixture_peer::uid_set(set, &f.live_uids)
                    else {
                        panic!("Malformed UID set reached the synthetic MOVE fixture");
                    };
                    let refused = since.is_some() && !condstore;
                    if refused {
                        reply = format!("{tag} BAD CONDSTORE is not advertised\r\n");
                    }
                    if mode == "unsolicited" && command.contains("(MESSAGE-ID)") {
                        let requested: BTreeSet<u32> = requested.iter().copied().collect();
                        if let Some((seq, uid)) = f
                            .live_uids
                            .iter()
                            .enumerate()
                            .find(|(_, uid)| !requested.contains(uid))
                        {
                            let flags = f.messages[uid]
                                .flags
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" ");
                            reply.push_str(&format!("* {} FETCH (FLAGS ({flags}))\r\n* {} FETCH (UID {uid} FLAGS ({flags}))\r\n* {} EXISTS\r\n", seq + 1, seq + 1, f.live_uids.len()));
                        }
                    }
                    for uid in requested.into_iter().filter(|_| !refused) {
                        let requested_modseq = root.join("copy-workflow").exists()
                            && condstore
                            && command
                                .split_ascii_whitespace()
                                .any(|field| field.trim_matches(['(', ')']) == "MODSEQ");
                        let modseq = (since.is_some() || requested_modseq)
                            .then(|| modseqs::of(root, selected.as_ref().unwrap(), uid));
                        if since
                            .zip(modseq)
                            .is_some_and(|(since, modseq)| modseq <= since)
                        {
                            continue;
                        }
                        if let Some(m) = f.messages.get(&uid) {
                            let body = if let Some((_, fields)) = command.split_once("BODY.PEEK[") {
                                let fields = fields.split_once(']').unwrap().0;
                                let mut header = header_for(root, selected.as_deref().unwrap(), m);
                                if scaled {
                                    header = crate::census::scale::header_fields(&header, fields);
                                }
                                format!(" BODY[{fields}] {{{}}}\r\n{header}", header.len())
                            } else {
                                String::new()
                            };
                            let modseq =
                                modseq.map(|m| format!(" MODSEQ ({m})")).unwrap_or_default();
                            reply.push_str(&format!("* 1 FETCH (UID {uid} FLAGS ({}) INTERNALDATE \"{}\" RFC822.SIZE {}{body}{modseq})\r\n", m.flags.iter().cloned().collect::<Vec<_>>().join(" "), m.internal_date, m.size));
                        }
                    }
                }
                _ if copy_fixture::admitted(root, command) => {
                    reply = copy_fixture::handle(
                        root,
                        &store,
                        &mut state,
                        selected.as_deref(),
                        tag,
                        command,
                    )?;
                }
                _ if command.starts_with("UID MOVE ") => {
                    let (set, target) = command[9..].split_once(' ').unwrap();
                    let target = unquote(target)?;
                    let source = selected.as_ref().unwrap();
                    let uids = set
                        .split(',')
                        .map(|u| u.parse::<u32>().unwrap())
                        .collect::<Vec<_>>();
                    assert!(uids.len() <= 500);
                    assert!(!uids.contains(&99));
                    if source == "INBOX" && !root.join("move-workflow").exists() {
                        assert_eq!(set, "4,8,19");
                        assert_eq!(target, "Filed/Receipts");
                    }
                    let race = std::fs::read_to_string(root.join("move-race")).unwrap_or_default();
                    let first = uids[0];
                    let origin = state.census.folders.get_mut(source).unwrap();
                    match race.as_str() {
                        "source_flag" => {
                            let m = origin.messages.get_mut(&first).unwrap();
                            m.flags.insert("$race-client".into());
                        }
                        "deleted_flag" => {
                            let m = origin.messages.get_mut(&first).unwrap();
                            m.flags.insert("\\Deleted".into());
                        }
                        "source_expunged" => {
                            origin.messages.remove(&first);
                            origin.live_uids.remove(&first);
                        }
                        _ => {}
                    }
                    let present: Vec<u32> = uids
                        .iter()
                        .copied()
                        .filter(|u| origin.live_uids.contains(u))
                        .collect();
                    let mut mapped = vec![];
                    let effected = if mode == "partial" {
                        &present[..1]
                    } else {
                        &present[..]
                    };
                    for uid in effected {
                        let mut m = state
                            .census
                            .folders
                            .get_mut(source)
                            .unwrap()
                            .messages
                            .remove(uid)
                            .unwrap();
                        state
                            .census
                            .folders
                            .get_mut(source)
                            .unwrap()
                            .live_uids
                            .remove(uid);
                        let next = state.synthetic_uid_next.get_mut(&target).unwrap();
                        m.uid = *next;
                        *next += 4;
                        mapped.push(m.uid);
                        let dest = state.census.folders.get_mut(&target).unwrap();
                        dest.live_uids.insert(m.uid);
                        dest.messages.insert(m.uid, m);
                    }
                    if race == "destination_flag" {
                        let dest = state.census.folders.get_mut(&target).unwrap();
                        let m = dest.messages.get_mut(&mapped[0]).unwrap();
                        m.flags.insert("$race-client".into());
                    }
                    if condstore {
                        modseqs::sync(root, &store, &mut state, true);
                    }
                    if scaled {
                        cache.save(&store, &mut state);
                    } else {
                        store.save_state(&mut state).unwrap();
                    }
                    modseqs::saved(root, &store);
                    modseq_changed = false;
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("move-wire-observations"))?;
                    log.write_all(b"UID MOVE\n")?;
                    let mut exact = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("move-wire-exact.jsonl"))?;
                    exact.write_all(&serde_json::to_vec(&serde_json::json!({"command":command,"source":source,"mapped":mapped,"validity":state.census.folders[&target].uidvalidity})).unwrap())?;
                    exact.write_all(b"\n")?;
                    exact.sync_all()?;
                    log.sync_all()?;
                    std::fs::write(root.join("move-effect-reached"), b"effect persisted")?;
                    crate::acquisition::backlog::boundary("move_provider_effect");
                    if mode == "lost" {
                        return Ok(());
                    }
                    if mode == "slow" {
                        drop(store);
                        drop(guard);
                        Peer::until_closed(&mut io);
                        return Ok(());
                    }
                    let moved = effected
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(",");
                    let code = format!(
                        "[COPYUID {} {moved} {}]",
                        if mode == "wrong_validity" {
                            203
                        } else {
                            state.census.folders[&target].uidvalidity
                        },
                        mapped
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    let expunged = (1..=effected.len())
                        .rev()
                        .map(|n| format!("* {n} EXPUNGE\r\n"))
                        .collect::<String>();
                    let flood = (0..2048)
                        .map(|n| format!("* {} EXISTS\r\n", n + 1))
                        .collect::<String>();
                    if mode == "expunge_rows" {
                        reply = format!("* OK {code} mapping\r\n{expunged}{tag} OK moved\r\n");
                    } else if mode == "flood_after_mapping" {
                        reply =
                            format!("* OK {code} mapping\r\n{expunged}{flood}{tag} OK moved\r\n");
                    } else if mode == "flood_before_mapping" {
                        reply = format!("{flood}{tag} OK {code} moved\r\n");
                    } else if mode == "missing" {
                        reply = format!("{tag} OK moved without mapping\r\n");
                    } else if mode == "untagged" {
                        reply = format!(
                            "* OK {code} mapping\r\n* 1 EXPUNGE\r\n* 4 EXISTS\r\n{tag} NO [OVERQUOTA] partial completion\r\n"
                        );
                    } else if mode == "partial" {
                        reply = format!("* OK {code} subset\r\n{tag} NO [OVERQUOTA] partial\r\n");
                    } else if mode == "conflicting" {
                        reply = format!(
                            "* OK {code} first\r\n{tag} OK [COPYUID {} {} {}] different mapping\r\n",
                            state.census.folders[&target].uidvalidity,
                            uids[0],
                            mapped[0] + 1
                        );
                    } else if mode == "malformed" {
                        reply = format!("{tag} OK [COPYUID 202 4,* 60,64] malformed\r\n");
                    } else if mode == "uidnotsticky" {
                        reply = format!(
                            "* OK {code} first\r\n{tag} OK [UIDNOTSTICKY] unstable identity\r\n"
                        );
                    } else {
                        reply = format!("{tag} OK {code} moved\r\n");
                    }
                }
                _ => panic!("Forbidden command reached the synthetic MOVE fixture"),
            }
        }
        if !reply.contains(&format!("{tag} ")) {
            reply.push_str(&format!("{tag} OK completed\r\n"));
        }
        if !scaled {
            store.save_state(&mut state).unwrap();
            modseqs::saved(root, &store);
        } else if command.starts_with("LOGIN ") || modseq_changed {
            cache.save(&store, &mut state);
            modseqs::saved(root, &store);
        }
        if root.join("copy-workflow").exists() {
            modseqs::persist(root);
        }
        drop(store);
        drop(guard);
        for chunk in reply
            .as_bytes()
            .chunks(if root.join("move-large").exists() {
                256
            } else {
                3
            })
        {
            io.get_mut().write_all(chunk)?;
            io.get_mut().flush()?;
        }
    }
}

/// Mod-sequences of a fake that advertises CONDSTORE (RFC 7162). Each
/// folder's HIGHESTMODSEQ persists with the provider state. Per-message
/// values live in memory for the installation and follow the state's
/// content: a new message or changed flags get a new mod-sequence, whoever
/// made the change. A fake that stops cleanly writes them to
/// `move-modseq.json`, and the next start takes back each folder whose
/// HIGHESTMODSEQ has not moved since. Otherwise every message counts as
/// changed at a new HIGHESTMODSEQ, which only makes clients read more.
mod modseqs {
    use super::*;
    use crate::acquisition::fixture_peer::{FileId, file_id};
    use std::{path::PathBuf, sync::Mutex};

    /// One folder's mod-sequences: per message, its own and the flags it
    /// had then.
    #[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
    struct Known {
        validity: u32,
        highest: u64,
        messages: BTreeMap<u32, (u64, BTreeSet<String>)>,
    }
    type Folders = BTreeMap<String, Known>;
    static STATE: Mutex<BTreeMap<PathBuf, (Option<FileId>, Folders)>> = Mutex::new(BTreeMap::new());

    pub(super) fn highest(folder: &FolderCensus) -> u64 {
        folder.highestmodseq.unwrap_or(1)
    }
    /// Brings the mod-sequences up to the state's content and returns true
    /// when a HIGHESTMODSEQ moved, so the state needs saving. Unless
    /// `force`d, a state whose file is unchanged since the last sync is
    /// taken as synced.
    pub(super) fn sync(
        root: &Path,
        store: &EncryptedStore,
        state: &mut AccountState,
        force: bool,
    ) -> bool {
        let mut all = STATE.lock().unwrap_or_else(|p| p.into_inner());
        let (synced, folders) = all.entry(root.to_owned()).or_default();
        let current = file_id(&store.path());
        if !force && synced.is_some() && *synced == current {
            return false;
        }
        let mut moved = vec![];
        for (name, folder) in &state.census.folders {
            let before = folder.highestmodseq.unwrap_or(1);
            let known = folders.entry(name.clone()).or_default();
            if known.validity != folder.uidvalidity || known.highest > before {
                *known = Known {
                    validity: folder.uidvalidity,
                    ..Known::default()
                };
            }
            let mut highest = before;
            let fresh = known.messages.is_empty();
            if fresh && !folder.messages.is_empty() {
                highest += 1;
            }
            for (uid, m) in &folder.messages {
                if known
                    .messages
                    .get(uid)
                    .is_none_or(|(_, flags)| *flags != m.flags)
                {
                    if !fresh {
                        highest += 1;
                    }
                    known.messages.insert(*uid, (highest, m.flags.clone()));
                }
            }
            let listed = known.messages.len();
            known
                .messages
                .retain(|uid, _| folder.messages.contains_key(uid));
            if known.messages.len() < listed {
                highest += 1;
            }
            known.highest = highest;
            if highest != before || folder.highestmodseq.is_none() {
                moved.push((name.clone(), highest));
            }
        }
        let changed = !moved.is_empty();
        for (name, highest) in moved {
            state
                .census
                .folders
                .get_mut(&name)
                .expect("synced folder")
                .highestmodseq = Some(highest);
        }
        *synced = if changed { None } else { current };
        changed
    }
    /// Records that the state on disk is what the last sync saw.
    pub(super) fn saved(root: &Path, store: &EncryptedStore) {
        let mut all = STATE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((synced, _)) = all.get_mut(root) {
            *synced = file_id(&store.path());
        }
    }
    pub(super) fn persist(root: &Path) {
        let all = STATE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, folders)) = all.get(root) {
            let _ = std::fs::write(
                root.join("move-modseq.json"),
                serde_json::to_vec(folders).unwrap(),
            );
        }
    }
    /// Takes back the saved mod-sequences of each folder whose
    /// UIDVALIDITY and HIGHESTMODSEQ are what they were when saved.
    pub(super) fn restore(root: &Path, state: &AccountState) {
        let Some(saved) = std::fs::read(root.join("move-modseq.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Folders>(&b).ok())
        else {
            return;
        };
        let mut all = STATE.lock().unwrap_or_else(|p| p.into_inner());
        let (synced, folders) = all.entry(root.to_owned()).or_default();
        *synced = None;
        for (name, known) in saved {
            if state.census.folders.get(&name).is_some_and(|f| {
                f.uidvalidity == known.validity && f.highestmodseq == Some(known.highest)
            }) {
                folders.insert(name, known);
            }
        }
    }
    pub(super) fn of(root: &Path, folder: &str, uid: u32) -> u64 {
        let all = STATE.lock().unwrap_or_else(|p| p.into_inner());
        all.get(root)
            .and_then(|(_, folders)| folders.get(folder))
            .and_then(|known| known.messages.get(&uid))
            .map_or(u64::MAX, |(modseq, _)| *modseq)
    }
}

/// The personal-scale mailbox `move-scale` asks for: that many invented
/// messages in the scale folders, besides the workflow's own.
fn scale_spec(root: &Path) -> Option<crate::census::scale::Spec> {
    let messages = std::fs::read_to_string(root.join("move-scale")).ok()?;
    Some(crate::census::scale::Spec {
        messages: messages.trim().parse().ok()?,
        ..crate::census::scale::PERSONAL
    })
}

/// Raw header the fake serves for a message: the shared invented header unless
/// a recipient profile assigns one by message size, which travels with a MOVE.
fn header_for(root: &Path, folder: &str, m: &MessageRecord) -> String {
    if let Some(raw) =
        scale_spec(root).and_then(|spec| crate::census::scale::raw_header(spec, folder, m.uid))
    {
        return raw;
    }
    std::fs::read(root.join("move-headers.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, String>>(&b).ok())
        .and_then(|mut h| h.remove(&m.size.to_string()))
        .unwrap_or_else(|| String::from_utf8(HEADER.to_vec()).unwrap())
}

/// Assigns the recipient profile to INBOX messages by UID, like mail delivered
/// with those headers, and records the matching parsed census headers.
pub(crate) fn recipient_profile(
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
    headers: &[(u32, &str)],
) {
    let (store, mut state) = external(repo, id, root);
    let mut raw = BTreeMap::new();
    let inbox = state.census.folders.get_mut("INBOX").unwrap();
    for (uid, header) in headers {
        let m = inbox.messages.get_mut(uid).unwrap();
        m.size = 2000 + u64::from(*uid);
        m.headers = parse_headers(header.as_bytes(), Direction::Received).unwrap();
        raw.insert(m.size.to_string(), (*header).to_owned());
    }
    store.save_state(&mut state).unwrap();
    std::fs::write(
        root.join("move-headers.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
}

fn setup(root: &Path) -> (Accounts, Account, Fake) {
    let (repo, account, create) = crate::acquisition::folder_provisioning::tests::install(root);
    drop(create);
    let fake = Fake::start(root, &account);
    (repo, account, fake)
}

fn identity(name: &str, validity: u32) -> MailboxIdentity {
    MailboxIdentity {
        name: ExactName::new(name).unwrap(),
        uidvalidity: UidValidity::new(validity).unwrap(),
    }
}

fn intent(connection: &mut Connection) -> MoveIntent {
    let source = identity("INBOX", 101);
    let messages = connection
        .readback(&source, &[4, 8, 19].map(|u| Uid::new(u).unwrap()).into())
        .unwrap()
        .messages;
    MoveIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        source,
        identity("Filed/Receipts", 202),
        messages,
    )
    .unwrap()
}

fn observe(connection: &mut Connection, evidence: &mut MoveEvidence) {
    let source = connection
        .readback(
            evidence.command().source(),
            &evidence.command().messages().keys().copied().collect(),
        )
        .unwrap();
    let mapped = evidence
        .mapping()
        .map(|m| m.pairs().values().copied().collect())
        .unwrap_or_default();
    let destination = connection
        .readback(evidence.command().destination(), &mapped)
        .unwrap();
    evidence.observe(source, destination).unwrap();
}

#[test]
fn actual_verified_tls_sparse_move_and_reverse_preserve_independent_messages_and_current_flags() {
    for mode in ["tagged", "untagged"] {
        let root = EvidenceRoot::new();
        let (repo, account, fake) = setup(root.path());
        std::fs::write(root.path().join("move-mode"), mode).unwrap();
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        let mut evidence = MoveEvidence::intent(intent(&mut connection));
        let permit = fake
            .source
            .permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )
            .unwrap();
        let mut persisted_mapping = false;
        connection
            .move_messages(&permit, &mut evidence, |e| {
                persisted_mapping |= e.mapping().is_some() && e.completion().is_none();
                Ok(())
            })
            .unwrap();
        assert!(persisted_mapping);
        observe(&mut connection, &mut evidence);
        assert!(
            evidence
                .resolutions()
                .values()
                .all(|r| matches!(r, Resolution::Verified { .. }))
        );
        drop(connection);
        let (store, mut independent) = external(&repo, &account.id, root.path());
        assert_eq!(independent.census.folders["INBOX"].live_uids, [99].into());
        assert!(
            independent.census.folders["INBOX"].messages[&99]
                .flags
                .contains("\\Deleted")
        );
        assert_eq!(
            independent.census.folders["Filed/Receipts"].live_uids,
            [40, 60, 64, 68].into()
        );
        independent
            .census
            .folders
            .get_mut("Filed/Receipts")
            .unwrap()
            .messages
            .get_mut(&60)
            .unwrap()
            .flags
            .insert("$later-client".into());
        store.save_state(&mut independent).unwrap();
        drop(store);
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        let source = identity("Filed/Receipts", 202);
        let current = connection
            .readback(&source, &[60, 64, 68].map(|u| Uid::new(u).unwrap()).into())
            .unwrap();
        let mut reverse = MoveEvidence::intent(
            MoveIntent::new(
                Purpose::ConditionalUndo,
                source,
                identity("INBOX", 101),
                current.messages,
            )
            .unwrap(),
        );
        connection
            .move_messages(&permit, &mut reverse, |_| Ok(()))
            .unwrap();
        observe(&mut connection, &mut reverse);
        assert!(
            reverse
                .resolutions()
                .values()
                .all(|r| matches!(r, Resolution::Verified { .. }))
        );
        drop(connection);
        let (_, independent) = external(&repo, &account.id, root.path());
        assert_eq!(
            independent.census.folders["INBOX"].live_uids,
            [99, 104, 108, 112].into()
        );
        assert!(
            independent.census.folders["INBOX"].messages[&104]
                .flags
                .contains("$later-client")
        );
        assert_eq!(
            independent.census.folders["Filed/Receipts"].live_uids,
            [40].into()
        );
        let commands = &independent.synthetic_folders.unwrap().commands;
        assert_eq!(
            std::fs::read_to_string(root.path().join("move-wire-observations")).unwrap(),
            "UID MOVE\nUID MOVE\n"
        );
        assert!(
            !commands
                .iter()
                .any(|c| ["COPY", "STORE", "EXPUNGE", "CLOSE", "CREATE"].contains(&c.as_str()))
        );
    }
}

#[test]
fn lost_mapping_never_becomes_success_after_reconnect_and_repeated_observation() {
    for mode in ["missing", "lost", "slow"] {
        let root = EvidenceRoot::new();
        let (_, account, fake) = setup(root.path());
        std::fs::write(root.path().join("move-mode"), mode).unwrap();
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        let mut evidence = MoveEvidence::intent(intent(&mut connection));
        let permit = fake
            .source
            .permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )
            .unwrap();
        let result = connection.move_messages(&permit, &mut evidence, |_| Ok(()));
        assert_eq!(result.is_ok(), mode == "missing");
        assert!(evidence.mapping().is_none());
        drop(connection);
        drop(fake);
        let fake = Fake::start(root.path(), &account);
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        observe(&mut connection, &mut evidence);
        assert!(
            evidence
                .resolutions()
                .values()
                .all(|r| *r == Resolution::Uncertain)
        );
        let before = serde_json::to_vec(&evidence).unwrap();
        observe(&mut connection, &mut evidence);
        assert_eq!(before, serde_json::to_vec(&evidence).unwrap());
        let permit = fake
            .source
            .permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )
            .unwrap();
        assert_eq!(
            connection.move_messages(&permit, &mut evidence, |_| Ok(())),
            Err(Error::Unsupported)
        );
    }
}

#[test]
fn wrong_source_binding_and_unverified_tls_never_authenticate() {
    let root = EvidenceRoot::new();
    let (repo, account, mut fake) = setup(root.path());
    assert!(
        fake.source
            .permit(
                &account.id,
                Provider::Gmail,
                &account.login,
                "test-bound-authority"
            )
            .is_err()
    );
    assert!(
        fake.source
            .permit(&account.id, account.provider, &account.login, "different")
            .is_err()
    );
    fake.source.tls_name = "wrong.example.test".into();
    assert!(matches!(
        fake.connect(&account, Cancellation::default()),
        Err(Error::Tls)
    ));
    let (_, independent) = external(&repo, &account.id, root.path());
    assert!(
        !independent
            .synthetic_folders
            .unwrap()
            .commands
            .iter()
            .any(|c| c == "LOGIN")
    );
}

#[test]
fn partial_no_resolves_only_mapped_entries_and_invalid_mapping_stays_conflicted() {
    for mode in [
        "partial",
        "conflicting",
        "malformed",
        "wrong_validity",
        "uidnotsticky",
    ] {
        let root = EvidenceRoot::new();
        let (_, account, fake) = setup(root.path());
        std::fs::write(root.path().join("move-mode"), mode).unwrap();
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        let mut evidence = MoveEvidence::intent(intent(&mut connection));
        let permit = fake
            .source
            .permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )
            .unwrap();
        let result = connection.move_messages(&permit, &mut evidence, |_| Ok(()));
        assert_eq!(result.is_ok(), mode == "partial");
        drop(connection);
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        observe(&mut connection, &mut evidence);
        if mode == "partial" {
            assert!(matches!(
                evidence.resolutions()[&Uid::new(4).unwrap()],
                Resolution::Verified { .. }
            ));
            assert_eq!(
                evidence.resolutions()[&Uid::new(8).unwrap()],
                Resolution::Uncertain
            );
            assert_eq!(
                evidence.resolutions()[&Uid::new(19).unwrap()],
                Resolution::Uncertain
            );
        } else {
            assert!(
                evidence
                    .resolutions()
                    .values()
                    .all(|r| *r == Resolution::Conflict)
            );
        }
        assert_eq!(
            connection.move_messages(&permit, &mut evidence, |_| Ok(())),
            Err(Error::Unsupported)
        );
    }
}

#[test]
fn cancelled_response_wait_retains_effect_and_never_starts_another_mutation() {
    let root = EvidenceRoot::new();
    let (repo, account, fake) = setup(root.path());
    std::fs::write(root.path().join("move-mode"), "slow").unwrap();
    let cancel = Cancellation::default();
    let mut connection = fake.connect(&account, cancel.clone()).unwrap();
    let mut evidence = MoveEvidence::intent(intent(&mut connection));
    let permit = fake
        .source
        .permit(
            &account.id,
            account.provider,
            &account.login,
            "test-bound-authority",
        )
        .unwrap();
    let signal = root.path().join("move-effect-reached");
    let stopping = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(5);
        while !signal.exists() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
        cancel.cancel();
    });
    assert_eq!(
        connection.move_messages(&permit, &mut evidence, |_| Ok(())),
        Err(Error::Cancelled)
    );
    stopping.join().unwrap();
    assert_eq!(evidence.completion(), Some(&Completion::Lost));
    drop(connection);
    drop(fake);
    let (_, independent) = external(&repo, &account.id, root.path());
    assert_eq!(independent.census.folders["INBOX"].live_uids, [99].into());
    assert_eq!(
        std::fs::read_to_string(root.path().join("move-wire-observations")).unwrap(),
        "UID MOVE\n"
    );
}

#[test]
fn failure_to_save_attempt_prevents_move_and_changed_metadata_blocks_admission() {
    let root = EvidenceRoot::new();
    let (repo, account, fake) = setup(root.path());
    let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
    let mut evidence = MoveEvidence::intent(intent(&mut connection));
    let permit = fake
        .source
        .permit(
            &account.id,
            account.provider,
            &account.login,
            "test-bound-authority",
        )
        .unwrap();
    assert_eq!(
        connection.move_messages(&permit, &mut evidence, |_| Err(Error::Limit)),
        Err(Error::Limit)
    );
    assert!(!root.path().join("move-wire-observations").exists());
    assert!(evidence.attempted());
    drop(connection);
    let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
    let mut evidence = MoveEvidence::intent(intent(&mut connection));
    let (store, mut independent) = external(&repo, &account.id, root.path());
    independent
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&4)
        .unwrap()
        .flags
        .insert("\\Flagged".into());
    store.save_state(&mut independent).unwrap();
    drop(store);
    assert_eq!(
        connection.move_messages(&permit, &mut evidence, |_| Ok(())),
        Err(Error::Unstable)
    );
    assert!(!evidence.attempted());
    assert!(!root.path().join("move-wire-observations").exists());
}

#[test]
fn malformed_slow_or_referred_greetings_and_failed_authentication_do_not_retry() {
    for mode in [
        "malformed_greeting",
        "referral",
        "slow_greeting",
        "authentication_failed",
    ] {
        let root = EvidenceRoot::new();
        let (repo, account, fake) = setup(root.path());
        std::fs::write(root.path().join("move-mode"), mode).unwrap();
        let result = fake.connect(&account, Cancellation::default());
        assert!(
            matches!(
                (mode, result),
                ("malformed_greeting", Err(Error::Protocol))
                    | ("referral", Err(Error::UnknownCode))
                    | ("slow_greeting", Err(Error::Deadline))
                    | ("authentication_failed", Err(Error::Authentication))
            ),
            "{mode}"
        );
        let (_, state) = external(&repo, &account.id, root.path());
        assert_eq!(
            state
                .synthetic_folders
                .unwrap()
                .commands
                .iter()
                .filter(|c| *c == "LOGIN")
                .count(),
            usize::from(mode == "authentication_failed")
        );
        assert!(!root.path().join("move-wire-observations").exists());
    }
}

/// The installed source that the HTTP fixture and its verifiers use has the
/// longest greeting deadline of any fixture source. A greeting trickled out
/// byte by byte still ends the connection at that deadline, before the
/// greeting completes and before any LOGIN.
#[test]
fn slow_greeting_ends_at_the_installed_sources_greeting_deadline() {
    let root = EvidenceRoot::new();
    let (mut repo, account, fake) = setup(root.path());
    fake.install(&mut repo);
    std::fs::write(root.path().join("move-mode"), "slow_greeting").unwrap();
    let source = repo.backlog_source.clone().unwrap();
    let permit = source
        .permit(
            &account.id,
            account.provider,
            &account.login,
            &source.authority,
        )
        .unwrap();
    let started = std::time::Instant::now();
    let result = Connection::connect(
        &permit,
        &SecretValue::new(b"invented".to_vec()),
        Cancellation::default(),
    );
    assert!(matches!(result, Err(Error::Deadline)));
    assert!(started.elapsed() < SLOW_GREETING);
    let (_, state) = external(&repo, &account.id, root.path());
    assert!(
        !state
            .synthetic_folders
            .unwrap()
            .commands
            .iter()
            .any(|c| c == "LOGIN")
    );
}

#[test]
fn untrusted_and_expired_tls_are_rejected_before_login() {
    for mode in ["untrusted", "expired"] {
        let root = EvidenceRoot::new();
        if mode == "expired" {
            std::fs::write(root.path().join("folder-mode"), "expired_cert").unwrap();
        }
        let (repo, account, mut fake) = setup(root.path());
        if mode == "untrusted" {
            let key = rcgen::KeyPair::generate().unwrap();
            fake.source.root = rcgen::CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .self_signed(&key)
                .unwrap()
                .der()
                .to_vec();
        }
        assert!(matches!(
            fake.connect(&account, Cancellation::default()),
            Err(Error::Tls)
        ));
        let (_, independent) = external(&repo, &account.id, root.path());
        assert!(
            !independent
                .synthetic_folders
                .unwrap()
                .commands
                .iter()
                .any(|c| c == "LOGIN")
        );
    }
}

impl Fake {
    pub(crate) fn install(&self, repo: &mut Accounts) {
        let mut source = self.source.clone();
        source.authority = "fixture-message-move/v1".into();
        source.command_timeout = crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT;
        source.operation_timeout = Duration::from_secs(120);
        repo.backlog_source = Some(Arc::new(source));
        let source = repo.backlog_source.as_ref().unwrap().clone();
        repo.imap_factory = Some(Arc::new(move |read, login, password, cancel| {
            let binding = RuntimeBinding {
                account: source.account.clone(),
                provider: read.provider(),
                login: login.into(),
                source: source.identity(),
                credential_key_recovery: "explicit-fixture-read-only".into(),
                workspace: "read-only-no-recipient-authority".into(),
                topology: "empty-personal-slash-profile".into(),
            };
            let permit = source
                .runtime_permit(&binding)
                .map_err(|_| crate::acquisition::AcquisitionError::InvalidInput)?;
            crate::acquisition::imap::ReadOnlyImap::loopback(&permit, read, login, password, cancel)
        }));
    }
}
pub(crate) fn workflow_install(root: &Path) -> (Accounts, Account, Fake) {
    std::fs::write(root.join("move-workflow"), b"explicit fixture").unwrap();
    let (mut repo, account) = if let Ok(id) = std::fs::read_to_string(root.join("move-account-id"))
    {
        let repo = persistent_test_accounts(root);
        let account = repo.load(&AccountId::new(&id).unwrap()).unwrap();
        (repo, account)
    } else {
        let (repo, account, create) = crate::acquisition::folder_provisioning::tests::install(root);
        drop(create);
        std::fs::write(root.join("move-account-id"), account.id.as_str()).unwrap();
        (repo, account)
    };
    let fake = Fake::start(root, &account);
    fake.install(&mut repo);
    if !std::fs::read_to_string(root.join("move-source"))
        .is_ok_and(|source| source == "create_only")
    {
        repo.folder_source = None;
    }
    if std::fs::read_to_string(root.join("move-source"))
        .is_ok_and(|source| matches!(source.as_str(), "read_only" | "create_only"))
    {
        repo.backlog_source = None;
    }
    if let Ok(epoch) = std::fs::read_to_string(root.join("workspace-authority")) {
        if !epoch.is_empty() {
            repo.install_synthetic_workspace(&account, &epoch);
        }
    }
    (repo, account, fake)
}

fn tagged_status(connection: &mut Connection, command: &str) -> (String, Vec<Vec<Token>>) {
    let mut status = String::new();
    let mut rows = vec![];
    connection
        .exchange(command.as_bytes(), |row, tagged| {
            if tagged {
                status = row.get(1).and_then(Token::atom).unwrap_or_default().into();
            } else {
                rows.push(row.to_vec());
            }
            Ok(())
        })
        .unwrap();
    (status, rows)
}

fn highest(rows: &[Vec<Token>]) -> Option<u64> {
    rows.iter().find_map(|row| {
        row.get(2)
            .and_then(Token::atom)?
            .strip_prefix("[HIGHESTMODSEQ ")?
            .strip_suffix(']')?
            .parse()
            .ok()
    })
}

/// The fake answers CONDSTORE's EXAMINE parameter and CHANGEDSINCE with BAD
/// unless it advertises CONDSTORE. When it does, HIGHESTMODSEQ and each
/// message's MODSEQ follow every change, whoever made it.
#[test]
fn fake_refuses_condstore_unless_advertised_and_then_tracks_every_change() {
    for advertised in [false, true] {
        let root = EvidenceRoot::new();
        if advertised {
            std::fs::write(root.path().join("move-condstore"), "advertised").unwrap();
        }
        let (repo, account, fake) = setup(root.path());
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        assert_eq!(connection.capabilities.contains("CONDSTORE"), advertised);
        let (status, rows) = tagged_status(&mut connection, "EXAMINE \"INBOX\" (CONDSTORE)");
        let since = "UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE 1)";
        if !advertised {
            assert_eq!(status, "BAD");
            assert_eq!(tagged_status(&mut connection, since).0, "BAD");
            continue;
        }
        assert_eq!(status, "OK");
        let before = highest(&rows).unwrap();
        let unchanged = format!("UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE {before})");
        let (status, rows) = tagged_status(&mut connection, &unchanged);
        assert_eq!((status.as_str(), rows.len()), ("OK", 0));
        let (store, mut state) = external(&repo, &account.id, root.path());
        state
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .get_mut(&8)
            .unwrap()
            .flags
            .insert("\\Flagged".into());
        store.save_state(&mut state).unwrap();
        drop(store);
        let (_, rows) = tagged_status(&mut connection, "EXAMINE \"INBOX\" (CONDSTORE)");
        let after = highest(&rows).unwrap();
        assert!(after > before);
        let (status, rows) = tagged_status(&mut connection, &unchanged);
        assert_eq!(status, "OK");
        assert_eq!(rows.len(), 1);
        assert_eq!(fetch_uid(&rows[0]).unwrap(), Some(Uid::new(8).unwrap()));
        assert!(rows[0].contains(&Token::Atom("MODSEQ".into())));
    }
}

/// Every check before a MOVE travels in one flight, and the MOVE leaves only
/// when all of them passed: a source whose state moved, or a destination
/// under another UIDVALIDITY, stops before any effect. Readbacks pipelined
/// together fail one by one, and rows never reach a readback whose SELECT
/// failed.
#[test]
fn pipelined_checks_stop_at_the_first_unsafe_response_before_any_effect() {
    for change in ["source_flags", "destination_validity"] {
        let root = EvidenceRoot::new();
        let (repo, account, fake) = setup(root.path());
        let mut connection = fake.connect(&account, Cancellation::default()).unwrap();
        let mut planned = intent(&mut connection);
        if change == "source_flags" {
            let (store, mut state) = external(&repo, &account.id, root.path());
            let inbox = state.census.folders.get_mut("INBOX").unwrap();
            inbox
                .messages
                .get_mut(&19)
                .unwrap()
                .flags
                .insert("$changed".into());
            store.save_state(&mut state).unwrap();
        } else {
            planned = MoveIntent::new(
                Purpose::Filing {
                    protect_flagged: true,
                },
                identity("INBOX", 101),
                identity("Filed/Receipts", 999),
                planned.messages().clone(),
            )
            .unwrap();
        }
        let mut evidence = MoveEvidence::intent(planned);
        let permit = fake
            .source
            .permit(
                &account.id,
                account.provider,
                &account.login,
                "test-bound-authority",
            )
            .unwrap();
        let mut persisted = 0;
        assert_eq!(
            connection.move_messages(&permit, &mut evidence, |_| {
                persisted += 1;
                Ok(())
            }),
            Err(Error::Unstable),
            "{change}"
        );
        assert_eq!(persisted, 0, "{change}: no attempt was recorded");
        assert!(!evidence.attempted());
        assert!(
            !root.path().join("move-wire-observations").exists(),
            "{change}: no MOVE reached the fixture"
        );
        let all: BTreeSet<Uid> = [4, 8, 19, 99].map(|u| Uid::new(u).unwrap()).into();
        let results = connection
            .readbacks(&[
                (&identity("INBOX", 999), &all),
                (&identity("INBOX", 101), &all),
                (&identity("Filed/Receipts", 202), &BTreeSet::new()),
            ])
            .unwrap();
        assert_eq!(results[0].as_ref().unwrap_err(), &Error::Unstable);
        assert_eq!(
            results[1]
                .as_ref()
                .unwrap()
                .messages
                .keys()
                .collect::<Vec<_>>(),
            all.iter().collect::<Vec<_>>()
        );
        assert!(results[2].as_ref().unwrap().messages.is_empty());
        connection.inventory().unwrap();
    }
}
