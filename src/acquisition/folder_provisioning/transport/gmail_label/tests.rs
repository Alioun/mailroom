use super::*;
use crate::{
    acquisition::{Account, Accounts, fixture_peer::Peer, persistent_test_accounts},
    census::{
        AccountState, Direction, EncryptedStore, FolderCensus, MessageRecord, ScanState,
        folder_provisioning::{FakeProvider, Namespace, Namespaces},
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

pub(crate) const ALL: &str = "[Gmail]/All Mail";
pub(crate) const PERSONAL: &str = "Privat/Studio";
pub(crate) const BUSINESS: &str = "Geschäftlich/Studio";
const STATIC: [&str; 2] = ["[Gmail]/Spam", "[Gmail]/Trash"];

/// Invented messages: (All Mail UID, X-GM-MSGID, labels, flags).
const MESSAGES: [(u32, u64, &[&str], &[&str]); 9] = [
    (4, 1001, &["\\Inbox"], &["\\Seen"]),
    (8, 1002, &["\\Inbox", "Andere"], &["\\Seen", "$unrelated"]),
    (19, 1003, &["\\Inbox", PERSONAL], &[]),
    (23, 1004, &["\\Inbox"], &["\\Flagged"]),
    (30, 1005, &[], &["\\Seen"]),
    (31, 1006, &["\\Inbox", "\\Important"], &[]),
    (40, 1007, &["\\Sent"], &["\\Seen"]),
    (41, 1008, &["\\Inbox"], &[]),
    (42, 1009, &["Andere"], &[]),
];

pub(crate) struct Fake {
    pub(in crate::acquisition) source: InstalledSource,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
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
    }
}

pub(crate) fn external(
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
) -> (EncryptedStore, AccountState) {
    let store = repo.fixture_sieve_store(id, root).unwrap();
    let state = store.load_state().unwrap();
    (store, state)
}

pub(crate) fn header(id: u64) -> String {
    let (from, to) = match id {
        1006 => ("updates@studio.example.com", "team@work.example.test"),
        1007 => ("owner@home.example.test", "friend@other.example.test"),
        1008 => ("someone@unrelated.example.net", "owner@home.example.test"),
        _ => ("updates@studio.example.com", "owner@home.example.test"),
    };
    format!(
        "From: {from}\r\nTo: {to}\r\nDelivered-To: {to}\r\nSubject: Studio update {id}\r\nMessage-ID: <gmail-{id}@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n"
    )
}

pub(crate) fn record(uid: u32, id: u64, labels: &[&str], flags: &[&str]) -> MessageRecord {
    MessageRecord {
        delivery: Default::default(),
        uid,
        size: 1000 + id,
        internal_date: "25-Oct-2026 12:00:00 +0000".into(),
        flags_observed: true,
        flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        gmail_message_id: Some(id.to_string()),
        gmail_thread_id: Some((id + 50_000).to_string()),
        gmail_labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        headers: parse_headers(header(id).as_bytes(), Direction::Received).unwrap(),
    }
}

fn mailbox(name: &str, attributes: &[&str]) -> Mailbox {
    Mailbox {
        name: ExactName::new(name).unwrap(),
        delimiter: Some("/".into()),
        attributes: attributes.iter().map(|a| (*a).to_owned()).collect(),
    }
}

/// The label a view mailbox shows. All Mail and the static Spam/Trash
/// mailboxes are not label views.
fn membership(name: &str) -> Option<String> {
    match name {
        "INBOX" => Some(INBOX.into()),
        "[Gmail]/Sent Mail" => Some("\\Sent".into()),
        "[Gmail]/Drafts" => Some("\\Draft".into()),
        _ if name == ALL || name == "[Gmail]" || STATIC.contains(&name) => None,
        _ => Some(name.into()),
    }
}

fn validity(name: &str) -> u32 {
    match name {
        "INBOX" => 21,
        "[Gmail]/Sent Mail" => 31,
        "[Gmail]/Drafts" => 32,
        _ => 40 + name.bytes().map(u32::from).sum::<u32>() % 1000,
    }
}

/// Recomputes each label view from canonical All Mail. A message gaining a
/// label receives a new UID in that view; one losing it leaves the view.
pub(crate) fn sync(state: &mut AccountState) {
    let canonical = state.census.folders[ALL].clone();
    let names: Vec<String> = state
        .synthetic_folders
        .as_ref()
        .unwrap()
        .inventory
        .mailboxes
        .iter()
        .filter(|m| m.selectable())
        .map(|m| m.name.decoded.clone())
        .collect();
    for name in names {
        let Some(label) = membership(&name) else {
            continue;
        };
        let folder = state
            .census
            .folders
            .entry(name.clone())
            .or_insert_with(|| FolderCensus {
                uidvalidity: validity(&name),
                state: ScanState::Complete,
                live_uids: Default::default(),
                messages: Default::default(),
            });
        let members: BTreeMap<_, _> = canonical
            .messages
            .values()
            .filter(|m| m.gmail_labels.contains(&label))
            .map(|m| (m.gmail_message_id.clone(), m))
            .collect();
        folder
            .messages
            .retain(|_, m| members.contains_key(&m.gmail_message_id));
        let present: BTreeMap<_, _> = folder
            .messages
            .iter()
            .map(|(uid, m)| (m.gmail_message_id.clone(), *uid))
            .collect();
        for (id, m) in &members {
            let uid = match present.get(id) {
                Some(uid) => *uid,
                None => {
                    let next = state.synthetic_uid_next.entry(name.clone()).or_insert(1);
                    let uid = *next;
                    *next += 1;
                    uid
                }
            };
            let mut copy = (*m).clone();
            copy.uid = uid;
            folder.messages.insert(uid, copy);
        }
        folder.live_uids = folder.messages.keys().copied().collect();
    }
    let listed = state_names(&state.synthetic_folders);
    state
        .census
        .folders
        .retain(|name, _| name == ALL || STATIC.contains(&name.as_str()) || listed.contains(name));
}

fn state_names(provider: &Option<FakeProvider>) -> Vec<String> {
    provider
        .as_ref()
        .unwrap()
        .inventory
        .mailboxes
        .iter()
        .map(|m| m.name.decoded.clone())
        .collect()
}

fn initialize(root: &Path, state: &mut AccountState, listener: &TcpListener) {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let mut mailboxes = vec![
        mailbox("Andere", &["\\HasNoChildren"]),
        mailbox("Geschäftlich", &["\\HasChildren"]),
        mailbox(BUSINESS, &["\\HasNoChildren"]),
        mailbox("INBOX", &["\\HasNoChildren"]),
        mailbox("Privat", &["\\HasChildren"]),
        mailbox(PERSONAL, &["\\HasNoChildren"]),
        mailbox("[Gmail]", &["\\HasChildren", "\\Noselect"]),
        mailbox(ALL, &["\\All", "\\HasNoChildren"]),
        mailbox("[Gmail]/Drafts", &["\\Drafts", "\\HasNoChildren"]),
        mailbox("[Gmail]/Sent Mail", &["\\HasNoChildren", "\\Sent"]),
        mailbox("[Gmail]/Spam", &["\\HasNoChildren", "\\Junk"]),
        mailbox("[Gmail]/Trash", &["\\HasNoChildren", "\\Trash"]),
    ];
    mailboxes.sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
    state.synthetic_folders = Some(FakeProvider {
        address: listener.local_addr().unwrap(),
        root: cert.der().to_vec(),
        key: key.serialize_der(),
        epoch: format!("gmail-{:032x}", rand::random::<u128>()),
        inventory: Inventory {
            capabilities: [
                "IMAP4REV1",
                "NAMESPACE",
                "SPECIAL-USE",
                "UIDPLUS",
                "MOVE",
                "X-GM-EXT-1",
            ]
            .map(String::from)
            .into(),
            namespaces: Namespaces {
                personal: vec![Namespace {
                    prefix: String::new(),
                    delimiter: Some("/".into()),
                }],
                other: vec![],
                shared: vec![],
            },
            mailboxes,
        },
        commands: vec![],
    });
    let mut all: BTreeMap<u32, MessageRecord> = MESSAGES
        .iter()
        .map(|(uid, id, labels, flags)| (*uid, record(*uid, *id, labels, flags)))
        .collect();
    let mut next = 100;
    if let Ok(large) = std::fs::read_to_string(root.join("gmail-large")) {
        let count: u32 = large.trim().parse().unwrap_or(501);
        for n in 0..count {
            let uid = 1000 + 2 * n;
            all.insert(uid, record(uid, 5000 + u64::from(n), &["\\Inbox"], &[]));
        }
        next = (1000 + 2 * count + 1).max(3000);
    }
    state.census.folders.clear();
    state.census.folders.insert(
        ALL.into(),
        FolderCensus {
            uidvalidity: 11,
            state: ScanState::Complete,
            live_uids: all.keys().copied().collect(),
            messages: all,
        },
    );
    for (name, uidvalidity, id) in [(STATIC[0], 71, 2001), (STATIC[1], 72, 2002)] {
        state.census.folders.insert(
            name.into(),
            FolderCensus {
                uidvalidity,
                state: ScanState::Complete,
                live_uids: [1].into(),
                messages: [(1, record(1, id, &[], &[]))].into(),
            },
        );
    }
    state.synthetic_uid_next.clear();
    state.synthetic_uid_next.insert(ALL.into(), next);
    sync(state);
}

impl Fake {
    pub(crate) fn start(root: &Path, account: &Account) -> Self {
        let repo = persistent_test_accounts(root);
        let store = repo.fixture_sieve_store(&account.id, root).unwrap();
        if !store.path().exists() {
            let primary = repo.store(account).unwrap();
            store.save(&primary.load().unwrap()).unwrap();
        }
        let mut state = store.load_state().unwrap();
        let initialized = state
            .synthetic_folders
            .as_ref()
            .is_some_and(|p| p.epoch.starts_with("gmail-"));
        let listener = TcpListener::bind(if initialized {
            state.synthetic_folders.as_ref().unwrap().address
        } else {
            "127.0.0.1:0".parse().unwrap()
        })
        .unwrap();
        listener.set_nonblocking(true).unwrap();
        if !initialized {
            initialize(root, &mut state, &listener);
            store.save_state(&mut state).unwrap();
        }
        let p = state.synthetic_folders.as_ref().unwrap();
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![p.root.clone().into()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(p.key.clone())),
                )
                .unwrap(),
        );
        let source = InstalledSource {
            account: account.id.clone(),
            login: account.login.clone(),
            address: p.address,
            tls_name: "localhost".into(),
            root: p.root.clone(),
            epoch: p.epoch.clone(),
            authority: AUTHORITY.into(),
            command_timeout: crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT,
            operation_timeout: Duration::from_secs(120),
        };
        drop(store);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let id = account.id.clone();
        let path = root.to_owned();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((tcp, _)) => {
                        let _ = Peer::new(tcp, stopping.clone())
                            .and_then(|tcp| serve(tcp, config.clone(), &repo, &id, &path));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            source,
            stop,
            worker: Some(worker),
        }
    }

    pub(crate) fn install(&self, repo: &mut Accounts) {
        repo.label_source = Some(Arc::new(self.source.clone()));
        let source = repo.label_source.as_ref().unwrap().clone();
        repo.imap_factory = Some(Arc::new(move |read, login, password, cancel| {
            let binding = RuntimeBinding {
                account: source.account.clone(),
                provider: read.provider(),
                login: login.into(),
                source: source.identity(),
                credential_key_recovery: "explicit-fixture-read-only".into(),
                workspace: "read-only-no-recipient-authority".into(),
                topology: "gmail-all-mail-profile".into(),
            };
            let permit = source
                .runtime_permit(&binding)
                .map_err(|_| crate::acquisition::AcquisitionError::InvalidInput)?;
            crate::acquisition::imap::ReadOnlyImap::loopback_gmail(
                &permit, read, login, password, cancel,
            )
        }));
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
    Ok(text)
}

