use super::*;
use crate::{
    acquisition::{
        SecretValue,
        fixture_peer::{LastSaved, Peer},
        persistent_test_accounts,
    },
    census::{
        AccountId,
        folder_provisioning::{ExactName, FakeProvider, Inventory, Mailbox, Namespace, Namespaces},
    },
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

pub(crate) struct Fixture {
    pub(crate) connections: Arc<AtomicUsize>,
    pub(in crate::acquisition) source: InstalledSource,
    root: PathBuf,
    busy: Busy,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
thread_local! {
    pub(super) static SAVE_TIME: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}
/// Splits a folder run's wall time into product journal saves and the rest
/// (provider exchanges and scheduling).
pub(super) struct RunTiming(std::time::Instant);
impl RunTiming {
    pub(super) fn start() -> Self {
        SAVE_TIME.with(|t| t.set(Duration::ZERO));
        Self(std::time::Instant::now())
    }
    pub(super) fn report(self, steps: usize) {
        let elapsed = self.0.elapsed();
        let saves = SAVE_TIME.with(|t| t.get());
        eprintln!(
            "folder.run.timing steps={steps} elapsed_ms={} journal_save_ms={} other_ms={}",
            elapsed.as_millis(),
            saves.as_millis(),
            elapsed.saturating_sub(saves).as_millis()
        );
    }
}
pub(super) fn report_failure(failure: Failure) {
    eprintln!("folder.failure={}", failure_code(failure));
}
fn failure_code(failure: Failure) -> &'static str {
    match failure {
        Failure::Cancelled => "cancelled",
        Failure::Deadline => "deadline",
        Failure::Connect => "connect",
        Failure::Tls => "tls",
        Failure::Authentication => "authentication",
        Failure::Unsupported => "unsupported",
        Failure::UnsupportedName => "unsupported_name",
        Failure::Protocol => "protocol",
        Failure::No => "no",
        Failure::Bad => "bad",
        Failure::Bye => "bye",
        Failure::Eof => "eof",
        Failure::Quota => "quota",
        Failure::UnknownCode => "unknown_code",
        Failure::Limit => "limit",
        Failure::Unstable => "unstable",
    }
}
#[test]
fn offline_diagnostics_preserve_folder_failure_contract() {
    for failure in [
        Failure::Cancelled,
        Failure::Deadline,
        Failure::Connect,
        Failure::Tls,
        Failure::Authentication,
        Failure::Unsupported,
        Failure::UnsupportedName,
        Failure::Protocol,
        Failure::No,
        Failure::Bad,
        Failure::Bye,
        Failure::Eof,
        Failure::Quota,
        Failure::UnknownCode,
        Failure::Limit,
        Failure::Unstable,
    ] {
        assert_eq!(protocol(failure), AcquisitionError::Protocol);
        assert!(
            failure_code(failure)
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        let mut busy = BUSY.lock().unwrap();
        if busy
            .get(&self.root)
            .is_some_and(|b| Arc::ptr_eq(b, &self.busy))
            && Arc::strong_count(&self.busy) == 2
        {
            busy.remove(&self.root);
        }
    }
}
/// Connections each in-process fixture has accepted and not finished, by root.
type Busy = Arc<(Mutex<usize>, Condvar)>;
static BUSY: Mutex<BTreeMap<PathBuf, Busy>> = Mutex::new(BTreeMap::new());
/// Marks one accepted connection as in progress until it is dropped, panics included.
struct Serving<'a>(&'a Busy);
impl<'a> Serving<'a> {
    fn start(busy: &'a Busy) -> Self {
        *busy.0.lock().unwrap() += 1;
        Self(busy)
    }
}
impl Drop for Serving<'_> {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.1.notify_all();
    }
}
/// Provider state as a test observes it: after the fake server has finished
/// every connection it accepted. A client that gave up at a deadline or a
/// cancellation returns while the server may still be handling its last
/// command, and that command belongs to what the test observes.
fn provider(repo: &Accounts, id: &AccountId, root: &Path) -> (EncryptedStore, AccountState) {
    let busy = BUSY.lock().unwrap().get(root).cloned();
    if let Some(busy) = busy {
        let (count, idle) = &*busy;
        let (count, wait) = idle
            .wait_timeout_while(count.lock().unwrap(), Duration::from_secs(60), |n| *n > 0)
            .unwrap();
        assert!(!wait.timed_out(), "folder.fixture.settle_timeout");
        drop(count);
    }
    served(repo, id, root)
}
/// Provider state as the fake server reads and writes it.
fn served(repo: &Accounts, id: &AccountId, root: &Path) -> (EncryptedStore, AccountState) {
    let store = repo
        .fixture_sieve_store(id, root)
        .unwrap_or_else(|error| panic!("folder.fixture.store_open={error:?}"));
    let state = store
        .load_state()
        .unwrap_or_else(|error| panic!("folder.fixture.store_load={error:?}"));
    (store, state)
}
fn mailbox(name: &str) -> Mailbox {
    Mailbox {
        name: ExactName::new(name).unwrap(),
        delimiter: Some("/".into()),
        attributes: BTreeSet::new(),
    }
}
impl Fixture {
    pub(crate) fn start(root: &Path, id: &AccountId) -> Self {
        let repo = persistent_test_accounts(root);
        let primary = repo.store(&repo.load(id).unwrap()).unwrap();
        let store = repo.fixture_sieve_store(id, root).unwrap();
        if !store.path().exists() {
            store.save(&primary.load().unwrap()).unwrap();
        }
        drop(primary);
        let mut saved = store.load_state().unwrap();
        let listener = TcpListener::bind(
            saved
                .synthetic_folders
                .as_ref()
                .map(|p| p.address)
                .unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()),
        )
        .unwrap();
        listener.set_nonblocking(true).unwrap();
        if saved.synthetic_folders.is_none() {
            let key = rcgen::KeyPair::generate().unwrap();
            let mut parameters = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
            if std::fs::read_to_string(root.join("folder-mode")).is_ok_and(|s| s == "expired_cert")
            {
                parameters.not_before = rcgen::date_time_ymd(2018, 1, 1);
                parameters.not_after = rcgen::date_time_ymd(2019, 1, 1);
            }
            let cert = parameters.self_signed(&key).unwrap();
            saved.synthetic_folders = Some(FakeProvider {
                address: listener.local_addr().unwrap(),
                root: cert.der().to_vec(),
                key: key.serialize_der(),
                epoch: format!("folders-{:032x}", rand::random::<u128>()),
                inventory: Inventory {
                    capabilities: ["IMAP4REV1", "NAMESPACE", "UTF8=ACCEPT"]
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
                    mailboxes: vec![mailbox("INBOX")],
                },
                commands: vec![],
            });
            store.save_state(&mut saved).unwrap();
        }
        let p = saved.synthetic_folders.as_ref().unwrap();
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
            account: id.clone(),
            address: p.address,
            tls_name: "localhost".into(),
            root: p.root.clone(),
            epoch: p.epoch.clone(),
            command_timeout: crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT,
            operation_timeout: Duration::from_secs(60),
        };
        drop(store);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = connections.clone();
        let root = root.to_owned();
        let busy = BUSY
            .lock()
            .unwrap()
            .entry(root.clone())
            .or_default()
            .clone();
        let serving = busy.clone();
        let fixture_root = root.clone();
        let id = id.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((tcp, _)) => {
                        let _serving = Serving::start(&serving);
                        accepted.fetch_add(1, Ordering::SeqCst);
                        let served = Peer::new(tcp, stopping.clone())
                            .and_then(|tcp| serve(tcp, config.clone(), &repo, &id, &root));
                        if let Err(error) = served {
                            eprintln!("folder.fixture.serve.io_kind={:?}", error.kind());
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            connections,
            source,
            root: fixture_root,
            busy,
            stop,
            worker: Some(worker),
        }
    }
    pub(crate) fn install(&self, repo: &mut Accounts) {
        let s = &self.source;
        repo.folder_source = Some(Arc::new(InstalledSource {
            account: s.account.clone(),
            address: s.address,
            tls_name: s.tls_name.clone(),
            root: s.root.clone(),
            epoch: s.epoch.clone(),
            command_timeout: s.command_timeout,
            operation_timeout: s.operation_timeout,
        }));
    }
}
fn send(io: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    for chunk in bytes.chunks(3) {
        io.write_all(chunk)?;
        io.flush()?;
    }
    Ok(())
}
fn unquote(value: &str) -> std::io::Result<String> {
    let value = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let c = chars.next().ok_or(std::io::ErrorKind::InvalidData)?;
            if !matches!(c, '\\' | '"') {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            result.push(c);
        } else {
            result.push(c);
        }
    }
    Ok(result)
}
fn serve(
    mut tcp: Peer,
    config: Arc<ServerConfig>,
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
) -> std::io::Result<()> {
    tcp.tcp.set_nodelay(true)?;
    if std::fs::read_to_string(root.join("folder-mode")).is_ok_and(|s| s == "stall_tls") {
        Peer::until_closed(&mut tcp);
        return Ok(());
    }
    let mut io = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        tcp,
    ));
    send(io.get_mut(), b"* OK synthetic IMAP\r\n")?;
    let mut namespaces = 0;
    let mut authenticated = false;
    let mut last = LastSaved::default();
    loop {
        let mut line = String::new();
        if io.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.len() > 16384 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let (tag, command) = line
            .trim_end()
            .split_once(' ')
            .ok_or(std::io::ErrorKind::InvalidData)?;
        let (kind, args) = command.split_once(' ').unwrap_or((command, ""));
        if ![
            "CAPABILITY",
            "LOGIN",
            "NAMESPACE",
            "LIST",
            "CREATE",
            "LOGOUT",
        ]
        .contains(&kind)
        {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mode = std::fs::read_to_string(root.join("folder-mode")).unwrap_or_default();
        let created = (kind == "CREATE").then(|| -> std::io::Result<ExactName> {
            ExactName::from_wire(&unquote(args)?)
                .map_err(|_| std::io::ErrorKind::InvalidData.into())
        });
        let creates = matches!(created, Some(Ok(_)))
            && !matches!(
                mode.as_str(),
                "quota" | "no" | "bad" | "bye" | "eof" | "unknown"
            );
        // One store transaction per command: the record, and for CREATE the
        // effect with it, so the fixture's cost does not multiply per command.
        let store = repo
            .fixture_sieve_store(id, root)
            .unwrap_or_else(|error| panic!("folder.fixture.store_open={error:?}"));
        let mut state = last.load(&store);
        let p = state.synthetic_folders.as_mut().unwrap();
        if p.commands.len() >= 4096 {
            return Ok(());
        }
        p.commands.push(kind.into());
        let mut effect_store = None;
        if creates {
            effect_store = Some(store);
        } else {
            last.save(&store, &mut state);
            drop(store);
        }
        let mut response = format!("{tag} OK done\r\n").into_bytes();
        match kind {
            "LOGIN" => {
                authenticated = true;
                if mode == "auth" {
                    response = format!("{tag} NO diagnostic suppressed\r\n").into_bytes();
                }
            }
            "CAPABILITY" => {
                response = format!(
                    "* CAPABILITY {}\r\n{tag} OK\r\n",
                    match mode.as_str() {
                        "rev2" => "IMAP4rev2 NAMESPACE",
                        "utf8only" => "IMAP4rev1 NAMESPACE UTF8=ONLY",
                        "no_namespace" => "IMAP4rev1",
                        _ => "IMAP4rev1 NAMESPACE UTF8=ACCEPT",
                    }
                )
                .into_bytes();
            }
            "NAMESPACE" => {
                namespaces += 1;
                let ns = match mode.as_str() {
                    "nil" => "NIL NIL NIL",
                    "nil_delimiter" => "((\"\" NIL)) NIL NIL",
                    "multiple" => "((\"\" \"/\")(\"Other/\" \"/\")) NIL NIL",
                    "prefix" => "((\"INBOX/\" \"/\")) NIL NIL",
                    "dot" => "((\"\" \".\")) NIL NIL",
                    "extension" => "((\"\" \"/\" \"X\" (\"value\"))) NIL NIL",
                    "changing" if namespaces % 2 == 0 => "((\"INBOX/\" \"/\")) NIL NIL",
                    _ => "((\"\" \"/\")) NIL NIL",
                };
                response = format!("* NAMESPACE {ns}\r\n{tag} OK\r\n").into_bytes();
            }
            "LIST" => {
                if args != "\"\" \"*\"" {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                response.clear();
                for m in &state
                    .synthetic_folders
                    .as_ref()
                    .unwrap()
                    .inventory
                    .mailboxes
                {
                    let attrs = m.attributes.iter().cloned().collect::<Vec<_>>().join(" ");
                    response.extend_from_slice(
                        format!(
                            "* LIST ({attrs}) \"/\" {{{}}}\r\n{}\r\n",
                            m.name.wire.len(),
                            m.name.wire
                        )
                        .as_bytes(),
                    );
                }
                if mode == "malformed_name" {
                    response.extend_from_slice(b"* LIST () \"/\" \"&AGE-\"\r\n");
                }
                if mode == "case_alias" {
                    response.extend_from_slice(b"* LIST () \"/\" \"inbox\"\r\n");
                }
                if mode == "list_dot" {
                    response.extend_from_slice(b"* LIST () \".\" \"Other\"\r\n");
                }
                if mode == "list_no" {
                    response.extend_from_slice(format!("{tag} NO incomplete\r\n").as_bytes());
                } else {
                    response.extend_from_slice(format!("{tag} OK\r\n").as_bytes());
                }
                if mode == "drip" {
                    for b in response {
                        io.get_mut().write_all(&[b])?;
                        io.get_mut().flush()?;
                        thread::sleep(Duration::from_millis(100));
                    }
                    continue;
                }
            }
            "CREATE" => {
                let name: ExactName = created.unwrap()?;
                if matches!(
                    mode.as_str(),
                    "quota" | "no" | "bad" | "bye" | "eof" | "unknown"
                ) {
                    response = match mode.as_str() {
                        "quota" => format!("{tag} NO [OVERQUOTA] unavailable\r\n"),
                        "no" => format!("{tag} NO Abgelehnt (Text {{ und \" ä\r\n"),
                        "bad" => format!("{tag} BAD rejected\r\n"),
                        "bye" => "* BYE closed\r\n".into(),
                        "eof" => return Ok(()),
                        _ => format!("{tag} OK [X-UNKNOWN] uncertain\r\n"),
                    }
                    .into_bytes();
                } else {
                    let store = effect_store.take().unwrap();
                    let p = state.synthetic_folders.as_mut().unwrap();
                    if p.inventory.get(&name.decoded).is_some() {
                        last.save(&store, &mut state);
                        response = format!("{tag} NO already exists\r\n").into_bytes();
                    } else {
                        let parts: Vec<_> = name.decoded.split('/').collect();
                        for i in 1..=parts.len() {
                            let decoded = parts[..i].join("/");
                            if p.inventory.get(&decoded).is_none() {
                                p.inventory.mailboxes.push(mailbox(&decoded));
                            }
                        }
                        if mode == "noinferiors_new" {
                            p.inventory
                                .mailboxes
                                .iter_mut()
                                .find(|m| m.name == name)
                                .unwrap()
                                .attributes
                                .insert("\\NOINFERIORS".into());
                        }
                        p.inventory
                            .mailboxes
                            .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
                        last.save(&store, &mut state);
                        drop(store);
                        boundary("folder_provider_effect");
                        if mode == "lost" {
                            return Ok(());
                        }
                    }
                }
            }
            "LOGOUT" => {
                send(io.get_mut(), format!("* BYE\r\n{tag} OK\r\n").as_bytes())?;
                return Ok(());
            }
            _ => unreachable!(),
        }
        send(io.get_mut(), &response)?;
        if mode == "stall_write" && kind == "CAPABILITY" && authenticated {
            use std::os::fd::AsRawFd;
            let size: libc::c_int = 4096;
            assert_eq!(
                unsafe {
                    libc::setsockopt(
                        io.get_ref().sock.tcp.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_RCVBUF,
                        (&size as *const libc::c_int).cast(),
                        std::mem::size_of_val(&size) as libc::socklen_t,
                    )
                },
                0
            );
            std::fs::write(root.join("write-stalled"), b"ready")?;
            while !root.join("release-write").exists() && !io.get_ref().sock.stopped() {
                thread::sleep(Duration::from_millis(5));
            }
            return Ok(());
        }
    }
}
pub(crate) fn install(root: &Path) -> (Accounts, Account, Fixture) {
    let repo = persistent_test_accounts(root);
    let (mut repo, account) = if repo.list().unwrap().is_empty() {
        let (repo, account, fake) = crate::acquisition::sieve_transaction::tests::install(root);
        crate::acquisition::sieve_transaction::tests::commit_fixture_transaction(&repo, &account);
        drop(fake);
        (repo, account)
    } else {
        let account = repo.list().unwrap().remove(0);
        (repo, account)
    };
    repo.sieve_source = None;
    repo.sieve_write = None;
    let fake = Fixture::start(root, &account.id);
    fake.install(&mut repo);
    (repo, account, fake)
}
fn digest(value: &impl serde::Serialize) -> String {
    crate::unsubscribe::digest(&serde_json::to_vec(value).unwrap())
}
fn unrelated(value: &AccountState) -> String {
    let mut v = serde_json::to_value(value).unwrap();
    for f in ["folder_provisioning", "revision"] {
        v.as_object_mut().unwrap().remove(f);
    }
    digest(&v)
}
fn populated(state: &AccountState) -> serde_json::Value {
    serde_json::json!({"census":!state.census.folders.is_empty(),"backlog":!state.backlog.is_empty(),"exports":!state.exports.is_empty(),"proposals":!state.proposals.history.is_empty(),"ledger":state.ledger.document.as_ref().is_some_and(|d| !d.entries.is_empty()),"invoices":!state.invoices.sends.is_empty(),"unsubscribe":!state.unsubscribe.history.is_empty(),"winddown":!state.winddown.history.is_empty(),"sieve":!state.sieve_transactions.history.is_empty(),"mailbox_effects":!state.synthetic_effects.is_empty(),"send_effects":!state.invoices.synthetic_received.is_empty(),"unsubscribe_effects":!state.unsubscribe.synthetic_acknowledgments.is_empty()})
}
#[test]
fn saved_workspace_omits_every_filing_action_and_reopen_never_creates() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, fake) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let before = unrelated(&saved);
    let p = repo
        .folder_preview(&account, &store, &saved, Cancellation::default())
        .unwrap_or_else(|e| {
            let (_, p) = provider(&repo, &account.id, root.path());
            panic!(
                "preview {e:?} safe commands {:?}",
                p.synthetic_folders.unwrap().commands
            )
        });
    assert!(
        p.operation.plan.entries.is_empty(),
        "Saved-account recipient authority remains conservative"
    );
    repo.folder_confirm(
        &account,
        &store,
        &mut saved,
        &p.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        saved.folder_provisioning.history[0].status,
        Status::Complete
    );
    assert!(
        saved.folder_provisioning.history[0]
            .steps
            .iter()
            .all(|s| s.acknowledgment && s.outcome == Outcome::Observed)
    );
    assert_eq!(unrelated(&saved), before);
    let (_, external) = provider(&repo, &account.id, root.path());
    let topology = external
        .synthetic_folders
        .as_ref()
        .unwrap()
        .inventory
        .clone();
    let commands = external
        .synthetic_folders
        .as_ref()
        .unwrap()
        .commands
        .clone();
    assert_eq!(topology.mailboxes.len(), 1);
    assert!(!commands.contains(&"CREATE".into()));
    let source = fake.source.identity();
    drop(store);
    drop(fake);
    let (repo, account, fake) = install(root.path());
    assert_eq!(fake.source.identity(), source);
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    assert_eq!(
        saved.folder_provisioning.history[0].status,
        Status::Complete
    );
    repo.folder_reconcile(
        &account,
        &store,
        &mut saved,
        &p.operation.id,
        Cancellation::default(),
    )
    .unwrap();
    let (_, external) = provider(&repo, &account.id, root.path());
    assert_eq!(
        external.synthetic_folders.as_ref().unwrap().inventory,
        topology
    );
    assert_eq!(
        external
            .synthetic_folders
            .as_ref()
            .unwrap()
            .commands
            .iter()
            .filter(|c| *c == "CREATE")
            .count(),
        commands.iter().filter(|c| *c == "CREATE").count()
    );
    let fresh = repo
        .folder_preview(&account, &store, &saved, Cancellation::default())
        .unwrap();
    assert!(
        fresh
            .operation
            .plan
            .entries
            .iter()
            .all(|e| e.existing.is_some())
    );
    repo.folder_confirm(
        &account,
        &store,
        &mut saved,
        &fresh.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(saved.folder_provisioning.history[1].steps.len(), 0);
    assert_eq!(unrelated(&saved), before);
}
#[test]
fn folder_fixture_worker() {
    let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let repo = persistent_test_accounts(root);
    let account = repo.list().unwrap().remove(0);
    let (external, mut saved) = provider(&repo, &account.id, root);
    let action = std::env::var("MAILROOM_FOLDERS_ACTION").unwrap_or_default();
    let p = saved.synthetic_folders.as_mut().unwrap();
    if action == "provider_only" {
        let value = serde_json::json!({"topology":digest(&p.inventory),"commands":p.commands});
        std::fs::write(
            root.join("folder-provider-observations.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        return;
    }
    match action.as_str() {
        "external" => {
            p.inventory.mailboxes.push(mailbox("External preserved"));
            p.inventory
                .mailboxes
                .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
        }
        "epoch" => p.epoch.push_str("-changed"),
        "delete_parent" => {
            let parent = p
                .inventory
                .mailboxes
                .iter()
                .find(|m| m.name.decoded != "INBOX")
                .map(|m| m.name.decoded.clone());
            if let Some(parent) = parent {
                p.inventory.mailboxes.retain(|m| m.name.decoded != parent);
            }
        }
        _ => (),
    }
    if !action.is_empty() {
        external.save_state(&mut saved).unwrap();
        return;
    }
    let p = saved.synthetic_folders.as_ref().unwrap();
    let topology = digest(&p.inventory);
    let source = digest(&(&p.epoch, p.address, &p.root));
    let commands = p.commands.clone();
    saved.synthetic_folders = None;
    saved.revision = 0;
    let external_unrelated = digest(&saved);
    drop(external);
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    let effects = crate::acquisition::sieve_transaction::tests::independent_effects(root, &store);
    let populated = populated(&state);
    let value = serde_json::json!({"populated":populated,"independent_effects":effects,"account":account.id,"schema":store.schema_version().unwrap(),"revision":state.revision,"journal":state.folder_provisioning,"topology":topology,"source":source,"commands":commands,"unrelated":unrelated(&state),"external_unrelated":external_unrelated});
    std::fs::write(
        root.join("folder-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

fn synthetic_operation(
    repo: &Accounts,
    account: &Account,
    store: &EncryptedStore,
    saved: &mut AccountState,
) -> String {
    let mut session = repo
        .folder_session(account, Cancellation::default())
        .unwrap();
    let before = session.observe().unwrap();
    session.logout();
    let mut input: crate::engine::PreviewInput =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    let (_, mut overrides) = crate::rules::config::documents(
        &input.documents.preset_yaml,
        &input.documents.override_yaml,
    )
    .unwrap();
    overrides.layout.personal_root = "Grüße & \"😀*%".into();
    overrides.layout.business_root = "Arbeit\\Ü".into();
    input.documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    let artifact = crate::engine::artifact(&input.documents, &input.context).unwrap();
    let plan = FolderPlan::build(artifact.folders(), &before).unwrap();
    assert!(!plan.entries.is_empty());
    let id = store.keys().binding_for(
        b"synthetic-folder-journal-test",
        &serde_json::to_vec(&plan).unwrap(),
    );
    saved.folder_provisioning.history.push(Operation {
        id: id.clone(),
        context: repo.folder_context(account, store).unwrap(),
        artifact: artifact.identity().into(),
        proof_binding: "a".repeat(64),
        receipt: "b".repeat(64),
        before,
        plan,
        status: Status::Running,
        steps: vec![],
    });
    store.save_state(saved).unwrap();
    id
}
#[test]
fn actual_tls_create_journal_with_explicit_synthetic_artifact_preserves_independent_reopen() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, fake) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    let initial = unrelated(&saved);
    synthetic_operation(&repo, &account, &store, &mut saved);
    repo.folder_run(
        &account,
        &store,
        &mut saved,
        0,
        Cancellation::default(),
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        saved.folder_provisioning.history[0].status,
        Status::Complete
    );
    assert!(
        saved.folder_provisioning.history[0]
            .steps
            .iter()
            .all(|s| s.outcome == Outcome::Observed && s.acknowledgment)
    );
    assert_eq!(unrelated(&saved), initial);
    let (_, external) = provider(&repo, &account.id, root.path());
    let p = external.synthetic_folders.as_ref().unwrap();
    let topology = p.inventory.clone();
    let commands = p.commands.clone();
    assert!(commands.contains(&"CREATE".into()));
    assert!(topology.mailboxes.len() > 2);
    assert_eq!(
        topology.get("Grüße & \"😀*%").unwrap().name.wire,
        "Gr&APwA3w-e &- \"&2D3eAA-*%"
    );
    let authority = fake.source.identity();
    drop(store);
    drop(fake);
    let (repo, account, fake) = install(root.path());
    assert_eq!(fake.source.identity(), authority);
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert_eq!(
        saved.folder_provisioning.history[0].status,
        Status::Complete
    );
    let (_, external) = provider(&repo, &account.id, root.path());
    let p = external.synthetic_folders.as_ref().unwrap();
    assert_eq!(p.inventory, topology);
    assert_eq!(p.commands, commands);
    assert_eq!(unrelated(&saved), initial);
}
#[test]
fn canonical_names_and_structural_plan_reject_aliases_and_nonselectable_leaves() {
    let cases = [
        ("Grüße", "Gr&APwA3w-e"),
        ("A&B", "A&-B"),
        ("😀", "&2D3eAA-"),
        ("quote\"\\*%", "quote\"\\*%"),
    ];
    for (decoded, wire) in cases {
        let n = ExactName::new(decoded).unwrap();
        assert_eq!(n.wire, wire);
        assert_eq!(ExactName::from_wire(wire).unwrap(), n);
    }
    for bad in ["&", "&AGE-", "&2AA-", "&APw=-", "&APw-&AN8-", "\u{00fc}"] {
        assert!(
            ExactName::from_wire(bad).is_err(),
            "Malformed or noncanonical wire must reject"
        );
    }
    assert!(ExactName::new("a\u{308}").is_err());
    assert!(ExactName::new("\u{212b}").is_err());
    let inventory = Inventory {
        capabilities: ["IMAP4REV1".into(), "NAMESPACE".into()].into(),
        namespaces: Namespaces {
            personal: vec![Namespace {
                prefix: "".into(),
                delimiter: Some("/".into()),
            }],
            other: vec![],
            shared: vec![],
        },
        mailboxes: vec![],
    };
    let leaves = ["Grüße/Quotes\"\\😀*%".into(), "Other/Leaf".into()].into();
    let p = FolderPlan::build(&leaves, &inventory).unwrap();
    assert_eq!(p.entries.len(), 4);
    assert!(
        p.entries[..2]
            .iter()
            .all(|e| e.role == model::Role::Ancestor)
    );
    let mut existing = inventory.clone();
    let mut parent = mailbox("Grüße");
    parent.attributes.insert("\\NOSELECT".into());
    existing.mailboxes.push(parent);
    assert!(FolderPlan::build(&leaves, &existing).is_ok());
    existing.mailboxes[0]
        .attributes
        .insert("\\NOINFERIORS".into());
    assert!(FolderPlan::build(&leaves, &existing).is_err());
    existing.mailboxes[0] = mailbox("Grüße/Quotes\"\\😀*%");
    existing.mailboxes[0].attributes.insert("\\NOSELECT".into());
    assert!(FolderPlan::build(&leaves, &existing).is_err());
    existing.mailboxes[0] = mailbox("other/leaf");
    assert!(FolderPlan::build(&leaves, &existing).is_err());
    assert!(FolderPlan::build(&["INBOX/Child".into()].into(), &inventory).is_err());
}
#[test]
fn namespace_encoding_and_tls_failures_never_create_or_retry_auth() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, mut fake) = install(root.path());
    let source = &mut fake.source;
    source.command_timeout = Duration::from_millis(250);
    let password = SecretValue::new(b"invented".to_vec());
    for mode in [
        "nil",
        "nil_delimiter",
        "multiple",
        "prefix",
        "dot",
        "extension",
        "changing",
        "rev2",
        "utf8only",
        "no_namespace",
        "malformed_name",
        "case_alias",
        "list_dot",
        "list_no",
        "auth",
        "drip",
    ] {
        std::fs::write(root.path().join("folder-mode"), mode).unwrap();
        let result = Session::connect(
            &source.permit(&account.id).unwrap(),
            &account.login,
            &password,
            Cancellation::default(),
        )
        .and_then(|mut s| s.observe());
        assert!(
            result.is_err(),
            "Unsupported or incomplete protocol must reject"
        );
    }
    std::fs::write(root.path().join("folder-mode"), "").unwrap();
    let (_, before) = provider(&repo, &account.id, root.path());
    let commands = before.synthetic_folders.as_ref().unwrap().commands.clone();
    source.tls_name = "wrong.invalid".into();
    assert!(
        Session::connect(
            &source.permit(&account.id).unwrap(),
            &account.login,
            &password,
            Cancellation::default()
        )
        .is_err()
    );
    source.tls_name = "localhost".into();
    source.root = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .unwrap()
        .cert
        .der()
        .to_vec();
    assert!(
        Session::connect(
            &source.permit(&account.id).unwrap(),
            &account.login,
            &password,
            Cancellation::default()
        )
        .is_err()
    );
    let (_, after) = provider(&repo, &account.id, root.path());
    let observed = &after.synthetic_folders.as_ref().unwrap().commands;
    assert_eq!(
        *observed, commands,
        "No IMAP credentials precede verified TLS"
    );
    assert!(!observed.contains(&"CREATE".into()));
}

#[test]
fn synthetic_folder_crash_worker() {
    let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let mut repo = persistent_test_accounts(root);
    let account = repo.list().unwrap().remove(0);
    let (external, saved) = provider(&repo, &account.id, root);
    let p = saved.synthetic_folders.as_ref().unwrap();
    repo.folder_source = Some(Arc::new(InstalledSource {
        account: account.id.clone(),
        address: p.address,
        tls_name: "localhost".into(),
        root: p.root.clone(),
        epoch: p.epoch.clone(),
        command_timeout: crate::acquisition::fixture_peer::FIXTURE_COMMAND_TIMEOUT,
        operation_timeout: Duration::from_secs(60),
    }));
    drop(external);
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    synthetic_operation(&repo, &account, &store, &mut saved);
    boundary("folder_consumed");
    let index = saved.folder_provisioning.history.len() - 1;
    let _ = repo.folder_run(
        &account,
        &store,
        &mut saved,
        index,
        Cancellation::default(),
        &|_| Ok(()),
    );
}
#[test]
fn stale_proof_source_password_and_key_never_authorize_folder_commands() {
    for change in ["state", "pin", "source", "password", "key", "login"] {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, mut account, mut fake) = install(root.path());
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        let p = repo
            .folder_preview(&account, &store, &saved, Cancellation::default())
            .unwrap();
        match change {
            "state" => {
                saved
                    .winddown
                    .dismissed
                    .insert("invented-new-owner-decision".into());
                store.save_state(&mut saved).unwrap();
            }
            "pin" => {
                saved.workspace.as_mut().unwrap().input.pins[0].expected =
                    crate::rules::proof::Destination::Folder {
                        name: "Changed".into(),
                    };
                store.save_state(&mut saved).unwrap();
            }
            "source" => {
                fake.source.epoch.push_str("-changed");
                fake.install(&mut repo);
            }
            _ => (),
        }
        drop(store);
        if change == "login" {
            account.login = "changed-login@example.test".into();
            std::fs::write(
                root.path()
                    .join("config")
                    .join(format!("{}.json", account.id.as_str())),
                serde_json::to_vec(&account).unwrap(),
            )
            .unwrap();
        }
        if change == "password" {
            repo.replace_password(&account.id, &SecretValue::new(b"replacement".to_vec()))
                .unwrap();
            account = repo.load(&account.id).unwrap();
        }
        if change == "key" {
            repo.rotate(&account.id, &root.path().join("mailboxes"))
                .unwrap();
            account = repo.load(&account.id).unwrap();
        }
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        assert!(
            repo.folder_confirm(
                &account,
                &store,
                &mut saved,
                &p.receipt,
                Cancellation::default(),
                |_| Ok(())
            )
            .is_err()
        );
        assert!(saved.folder_provisioning.history.is_empty());
        let (_, state) = provider(&repo, &account.id, root.path());
        assert!(
            !state
                .synthetic_folders
                .unwrap()
                .commands
                .contains(&"CREATE".into())
        );
    }
}
#[test]
fn partial_create_refusal_lost_ack_reconciliation_and_cancel_preserve_effects() {
    for mode in ["quota", "no", "bad", "bye", "eof", "unknown", "lost"] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fake) = install(root.path());
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        let initial = unrelated(&saved);
        let id = synthetic_operation(&repo, &account, &store, &mut saved);
        std::fs::write(root.path().join("folder-mode"), mode).unwrap();
        assert!(
            repo.folder_run(
                &account,
                &store,
                &mut saved,
                0,
                Cancellation::default(),
                &|_| Ok(())
            )
            .is_err()
        );
        drop(store);
        std::fs::write(root.path().join("folder-mode"), "").unwrap();
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        assert!(matches!(
            saved.folder_provisioning.history[0].status,
            Status::Uncertain | Status::Refused
        ));
        repo.folder_reconcile(&account, &store, &mut saved, &id, Cancellation::default())
            .unwrap();
        assert_eq!(
            saved.folder_provisioning.history[0].status,
            Status::FreshReview
        );
        let step = &saved.folder_provisioning.history[0].steps[0];
        assert_eq!(
            step.outcome,
            if mode == "lost" {
                Outcome::Observed
            } else {
                Outcome::Absent
            }
        );
        assert!(!step.acknowledgment);
        let revision = saved.revision;
        let (_, before) = provider(&repo, &account.id, root.path());
        let commands = before.synthetic_folders.unwrap().commands;
        repo.folder_reconcile(&account, &store, &mut saved, &id, Cancellation::default())
            .unwrap();
        assert_eq!(saved.revision, revision);
        let (_, after) = provider(&repo, &account.id, root.path());
        assert_eq!(
            after
                .synthetic_folders
                .unwrap()
                .commands
                .iter()
                .filter(|c| *c == "CREATE")
                .count(),
            commands.iter().filter(|c| *c == "CREATE").count()
        );
        assert_eq!(unrelated(&saved), initial);
    }
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    synthetic_operation(&repo, &account, &store, &mut saved);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        repo.folder_run(&account, &store, &mut saved, 0, cancel, &|_| Ok(()))
            .is_err()
    );
    assert!(saved.folder_provisioning.history[0].steps.is_empty());
}

