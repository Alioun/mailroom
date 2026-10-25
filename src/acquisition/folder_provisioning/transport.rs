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
const TOTAL: usize = 16 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(5);

pub(in crate::acquisition) mod message_move;

struct Endpoint<'a> {
    address: SocketAddr,
    tls_name: &'a str,
    root: &'a [u8],
    identity: String,
    command_timeout: Duration,
    operation_timeout: Duration,
}

pub(in crate::acquisition) struct InstalledSource {
    pub account: AccountId,
    pub address: SocketAddr,
    pub tls_name: String,
    pub root: Vec<u8>,
    pub epoch: String,
    pub command_timeout: Duration,
    pub operation_timeout: Duration,
}
pub(super) struct CreatePermit<'a>(&'a InstalledSource);
impl InstalledSource {
    pub(in crate::acquisition) fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            &self.account,
            self.address,
            &self.tls_name,
            &self.root,
            &self.epoch,
            crate::census::folder_provisioning::PROFILE,
        ))
        .expect("installed identity");
        data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
    }
    pub(super) fn permit(&self, account: &AccountId) -> Result<CreatePermit<'_>, Error> {
        if self.account != *account
            || !self.address.ip().is_loopback()
            || self.epoch.is_empty()
            || self.root.is_empty()
            || self.tls_name.is_empty()
        {
            return Err(Error::Unsupported);
        }
        Ok(CreatePermit(self))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
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
    received: usize,
    written: usize,
    remaining: usize,
    tag: u32,
    code_limit: usize,
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
        let source = permit.0;
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
        let tcp = TcpStream::connect_timeout(
            &source.address,
            Duration::from_millis(250).min(end.saturating_duration_since(Instant::now())),
        )
        .map_err(|_| Error::Connect)?;
        tcp.set_nonblocking(true).map_err(|_| Error::Connect)?;
        tcp.set_nodelay(true).map_err(|_| Error::Connect)?;
        let timeout = source.command_timeout.min(Duration::from_secs(10));
        let command_end = (Instant::now() + timeout).min(end);
        let mut builder = TlsConnector::builder();
        builder.min_protocol_version(Some(native_tls::Protocol::Tlsv12));
        builder.disable_built_in_roots(true);
        builder.add_root_certificate(
            native_tls::Certificate::from_der(source.root).map_err(|_| Error::Tls)?,
        );
        let connector = builder.build().map_err(|_| Error::Tls)?;
        let mut handshake = connector.connect(source.tls_name, tcp);
        let stream = loop {
            guard(&cancel, command_end)?;
            match handshake {
                Ok(s) => break s,
                Err(HandshakeError::WouldBlock(mid)) => {
                    std::thread::sleep(POLL);
                    handshake = mid.handshake();
                }
                Err(_) => return Err(Error::Tls),
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
            received: 0,
            written: 0,
            remaining: RESPONSE,
            tag: 0,
            code_limit: 256,
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
    fn check(&self) -> Result<(), Error> {
        guard(&self.cancel, self.end.min(self.command_end))
    }
    fn byte(&mut self) -> Result<u8, Error> {
        self.check()?;
        if self.remaining == 0 || self.received >= TOTAL {
            return Err(Error::Limit);
        }
        loop {
            self.check()?;
            let mut b = [0];
            match self.stream.read(&mut b) {
                Ok(0) => return Err(Error::Eof),
                Ok(_) => {
                    self.remaining -= 1;
                    self.received += 1;
                    return Ok(b[0]);
                }
                Err(e) if pending(&e) => std::thread::sleep(POLL),
                Err(_) => return Err(Error::Eof),
            }
        }
    }
    fn row(&mut self) -> Result<Vec<Token>, Error> {
        let mut tokens = vec![];
        let mut next = self.byte()?;
        let mut depth = 0usize;
        loop {
            if tokens.len() >= 512 {
                return Err(Error::Limit);
            }
            match next {
                b' ' => {
                    next = self.byte()?;
                    continue;
                }
                b'\r' => {
                    if self.byte()? != b'\n' || depth != 0 {
                        return Err(Error::Protocol);
                    }
                    return Ok(tokens);
                }
                b'(' => {
                    depth += 1;
                    if depth > 8 {
                        return Err(Error::Limit);
                    }
                    tokens.push(Token::Open);
                    next = self.byte()?;
                }
                b')' => {
                    depth = depth.checked_sub(1).ok_or(Error::Protocol)?;
                    tokens.push(Token::Close);
                    next = self.byte()?;
                }
                b'"' => {
                    let mut value = vec![];
                    loop {
                        let b = self.byte()?;
                        if b == b'"' {
                            break;
                        }
                        let b = if b == b'\\' {
                            let b = self.byte()?;
                            if !matches!(b, b'"' | b'\\') {
                                return Err(Error::Protocol);
                            }
                            b
                        } else {
                            b
                        };
                        if !(0x20..=0x7e).contains(&b) {
                            return Err(Error::Protocol);
                        }
                        value.push(b);
                        if value.len() > 2048 {
                            return Err(Error::Limit);
                        }
                    }
                    tokens.push(Token::String(value));
                    next = self.byte()?;
                    if !matches!(next, b' ' | b')' | b'\r') {
                        return Err(Error::Protocol);
                    }
                }
                b'{' => {
                    let mut digits = vec![];
                    loop {
                        let b = self.byte()?;
                        if b == b'}' {
                            break;
                        }
                        if !b.is_ascii_digit() || digits.len() >= 6 {
                            return Err(Error::Protocol);
                        }
                        digits.push(b);
                    }
                    let n: usize = std::str::from_utf8(&digits)
                        .map_err(|_| Error::Protocol)?
                        .parse()
                        .map_err(|_| Error::Protocol)?;
                    if n > 2048 || n > self.remaining {
                        return Err(Error::Limit);
                    }
                    if self.byte()? != b'\r' || self.byte()? != b'\n' {
                        return Err(Error::Protocol);
                    }
                    let mut value = Vec::with_capacity(n);
                    for _ in 0..n {
                        value.push(self.byte()?);
                    }
                    tokens.push(Token::String(value));
                    next = self.byte()?;
                    if !matches!(next, b' ' | b')' | b'\r') {
                        return Err(Error::Protocol);
                    }
                }
                b if b.is_ascii_graphic() && !matches!(b, b'"' | b'{' | b'}') => {
                    let mut value = vec![b];
                    loop {
                        let b = self.byte()?;
                        if matches!(b, b' ' | b'(' | b')' | b'\r') {
                            next = b;
                            break;
                        }
                        if !b.is_ascii_graphic()
                            || matches!(b, b'"' | b'{' | b'}')
                            || value.len() >= 256
                        {
                            return Err(Error::Protocol);
                        }
                        value.push(b);
                    }
                    tokens.push(Token::Atom(
                        String::from_utf8(value).map_err(|_| Error::Protocol)?,
                    ));
                    if tokens.len() == 2
                        && matches!(tokens[1].atom(), Some("OK" | "NO" | "BAD" | "BYE"))
                        && next == b' '
                    {
                        let mut text = vec![];
                        loop {
                            let b = self.byte()?;
                            if b == b'\r' {
                                if self.byte()? != b'\n' {
                                    return Err(Error::Protocol);
                                }
                                break;
                            }
                            if b == 0 || b == b'\n' || text.len() >= 8192.max(self.code_limit + 1) {
                                return Err(Error::Limit);
                            }
                            text.push(b);
                        }
                        if text.first() == Some(&b'[') {
                            let close = text
                                .iter()
                                .position(|b| *b == b']')
                                .ok_or(Error::Protocol)?;
                            if close > self.code_limit || !text[..=close].is_ascii() {
                                return Err(Error::Protocol);
                            }
                            tokens.push(Token::Atom(
                                String::from_utf8(text[..=close].to_vec())
                                    .map_err(|_| Error::Protocol)?,
                            ));
                        }
                        return Ok(tokens);
                    }
                }
                _ => return Err(Error::Protocol),
            }
        }
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
    fn begin(&mut self, command: &[u8]) -> Result<String, Error> {
        self.check()?;
        self.command_end = (Instant::now() + self.timeout).min(self.end);
        self.remaining = RESPONSE;
        self.tag = self.tag.checked_add(1).ok_or(Error::Limit)?;
        let tag = format!("F{}", self.tag);
        let bytes = Zeroizing::new([tag.as_bytes(), b" ", command, b"\r\n"].concat());
        if bytes.len() > TOTAL.saturating_sub(self.written) {
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
                Err(_) => return Err(Error::Eof),
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
                Err(_) => return Err(Error::Eof),
            }
        }
        Ok(tag)
    }
    fn execute(&mut self, command: &[u8]) -> Result<Vec<Vec<Token>>, Error> {
        let tag = self.begin(command)?;
        let mut rows = vec![];
        loop {
            let row = self.row()?;
            let first = row.first().and_then(Token::atom).ok_or(Error::Protocol)?;
            if first == tag {
                Self::status(&row, 1)?;
                return Ok(rows);
            }
            if first != "*" || row.len() < 2 {
                return Err(Error::Protocol);
            }
            if matches!(row[1].atom(), Some("OK" | "NO" | "BAD" | "BYE")) {
                Self::status(&row, 1)?;
            } else {
                if rows.len() >= 256 {
                    return Err(Error::Limit);
                }
                rows.push(row);
            }
        }
    }
    fn capabilities(&mut self) -> Result<BTreeSet<String>, Error> {
        let rows = self.execute(b"CAPABILITY")?;
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
    fn namespaces(&mut self) -> Result<Namespaces, Error> {
        let rows = self.execute(b"NAMESPACE")?;
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
        let capabilities = self.capabilities()?;
        let namespaces = self.namespaces()?;
        let rows = self.execute(b"LIST \"\" \"*\"")?;
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
        if self.namespaces()? != namespaces || self.capabilities()? != capabilities {
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
    pub(super) fn create(
        &mut self,
        permit: &CreatePermit<'_>,
        name: &ExactName,
    ) -> Result<(), Error> {
        if self.source != permit.0.identity()
            || ExactName::from_wire(&name.wire)? != *name
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
}