fn wire(name: &str) -> String {
    String::from_utf8(quoted(ExactName::new(name).unwrap().wire.as_bytes()).unwrap()).unwrap()
}

/// Splits a parenthesized STORE label list into decoded labels.
fn labels(list: &str) -> std::io::Result<Vec<String>> {
    let inner = list
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let mut result = vec![];
    let mut rest = inner.trim_start();
    while !rest.is_empty() {
        let (token, tail) = if rest.starts_with('"') {
            let mut end = 1;
            let bytes = rest.as_bytes();
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            (
                &rest[..=end.min(rest.len() - 1)],
                &rest[(end + 1).min(rest.len())..],
            )
        } else {
            rest.split_at(rest.find(' ').unwrap_or(rest.len()))
        };
        result.push(if token.starts_with('"') {
            let text = unquote(token)?;
            if text.starts_with('\\') {
                text
            } else {
                ExactName::from_wire(&text)
                    .map_err(|_| std::io::ErrorKind::InvalidData)?
                    .decoded
            }
        } else if token.starts_with('\\') {
            token.to_owned()
        } else {
            ExactName::from_wire(token)
                .map_err(|_| std::io::ErrorKind::InvalidData)?
                .decoded
        });
        rest = tail.trim_start();
    }
    Ok(result)
}