pub(crate) fn commit_saved_workspace_folders(repo: &mut Accounts, account: &Account, root: &Path) {
    repo.install_synthetic_workspace(account, "explicit-recovery-preservation");
    let store = repo.store(account).unwrap();
    let mut saved = store.load_state().unwrap();
    let ledger = crate::ledger::InstalledLedger::install(root.join("ledger")).unwrap();
    let guard = ledger.lock().unwrap();
    let original = guard.read().unwrap();
    let mut document = original.document;
    document.enabled = true;
    document.entries.push(serde_json::from_value(serde_json::json!({
        "address":"retained@example.test", "service":"Invented retained service",
        "folder":"Aufbewahrt", "first_seen":"synthetic historical value",
        "status":"active", "note":"Synthetic preservation evidence",
        "association":{"sender_domains":["sender.example"],"approved_evidence":"Explicit synthetic association"},
        "owner_extension":{"preserve":["one",2]}
    })).unwrap());
    let snapshot = guard.replace(&original.content_version, &document).unwrap();
    let context = repo.workspace_context(account, &store).unwrap();
    crate::ledger::reconcile(&mut saved, &snapshot, &context).unwrap();
    store.save_state(&mut saved).unwrap();
    let proof = crate::rules::workspace::preview(&saved, &context, store.keys()).unwrap();
    crate::rules::workspace::confirm_export(&mut saved, &context, store.keys(), &proof.receipt)
        .unwrap();
    store.save_state(&mut saved).unwrap();
    assert!(
        populated(&saved)
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == true)
    );
    let initial = unrelated(&saved);
    let independent =
        crate::acquisition::sieve_transaction::tests::independent_effects(root, &store);
    let external_unrelated = || {
        let (_, mut state) = provider(repo, &account.id, root);
        state.synthetic_folders = None;
        state.revision = 0;
        digest(&state)
    };
    let external = external_unrelated();
    let preview = repo
        .folder_preview(account, &store, &saved, Cancellation::default())
        .unwrap();
    assert!(!preview.operation.plan.logical_leaves.is_empty());
    repo.folder_confirm(
        account,
        &store,
        &mut saved,
        &preview.receipt,
        Cancellation::default(),
        |state| {
            if guard.read()?.content_version != state.ledger.source_version {
                return Err("Synthetic ledger changed".into());
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        saved.folder_provisioning.history.last().unwrap().status,
        Status::Complete
    );
    assert_eq!(unrelated(&saved), initial);
    assert_eq!(external_unrelated(), external);
    assert_eq!(
        crate::acquisition::sieve_transaction::tests::independent_effects(root, &store),
        independent
    );
}
pub(crate) fn later_folder_change(repo: &Accounts, account: &Account, root: &Path) {
    let (store, mut state) = provider(repo, &account.id, root);
    let p = state.synthetic_folders.as_mut().unwrap();
    p.inventory
        .mailboxes
        .push(mailbox("Independent post-capture folder"));
    p.inventory
        .mailboxes
        .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
    store.save_state(&mut state).unwrap();
}

#[test]
fn expired_certificate_and_stalled_tls_obey_verification_cancellation_and_deadline() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("folder-mode"), "expired_cert").unwrap();
    let (repo, account, fake) = install(root.path());
    let result = Session::connect(
        &fake.source.permit(&account.id).unwrap(),
        &account.login,
        &SecretValue::new(b"invented".to_vec()),
        Cancellation::default(),
    );
    if let Err(failure) = &result {
        report_failure(*failure);
    }
    assert!(matches!(result, Err(Failure::Tls)));
    let (_, state) = provider(&repo, &account.id, root.path());
    assert!(state.synthetic_folders.unwrap().commands.is_empty());
    drop(fake);
    let root = tempfile::tempdir().unwrap();
    let (repo, account, mut fake) = install(root.path());
    std::fs::write(root.path().join("folder-mode"), "stall_tls").unwrap();
    fake.source.command_timeout = Duration::from_millis(100);
    let result = Session::connect(
        &fake.source.permit(&account.id).unwrap(),
        &account.login,
        &SecretValue::new(b"invented".to_vec()),
        Cancellation::default(),
    );
    assert!(matches!(result, Err(Failure::Deadline)));
    let cancel = Cancellation::default();
    let stop = cancel.clone();
    let thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        stop.cancel();
    });
    let result = Session::connect(
        &fake.source.permit(&account.id).unwrap(),
        &account.login,
        &SecretValue::new(b"invented".to_vec()),
        cancel,
    );
    assert!(matches!(result, Err(Failure::Cancelled)));
    thread.join().unwrap();
    let (_, state) = provider(&repo, &account.id, root.path());
    assert!(state.synthetic_folders.unwrap().commands.is_empty());
}
#[test]
fn encrypted_rotation_and_exact_forgetting_own_folder_state_without_a_new_resource() {
    let root = tempfile::tempdir().unwrap();
    let (mut repo, account, fake) = install(root.path());
    commit_saved_workspace_folders(&mut repo, &account, root.path());
    drop(fake);
    let before = repo.store(&account).unwrap().load_state().unwrap();
    let (_, external) = provider(&repo, &account.id, root.path());
    let original = external.synthetic_folders.as_ref().unwrap();
    let topology = original.inventory.clone();
    let source = digest(&(&original.epoch, original.address, &original.root));
    repo.rotate(&account.id, &root.path().join("mailboxes"))
        .unwrap();
    let current = repo.load(&account.id).unwrap();
    let after = repo.store(&current).unwrap().load_state().unwrap();
    assert!(after.folder_provisioning == before.folder_provisioning);
    assert_eq!(unrelated(&after), unrelated(&before));
    let (_, external) = provider(&repo, &account.id, root.path());
    let p = external.synthetic_folders.as_ref().unwrap();
    assert_eq!(p.inventory, topology);
    assert_eq!(digest(&(&p.epoch, p.address, &p.root)), source);
    let marker = root.path().join("mailboxes/unrelated-preserve");
    std::fs::write(&marker, b"unrelated").unwrap();
    let offer = repo
        .forget_preview(&current.id, &root.path().join("mailboxes"))
        .unwrap();
    repo.forget(&current.id, &root.path().join("mailboxes"), &offer, true)
        .unwrap();
    assert!(repo.load(&current.id).is_err());
    assert_eq!(std::fs::read(marker).unwrap(), b"unrelated");
}

#[test]
fn newly_observed_noinferiors_stops_before_any_further_create() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = install(root.path());
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    synthetic_operation(&repo, &account, &store, &mut saved);
    std::fs::write(root.path().join("folder-mode"), "noinferiors_new").unwrap();
    assert!(
        repo.folder_run(
            &account,
            &store,
            &mut saved,
            0,
            Cancellation::default(),
            &|_| Ok(())
        )
        .is_err()
    );
    assert_eq!(
        saved.folder_provisioning.history[0].status,
        Status::Conflict
    );
    let (_, external) = provider(&repo, &account.id, root.path());
    assert_eq!(
        external
            .synthetic_folders
            .unwrap()
            .commands
            .iter()
            .filter(|c| *c == "CREATE")
            .count(),
        1
    );
}

#[cfg(test)]
pub(super) fn progress_evidence(state: &AccountState) {
    if let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") {
        let value = serde_json::json!({"revision":state.revision,"unrelated":unrelated(state),"journal":state.folder_provisioning});
        std::fs::write(
            Path::new(&root).join("folder-progress.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
}
