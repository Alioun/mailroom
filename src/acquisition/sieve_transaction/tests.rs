use super::*;
use crate::{
    acquisition::{
        Provider, SecretValue,
        fixture_peer::{FIXTURE_COMMAND_TIMEOUT, LastSaved, Peer},
        managesieve::{FixtureOwnership, InstalledSource},
        persistent_test_accounts,
    },
    census::{AccountId, sieve_transaction::FakeProvider},
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub(crate) const OX_HOST: &[u8] = b"require [\"fileinto\"];\r\n# OK\r\n# \xc3\xa4 {9} <img src='https://remote.invalid/x'>\r\n## Flag: |UniqueId:0|Rulename: Travel\r\nif address :is \"from\" \"robot@travel.example\"\r\n{\r\n    fileinto \"Reisen\";\r\n    stop;\r\n}\r\n";

pub(crate) struct Fixture {
    pub(in crate::acquisition) source: InstalledSource,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
fn provider(repo: &Accounts, id: &AccountId, root: &Path) -> (EncryptedStore, AccountState) {
    let store = repo.fixture_sieve_store(id, root).unwrap();
    let state = store.load_state().unwrap();
    (store, state)
}
impl Fixture {
    pub(crate) fn start(root: &Path, id: &AccountId, active: bool) -> Self {
        let repo = persistent_test_accounts(root);
        let journal = repo.store(&repo.load(id).unwrap()).unwrap();
        let store = repo.fixture_sieve_store(id, root).unwrap();
        if !store.path().exists() {
            store.save(&journal.load().unwrap()).unwrap();
        }
        drop(journal);
        let mut state = store.load_state().unwrap();
        let listener = if let Some(p) = &state.synthetic_sieve {
            TcpListener::bind(p.address).unwrap()
        } else {
            TcpListener::bind("127.0.0.1:0").unwrap()
        };
        listener.set_nonblocking(true).unwrap();
        if state.synthetic_sieve.is_none() {
            let key = rcgen::KeyPair::generate().unwrap();
            let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .self_signed(&key)
                .unwrap();
            let mut caps = std::collections::BTreeMap::new();
            for (k, v) in [
                ("VERSION", "1.0"),
                ("IMPLEMENTATION", "durable fixture"),
                ("SASL", "PLAIN"),
                ("SIEVE", "fileinto imap4flags"),
            ] {
                caps.insert(k.into(), v.into());
            }
            state.synthetic_sieve = Some(FakeProvider {
                address: listener.local_addr().unwrap(),
                root: cert.der().to_vec(),
                key: key.serialize_der(),
                epoch: format!("fixture-owned-{:032x}", rand::random::<u128>()),
                image: Image {
                    capabilities: caps,
                    names: vec![
                        crate::census::sieve::ScriptName {
                            name: "Open-Xchange".into(),
                            active,
                        },
                        crate::census::sieve::ScriptName {
                            name: "foreign\"\\ä".into(),
                            active: false,
                        },
                    ],
                    bodies: vec![
                        Body::new("Open-Xchange".into(), OX_HOST.to_vec()),
                        Body::new("foreign\"\\ä".into(), b"keep;\r\n# untouched\r\n".to_vec()),
                    ],
                },
                commands: vec![],
            });
            store.save_state(&mut state).unwrap();
        }
        let p = state.synthetic_sieve.as_ref().unwrap();
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
            address: p.address,
            tls_name: "localhost".into(),
            root: p.root.clone(),
            epoch: p.epoch.clone(),
            command_timeout: FIXTURE_COMMAND_TIMEOUT,
            attempt_timeout: Duration::from_secs(60),
        };
        drop(store);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let root = root.to_owned();
        let id = id.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (tcp, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                let _ = Peer::new(tcp, stopping.clone())
                    .and_then(|tcp| serve(tcp, config.clone(), &repo, &id, &root));
            }
        });
        Self {
            source,
            stop,
            worker: Some(worker),
        }
    }
    pub(crate) fn install(&self, repo: &mut Accounts, id: &AccountId) {
        repo.sieve_source = Some(Arc::new(InstalledSource {
            address: self.source.address,
            tls_name: self.source.tls_name.clone(),
            root: self.source.root.clone(),
            epoch: self.source.epoch.clone(),
            command_timeout: self.source.command_timeout,
            attempt_timeout: self.source.attempt_timeout,
        }));
        repo.sieve_write = Some(Arc::new(FixtureOwnership {
            account: id.clone(),
            target: "Open-Xchange".into(),
            epoch: self.source.epoch.clone(),
            source_identity: self.source.identity(),
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
fn literal(io: &mut impl Read, text: &str) -> std::io::Result<(Vec<u8>, String)> {
    let n = text
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix("+}\r\n"))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n <= 1024 * 1024)
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let mut bytes = vec![0; n];
    io.read_exact(&mut bytes)?;
    let mut tail = vec![];
    loop {
        let mut b = [0];
        io.read_exact(&mut b)?;
        tail.push(b[0]);
        if tail.ends_with(b"\r\n") {
            break;
        }
        if tail.len() > 2048 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
    Ok((
        bytes,
        String::from_utf8(tail).map_err(|_| std::io::ErrorKind::InvalidData)?,
    ))
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn serve(
    mut tcp: Peer,
    config: Arc<ServerConfig>,
    repo: &Accounts,
    id: &AccountId,
    root: &Path,
) -> std::io::Result<()> {
    tcp.tcp.set_nodelay(true)?;
    send(&mut tcp,b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\n\"VERSION\" \"1.0\"\r\n\"STARTTLS\"\r\nOK\r\n")?;
    let mut plain = BufReader::new(tcp);
    let mut line = String::new();
    plain.read_line(&mut line)?;
    if line != "STARTTLS\r\n" {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    send(plain.get_mut(), b"OK\r\n")?;
    let mut io = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        plain.into_inner(),
    ));
    send(io.get_mut(),b"\"IMPLEMENTATION\" \"fake\"\r\n\"SIEVE\" \"fileinto\"\r\n\"VERSION\" \"1.0\"\r\n\"SASL\" \"PLAIN\"\r\nOK\r\n")?;
    let mut last = LastSaved::default();
    loop {
        let mut line = String::new();
        if io.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let (kind, args) = line.split_once(' ').unwrap_or((line.trim_end(), ""));
        let mode = std::fs::read_to_string(root.join("transaction-mode")).unwrap_or_default();
        if ![
            "AUTHENTICATE",
            "CAPABILITY",
            "LISTSCRIPTS",
            "GETSCRIPT",
            "CHECKSCRIPT",
            "HAVESPACE",
            "PUTSCRIPT",
            "SETACTIVE",
            "LOGOUT",
        ]
        .contains(&kind)
        {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        // Read-only commands answer from the state saved with their record, so
        // each costs one store transaction like the commands that write.
        let store = repo.fixture_sieve_store(id, root).unwrap();
        let mut state = last.load(&store);
        let p = state.synthetic_sieve.as_mut().unwrap();
        if p.commands.len() >= 4096 {
            send(io.get_mut(), b"BYE\r\n")?;
            return Ok(());
        }
        p.commands.push(kind.into());
        last.save(&store, &mut state);
        drop(store);
        let mut response = b"OK (X-UNKNOWN (ONE TWO)) \"diagnostic\"\r\n".to_vec();
        match kind {
            "AUTHENTICATE" => {
                if !args.starts_with("\"PLAIN\" \"") {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
            }
            "CAPABILITY" => {
                response = vec![];
                for (k, v) in &state.synthetic_sieve.as_ref().unwrap().image.capabilities {
                    response
                        .extend_from_slice(format!("{} {}\r\n", quoted(k), quoted(v)).as_bytes());
                }
                response.extend_from_slice(b"OK\r\n");
            }
            "LISTSCRIPTS" => {
                response = vec![];
                for n in &state.synthetic_sieve.as_ref().unwrap().image.names {
                    response.extend_from_slice(
                        format!(
                            "{}{}\r\n",
                            quoted(&n.name),
                            if n.active { " ACTIVE" } else { "" }
                        )
                        .as_bytes(),
                    );
                }
                response.extend_from_slice(b"OK\r\n");
            }
            "GETSCRIPT" => {
                let (name, tail) = literal(&mut io, args)?;
                if tail != "\r\n" {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                let p = state.synthetic_sieve.as_ref().unwrap();
                if let Some(body) = p.image.bodies.iter().find(|b| b.name.as_bytes() == name) {
                    response = [
                        format!("{{{}}}\r\n", body.bytes.len()).as_bytes(),
                        &body.bytes,
                        b"\r\nOK\r\n",
                    ]
                    .concat();
                } else {
                    response = b"NO (NONEXISTENT)\r\n".to_vec();
                }
            }
            "CHECKSCRIPT" => {
                let (_, tail) = literal(&mut io, args)?;
                if tail != "\r\n" {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                response = match mode.as_str() {
                    "check_refused" => b"NO \"syntax\"\r\n".to_vec(),
                    "check_unsupported" => b"NO (UNKNOWN-COMMAND)\r\n".to_vec(),
                    "check_bye" => b"BYE\r\n".to_vec(),
                    "check_malformed" => b"OK (BROKEN\r\n".to_vec(),
                    _ => response,
                };
                if mode == "check_eof" {
                    return Ok(());
                }
                if mode == "check_stall" {
                    Peer::until_closed(&mut io);
                    return Ok(());
                }
                if mode == "check_drip" {
                    for byte in b"OK \"slow\"\r\n" {
                        io.get_mut().write_all(&[*byte])?;
                        io.get_mut().flush()?;
                        thread::sleep(Duration::from_millis(100));
                    }
                    continue;
                }
            }
            "HAVESPACE" => {
                let (_, tail) = literal(&mut io, args)?;
                if tail.trim().parse::<u32>().is_err() {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                if mode == "space_quota" {
                    response = b"NO (QUOTA/MAXSIZE)\r\n".to_vec();
                }
            }
            "PUTSCRIPT" => {
                let (name, tail) = literal(&mut io, args)?;
                let (bytes, tail) = literal(
                    &mut io,
                    tail.strip_prefix(' ')
                        .ok_or(std::io::ErrorKind::InvalidData)?,
                )?;
                if tail != "\r\n" {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                if mode == "put_quota" {
                    response = b"NO (QUOTA/MAXSIZE)\r\n".to_vec();
                } else {
                    let store = repo.fixture_sieve_store(id, root).unwrap();
                    let mut state = last.load(&store);
                    let p = state.synthetic_sieve.as_mut().unwrap();
                    let name =
                        String::from_utf8(name).map_err(|_| std::io::ErrorKind::InvalidData)?;
                    p.image.bodies.retain(|b| b.name != name);
                    p.image.bodies.push(Body::new(name.clone(), bytes));
                    if !p.image.names.iter().any(|n| n.name == name) {
                        p.image.names.push(crate::census::sieve::ScriptName {
                            name,
                            active: false,
                        });
                    }
                    p.image.names.sort_by(|a, b| a.name.cmp(&b.name));
                    p.image.bodies.sort_by(|a, b| a.name.cmp(&b.name));
                    last.save(&store, &mut state);
                    drop(store);
                    boundary("provider_upload_effect");
                    if mode == "put_lost" {
                        return Ok(());
                    }
                }
            }
            "SETACTIVE" => {
                let (name, tail) = literal(&mut io, args)?;
                if tail != "\r\n" {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                let store = repo.fixture_sieve_store(id, root).unwrap();
                let mut state = last.load(&store);
                let p = state.synthetic_sieve.as_mut().unwrap();
                if !name.is_empty() && !p.image.names.iter().any(|n| n.name.as_bytes() == name) {
                    response = b"NO (NONEXISTENT)\r\n".to_vec();
                } else {
                    for n in &mut p.image.names {
                        n.active = n.name.as_bytes() == name;
                    }
                    last.save(&store, &mut state);
                    drop(store);
                    boundary("provider_activation_effect");
                    if mode == "activation_lost" {
                        return Ok(());
                    }
                }
            }
            "LOGOUT" => {
                send(io.get_mut(), b"OK\r\n")?;
                return Ok(());
            }
            _ => unreachable!(),
        }
        send(io.get_mut(), &response)?;
    }
}

fn setup(root: &Path, active: bool) -> (Accounts, Account, Fixture) {
    let mut repo = persistent_test_accounts(root);
    let account = repo
        .create(
            Provider::MailboxOrg,
            "fixture@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    super::super::synthetic::scan(&repo, &account.id, Cancellation::default(), |_| {}).unwrap();
    let fixture = Fixture::start(root, &account.id, active);
    fixture.install(&mut repo, &account.id);
    (repo, account, fixture)
}
pub(crate) fn install(root: &Path) -> (Accounts, Account, Fixture) {
    let mut repo = persistent_test_accounts(root);
    if repo.list().unwrap().is_empty() {
        let (repo, account, fake) = setup(root, true);
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        super::super::sieve_discovery::tests::seed(&mut state, store.keys());
        store.save_state(&mut state).unwrap();
        install_independent_effects(root, &store, &state);
        drop(store);
        (repo, account, fake)
    } else {
        let account = repo.list().unwrap().remove(0);
        let fake = Fixture::start(root, &account.id, true);
        fake.install(&mut repo, &account.id);
        (repo, account, fake)
    }
}

fn install_independent_effects(root: &Path, store: &EncryptedStore, state: &AccountState) {
    let effects = root.join("mailboxes");
    let mut mailbox =
        crate::backlog::SyntheticMailbox::open(&effects, store, &state.census).unwrap();
    let folder = &mailbox.snapshot().census.folders["INBOX"];
    let message = folder.messages.values().next().unwrap().clone();
    let change = crate::backlog::Change {
        source: crate::backlog::Identity {
            folder: "INBOX".into(),
            uidvalidity: folder.uidvalidity,
            uid: message.uid,
            message_id_hash: message.headers.message_id_hash.clone().unwrap(),
            gmail_id: message.gmail_message_id.clone(),
        },
        flags: message.flags.clone(),
        original_labels: message.gmail_labels.clone(),
        destination: "Retained independent effect".into(),
        sender: "fixture sender".into(),
        operation: crate::backlog::Operation::Imap {
            method: crate::backlog::MoveMethod::UidMove,
        },
    };
    mailbox
        .apply("before-sieve-independent-move", &change, true)
        .unwrap();
    drop(mailbox);
    let mut invoice = crate::invoice::SyntheticTransport::open(&effects, store, state).unwrap();
    invoice
        .submit(
            "before-sieve-independent-send",
            &state.invoices.sends[0].plan,
        )
        .unwrap();
    drop(invoice);
    let mut unsubscribe =
        crate::unsubscribe::InstalledSynthetic::open(&effects, store, state).unwrap();
    let raw = message.headers.evidence["list-unsubscribe"][0]
        .split(',')
        .map(str::trim)
        .find_map(|value| {
            value
                .strip_prefix("<https://")
                .and_then(|_| value.strip_prefix('<'))
                .and_then(|v| v.strip_suffix('>'))
        })
        .unwrap();
    let post = unsubscribe.post(raw).unwrap();
    let mut entry = state.unsubscribe.history[0].clone();
    entry.method = crate::unsubscribe::Method::OneClick;
    entry.payload_digest = post.request_digest();
    entry.host = Some(post.host().into());
    unsubscribe
        .submit(
            "before-sieve-independent-unsubscribe",
            &entry,
            &message,
            "fixture@example.test",
        )
        .unwrap();
}

pub(crate) fn later_provider_change(repo: &Accounts, account: &Account) {
    let permit = repo.write_permit(account).unwrap();
    let mut session = repo
        .transaction_session(account, Cancellation::default())
        .unwrap();
    session
        .put_script(
            &permit,
            "Open-Xchange",
            b"keep;\r\n# independent post-capture editor\r\n",
        )
        .unwrap();
    session.logout().unwrap();
}

pub(crate) fn independent_effects(root: &Path, store: &EncryptedStore) -> [String; 3] {
    let directory = root.join("mailboxes");
    let mut mailbox = EncryptedStore::new(directory.clone(), store.keys().synthetic_mailbox_keys())
        .unwrap()
        .load_state()
        .unwrap();
    mailbox.synthetic_sieve = None;
    mailbox.synthetic_folders = None;
    mailbox.revision = 0;
    let invoice = EncryptedStore::new(
        directory.join("invoice-transport"),
        store.keys().synthetic_invoice_keys(),
    )
    .unwrap()
    .load_state()
    .unwrap();
    let unsubscribe = EncryptedStore::new(
        directory.join("unsubscribe-transport"),
        store.keys().synthetic_unsubscribe_keys(),
    )
    .unwrap()
    .load_state()
    .unwrap();
    [mailbox, invoice, unsubscribe]
        .map(|s| crate::unsubscribe::digest(&serde_json::to_vec(&s).unwrap()))
}

#[test]
fn transaction_fixture_worker() {
    let Some(root) = std::env::var_os("MAILROOM_SIEVE_TRANSACTIONS_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let repo = persistent_test_accounts(root);
    let account = repo.list().unwrap().remove(0);
    let mode = std::env::var("MAILROOM_SIEVE_WORKER_ACTION").unwrap_or_default();
    if mode == "business_copy" {
        return add_business_travel_copy(&repo, &account);
    }
    if [
        "foreign_edit",
        "foreign_unparseable",
        "foreign_unsafe",
        "foreign_stop",
        "marker_damaged",
        "inactive_host",
        "owned_collision",
    ]
    .contains(&mode.as_str())
    {
        return worker(root, &mode);
    }
    let (external, mut provider_state) = provider(&repo, &account.id, root);
    let p = provider_state.synthetic_sieve.as_mut().unwrap();
    match mode.as_str() {
        "external_target" => {
            p.image.bodies[0] = Body::new(
                "Open-Xchange".into(),
                b"keep;\r\n# external edit\r\n".to_vec(),
            )
        }
        "external_active" => {
            for n in &mut p.image.names {
                n.active = n.name != "Open-Xchange";
            }
        }
        "missing_target" => {
            p.image.names.remove(0);
            p.image.bodies.remove(0);
        }
        "active_target" => {
            for n in &mut p.image.names {
                n.active = n.name == "Open-Xchange";
            }
        }
        "external_epoch" => p.epoch.push_str("-changed"),
        _ => {}
    }
    if !mode.is_empty() {
        external.save_state(&mut provider_state).unwrap();
        return;
    }
    drop(external);
    let p = provider_state.synthetic_sieve.as_ref().unwrap();
    fn image_digest(v: &impl serde::Serialize) -> String {
        use sha2::{Digest, Sha256};
        data_encoding::HEXLOWER.encode(&Sha256::digest(serde_json::to_vec(v).unwrap()))
    }
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    let mut unrelated = serde_json::to_value(&saved).unwrap();
    for field in ["revision", "sieve_transactions", "sieve"] {
        unrelated.as_object_mut().unwrap().remove(field);
    }
    let value = serde_json::json!({"account":account.id,"schema":store.schema_version().unwrap(),"revision":saved.revision,"unrelated":image_digest(&unrelated),"external_unrelated":independent_effects(root, &store),"provider_image":image_digest(&p.image),"provider_target":p.image.body("Open-Xchange").map(|b|&b.sha256),"provider_foreign":crate::census::sieve_transaction::foreign(&p.image, "Open-Xchange").map(|f| image_digest(&f)),"provider_owned":p.image.body("mailroom").map(|b|&b.sha256),"names":p.image.names.iter().map(|n|&n.name).collect::<Vec<_>>(),"active":p.image.active(),"commands":p.commands,"placement":saved.sieve_placement,"transactions":saved.sieve_transactions.history.iter().map(|t|serde_json::json!({"id":t.id,"status":t.status,"purpose":t.composition.as_ref().map(|c|c.purpose),"steps":t.steps.iter().map(|s|serde_json::json!({"effect":s.effect,"outcome":s.outcome,"acknowledgment":s.acknowledgment})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
    std::fs::write(
        root.join("transaction-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

pub(crate) fn commit_fixture_transaction(repo: &Accounts, account: &Account) {
    let store = repo.store(account).unwrap();
    let mut saved = store.load_state().unwrap();
    repo.record_placement(account, &store, &mut saved, Some(Placement::Block {}))
        .unwrap();
    let preview = repo
        .transaction_preview(
            account,
            &store,
            &saved,
            Request::Deploy,
            Cancellation::default(),
        )
        .unwrap();
    repo.transaction_confirm(
        account,
        &store,
        &mut saved,
        Request::Deploy,
        &preview.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
}

fn placed(root: &Path, placement: Option<Placement>) -> (Accounts, Account, Fixture) {
    let (repo, account, fixture) = setup(root, true);
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    super::super::sieve_discovery::tests::seed(&mut state, store.keys());
    store.save_state(&mut state).unwrap();
    repo.record_placement(&account, &store, &mut state, placement)
        .unwrap();
    (repo, account, fixture)
}
fn review(repo: &Accounts, account: &Account, request: Request<'_>) -> Previewed {
    let store = repo.store(account).unwrap();
    let state = store.load_state().unwrap();
    repo.transaction_review(account, &store, &state, request, Cancellation::default())
        .unwrap()
}
fn confirm(repo: &Accounts, account: &Account, request: Request<'_>, receipt: &str) -> bool {
    let store = repo.store(account).unwrap();
    let mut state = store.load_state().unwrap();
    repo.transaction_confirm(
        account,
        &store,
        &mut state,
        request,
        receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .is_ok()
}
fn ready(previewed: Previewed) -> Preview {
    match previewed {
        Previewed::Ready(p) => *p,
        Previewed::Refused { refusal, .. } => panic!("refused: {refusal:?}"),
    }
}
fn refusal(previewed: Previewed) -> Refusal {
    match previewed {
        Previewed::Refused { refusal, .. } => refusal,
        Previewed::Ready(_) => panic!("composition must refuse"),
    }
}
fn image(repo: &Accounts, account: &Account, root: &Path) -> FakeProvider {
    provider(repo, &account.id, root).1.synthetic_sieve.unwrap()
}
fn writes(p: &FakeProvider) -> usize {
    p.commands
        .iter()
        .filter(|c| *c == "PUTSCRIPT" || *c == "SETACTIVE")
        .count()
}
fn worker(root: &Path, mode: &str) {
    let repo = persistent_test_accounts(root);
    let account = repo.list().unwrap().remove(0);
    let (store, mut state) = provider(&repo, &account.id, root);
    let p = state.synthetic_sieve.as_mut().unwrap();
    let host = p
        .image
        .bodies
        .iter_mut()
        .find(|b| b.name == "Open-Xchange")
        .unwrap();
    let mut bytes = host.bytes.clone();
    match mode {
        "foreign_edit" => bytes.extend_from_slice(b"## Flag: |UniqueId:7|Rulename: Later owner rule\r\nif header :is \"subject\" \"invented later\"\r\n{\r\n    fileinto \"Later\";\r\n    stop;\r\n}\r\n"),
        "foreign_unparseable" => bytes.extend_from_slice(b"if header :is \"subject\" {\r\n"),
        "foreign_unsafe" => bytes.extend_from_slice(b"## Flag: |UniqueId:8|Rulename: Keep going\r\nif exists \"list-id\"\r\n{\r\n    fileinto \"Lists\";\r\n}\r\n"),
        "foreign_stop" => bytes.extend_from_slice(b"stop;\r\n"),
        "marker_damaged" => bytes.extend_from_slice(b"# mailroom:block-begin v2\r\n"),
        "inactive_host" => {
            for n in &mut p.image.names {
                n.active = false;
            }
        }
        "owned_collision" => {
            p.image.names.push(crate::census::sieve::ScriptName {
                name: "mailroom".into(),
                active: false,
            });
            p.image
                .bodies
                .push(Body::new("mailroom".into(), b"keep;\r\n".to_vec()));
            p.image.names.sort_by(|a, b| a.name.cmp(&b.name));
            p.image.bodies.sort_by(|a, b| a.name.cmp(&b.name));
        }
        "missing_host" => {
            p.image.names.retain(|n| n.name != "Open-Xchange");
            p.image.bodies.retain(|b| b.name != "Open-Xchange");
        }
        _ => unreachable!(),
    }
    if let Some(host) = p.image.bodies.iter_mut().find(|b| b.name == "Open-Xchange") {
        *host = Body::new(host.name.clone(), bytes);
    }
    store.save_state(&mut state).unwrap();
}
fn include() -> Placement {
    Placement::Include {
        script: "mailroom".into(),
    }
}

#[test]
fn state_proof_placement_source_password_and_key_changes_invalidate_exact_approval_without_effects()
{
    for change in ["state", "pin", "placement", "source", "password", "key"] {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, mut account, mut fake) = placed(root.path(), Some(Placement::Block {}));
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let preview = repo
            .transaction_preview(
                &account,
                &store,
                &state,
                Request::Deploy,
                Cancellation::default(),
            )
            .unwrap();
        match change {
            "state" => {
                state.winddown.dismissed.insert("new-owner-decision".into());
                store.save_state(&mut state).unwrap();
            }
            "pin" => {
                state.workspace.as_mut().unwrap().input.pins[0].expected =
                    crate::rules::proof::Destination::Folder {
                        name: "Different".into(),
                    };
                store.save_state(&mut state).unwrap();
            }
            "placement" => repo
                .record_placement(&account, &store, &mut state, Some(include()))
                .unwrap(),
            "source" => {
                fake.source.epoch.push_str("-changed");
                fake.install(&mut repo, &account.id);
            }
            _ => {}
        }
        drop(store);
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
        assert!(
            !confirm(&repo, &account, Request::Deploy, &preview.receipt),
            "{change}"
        );
        let store = repo.store(&account).unwrap();
        assert!(
            store
                .load_state()
                .unwrap()
                .sieve_transactions
                .history
                .is_empty()
        );
        assert_eq!(writes(&image(&repo, &account, root.path())), 0);
    }
}

#[test]
fn census_save_and_rotation_preserve_transaction_and_independent_provider_history() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = install(root.path());
    let effects_before = independent_effects(root.path(), &repo.store(&account).unwrap());
    commit_fixture_transaction(&repo, &account);
    let store = repo.store(&account).unwrap();
    let before = store.load_state().unwrap();
    let (_, external) = provider(&repo, &account.id, root.path());
    let external_before = serde_json::to_vec(&external).unwrap();
    store.save(&before.census).unwrap();
    let saved = store.load_state().unwrap();
    assert!(saved.sieve_transactions == before.sieve_transactions);
    assert!(saved.sieve_placement == Some(Placement::Block {}));
    drop(store);
    repo.rotate(&account.id, &root.path().join("mailboxes"))
        .unwrap();
    let account = repo.load(&account.id).unwrap();
    let store = repo.store(&account).unwrap();
    let after = store.load_state().unwrap();
    assert_eq!(independent_effects(root.path(), &store), effects_before);
    assert!(after.sieve_transactions == before.sieve_transactions);
    let (_, external) = provider(&repo, &account.id, root.path());
    assert!(serde_json::to_vec(&external).unwrap() == external_before);
    let id = after.sieve_transactions.history[0].id.clone();
    assert!(
        repo.transaction_preview(
            &account,
            &store,
            &after,
            Request::Compensation(&id),
            Cancellation::default()
        )
        .is_err()
    );
}

#[test]
fn validation_failure_preserves_discovery_and_effect_intent_makes_it_historical() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fake) = install(root.path());
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    repo.record_placement(&account, &store, &mut saved, Some(Placement::Block {}))
        .unwrap();
    let baseline = saved.sieve.last_successful.clone();
    drop(store);
    let preview = ready(review(&repo, &account, Request::Deploy));
    std::fs::write(root.path().join("transaction-mode"), "check_refused").unwrap();
    assert!(!confirm(&repo, &account, Request::Deploy, &preview.receipt));
    let saved = repo.store(&account).unwrap().load_state().unwrap();
    assert!(saved.sieve.last_successful == baseline);
    assert!(saved.sieve.invalidated_by_write.is_none());
    std::fs::write(root.path().join("transaction-mode"), "").unwrap();
    let fresh = ready(review(&repo, &account, Request::Deploy));
    assert!(confirm(&repo, &account, Request::Deploy, &fresh.receipt));
    let store = repo.store(&account).unwrap();
    let saved = store.load_state().unwrap();
    assert!(saved.sieve.last_successful == baseline);
    assert!(
        !saved
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
    drop(store);
    repo.discover_sieve(&account.id, Cancellation::default())
        .unwrap();
    let store = repo.store(&account).unwrap();
    assert!(
        store
            .load_state()
            .unwrap()
            .sieve
            .current(Some(&repo.sieve_context(&account, &store).unwrap()), false)
    );
}

#[test]
fn deploy_refuses_until_an_explicit_valid_placement_is_recorded() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fixture) = placed(root.path(), None);
    assert_eq!(
        refusal(review(&repo, &account, Request::Deploy)),
        Refusal::PlacementUnchosen
    );
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    for invalid in ["Open-Xchange", "quote\"name", ""] {
        assert!(
            repo.record_placement(
                &account,
                &store,
                &mut state,
                Some(Placement::Include {
                    script: invalid.into()
                })
            )
            .is_err()
        );
    }
    assert!(state.sieve_placement.is_none());
    assert_eq!(writes(&image(&repo, &account, root.path())), 0);
    assert!(
        store
            .load_state()
            .unwrap()
            .sieve_transactions
            .history
            .is_empty()
    );
}

#[test]
fn both_placements_compose_after_existing_rules_redeploy_switch_remove_and_compensate() {
    for placement in [Placement::Block {}, include()] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = placed(root.path(), Some(placement.clone()));
        let original = image(&repo, &account, root.path()).image;
        let foreign = std::str::from_utf8(OX_HOST).unwrap();
        let p = ready(review(&repo, &account, Request::Deploy));
        let precedence = p.precedence.as_ref().unwrap();
        assert_eq!(precedence.foreign_rules[0].name, "Travel");
        assert_eq!(precedence.counts.existing_wins, 1);
        assert_eq!(precedence.counts.conflicting, 1);
        assert_eq!(precedence.counts.mailroom, 2);
        assert!(precedence.decisions.iter().all(|d| d.rule == "Travel"
            && d.mailroom == crate::rules::proof::Destination::Inbox
            && d.existing
                == Some(composition::ForeignDest::Folder {
                    name: "Reisen".into()
                })));
        let host = &p.transaction.after.body("Open-Xchange").unwrap().bytes;
        assert!(host.starts_with(b"require [\"fileinto\"];\r\n"));
        assert_eq!(
            crate::census::sieve_transaction::foreign(&p.transaction.after, "Open-Xchange")
                .unwrap(),
            foreign
        );
        let rules_end = host.windows(3).rposition(|w| w == b"}\r\n").unwrap();
        assert!(
            std::str::from_utf8(&host[rules_end..])
                .unwrap()
                .contains("# mailroom:"),
            "Mailroom content follows every existing rule"
        );
        assert_eq!(
            p.effects,
            if placement == (Placement::Block {}) {
                vec![Effect::Upload]
            } else {
                vec![Effect::UploadOwned, Effect::Upload]
            }
        );
        assert!(confirm(&repo, &account, Request::Deploy, &p.receipt));
        let deployed = image(&repo, &account, root.path());
        assert!(deployed.image == p.transaction.after);
        assert_eq!(deployed.image.active(), Some("Open-Xchange"));
        assert_eq!(
            deployed.image.body("foreign\"\\ä"),
            original.body("foreign\"\\ä")
        );
        let journal = repo
            .store(&account)
            .unwrap()
            .load_state()
            .unwrap()
            .sieve_transactions;
        let deploy = journal.history.last().unwrap();
        assert_eq!(deploy.status, Status::Complete);
        assert!(
            deploy
                .steps
                .iter()
                .all(|s| s.acknowledgment && s.outcome == Outcome::Observed)
        );
        {
            let store = repo.store(&account).unwrap();
            assert!(
                repo.transaction_preview(
                    &account,
                    &store,
                    &store.load_state().unwrap(),
                    Request::Deploy,
                    Cancellation::default()
                )
                .is_err(),
                "re-deploy of identical content has no effect to approve"
            );
        }
        let other = if placement == (Placement::Block {}) {
            include()
        } else {
            Placement::Block {}
        };
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        repo.record_placement(&account, &store, &mut state, Some(other.clone()))
            .unwrap();
        drop(store);
        let switched = ready(review(&repo, &account, Request::Deploy));
        assert!(confirm(&repo, &account, Request::Deploy, &switched.receipt));
        let after_switch = image(&repo, &account, root.path()).image;
        assert_eq!(
            crate::census::sieve_transaction::foreign(&after_switch, "Open-Xchange").unwrap(),
            foreign
        );
        let (_, found) = composition::strip(
            &after_switch.body("Open-Xchange").unwrap().bytes,
            "Open-Xchange",
        )
        .unwrap();
        assert_eq!(found.block, other == Placement::Block {});
        assert_eq!(found.include.is_some(), other != Placement::Block {});
        let removal = ready(review(&repo, &account, Request::Removal));
        assert_eq!(removal.effects, vec![Effect::Upload]);
        assert!(confirm(&repo, &account, Request::Removal, &removal.receipt));
        let removed = image(&repo, &account, root.path()).image;
        assert_eq!(removed.body("Open-Xchange").unwrap().bytes, OX_HOST);
        assert_eq!(removed.active(), Some("Open-Xchange"));
        assert_eq!(
            removed.body("mailroom").is_some(),
            true,
            "the owned script stays without DELETE"
        );
        assert_eq!(
            refusal(review(&repo, &account, Request::Removal)),
            Refusal::NothingToRemove
        );
        let first = deploy.id.clone();
        let store = repo.store(&account).unwrap();
        assert!(
            repo.transaction_preview(
                &account,
                &store,
                &store.load_state().unwrap(),
                Request::Compensation(&first),
                Cancellation::default()
            )
            .is_err(),
            "compensation refuses once later changes replaced the recorded postimage"
        );
    }
}

/// A business message the existing Travel rule files with `stop`, from a sender the
/// workspace approves for accounting copies.
/// The account also needs synthetic workspace authority so forwards compile.
pub(crate) fn add_business_travel_copy(repo: &Accounts, account: &Account) {
    let store = repo.store(account).unwrap();
    let mut state = store.load_state().unwrap();
    let workspace = state.workspace.as_mut().unwrap();
    let documents = &mut workspace.input.documents;
    let (_, mut overrides) =
        crate::rules::config::documents(&documents.preset_yaml, &documents.override_yaml).unwrap();
    for forward in &mut overrides.forwards {
        if let crate::rules::config::ForwardTrigger::ApprovedSenders { addresses } =
            &mut forward.trigger
        {
            addresses.insert("robot@travel.example".into());
        }
    }
    documents.override_yaml = serde_yaml_ng::to_string(&overrides).unwrap();
    let input = crate::engine::PreviewInput {
        documents: documents.clone(),
        context: crate::rules::EngineContext::local(
            state.census.account.as_str().into(),
            crate::rules::Engine::MailboxSieve,
        ),
        messages: vec![serde_json::from_value(serde_json::json!({
            "id": "business-travel",
            "headers": "From: robot@travel.example\r\nTo: team@work.example.test\r\nDelivered-To: team@work.example.test\r\nSubject: business booking invoice\r\nMessage-ID: <business-travel@example.test>\r\nDate: Sat, 24 Oct 2026 00:00:00 +0000\r\n\r\n",
            "current_folder": "INBOX"
        }))
        .unwrap()],
        as_of: workspace.input.as_of,
        pins: vec![],
        baseline: crate::rules::proof::Baseline::Current,
        previous: None,
    };
    let mut census = crate::engine::synthetic_census(&input).unwrap();
    census.account = state.census.account.clone();
    let (folder, extra) = census.folders.pop_first().unwrap();
    let target = state.census.folders.entry(folder).or_insert(extra.clone());
    let uid = target.messages.values().map(|m| m.uid).max().unwrap_or(0) + 1;
    let mut message = extra.messages.into_values().next().unwrap();
    message.uid = uid;
    target.messages.insert(uid, message);
    store.save_state(&mut state).unwrap();
}
fn with_business_travel_copy(root: &Path, placement: Placement) -> (Accounts, Account, Fixture) {
    let (mut repo, account, fixture) = placed(root, Some(placement));
    repo.install_synthetic_workspace(&account, "explicit-copy-first");
    add_business_travel_copy(&repo, &account);
    (repo, account, fixture)
}

#[test]
fn existing_filing_rule_with_stop_still_lets_the_copy_happen_under_both_placements() {
    for placement in [Placement::Block {}, include()] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = with_business_travel_copy(root.path(), placement.clone());
        let p = ready(review(&repo, &account, Request::Deploy));
        let precedence = p.precedence.as_ref().unwrap();
        let copied: Vec<_> = precedence
            .copies
            .iter()
            .filter(|c| c.status == "copied")
            .collect();
        assert_eq!(copied.len(), 1, "{placement:?}");
        assert_eq!(copied[0].rule, "forward.approved");
        assert_eq!(copied[0].recipient, "archive@example.net");
        assert_eq!(
            copied[0].filed_by, "existing_rule",
            "Travel files it with stop"
        );
        let taken = precedence
            .decisions
            .iter()
            .find(|d| d.id == copied[0].id)
            .unwrap();
        assert_eq!(taken.rule, "Travel");
        assert_eq!(taken.copies, 1);
        assert_eq!(
            taken.existing,
            Some(composition::ForeignDest::Folder {
                name: "Reisen".into()
            })
        );
        let counts = precedence.counts.copies.as_ref().unwrap();
        assert_eq!(
            (
                counts.copied,
                counts.beside_existing,
                counts.guard_violations
            ),
            (1, 1, 0)
        );
        let host = std::str::from_utf8(&p.transaction.after.body("Open-Xchange").unwrap().bytes)
            .unwrap()
            .to_owned();
        let copy = host.find("# mailroom:copy-begin v1").unwrap();
        assert!(copy < host.find("Rulename: Travel").unwrap());
        let segment = &host[copy..host.find("# mailroom:copy-end v1").unwrap()];
        assert!(segment.contains("redirect :copy \"archive@example.net\";"));
        assert!(!segment.contains("fileinto") && !segment.contains("stop;"));
        let composition = p.transaction.composition.as_ref().unwrap();
        assert_eq!(composition.semantics, composition::SEMANTICS);
        assert!(confirm(&repo, &account, Request::Deploy, &p.receipt));
        let deployed = image(&repo, &account, root.path()).image;
        assert!(deployed == p.transaction.after);
        let removal = ready(review(&repo, &account, Request::Removal));
        assert!(confirm(&repo, &account, Request::Removal, &removal.receipt));
        let removed = image(&repo, &account, root.path()).image;
        assert_eq!(removed.body("Open-Xchange").unwrap().bytes, OX_HOST);
    }
}

#[test]
fn compensation_restores_the_exact_composed_preimage() {
    for placement in [Placement::Block {}, include()] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = placed(root.path(), Some(placement.clone()));
        let before = image(&repo, &account, root.path()).image;
        let p = ready(review(&repo, &account, Request::Deploy));
        assert!(confirm(&repo, &account, Request::Deploy, &p.receipt));
        let id = p.transaction.id.clone();
        let c = ready(review(&repo, &account, Request::Compensation(&id)));
        assert_eq!(c.effects, vec![Effect::Upload]);
        assert!(confirm(
            &repo,
            &account,
            Request::Compensation(&id),
            &c.receipt
        ));
        let after = image(&repo, &account, root.path()).image;
        assert_eq!(after.body("Open-Xchange"), before.body("Open-Xchange"));
        assert_eq!(after.active(), before.active());
        if placement == (Placement::Block {}) {
            assert!(after == before);
        } else {
            assert!(after.body("mailroom").is_some());
        }
    }
}

#[test]
fn foreign_edit_between_preview_and_write_is_refused_without_effects() {
    for placement in [Placement::Block {}, include()] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = placed(root.path(), Some(placement));
        let p = ready(review(&repo, &account, Request::Deploy));
        worker(root.path(), "foreign_edit");
        let edited = image(&repo, &account, root.path());
        assert!(!confirm(&repo, &account, Request::Deploy, &p.receipt));
        let after = image(&repo, &account, root.path());
        assert_eq!(writes(&after), 0);
        assert!(after.image == edited.image);
        let fresh = ready(review(&repo, &account, Request::Deploy));
        assert_eq!(fresh.precedence.as_ref().unwrap().foreign_rules.len(), 2);
        assert!(confirm(&repo, &account, Request::Deploy, &fresh.receipt));
        let deployed = image(&repo, &account, root.path()).image;
        assert_eq!(
            crate::census::sieve_transaction::foreign(&deployed, "Open-Xchange")
                .unwrap()
                .as_bytes(),
            edited.image.body("Open-Xchange").unwrap().bytes.as_slice()
        );
    }
}

#[test]
fn unsafe_unparseable_damaged_inactive_missing_and_colliding_hosts_block_with_reports() {
    for (mode, expected) in [
        ("foreign_unparseable", "unparseable"),
        ("foreign_unsafe", "unsafe_rule"),
        ("foreign_stop", "mailroom_unreachable"),
        ("marker_damaged", "marker_damaged"),
        ("inactive_host", "host_inactive"),
        ("owned_collision", "owned_name_foreign"),
        ("missing_host", "host_missing"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = placed(root.path(), Some(include()));
        worker(root.path(), mode);
        let before = image(&repo, &account, root.path());
        let previewed = review(&repo, &account, Request::Deploy);
        let json = serde_json::to_value(&previewed).unwrap();
        assert_eq!(json["outcome"], "refused", "{mode}");
        assert_eq!(json["refusal"]["reason"], expected, "{mode}");
        if expected == "unsafe_rule" {
            assert_eq!(json["refusal"]["rule"], "Keep going");
        }
        if mode != "inactive_host" && mode != "missing_host" {
            assert!(
                matches!(
                    review(&repo, &account, Request::Removal),
                    Previewed::Refused { .. }
                ),
                "{mode}"
            );
        }
        let after = image(&repo, &account, root.path());
        assert_eq!(writes(&after), 0, "{mode}");
        assert!(after.image == before.image, "{mode}");
    }
}

#[test]
fn existing_rule_precedence_reaches_pins_and_blocks_conflicting_deploys() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fixture) = placed(root.path(), Some(Placement::Block {}));
    let store = repo.store(&account).unwrap();
    let state = store.load_state().unwrap();
    let pin = &state.workspace.as_ref().unwrap().input.pins[0];
    let from = state
        .census
        .folders
        .values()
        .flat_map(|f| f.messages.values())
        .find(|m| m.headers.message_id_hash.as_ref() == Some(&pin.message_id_hash))
        .unwrap()
        .headers
        .from[0]
        .address
        .clone();
    drop(store);
    let (external, mut provider_state) = provider(&repo, &account.id, root.path());
    let p = provider_state.synthetic_sieve.as_mut().unwrap();
    let host = p
        .image
        .bodies
        .iter_mut()
        .find(|b| b.name == "Open-Xchange")
        .unwrap();
    let mut bytes = format!("## Flag: |UniqueId:3|Rulename: Pinned sender\r\nif address :is \"from\" \"{from}\"\r\n{{\r\n    fileinto \"Elsewhere\";\r\n    stop;\r\n}}\r\n").into_bytes();
    bytes.splice(0..0, host.bytes.clone());
    *host = Body::new(host.name.clone(), bytes);
    external.save_state(&mut provider_state).unwrap();
    drop(external);
    let Previewed::Refused {
        refusal,
        precedence: Some(precedence),
        ..
    } = review(&repo, &account, Request::Deploy)
    else {
        panic!("conflicting pinned precedence must block");
    };
    assert_eq!(refusal, Refusal::ProofBlocked);
    assert_eq!(precedence.gate, "blocked");
    assert_eq!(precedence.pins[0].status, "fail");
    assert!(
        precedence
            .decisions
            .iter()
            .any(|d| d.rule == "Pinned sender"
                && d.message_id_hash.as_ref() == Some(&pin.message_id_hash)
                && d.conflicting)
    );
    assert_eq!(writes(&image(&repo, &account, root.path())), 0);
}

#[test]
fn exact_utf8_octets_crlf_terminal_text_limits_and_write_permit_separation() {
    let root = tempfile::tempdir().unwrap();
    let (mut repo, account, fixture) = setup(root.path(), true);
    let permit = repo.write_permit(&account).unwrap();
    let bytes = b"# \xc3\xa4\r\n# OK\r\n# {17} \\\"\r\nkeep;\r\n\xff";
    let mut session = repo
        .transaction_session(&account, Cancellation::default())
        .unwrap();
    session.check_script(&permit, bytes).unwrap();
    session
        .have_space(&permit, "Open-Xchange", bytes.len())
        .unwrap();
    session.put_script(&permit, "Open-Xchange", bytes).unwrap();
    assert!(session.get("Open-Xchange").unwrap() == bytes);
    session
        .put_script(&permit, "mailroom ä", b"keep;\r\n")
        .unwrap();
    assert!(session.get("mailroom ä").unwrap() == b"keep;\r\n");
    assert_eq!(
        session
            .list()
            .unwrap()
            .iter()
            .map(|n| (n.name.as_str(), n.active))
            .filter(|(_, active)| *active)
            .collect::<Vec<_>>(),
        [("Open-Xchange", true)],
        "a new owned script is never implicitly activated"
    );
    assert_eq!(
        session.put_script(&permit, "Open-Xchange", &vec![b'x'; 1024 * 1024 + 1]),
        Err(Error::Limit)
    );
    assert_eq!(
        session.put_script(&permit, "bad\nname", b"keep;"),
        Err(Error::Limit)
    );
    session.logout().unwrap();
    repo.sieve_write = None;
    assert!(repo.write_permit(&account).is_err());
    assert!(repo.sieve_installed());
    let other = repo
        .create(
            Provider::MailboxOrg,
            "other@example.test".into(),
            &SecretValue::new(b"invented".to_vec()),
        )
        .unwrap();
    fixture.install(&mut repo, &account.id);
    assert!(repo.write_permit(&other).is_err());
}

#[test]
fn validation_space_and_upload_refusals_never_hide_later_failure() {
    for mode in [
        "check_refused",
        "check_unsupported",
        "check_malformed",
        "check_bye",
        "check_eof",
        "space_quota",
        "put_quota",
    ] {
        for placement in [Placement::Block {}, include()] {
            let root = tempfile::tempdir().unwrap();
            let (repo, account, _fixture) = placed(root.path(), Some(placement.clone()));
            let p = ready(review(&repo, &account, Request::Deploy));
            std::fs::write(root.path().join("transaction-mode"), mode).unwrap();
            assert!(!confirm(&repo, &account, Request::Deploy, &p.receipt));
            let external = image(&repo, &account, root.path());
            assert!(external.image == p.transaction.before);
            assert_eq!(
                external
                    .commands
                    .iter()
                    .filter(|c| *c == "PUTSCRIPT")
                    .count(),
                usize::from(mode == "put_quota")
            );
            assert_eq!(
                external
                    .commands
                    .iter()
                    .filter(|c| *c == "CHECKSCRIPT")
                    .count(),
                if mode.starts_with("check") {
                    1
                } else if placement == (Placement::Block {}) {
                    1
                } else {
                    2
                },
                "{mode} validates every uploaded script before any write"
            );
            assert!(!external.commands.iter().any(|c| c == "SETACTIVE"));
            let state = repo.store(&account).unwrap().load_state().unwrap();
            assert!(state.sieve.last_successful.is_none());
        }
    }
}

#[test]
fn lost_uploads_reconcile_without_repeating_and_resume_only_through_fresh_review() {
    for (placement, copies) in [
        (Placement::Block {}, false),
        (include(), false),
        (Placement::Block {}, true),
        (include(), true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = if copies {
            with_business_travel_copy(root.path(), placement.clone())
        } else {
            placed(root.path(), Some(placement.clone()))
        };
        let p = ready(review(&repo, &account, Request::Deploy));
        std::fs::write(root.path().join("transaction-mode"), "put_lost").unwrap();
        assert!(!confirm(&repo, &account, Request::Deploy, &p.receipt));
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let id = state.sieve_transactions.history[0].id.clone();
        assert_eq!(
            state.sieve_transactions.history[0].status,
            Status::Uncertain
        );
        let commands = image(&repo, &account, root.path()).commands;
        for _ in 0..2 {
            repo.transaction_reconcile(&account, &store, &mut state, &id, Cancellation::default())
                .unwrap();
        }
        drop(store);
        let after = image(&repo, &account, root.path()).commands;
        assert_eq!(
            commands.iter().filter(|c| *c == "PUTSCRIPT").count(),
            after.iter().filter(|c| *c == "PUTSCRIPT").count()
        );
        std::fs::write(root.path().join("transaction-mode"), "").unwrap();
        if placement == (Placement::Block {}) {
            assert_eq!(state.sieve_transactions.history[0].status, Status::Complete);
            let c = ready(review(&repo, &account, Request::Compensation(&id)));
            assert!(confirm(
                &repo,
                &account,
                Request::Compensation(&id),
                &c.receipt
            ));
            assert!(image(&repo, &account, root.path()).image == p.transaction.before);
        } else {
            assert_eq!(
                state.sieve_transactions.history[0].status,
                Status::FreshReview,
                "only the owned script arrived; the host still lacks the include"
            );
            let fresh = ready(review(&repo, &account, Request::Deploy));
            assert_eq!(fresh.effects, vec![Effect::Upload]);
            assert!(confirm(&repo, &account, Request::Deploy, &fresh.receipt));
            assert!(image(&repo, &account, root.path()).image == p.transaction.after);
        }
    }
}

#[test]
fn legacy_whole_replacement_records_stay_readable_and_compensable() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account, _fixture) = setup(root.path(), false);
    let store = repo.store(&account).unwrap();
    let mut state = store.load_state().unwrap();
    super::super::sieve_discovery::tests::seed(&mut state, store.keys());
    let before = image(&repo, &account, root.path()).image;
    let mut uploaded = before.clone();
    uploaded.bodies[0] = Body::new(
        "Open-Xchange".into(),
        b"keep;\r\n# legacy whole replacement\r\n".to_vec(),
    );
    let mut activated = uploaded.clone();
    activated.names[0].active = true;
    let digest = |v: &str| data_encoding::HEXLOWER.encode(&Sha256::digest(v.as_bytes()));
    let step = |effect, before: &Image, after: &Image| Step {
        effect,
        before: before.clone(),
        after: after.clone(),
        outcome: Outcome::Observed,
        acknowledgment: true,
        failure: None,
    };
    state.sieve_transactions.history.push(Transaction {
        id: digest("id"),
        compensates: None,
        context: repo.sieve_context(&account, &store).unwrap(),
        target: "Open-Xchange".into(),
        ownership_epoch: repo.write_permit(&account).unwrap().ownership.epoch.clone(),
        artifact: digest("artifact"),
        proof_binding: digest("proof"),
        receipt: digest("receipt"),
        before: before.clone(),
        after: activated.clone(),
        status: Status::Complete,
        validation: Some(Ok(())),
        space: Some(Ok(())),
        steps: vec![
            step(Effect::Upload, &before, &uploaded),
            step(Effect::Activate, &uploaded, &activated),
        ],
        composition: None,
    });
    store.save_state(&mut state).unwrap();
    let serialized = serde_json::to_value(&store.load_state().unwrap().sieve_transactions).unwrap();
    assert!(serialized["history"][0].get("composition").is_none());
    drop(store);
    let (external, mut provider_state) = provider(&repo, &account.id, root.path());
    provider_state.synthetic_sieve.as_mut().unwrap().image = activated;
    external.save_state(&mut provider_state).unwrap();
    drop(external);
    let id = digest("id");
    let c = ready(review(&repo, &account, Request::Compensation(&id)));
    assert_eq!(c.effects, vec![Effect::Upload, Effect::Disable]);
    assert!(c.transaction.composition.is_none());
    assert!(confirm(
        &repo,
        &account,
        Request::Compensation(&id),
        &c.receipt
    ));
    assert!(image(&repo, &account, root.path()).image == before);
}

#[test]
fn absolute_slow_drip_and_cancellation_bound_write_validation() {
    for mode in ["check_stall", "check_drip"] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = setup(root.path(), false);
        let cancel = Cancellation::default();
        let mut session = repo.transaction_session(&account, cancel.clone()).unwrap();
        session.set_command_timeout(Duration::from_millis(500));
        let permit = repo.write_permit(&account).unwrap();
        std::fs::write(root.path().join("transaction-mode"), mode).unwrap();
        if mode == "check_stall" {
            let c = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(35));
                c.cancel();
            });
        }
        assert_eq!(
            session.check_script(&permit, b"keep;\r\n"),
            Err(if mode == "check_stall" {
                Error::Cancelled
            } else {
                Error::Deadline
            })
        );
    }
}
