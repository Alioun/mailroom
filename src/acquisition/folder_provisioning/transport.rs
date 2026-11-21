use super::super::{Cancellation, SecretValue};
use crate::census::{
    AccountId,
    folder_provisioning::{ExactName, Failure as Error, Inventory, Mailbox, Namespace, Namespaces},
};
use native_tls::{HandshakeError, TlsConnector, TlsStream};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const RESPONSE: usize = 512 * 1024;
/// Initial byte budget of a session, read and written. Like the deadline,
/// each completed command adds one response's worth, so a long run of batches
/// on one session is bounded per command instead of cut off after about 16
/// MiB, which a run of about 46 MOVE batches of 500 messages reached.
const TOTAL: usize = 16 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(5);
/// Each completed command extends the session's overall deadline by this much,
/// on top of the initial operation budget. A plan of any size can finish while
/// the server averages under 5 s per command; one that averages more is cut off
/// after the initial budget runs out. Every single command still has its own
/// deadline, and Cancel is checked while waiting on every byte. Basis: about
/// 230 ms per command on the loopback fixture at low load, and cold TLS
/// handshakes of up to 2 s under heavy load.
const PROGRESS_CREDIT: Duration = Duration::from_secs(5);

pub(in crate::acquisition) mod gmail_label;
pub(in crate::acquisition) mod message_move;
mod wire;

struct Endpoint<'a> {
    address: SocketAddr,
    tls_name: &'a str,
    root: &'a [u8],
    identity: String,
    command_timeout: Duration,
    operation_timeout: Duration,
}

pub(in crate::acquisition) const LIVE_FOLDER_EPOCH: &str = "live-imap-folders/v1";

#[derive(Clone)]
pub(in crate::acquisition) struct InstalledSource {
    pub account: AccountId,
    pub address: SocketAddr,
    pub tls_name: String,
    pub root: Vec<u8>,
    pub epoch: String,
    pub command_timeout: Duration,
    pub operation_timeout: Duration,
}
pub(super) struct CreatePermit<'a>(std::borrow::Cow<'a, InstalledSource>);
impl InstalledSource {
    pub(in crate::acquisition) fn is_live(&self) -> bool {
        self.epoch == LIVE_FOLDER_EPOCH
            && self.root.is_empty()
            && matches!(
                self.tls_name.as_str(),
                "imap.mailbox.org" | "imap.gmail.com"
            )
            && !self.address.ip().is_loopback()
    }
    pub(in crate::acquisition) fn real_for_account(
        account: &super::super::Account,
    ) -> Result<Self, Error> {
        use std::net::ToSocketAddrs;
        let host = account.provider.host();
        let address = (host, 993)
            .to_socket_addrs()
            .map_err(|_| Error::Connect)?
            .find(|a| !a.ip().is_loopback())
            .ok_or(Error::Connect)?;
        Ok(Self {
            account: account.id.clone(),
            address,
            tls_name: host.to_owned(),
            root: Vec::new(),
            epoch: LIVE_FOLDER_EPOCH.to_owned(),
            command_timeout: Duration::from_secs(10),
            operation_timeout: Duration::from_secs(60),
        })
    }
    pub(in crate::acquisition) fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = if self.is_live() {
            serde_json::to_vec(&(
                "live-imap-folders/v1",
                &self.account,
                &self.tls_name,
                &self.epoch,
                crate::census::folder_provisioning::PROFILE,
            ))
            .expect("live folder identity")
        } else {
            serde_json::to_vec(&(
                &self.account,
                self.address,
                &self.tls_name,
                &self.root,
                &self.epoch,
                crate::census::folder_provisioning::PROFILE,
            ))
            .expect("installed identity")
        };
        data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
    }
    pub(super) fn permit(&self, account: &AccountId) -> Result<CreatePermit<'_>, Error> {
        if self.account != *account {
            return Err(Error::Unsupported);
        }
        if self.is_live() {
            return Ok(CreatePermit(std::borrow::Cow::Borrowed(self)));
        }
        if !self.address.ip().is_loopback()
            || self.epoch.is_empty()
            || self.root.is_empty()
            || self.tls_name.is_empty()
        {
            return Err(Error::Unsupported);
        }
        Ok(CreatePermit(std::borrow::Cow::Borrowed(self)))
    }
    pub(super) fn permit_owned(self, account: &AccountId) -> Result<CreatePermit<'static>, Error> {
        self.permit(account)?;
        Ok(CreatePermit(std::borrow::Cow::Owned(self)))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Atom(String),
    String(Vec<u8>),
    Open,
    Close,
}
impl Token {
    fn atom(&self) -> Option<&str> {
        if let Self::Atom(s) = self {
            Some(s)
        } else {
            None
        }
    }
    fn text(&self) -> Result<String, Error> {
        match self {
            Self::String(s) => String::from_utf8(s.clone()).map_err(|_| Error::Protocol),
            Self::Atom(s) => Ok(s.clone()),
            _ => Err(Error::Protocol),
        }
    }
}
pub(super) struct Session {
    #[cfg(test)]
    pending_writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    source: String,
    stream: TlsStream<TcpStream>,
    cancel: Cancellation,
    end: Instant,
    command_end: Instant,
    timeout: Duration,
    credit: Duration,
    received: usize,
    written: usize,
    budget: usize,
    remaining: usize,
    tag: u32,
    code_limit: usize,
    /// Set when a pipelined flight ended without reading every response, so
    /// the stream position is unknown and no later command may use it.
    broken: bool,
    /// When the session last finished a flight. Time spent idle between
    /// flights, such as local work between a preview's reads and its run,
    /// is added to the overall deadline: that deadline bounds time spent
    /// waiting on the server, not time the client spends elsewhere.
    idle: Option<Instant>,
}

