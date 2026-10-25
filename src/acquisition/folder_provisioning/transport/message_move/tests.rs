use super::*;
use crate::{
    acquisition::{Account, Accounts, persistent_test_accounts},
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

const HEADER: &[u8] = b"Message-ID: <identical@example.test>\r\n\r\n";

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

struct Fake {
    source: InstalledSource,
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
    }
}

impl Fake {
    fn start(root: &Path, account: &Account) -> Self {
        let repo = persistent_test_accounts(root);
        let (store, mut state) = external(&repo, &account.id, root);
        let p = state.synthetic_folders.as_mut().unwrap();
        let listener = TcpListener::bind(p.address).unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![p.root.clone().into()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(p.key.clone())),
                )
                .unwrap(),
        );
        p.inventory
            .capabilities
            .extend(["MOVE".into(), "UIDPLUS".into()]);
        p.inventory.mailboxes = vec!["Filed/Receipts", "INBOX"]
            .into_iter()
            .map(|name| Mailbox {
                name: ExactName::new(name).unwrap(),
                delimiter: Some("/".into()),
                attributes: BTreeSet::new(),
            })
            .collect();
        if !state.census.folders.contains_key("Filed/Receipts") {
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
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let id = account.id.clone();
        let path = root.to_owned();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((tcp, _)) => {
                        let _ = serve(tcp, config.clone(), &repo, &id, &path);
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
    Ok(text)
}

fn serve(
    tcp: TcpStream,
    config: Arc<ServerConfig>,
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
) -> std::io::Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_read_timeout(Some(Duration::from_secs(2)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(2)))?;
    tcp.set_nodelay(true)?;
    let mut io = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        tcp,
    ));
    io.get_mut().write_all(b"* OK synthetic MOVE fixture\r\n")?;
    io.get_mut().flush()?;
    let mut selected = None;
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
        let mode = std::fs::read_to_string(root.join("move-mode")).unwrap_or_default();
        let mut reply = String::new();
        if command == "CAPABILITY" {
            reply = "* CAPABILITY IMAP4rev1 NAMESPACE MOVE UIDPLUS\r\n".into();
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
                "LIST \"\" \"*\"" => {
                    for m in &state
                        .synthetic_folders
                        .as_ref()
                        .unwrap()
                        .inventory
                        .mailboxes
                    {
                        reply.push_str(&format!(
                            "* LIST () \"/\" {}\r\n",
                            String::from_utf8(quoted(m.name.wire.as_bytes()).unwrap()).unwrap()
                        ));
                    }
                }
                _ if command.starts_with("SELECT ") => {
                    let name = unquote(&command[7..])?;
                    let folder = &state.census.folders[&name];
                    reply = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] instance\r\n{tag} OK [READ-WRITE] selected\r\n",
                        folder.live_uids.len(),
                        folder.uidvalidity
                    );
                    selected = Some(name);
                }
                _ if command.starts_with("UID FETCH ") => {
                    let set = command[10..].split_once(' ').unwrap().0;
                    let f = &state.census.folders[selected.as_ref().unwrap()];
                    for uid in set.split(',').map(|u| u.parse::<u32>().unwrap()) {
                        if let Some(m) = f.messages.get(&uid) {
                            reply.push_str(&format!("* 1 FETCH (UID {uid} FLAGS ({}) INTERNALDATE \"{}\" RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{})\r\n",m.flags.iter().cloned().collect::<Vec<_>>().join(" "),m.internal_date,m.size,HEADER.len(),std::str::from_utf8(HEADER).unwrap()));
                        }
                    }
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
                    if source == "INBOX" {
                        assert_eq!(set, "4,8,19");
                        assert_eq!(target, "Filed/Receipts");
                    }
                    let mut mapped = vec![];
                    let effected = if mode == "partial" {
                        &uids[..1]
                    } else {
                        &uids[..]
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
                    store.save_state(&mut state).unwrap();
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("move-wire-observations"))?;
                    log.write_all(b"UID MOVE\n")?;
                    log.sync_all()?;
                    std::fs::write(root.join("move-effect-reached"), b"effect persisted")?;
                    if mode == "lost" {
                        return Ok(());
                    }
                    if mode == "slow" {
                        thread::sleep(Duration::from_millis(1200));
                        return Ok(());
                    }
                    let code = format!(
                        "[COPYUID {} {set} {}]",
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
                    if mode == "missing" {
                        reply = format!("{tag} OK moved without mapping\r\n");
                    } else if mode == "untagged" {
                        reply = format!(
                            "* OK {code} mapping\r\n* 1 EXPUNGE\r\n* 4 EXISTS\r\n{tag} NO [OVERQUOTA] partial completion\r\n"
                        );
                    } else if mode == "partial" {
                        reply = format!(
                            "* OK [COPYUID {} {} {}] subset\r\n{tag} NO [OVERQUOTA] partial\r\n",
                            state.census.folders[&target].uidvalidity, uids[0], mapped[0]
                        );
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
        store.save_state(&mut state).unwrap();
        drop(store);
        for chunk in reply.as_bytes().chunks(3) {
            io.get_mut().write_all(chunk)?;
            io.get_mut().flush()?;
        }
    }
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