fn forbidden(root: &Path, command: &str) -> ! {
    let kind = command.split(' ').take(2).collect::<Vec<_>>().join(" ");
    std::fs::write(root.join("gmail-forbidden"), kind).unwrap();
    panic!("Forbidden command reached the synthetic Gmail fixture");
}

fn label_list(m: &MessageRecord, mode: &str) -> String {
    m.gmail_labels
        .iter()
        .map(|l| {
            if l.starts_with('\\') {
                l.clone()
            } else if mode == "case_fold" {
                wire(&l.to_lowercase())
            } else {
                wire(l)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn race(state: &mut AccountState, root: &Path, phase: &str, uids: &[u32]) {
    let path = root.join("gmail-race");
    let Ok(race) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(kind) = race.strip_prefix(&format!("{phase}:")) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let Some(first) = uids.first() else {
        return;
    };
    let m = state
        .census
        .folders
        .get_mut(ALL)
        .unwrap()
        .messages
        .get_mut(first)
        .unwrap();
    match kind {
        "flag" => {
            m.flags.insert("\\Flagged".into());
        }
        "label" => {
            m.gmail_labels.insert("Andere".into());
        }
        "archive" => {
            m.gmail_labels.remove(INBOX);
        }
        _ => panic!("unknown Gmail race"),
    }
}

/// Applies a label delta to canonical All Mail, as Gmail is assumed to:
/// additive, idempotent, and creating an unknown user label implicitly.
fn store_labels(state: &mut AccountState, gmail_ids: &[String], add: bool, values: &[String]) {
    for value in values {
        let known = state
            .synthetic_folders
            .as_ref()
            .unwrap()
            .inventory
            .mailboxes
            .iter()
            .any(|m| m.name.decoded == *value);
        if add && !value.starts_with('\\') && !known {
            let p = state.synthetic_folders.as_mut().unwrap();
            p.inventory
                .mailboxes
                .push(mailbox(value, &["\\HasNoChildren"]));
            p.inventory
                .mailboxes
                .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
        }
    }
    let all = state.census.folders.get_mut(ALL).unwrap();
    for m in all.messages.values_mut() {
        if m.gmail_message_id
            .as_ref()
            .is_some_and(|id| gmail_ids.contains(id))
        {
            for value in values {
                if add {
                    m.gmail_labels.insert(value.clone());
                } else {
                    m.gmail_labels.remove(value);
                }
            }
        }
    }
}

fn serve(
    tcp: Peer,
    config: Arc<ServerConfig>,
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
) -> std::io::Result<()> {
    tcp.tcp.set_nodelay(true)?;
    let mut io = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        tcp,
    ));
    io.get_mut()
        .write_all(b"* OK synthetic Gmail fixture\r\n")?;
    io.get_mut().flush()?;
    let mut selected: Option<String> = None;
    let mut authenticated = false;
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
        let (store, mut state) = external(repo, id, root);
        let mode = std::fs::read_to_string(root.join("gmail-mode")).unwrap_or_default();
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
                            wire(&m.name.decoded)
                        ));
                    }
                }
                _ if command.starts_with("SELECT ") || command.starts_with("EXAMINE ") => {
                    let read_only = command.starts_with("EXAMINE ");
                    let wire_name = unquote(&command[if read_only { 8 } else { 7 }..])?;
                    let name = ExactName::from_wire(&wire_name)
                        .map_err(|_| std::io::ErrorKind::InvalidData)?
                        .decoded;
                    let Some(folder) = state.census.folders.get(&name) else {
                        reply = format!("{tag} NO [NONEXISTENT] unknown mailbox\r\n");
                        selected = None;
                        drop(store);
                        io.get_mut().write_all(reply.as_bytes())?;
                        io.get_mut().flush()?;
                        continue;
                    };
                    reply = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] instance\r\n{tag} OK [{}] selected\r\n",
                        folder.live_uids.len(),
                        folder.uidvalidity,
                        if read_only { "READ-ONLY" } else { "READ-WRITE" }
                    );
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
                    let name = ExactName::from_wire(&unquote(name)?)
                        .map_err(|_| std::io::ErrorKind::InvalidData)?
                        .decoded;
                    reply = format!(
                        "* STATUS {} (MESSAGES {})\r\n",
                        wire(&name),
                        state
                            .census
                            .folders
                            .get(&name)
                            .map_or(0, |f| f.live_uids.len())
                    );
                }
                "LOGOUT" => {
                    io.get_mut()
                        .write_all(format!("* BYE\r\n{tag} OK logout\r\n").as_bytes())?;
                    io.get_mut().flush()?;
                    return Ok(());
                }
                _ if command.starts_with("UID FETCH ") => {
                    let (set, items) = command[10..].split_once(' ').unwrap();
                    let f = &state.census.folders[selected.as_ref().unwrap()];
                    let items = items
                        .strip_prefix('(')
                        .and_then(|s| s.strip_suffix(')'))
                        .unwrap();
                    let mut requested = vec![];
                    let mut rest = items;
                    while !rest.is_empty() {
                        if let Some(body) = rest.strip_prefix("BODY.PEEK[") {
                            let end = body.find(']').unwrap();
                            requested.push(format!("BODY[{}]", &body[..end]));
                            rest = &body[end + 1..];
                            if rest.starts_with('<') {
                                rest = &rest[rest.find('>').unwrap() + 1..];
                            }
                        } else {
                            let end = rest.find(' ').unwrap_or(rest.len());
                            requested.push(rest[..end].to_owned());
                            rest = &rest[end..];
                        }
                        rest = rest.trim_start();
                    }
                    for uid in set.split(',').map(|u| u.parse::<u32>().unwrap()) {
                        let Some(m) = f.messages.get(&uid) else {
                            continue;
                        };
                        let seq = f.live_uids.iter().position(|u| *u == uid).unwrap() + 1;
                        let gmail = m.gmail_message_id.as_deref().unwrap();
                        let parts = requested
                            .iter()
                            .map(|item| match item.as_str() {
                                "UID" => format!("UID {uid}"),
                                "FLAGS" => format!(
                                    "FLAGS ({})",
                                    m.flags.iter().cloned().collect::<Vec<_>>().join(" ")
                                ),
                                "INTERNALDATE" => format!("INTERNALDATE \"{}\"", m.internal_date),
                                "RFC822.SIZE" => format!("RFC822.SIZE {}", m.size),
                                "X-GM-MSGID" => format!("X-GM-MSGID {gmail}"),
                                "X-GM-THRID" => {
                                    format!("X-GM-THRID {}", m.gmail_thread_id.as_deref().unwrap())
                                }
                                "X-GM-LABELS" => format!("X-GM-LABELS ({})", label_list(m, &mode)),
                                body if body.starts_with("BODY[") => {
                                    let text = header(gmail.parse().unwrap());
                                    format!("{body} {{{}}}\r\n{text}", text.len())
                                }
                                other => panic!("Unexpected Gmail FETCH item {other}"),
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        reply.push_str(&format!("* {seq} FETCH ({parts})\r\n"));
                    }
                }
                _ if command.starts_with("UID STORE ") => {
                    let (set, rest) = command[10..].split_once(' ').unwrap();
                    let (operation, list) = rest.split_once(' ').unwrap();
                    let add = match operation {
                        "+X-GM-LABELS" => true,
                        "-X-GM-LABELS" => false,
                        _ => forbidden(root, command),
                    };
                    let values = labels(list)?;
                    let name = selected.clone().unwrap();
                    let uids: Vec<u32> = set.split(',').map(|u| u.parse().unwrap()).collect();
                    assert!(uids.len() <= 500);
                    race(&mut state, root, "before", &uids);
                    let present: Vec<u32> = uids
                        .iter()
                        .copied()
                        .filter(|u| state.census.folders[&name].messages.contains_key(u))
                        .collect();
                    let effected: Vec<u32> = match mode.as_str() {
                        "refuse" => vec![],
                        "bad_remove" if !add => vec![],
                        "partial" => present.iter().copied().take(1).collect(),
                        _ => present.clone(),
                    };
                    let ids: Vec<String> = effected
                        .iter()
                        .map(|u| {
                            state.census.folders[&name].messages[u]
                                .gmail_message_id
                                .clone()
                                .unwrap()
                        })
                        .collect();
                    store_labels(&mut state, &ids, add, &values);
                    if mode == "uid_moves" && !effected.is_empty() {
                        let all = state.census.folders.get_mut(ALL).unwrap();
                        let next = state.synthetic_uid_next.get_mut(ALL).unwrap();
                        for uid in &effected {
                            let mut m = all.messages.remove(uid).unwrap();
                            m.uid = *next;
                            *next += 1;
                            all.messages.insert(m.uid, m);
                        }
                        all.live_uids = all.messages.keys().copied().collect();
                    }
                    race(&mut state, root, "after", &effected);
                    sync(&mut state);
                    store.save_state(&mut state).unwrap();
                    let mut exact = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("gmail-wire-exact.jsonl"))?;
                    exact.write_all(&serde_json::to_vec(&serde_json::json!({"command":command,"selected":name,"effected":effected,"validity":state.census.folders[&name].uidvalidity})).unwrap())?;
                    exact.write_all(b"\n")?;
                    exact.sync_all()?;
                    crate::acquisition::gmail_backlog::boundary("gmail_provider_effect");
                    if mode == "lost" {
                        return Ok(());
                    }
                    let f = &state.census.folders[&name];
                    for uid in &effected {
                        if let Some(m) = f.messages.get(uid) {
                            let seq = f.live_uids.iter().position(|u| u == uid).unwrap() + 1;
                            reply.push_str(&format!(
                                "* {seq} FETCH (X-GM-LABELS ({}) UID {uid})\r\n",
                                label_list(m, &mode)
                            ));
                        }
                    }
                    reply.push_str(&match mode.as_str() {
                        "refuse" => format!("{tag} NO [LIMIT] refused\r\n"),
                        "bad_remove" if !add => format!("{tag} BAD unsupported removal\r\n"),
                        "partial" => format!("{tag} NO partial store\r\n"),
                        _ => format!("{tag} OK Success\r\n"),
                    });
                }
                _ => forbidden(root, command),
            }
        }
        if !reply.contains(&format!("{tag} ")) {
            reply.push_str(&format!("{tag} OK completed\r\n"));
        }
        if command.starts_with("LOGIN ") {
            store.save_state(&mut state).unwrap();
        }
        drop(store);
        for chunk in reply.as_bytes().chunks(512) {
            io.get_mut().write_all(chunk)?;
            io.get_mut().flush()?;
        }
    }
}