/// One command of a pipelined flight. Its handler sees each response row,
/// untagged then tagged. Steps of the same unit stand or fall together: once
/// one fails, the unit's later responses are read past without their
/// handlers, so rows never reach a step whose precondition failed.
pub(super) struct Step<'a> {
    command: Zeroizing<Vec<u8>>,
    unit: usize,
    handler: Box<dyn FnMut(&[Token], bool) -> Result<(), Error> + 'a>,
}
impl<'a> Step<'a> {
    pub(super) fn new(
        command: Vec<u8>,
        handler: impl FnMut(&[Token], bool) -> Result<(), Error> + 'a,
    ) -> Self {
        Self {
            command: Zeroizing::new(command),
            unit: 0,
            handler: Box::new(handler),
        }
    }
    pub(super) fn unit(mut self, unit: usize) -> Self {
        self.unit = unit;
        self
    }
    /// Collects a command's untagged data rows the way `execute` does.
    fn collect(command: &[u8], rows: &'a mut Vec<Vec<Token>>) -> Self {
        Self::new(command.to_vec(), move |row, tagged| {
            if tagged {
                return Session::status(row, 1);
            }
            if row.len() < 2 {
                return Err(Error::Protocol);
            }
            if matches!(row[1].atom(), Some("OK" | "NO" | "BAD" | "BYE")) {
                Session::status(row, 1)
            } else if rows.len() >= 256 {
                Err(Error::Limit)
            } else {
                rows.push(row.to_vec());
                Ok(())
            }
        })
    }
}
impl wire::Bytes for Session {
    fn byte(&mut self) -> Result<u8, Error> {
        Session::byte(self)
    }
    fn remaining(&self) -> usize {
        self.remaining
    }
}
fn guard(cancel: &Cancellation, end: Instant) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= end {
        return Err(Error::Deadline);
    }
    Ok(())
}
fn pending(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}
fn quoted(value: &[u8]) -> Result<Vec<u8>, Error> {
    if value.len() > 4096 || !value.iter().all(|b| (0x20..=0x7e).contains(b)) {
        return Err(Error::Unsupported);
    }
    let mut result = vec![b'"'];
    for b in value {
        if matches!(*b, b'"' | b'\\') {
            result.push(b'\\');
        }
        result.push(*b);
    }
    result.push(b'"');
    Ok(result)
}
impl Session {
    pub(super) fn connect(
        permit: &CreatePermit<'_>,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, Error> {
        let source = &*permit.0;
        Self::open(
            Endpoint {
                address: source.address,
                tls_name: &source.tls_name,
                root: &source.root,
                identity: source.identity(),
                command_timeout: source.command_timeout,
                operation_timeout: source.operation_timeout,
            },
            login,
            password,
            cancel,
        )
    }

