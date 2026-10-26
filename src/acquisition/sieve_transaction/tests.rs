use super::*;
use crate::{
    acquisition::{
        Provider, SecretValue,
        fixture_peer::Peer,
        managesieve::{FixtureOwnership, InstalledSource},
        persistent_test_accounts,
    },
    census::{AccountId, sieve_transaction::FakeProvider},
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
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
                address: listener.local_addr().unwrap(), root: cert.der().to_vec(), key: key.serialize_der(), epoch: format!("fixture-owned-{:032x}", rand::random::<u128>()),
                image: Image { capabilities: caps, names: vec![crate::census::sieve::ScriptName { name:"Open-Xchange".into(), active },crate::census::sieve::ScriptName { name:"foreign\"\\ä".into(),active:false }], bodies: vec![Body::new("Open-Xchange".into(), b"keep;\r\n# OK\r\n# \xc3\xa4 {9} <img src='https://remote.invalid/x'>\r\n".to_vec()), Body::new("foreign\"\\ä".into(), b"keep;\r\n# untouched\r\n".to_vec())] }, commands: vec![],
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
            command_timeout: Duration::from_secs(2),
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
        {
            let (store, mut state) = provider(repo, id, root);
            let p = state.synthetic_sieve.as_mut().unwrap();
            if p.commands.len() >= 4096 {
                send(io.get_mut(), b"BYE\r\n")?;
                return Ok(());
            }
            p.commands.push(kind.into());
            store.save_state(&mut state).unwrap();
        }
        let mut response = b"OK (X-UNKNOWN (ONE TWO)) \"diagnostic\"\r\n".to_vec();
        match kind {
            "AUTHENTICATE" => {
                if !args.starts_with("\"PLAIN\" \"") {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
            }
            "CAPABILITY" => {
                let (_, state) = provider(repo, id, root);
                response = vec![];
                for (k, v) in &state.synthetic_sieve.as_ref().unwrap().image.capabilities {
                    response
                        .extend_from_slice(format!("{} {}\r\n", quoted(k), quoted(v)).as_bytes());
                }
                response.extend_from_slice(b"OK\r\n");
            }
            "LISTSCRIPTS" => {
                let (_, state) = provider(repo, id, root);
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
                let (_, state) = provider(repo, id, root);
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
                    let (store, mut state) = provider(repo, id, root);
                    let p = state.synthetic_sieve.as_mut().unwrap();
                    let body = p
                        .image
                        .bodies
                        .iter_mut()
                        .find(|b| b.name.as_bytes() == name)
                        .ok_or(std::io::ErrorKind::InvalidData)?;
                    *body = Body::new(body.name.clone(), bytes);
                    store.save_state(&mut state).unwrap();
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
                let (store, mut state) = provider(repo, id, root);
                let p = state.synthetic_sieve.as_mut().unwrap();
                if !name.is_empty() && !p.image.names.iter().any(|n| n.name.as_bytes() == name) {
                    response = b"NO (NONEXISTENT)\r\n".to_vec();
                } else {
                    for n in &mut p.image.names {
                        n.active = n.name.as_bytes() == name;
                    }
                    store.save_state(&mut state).unwrap();
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
        let (repo, account, fake) = setup(root, false);
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        super::super::sieve_discovery::tests::seed(&mut state, store.keys());
        store.save_state(&mut state).unwrap();
        install_independent_effects(root, &store, &state);
        drop(store);
        (repo, account, fake)
    } else {
        let account = repo.list().unwrap().remove(0);
        let fake = Fixture::start(root, &account.id, false);
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
        .put_script(&permit, b"keep;\r\n# independent post-capture editor\r\n")
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
    let value = serde_json::json!({"account":account.id,"schema":store.schema_version().unwrap(),"revision":saved.revision,"unrelated":image_digest(&unrelated),"external_unrelated":independent_effects(root, &store),"provider_image":image_digest(&p.image),"provider_target":p.image.body("Open-Xchange").map(|b|&b.sha256),"active":p.image.active(),"commands":p.commands,"transactions":saved.sieve_transactions.history.iter().map(|t|serde_json::json!({"id":t.id,"status":t.status,"steps":t.steps.iter().map(|s|serde_json::json!({"effect":s.effect,"outcome":s.outcome,"acknowledgment":s.acknowledgment})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
    std::fs::write(
        root.join("transaction-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

pub(crate) fn commit_fixture_transaction(repo: &Accounts, account: &Account) {
    let store = repo.store(account).unwrap();
    let mut saved = store.load_state().unwrap();
    let preview = repo
        .transaction_preview(account, &store, &saved, None, Cancellation::default())
        .unwrap();
    repo.transaction_confirm(
        account,
        &store,
        &mut saved,
        None,
        &preview.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
}

#[test]
fn state_proof_source_password_and_key_changes_invalidate_exact_approval_without_effects() {
    for change in ["state", "pin", "source", "password", "key"] {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, mut account, mut fake) = install(root.path());
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let preview = repo
            .transaction_preview(&account, &store, &state, None, Cancellation::default())
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
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        assert!(
            repo.transaction_confirm(
                &account,
                &store,
                &mut state,
                None,
                &preview.receipt,
                Cancellation::default(),
                |_| Ok(())
            )
            .is_err(),
            "{change}"
        );
        assert!(state.sieve_transactions.history.is_empty());
        let (_, external) = provider(&repo, &account.id, root.path());
        assert!(
            !external
                .synthetic_sieve
                .unwrap()
                .commands
                .iter()
                .any(|c| c == "PUTSCRIPT" || c == "SETACTIVE")
        );
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
    assert!(store.load_state().unwrap().sieve_transactions == before.sieve_transactions);
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
        repo.transaction_preview(&account, &store, &after, Some(&id), Cancellation::default())
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
    let baseline = saved.sieve.last_successful.clone();
    let preview = repo
        .transaction_preview(&account, &store, &saved, None, Cancellation::default())
        .unwrap();
    std::fs::write(root.path().join("transaction-mode"), "check_refused").unwrap();
    assert!(
        repo.transaction_confirm(
            &account,
            &store,
            &mut saved,
            None,
            &preview.receipt,
            Cancellation::default(),
            |_| Ok(())
        )
        .is_err()
    );
    assert!(saved.sieve.last_successful == baseline);
    assert!(saved.sieve.invalidated_by_write.is_none());
    std::fs::write(root.path().join("transaction-mode"), "").unwrap();
    let fresh = repo
        .transaction_preview(&account, &store, &saved, None, Cancellation::default())
        .unwrap();
    repo.transaction_confirm(
        &account,
        &store,
        &mut saved,
        None,
        &fresh.receipt,
        Cancellation::default(),
        |_| Ok(()),
    )
    .unwrap();
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
fn actual_tls_persisted_active_and_inactive_replacement_and_separate_compensation() {
    for active in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = setup(root.path(), active);
        let store = repo.store(&account).unwrap();
        let mut saved = store.load_state().unwrap();
        super::super::sieve_discovery::tests::seed(&mut saved, store.keys());
        store.save_state(&mut saved).unwrap();
        let p = repo
            .transaction_preview(&account, &store, &saved, None, Cancellation::default())
            .unwrap();
        repo.transaction_confirm(
            &account,
            &store,
            &mut saved,
            None,
            &p.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
        .unwrap();
        let transaction = saved.sieve_transactions.history.last().unwrap();
        assert_eq!(transaction.status, Status::Complete);
        assert!(
            transaction
                .steps
                .iter()
                .all(|s| s.acknowledgment && s.outcome == Outcome::Observed)
        );
        let id = transaction.id.clone();
        let (_, external) = provider(&repo, &account.id, root.path());
        assert!(external.synthetic_sieve.as_ref().unwrap().image == p.transaction.after);
        let c = repo
            .transaction_preview(&account, &store, &saved, Some(&id), Cancellation::default())
            .unwrap();
        assert_eq!(c.effects.contains(&Effect::Disable), !active);
        repo.transaction_confirm(
            &account,
            &store,
            &mut saved,
            Some(&id),
            &c.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
        .unwrap();
        let (_, external) = provider(&repo, &account.id, root.path());
        assert!(external.synthetic_sieve.unwrap().image == p.transaction.before);
    }
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
    session.have_space(&permit, bytes.len()).unwrap();
    session.put_script(&permit, bytes).unwrap();
    assert!(session.get("Open-Xchange").unwrap() == bytes);
    assert_eq!(
        session
            .list()
            .unwrap()
            .iter()
            .find(|n| n.name == "Open-Xchange")
            .unwrap()
            .active,
        true
    );
    assert_eq!(
        session.put_script(&permit, &vec![b'x'; 1024 * 1024 + 1]),
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
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = setup(root.path(), true);
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        super::super::sieve_discovery::tests::seed(&mut state, store.keys());
        store.save_state(&mut state).unwrap();
        let p = repo
            .transaction_preview(&account, &store, &state, None, Cancellation::default())
            .unwrap();
        std::fs::write(root.path().join("transaction-mode"), mode).unwrap();
        assert!(
            repo.transaction_confirm(
                &account,
                &store,
                &mut state,
                None,
                &p.receipt,
                Cancellation::default(),
                |_| Ok(())
            )
            .is_err()
        );
        let (_, external) = provider(&repo, &account.id, root.path());
        let external = external.synthetic_sieve.as_ref().unwrap();
        assert!(external.image == p.transaction.before);
        assert_eq!(
            external
                .commands
                .iter()
                .filter(|c| *c == "PUTSCRIPT")
                .count(),
            usize::from(mode == "put_quota")
        );
        assert!(!external.commands.iter().any(|c| c == "SETACTIVE"));
        assert!(state.sieve.last_successful.is_none());
    }
}

#[test]
fn lost_upload_or_activation_reconciles_without_repeating_and_compensates_conditionally() {
    for mode in ["put_lost", "activation_lost"] {
        let root = tempfile::tempdir().unwrap();
        let (repo, account, _fixture) = setup(root.path(), false);
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        super::super::sieve_discovery::tests::seed(&mut state, store.keys());
        store.save_state(&mut state).unwrap();
        let p = repo
            .transaction_preview(&account, &store, &state, None, Cancellation::default())
            .unwrap();
        std::fs::write(root.path().join("transaction-mode"), mode).unwrap();
        assert!(
            repo.transaction_confirm(
                &account,
                &store,
                &mut state,
                None,
                &p.receipt,
                Cancellation::default(),
                |_| Ok(())
            )
            .is_err()
        );
        let id = state.sieve_transactions.history[0].id.clone();
        assert_eq!(
            state.sieve_transactions.history[0].status,
            Status::Uncertain
        );
        let (_, external) = provider(&repo, &account.id, root.path());
        let commands = external.synthetic_sieve.unwrap().commands;
        for _ in 0..2 {
            repo.transaction_reconcile(&account, &store, &mut state, &id, Cancellation::default())
                .unwrap();
        }
        assert_eq!(
            state.sieve_transactions.history[0].status,
            if mode == "put_lost" {
                Status::FreshReview
            } else {
                Status::Complete
            }
        );
        let (_, external) = provider(&repo, &account.id, root.path());
        let after = external.synthetic_sieve.unwrap().commands;
        for command in ["PUTSCRIPT", "SETACTIVE"] {
            assert_eq!(
                commands.iter().filter(|c| c.as_str() == command).count(),
                after.iter().filter(|c| c.as_str() == command).count()
            );
        }
        std::fs::write(root.path().join("transaction-mode"), "").unwrap();
        let c = repo
            .transaction_preview(&account, &store, &state, Some(&id), Cancellation::default())
            .unwrap();
        repo.transaction_confirm(
            &account,
            &store,
            &mut state,
            Some(&id),
            &c.receipt,
            Cancellation::default(),
            |_| Ok(()),
        )
        .unwrap();
        let (_, external) = provider(&repo, &account.id, root.path());
        assert!(external.synthetic_sieve.unwrap().image == p.transaction.before);
    }
}

#[test]
fn absolute_slow_drip_and_cancellation_bound_write_validation() {
    for mode in ["check_stall", "check_drip"] {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, account, mut fixture) = setup(root.path(), false);
        fixture.source.command_timeout = Duration::from_millis(500);
        fixture.install(&mut repo, &account.id);
        let cancel = Cancellation::default();
        let mut session = repo.transaction_session(&account, cancel.clone()).unwrap();
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