/// Edits the fake as another client would, between application actions.
pub(crate) fn other_client(repo: &Accounts, id: &AccountId, root: &Path, action: &str) {
    let (store, mut state) = external(repo, id, root);
    let gmail = |state: &mut AccountState, id: &str| -> u32 {
        *state.census.folders[ALL]
            .messages
            .iter()
            .find(|(_, m)| m.gmail_message_id.as_deref() == Some(id))
            .unwrap()
            .0
    };
    let parts: Vec<&str> = action.split(':').collect();
    match parts.as_slice() {
        ["flag", id] => {
            let uid = gmail(&mut state, id);
            let all = state.census.folders.get_mut(ALL).unwrap();
            all.messages
                .get_mut(&uid)
                .unwrap()
                .flags
                .insert("\\Flagged".into());
        }
        ["label", id, label] => {
            store_labels(&mut state, &[(*id).into()], true, &[(*label).into()]);
        }
        ["unlabel", id, label] => {
            store_labels(&mut state, &[(*id).into()], false, &[(*label).into()]);
        }
        ["validity"] => state.census.folders.get_mut(ALL).unwrap().uidvalidity += 1,
        ["no_extension"] => {
            state
                .synthetic_folders
                .as_mut()
                .unwrap()
                .inventory
                .capabilities
                .remove("X-GM-EXT-1");
        }
        ["drop_label", name] => {
            state
                .synthetic_folders
                .as_mut()
                .unwrap()
                .inventory
                .mailboxes
                .retain(|m| m.name.decoded != *name);
            state.census.folders.remove(*name);
            let all = state.census.folders.get_mut(ALL).unwrap();
            for m in all.messages.values_mut() {
                m.gmail_labels.remove(*name);
            }
        }
        _ => panic!("unknown other-client action {action}"),
    }
    sync(&mut state);
    store.save_state(&mut state).unwrap();
}