    fn open(
        source: Endpoint<'_>,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, Error> {
        let end = Instant::now() + source.operation_timeout.min(Duration::from_secs(60));
        guard(&cancel, end)?;
        let user = Zeroizing::new(quoted(login.as_bytes())?);
        let pass = Zeroizing::new(quoted(password.bytes())?);
        if login.is_empty() || password.bytes().is_empty() {
            return Err(Error::Authentication);
        }
        let connect_timeout = if source.root.is_empty() {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(250)
        };
        let tcp = TcpStream::connect_timeout(
            &source.address,
            connect_timeout.min(end.saturating_duration_since(Instant::now())),
        )
        .map_err(|_error| {
            #[cfg(test)]
            eprintln!("folder.transport.connect.io_kind={:?}", _error.kind());
            Error::Connect
        })?;
        tcp.set_nonblocking(true).map_err(|_| Error::Connect)?;
        tcp.set_nodelay(true).map_err(|_| Error::Connect)?;
        let timeout = source.command_timeout.min(Duration::from_secs(10));
        let command_end = (Instant::now() + timeout).min(end);
        let mut builder = TlsConnector::builder();
        builder.min_protocol_version(Some(native_tls::Protocol::Tlsv12));
        if !source.root.is_empty() {
            builder.disable_built_in_roots(true);
            builder.add_root_certificate(
                native_tls::Certificate::from_der(source.root).map_err(|_| Error::Tls)?,
            );
        }
        let connector = builder.build().map_err(|_| Error::Tls)?;
        let mut handshake = connector.connect(source.tls_name, tcp);
        let stream = loop {
            guard(&cancel, command_end).inspect_err(|_error| {
                #[cfg(test)]
                eprintln!("folder.transport.handshake.failure={_error:?}");
            })?;
            match handshake {
                Ok(s) => break s,
                Err(HandshakeError::WouldBlock(mid)) => {
                    std::thread::sleep(POLL);
                    handshake = mid.handshake();
                }
                Err(_) => {
                    #[cfg(test)]
                    eprintln!("folder.transport.handshake.failure=Tls");
                    return Err(Error::Tls);
                }
            }
        };
        let mut s = Self {
            #[cfg(test)]
            pending_writes: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            source: source.identity,
            stream,
            cancel,
            end,
            command_end,
            timeout,
            credit: PROGRESS_CREDIT,
            received: 0,
            written: 0,
            budget: TOTAL,
            remaining: RESPONSE,
            tag: 0,
            code_limit: 256,
            broken: false,
            idle: None,
        };
        let greeting = s.row()?;
        if greeting.first().and_then(Token::atom) != Some("*")
            || greeting.get(1).and_then(Token::atom) != Some("OK")
        {
            return Err(Error::Protocol);
        }
        Self::status(&greeting, 1)?;
        s.capabilities()?;
        let login_command = Zeroizing::new([b"LOGIN ".as_slice(), &user, b" ", &pass].concat());
        s.execute(&login_command).map_err(|e| {
            if matches!(e, Error::No | Error::Bad) {
                Error::Authentication
            } else {
                e
            }
        })?;
        s.capabilities()?;
        Ok(s)
    }
    /// Records a command's tagged completion, which earns the session more time.
    fn completed(&mut self) {
        self.end += self.credit;
        self.budget = self.budget.saturating_add(RESPONSE);
    }
    fn check(&self) -> Result<(), Error> {
        let result = guard(&self.cancel, self.end.min(self.command_end));
        #[cfg(test)]
        if result == Err(Error::Deadline) {
            eprintln!(
                "folder.transport.deadline budget={} commands={}",
                if self.end <= self.command_end {
                    "operation"
                } else {
                    "command"
                },
                self.tag
            );
        }
        result
    }
    fn byte(&mut self) -> Result<u8, Error> {
        self.check()?;
        if self.remaining == 0 || self.received >= self.budget {
            return Err(Error::Limit);
        }
        loop {
            self.check()?;
            let mut b = [0];
            match self.stream.read(&mut b) {
                Ok(0) => {
                    #[cfg(test)]
                    eprintln!("folder.transport.read.peer_closed");
                    return Err(Error::Eof);
                }
                Ok(_) => {
                    self.remaining -= 1;
                    self.received += 1;
                    return Ok(b[0]);
                }
                Err(e) if pending(&e) => std::thread::sleep(POLL),
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("folder.transport.read.io_kind={:?}", _error.kind());
                    return Err(Error::Eof);
                }
            }
        }
    }
    fn row(&mut self) -> Result<Vec<Token>, Error> {
        let code_limit = self.code_limit;
        wire::row(self, code_limit)
    }
    fn status(row: &[Token], index: usize) -> Result<(), Error> {
        let status = row
            .get(index)
            .and_then(Token::atom)
            .ok_or(Error::Protocol)?;
        if status == "BYE" {
            return Err(Error::Bye);
        }
        if let Some(code) = row
            .get(index + 1)
            .and_then(Token::atom)
            .filter(|s| s.starts_with('['))
        {
            return Err(if code.starts_with("[OVERQUOTA") {
                Error::Quota
            } else if status == "NO" && code.starts_with("[ALREADYEXISTS") {
                // RFC 5530 §3: the target already exists, so nothing was done.
                Error::No
            } else {
                Error::UnknownCode
            });
        }
        match status {
            "OK" => Ok(()),
            "NO" => Err(Error::No),
            "BAD" => Err(Error::Bad),
            _ => Err(Error::Protocol),
        }
    }
    /// Sends commands in one write and returns their tags. Only the overall
    /// deadline applies here: time spent between commands is not a stalled
    /// command, and each response gets its own deadline when it is awaited.
    fn send(&mut self, commands: &[&[u8]]) -> Result<Vec<String>, Error> {
        if let Some(idle) = self.idle.take() {
            self.end += idle.elapsed();
        }
        guard(&self.cancel, self.end)?;
        if self.broken {
            return Err(Error::Protocol);
        }
        self.command_end = (Instant::now() + self.timeout).min(self.end);
        let mut tags = vec![];
        let mut bytes = Zeroizing::new(vec![]);
        for command in commands {
            self.tag = self.tag.checked_add(1).ok_or(Error::Limit)?;
            let tag = format!("F{}", self.tag);
            bytes.extend_from_slice(tag.as_bytes());
            bytes.push(b' ');
            bytes.extend_from_slice(command);
            bytes.extend_from_slice(b"\r\n");
            tags.push(tag);
        }
        if bytes.len() > self.budget.saturating_sub(self.written) {
            return Err(Error::Limit);
        }
        self.written += bytes.len();
        let mut offset = 0;
        while offset < bytes.len() {
            self.check()?;
            match self.stream.write(&bytes[offset..]) {
                Ok(0) => return Err(Error::Eof),
                Ok(n) => offset += n,
                Err(e) if pending(&e) => {
                    #[cfg(test)]
                    self.pending_writes
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(POLL);
                }
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("folder.transport.write.io_kind={:?}", _error.kind());
                    return Err(Error::Eof);
                }
            }
        }
        loop {
            self.check()?;
            match self.stream.flush() {
                Ok(()) => break,
                Err(e) if pending(&e) => {
                    #[cfg(test)]
                    self.pending_writes
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(POLL);
                }
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("folder.transport.flush.io_kind={:?}", _error.kind());
                    return Err(Error::Eof);
                }
            }
        }
        Ok(tags)
    }
    /// Reads one command's response up to its tagged line. A handler error
    /// is that command's result; the rest of its response is still read, so
    /// the stream stays aligned. Any other error leaves the stream unusable.
    fn response(
        &mut self,
        tag: &str,
        handler: Option<&mut (dyn FnMut(&[Token], bool) -> Result<(), Error> + '_)>,
    ) -> Result<Result<(), Error>, Error> {
        self.command_end = (Instant::now() + self.timeout).min(self.end);
        self.remaining = RESPONSE;
        let mut handler = handler;
        let mut failed: Option<Error> = None;
        for _ in 0..2048 {
            let response = self.row()?;
            let first = response
                .first()
                .and_then(Token::atom)
                .ok_or(Error::Protocol)?;
            let tagged = first == tag;
            if !tagged && first != "*" {
                return Err(Error::Protocol);
            }
            if tagged {
                self.completed();
            }
            if response.get(1).and_then(Token::atom) == Some("BYE") {
                return Err(Error::Bye);
            }
            if failed.is_none() {
                if let Some(handler) = handler.as_mut() {
                    failed = handler(&response, tagged).err();
                }
            }
            if tagged {
                return Ok(failed.map_or(Ok(()), Err));
            }
        }
        Err(Error::Limit)
    }
    /// Pipelines `steps` (RFC 3501 section 5.5) and returns each unit's
    /// result, indexed by unit. Responses are read strictly in order, so rows
    /// belong to the command whose tagged line follows them; a server must
    /// finish a command that changes the selected mailbox before starting
    /// the next. A step runs its handler only while its unit has not failed.
    pub(super) fn flight(&mut self, steps: Vec<Step<'_>>) -> Result<Vec<Result<(), Error>>, Error> {
        let units = steps.iter().map(|s| s.unit + 1).max().unwrap_or(0);
        let commands: Vec<&[u8]> = steps.iter().map(|s| s.command.as_slice()).collect();
        let tags = self.send(&commands)?;
        let mut results: Vec<Result<(), Error>> = (0..units).map(|_| Ok(())).collect();
        for (mut step, tag) in steps.into_iter().zip(tags) {
            let handler = results[step.unit].is_ok().then_some(
                &mut *step.handler as &mut dyn FnMut(&[Token], bool) -> Result<(), Error>,
            );
            match self.response(&tag, handler) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    if results[step.unit].is_ok() {
                        results[step.unit] = Err(error);
                    }
                }
                Err(error) => {
                    self.broken = true;
                    return Err(error);
                }
            }
        }
        self.idle = Some(Instant::now());
        Ok(results)
    }
    /// Pipelines `commands`, all of which must succeed, and returns each
    /// one's untagged data rows.
    fn execute_all(&mut self, commands: &[&[u8]]) -> Result<Vec<Vec<Vec<Token>>>, Error> {
        let mut rows: Vec<Vec<Vec<Token>>> = vec![vec![]; commands.len()];
        let steps = commands
            .iter()
            .zip(rows.iter_mut())
            .map(|(command, rows)| Step::collect(command, rows))
            .collect();
        self.flight(steps)?.pop().unwrap_or(Ok(()))?;
        Ok(rows)
    }
    fn execute(&mut self, command: &[u8]) -> Result<Vec<Vec<Token>>, Error> {
        Ok(self.execute_all(&[command])?.pop().unwrap_or_default())
    }
    fn capabilities(&mut self) -> Result<BTreeSet<String>, Error> {
        let rows = self.execute(b"CAPABILITY")?;
        Self::parse_capabilities(rows)
    }
    fn parse_capabilities(rows: Vec<Vec<Token>>) -> Result<BTreeSet<String>, Error> {
        if rows.len() != 1 || rows[0].get(1).and_then(Token::atom) != Some("CAPABILITY") {
            return Err(Error::Protocol);
        }
        let mut caps = BTreeSet::new();
        for t in &rows[0][2..] {
            if !caps.insert(t.atom().ok_or(Error::Protocol)?.to_ascii_uppercase()) {
                return Err(Error::Protocol);
            }
        }
        if caps.len() > 128
            || !caps.contains("IMAP4REV1")
            || !caps.contains("NAMESPACE")
            || caps.contains("UTF8=ONLY")
            || caps.contains("IMAP4REV2")
        {
            return Err(Error::Unsupported);
        }
        Ok(caps)
    }
    fn parse_namespaces(rows: Vec<Vec<Token>>) -> Result<Namespaces, Error> {
        if rows.len() != 1 || rows[0].get(1).and_then(Token::atom) != Some("NAMESPACE") {
            return Err(Error::Protocol);
        }
        let mut tokens = rows[0][2..].iter();
        fn class<'a>(
            tokens: &mut impl Iterator<Item = &'a Token>,
        ) -> Result<Vec<Namespace>, Error> {
            match tokens.next() {
                Some(Token::Atom(s)) if s == "NIL" => return Ok(vec![]),
                Some(Token::Open) => (),
                _ => return Err(Error::Protocol),
            }
            let mut namespaces = vec![];
            loop {
                match tokens.next() {
                    Some(Token::Close) => return Ok(namespaces),
                    Some(Token::Open) => (),
                    _ => return Err(Error::Protocol),
                }
                let prefix = tokens.next().ok_or(Error::Protocol)?.text()?;
                let delimiter = match tokens.next() {
                    Some(Token::Atom(s)) if s == "NIL" => None,
                    Some(t) => Some(t.text()?),
                    _ => return Err(Error::Protocol),
                };
                if delimiter
                    .as_ref()
                    .is_some_and(|d| d.len() != 1 || !d.is_ascii())
                {
                    return Err(Error::Unsupported);
                }
                if tokens.next() != Some(&Token::Close) {
                    return Err(Error::Unsupported);
                }
                namespaces.push(Namespace { prefix, delimiter });
                if namespaces.len() > 8 {
                    return Err(Error::Limit);
                }
            }
        }
        let ns = Namespaces {
            personal: class(&mut tokens)?,
            other: class(&mut tokens)?,
            shared: class(&mut tokens)?,
        };
        if tokens.next().is_some() {
            return Err(Error::Protocol);
        }
        if !ns.supported() {
            return Err(Error::Unsupported);
        }
        Ok(ns)
    }
    pub(super) fn observe(&mut self) -> Result<Inventory, Error> {
        let mut rows = InventoryRows::default();
        let steps = rows.steps();
        self.flight(steps)?.pop().unwrap_or(Ok(()))?;
        rows.inventory()
    }
    pub(super) fn create(
        &mut self,
        permit: &CreatePermit<'_>,
        name: &ExactName,
    ) -> Result<(), Error> {
        if self.source != permit.0.identity() {
            return Err(Error::Unsupported);
        }
        self.create_exact(name)
    }
    /// One CREATE of an exact, canonically encoded name. The caller has
    /// checked that the session belongs to its permit.
    fn create_exact(&mut self, name: &ExactName) -> Result<(), Error> {
        if ExactName::from_wire(&name.wire)? != *name
            || name
                .decoded
                .split('/')
                .any(|s| s.is_empty() || s.eq_ignore_ascii_case("INBOX"))
        {
            return Err(Error::Unsupported);
        }
        let args = quoted(name.wire.as_bytes())?;
        let rows = self.execute(&[b"CREATE ".as_slice(), &args].concat())?;
        if !rows.is_empty() {
            return Err(Error::UnknownCode);
        }
        Ok(())
    }
    pub(super) fn logout(&mut self) {
        let _ = self.execute(b"LOGOUT");
    }
}

/// The responses of one topology observation: CAPABILITY, NAMESPACE, LIST,
/// then NAMESPACE and CAPABILITY again, which must repeat the first answers.
#[derive(Default)]
pub(super) struct InventoryRows([Vec<Vec<Token>>; 5]);
impl InventoryRows {
    const COMMANDS: [&'static [u8]; 5] = [
        b"CAPABILITY",
        b"NAMESPACE",
        b"LIST \"\" \"*\"",
        b"NAMESPACE",
        b"CAPABILITY",
    ];
    pub(super) fn steps(&mut self) -> Vec<Step<'_>> {
        Self::COMMANDS
            .iter()
            .zip(self.0.iter_mut())
            .map(|(command, rows)| Step::collect(command, rows))
            .collect()
    }
    /// The observed inventory, which must have been read under `expected`
    /// capabilities.
    pub(super) fn finish(self, expected: &BTreeSet<String>) -> Result<Inventory, Error> {
        let inventory = self.inventory()?;
        if inventory.capabilities != *expected {
            return Err(Error::Unstable);
        }
        Ok(inventory)
    }
    fn inventory(self) -> Result<Inventory, Error> {
        let [
            capabilities,
            namespaces,
            rows,
            namespaces_after,
            capabilities_after,
        ] = self.0;
        let capabilities = Session::parse_capabilities(capabilities)?;
        let namespaces = Session::parse_namespaces(namespaces)?;
        let mut mailboxes = vec![];
        for row in rows {
            if row.get(1).and_then(Token::atom) != Some("LIST") || row.get(2) != Some(&Token::Open)
            {
                return Err(Error::Protocol);
            }
            let mut at = 3;
            let mut attributes = BTreeSet::new();
            while row.get(at) != Some(&Token::Close) {
                let attr = row.get(at).and_then(Token::atom).ok_or(Error::Protocol)?;
                if !attr.starts_with('\\')
                    || !attributes.insert(attr.to_ascii_uppercase())
                    || attributes.len() > 32
                {
                    return Err(Error::Protocol);
                }
                at += 1;
            }
            let delimiter = match row.get(at + 1) {
                Some(Token::Atom(s)) if s == "NIL" => None,
                Some(t) => Some(t.text()?),
                _ => return Err(Error::Protocol),
            };
            let wire = row.get(at + 2).ok_or(Error::Protocol)?.text()?;
            if row.len() != at + 3 {
                return Err(Error::Unsupported);
            }
            mailboxes.push(Mailbox {
                name: ExactName::from_wire(&wire)?,
                delimiter,
                attributes,
            });
        }
        mailboxes.sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
        if Session::parse_namespaces(namespaces_after)? != namespaces
            || Session::parse_capabilities(capabilities_after)? != capabilities
        {
            return Err(Error::Unstable);
        }
        let inventory = Inventory {
            capabilities,
            namespaces,
            mailboxes,
        };
        inventory.validate()?;
        Ok(inventory)
    }
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use std::{os::fd::AsRawFd, sync::atomic::Ordering};

    #[test]
    fn saturated_verified_tls_writes_reach_pending_and_obey_cancel_and_absolute_deadline() {
        for cancelled in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (_repo, account, fake) =
                crate::acquisition::folder_provisioning::tests::install(root.path());
            std::fs::write(root.path().join("folder-mode"), "stall_write").unwrap();
            let cancel = Cancellation::default();
            let mut session = Session::connect(
                &fake.source.permit(&account.id).unwrap(),
                &account.login,
                &SecretValue::new(b"invented".to_vec()),
                cancel.clone(),
            )
            .unwrap();
            let ready_end = Instant::now() + Duration::from_secs(2);
            while !root.path().join("write-stalled").exists() {
                assert!(Instant::now() < ready_end);
                std::thread::sleep(POLL);
            }
            let size: libc::c_int = 4096;
            assert_eq!(
                unsafe {
                    libc::setsockopt(
                        session.stream.get_ref().as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_SNDBUF,
                        (&size as *const libc::c_int).cast(),
                        std::mem::size_of_val(&size) as libc::socklen_t,
                    )
                },
                0
            );
            let fill_end = Instant::now() + Duration::from_secs(1);
            let padding = vec![b'X'; 64 * 1024];
            let mut saturated = false;
            let mut filled = 0;
            while filled < TOTAL / 2 && Instant::now() < fill_end {
                match session.stream.write(&padding) {
                    Ok(n) => filled += n,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        saturated = true;
                        break;
                    }
                    Err(_) => panic!("Synthetic TLS saturation failed"),
                }
            }
            assert!(
                saturated,
                "Actual TLS write must report WouldBlock after bounded synthetic padding"
            );
            session.timeout = Duration::from_secs(2);
            session.command_end = Instant::now() + Duration::from_secs(2);
            session.end = Instant::now()
                + if cancelled {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(150)
                };
            let observed = session.pending_writes.clone();
            let stopping = if cancelled {
                Some(std::thread::spawn(move || {
                    let end = Instant::now() + Duration::from_secs(2);
                    while observed.load(Ordering::SeqCst) == 0 {
                        assert!(
                            Instant::now() < end,
                            "Actual TLS write must reach pending before cancellation"
                        );
                        std::thread::sleep(POLL);
                    }
                    cancel.cancel();
                }))
            } else {
                None
            };
            let started = Instant::now();
            let result = session.execute(b"CAPABILITY");
            assert_eq!(
                result.unwrap_err(),
                if cancelled {
                    Error::Cancelled
                } else {
                    Error::Deadline
                }
            );
            assert!(
                session.pending_writes.load(Ordering::SeqCst) > 0,
                "Observed actual native TLS/socket pending write, not a missing response"
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            if let Some(stopping) = stopping {
                stopping.join().unwrap();
            }
            std::fs::write(root.path().join("release-write"), b"release").unwrap();
            drop(session);
        }
    }

    /// The initial budget alone would end both sessions after 4 s. Completed
    /// commands extend it, so a progressing session outlives it, and without
    /// that credit the same session stops at the overall deadline. A 20 ms
    /// link keeps the session waiting on the server, which is what the
    /// deadline bounds, and keeps the commands within the fixture's log.
    #[test]
    fn completed_commands_extend_the_overall_deadline_and_without_progress_credit_it_still_ends() {
        let root = tempfile::tempdir().unwrap();
        let (_repo, account, fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        std::fs::write(root.path().join("folder-link"), "delay_ms=20").unwrap();
        let source = InstalledSource {
            account: fake.source.account.clone(),
            address: fake.source.address,
            tls_name: fake.source.tls_name.clone(),
            root: fake.source.root.clone(),
            epoch: fake.source.epoch.clone(),
            command_timeout: fake.source.command_timeout,
            operation_timeout: Duration::from_secs(4),
        };
        let password = SecretValue::new(b"invented".to_vec());
        for credited in [true, false] {
            let started = Instant::now();
            let mut session = Session::connect(
                &source.permit(&account.id).unwrap(),
                &account.login,
                &password,
                Cancellation::default(),
            )
            .unwrap();
            if !credited {
                session.credit = Duration::ZERO;
                session.end = started + source.operation_timeout;
            }
            let mut result = Ok(());
            while result.is_ok() && started.elapsed() < Duration::from_secs(9) {
                result = session.observe().map(|_| ());
            }
            if credited {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err(), Error::Deadline);
                assert!(started.elapsed() < Duration::from_secs(9));
            }
        }
    }

    /// The overall deadline bounds time spent waiting on the server. A
    /// session left idle past it, while its client works on something else,
    /// still runs its next command, which keeps its own deadline.
    #[test]
    fn idle_time_between_flights_does_not_use_up_the_overall_deadline() {
        let root = tempfile::tempdir().unwrap();
        let (_repo, account, fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        let source = InstalledSource {
            account: fake.source.account.clone(),
            address: fake.source.address,
            tls_name: fake.source.tls_name.clone(),
            root: fake.source.root.clone(),
            epoch: fake.source.epoch.clone(),
            command_timeout: fake.source.command_timeout,
            operation_timeout: Duration::from_secs(2),
        };
        let mut session = Session::connect(
            &source.permit(&account.id).unwrap(),
            &account.login,
            &SecretValue::new(b"invented".to_vec()),
            Cancellation::default(),
        )
        .unwrap();
        session.credit = Duration::ZERO;
        session.end = Instant::now() + Duration::from_secs(1);
        std::thread::sleep(Duration::from_millis(2500));
        session.observe().unwrap();
    }

    /// A session that has read past its initial byte budget keeps going
    /// while its commands complete, and stops once completed commands no
    /// longer cover what it reads.
    #[test]
    fn completed_commands_extend_the_byte_budget() {
        let root = tempfile::tempdir().unwrap();
        let (_repo, account, fake) =
            crate::acquisition::folder_provisioning::tests::install(root.path());
        let password = SecretValue::new(b"invented".to_vec());
        let mut session = Session::connect(
            &fake.source.permit(&account.id).unwrap(),
            &account.login,
            &password,
            Cancellation::default(),
        )
        .unwrap();
        session.received = TOTAL + 1;
        session.budget = TOTAL + 2 * RESPONSE;
        for _ in 0..3 {
            session.observe().unwrap();
        }
        assert!(session.received > TOTAL && session.budget > TOTAL + 2 * RESPONSE);
        session.received = session.budget;
        assert_eq!(session.observe().unwrap_err(), Error::Limit);
    }
}