pub(crate) fn workflow_install(root: &Path) -> (Accounts, Account, Fake) {
    let mut repo = persistent_test_accounts(root);
    let (account, fresh) = match std::fs::read_to_string(root.join("gmail-account-id")) {
        Ok(id) => (repo.load(&AccountId::new(&id).unwrap()).unwrap(), false),
        Err(_) => {
            let account = repo
                .create(
                    Provider::Gmail,
                    "owner@gmail.example.test".into(),
                    &SecretValue::new(b"invented".to_vec()),
                )
                .unwrap();
            std::fs::write(root.join("gmail-account-id"), account.id.as_str()).unwrap();
            (account, true)
        }
    };
    let fake = Fake::start(root, &account);
    fake.install(&mut repo);
    if let Ok(epoch) = std::fs::read_to_string(root.join("workspace-authority")) {
        if !epoch.is_empty() {
            repo.install_synthetic_workspace(&account, &epoch);
        }
    }
    if fresh {
        repo.scan(&account.id, Cancellation::default(), |_| {})
            .unwrap();
        save_workspace(&repo, &account);
    }
    if std::fs::read_to_string(root.join("gmail-source")).is_ok_and(|s| s == "absent") {
        repo.label_source = None;
    }
    (repo, account, fake)
}

pub(crate) fn save_workspace(repo: &Accounts, account: &Account) {
    let store = repo.store(account).unwrap();
    let mut state = store.load_state().unwrap();
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../examples/sample.json")).unwrap();
    let mut documents: crate::engine::Documents =
        serde_json::from_value(sample["documents"].clone()).unwrap();
    documents.override_yaml = documents.override_yaml.replacen(
        "major: 1\n",
        "major: 1\nlayout: {language: de, personal_root: Privat, business_root: Geschäftlich, new_addresses: Neue Adressen, retired_addresses: Stillgelegte Adressen}\n",
        1,
    );
    state.workspace = Some(crate::rules::workspace::Workspace {
        revision: state.workspace.as_ref().map_or(1, |w| w.revision + 1),
        input: crate::rules::workspace::WorkspaceInput {
            documents,
            pins: vec![],
            as_of: 1792886400,
            baseline: crate::rules::proof::Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
}
