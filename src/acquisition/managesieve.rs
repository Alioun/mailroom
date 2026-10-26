use super::{Cancellation, SecretValue};
use crate::census::sieve::{Capabilities, ScriptName};
use native_tls::{HandshakeError, TlsConnector, TlsStream};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const RESPONSE: usize = 64 * 1024;
pub(crate) const SCRIPT: usize = 1024 * 1024;
const ATTEMPT: usize = 4 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(5);

pub(crate) use crate::census::sieve::Failure as Error;

pub(super) struct InstalledSource {
    pub address: SocketAddr,
    pub tls_name: String,
    pub root: Vec<u8>,
    pub epoch: String,
    pub command_timeout: Duration,
    pub attempt_timeout: Duration,
}
impl InstalledSource {
    pub(super) fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let identity = serde_json::to_vec(&(&self.epoch, self.address, &self.tls_name, &self.root))
            .expect("installed source identity");
        data_encoding::HEXLOWER.encode(&Sha256::digest(identity))
    }
    pub(super) fn permit(&self) -> Result<SieveReadPermit<'_>, Error> {
        if !self.address.ip().is_loopback() || self.epoch.is_empty() {
            return Err(Error::Unsupported);
        }
        Ok(SieveReadPermit(self))
    }
}
pub(super) struct FixtureOwnership {
    pub account: crate::census::AccountId,
    pub target: String,
    pub epoch: String,
    pub source_identity: String,
}
pub(super) struct SieveWritePermit<'a> {
    source: &'a InstalledSource,
    pub ownership: &'a FixtureOwnership,
}
impl InstalledSource {
    pub(super) fn write_permit<'a>(
        &'a self,
        ownership: &'a FixtureOwnership,
        account: &crate::census::AccountId,
    ) -> Result<SieveWritePermit<'a>, Error> {
        self.permit()?;
        if ownership.account != *account
            || ownership.epoch.is_empty()
            || ownership.source_identity != self.identity()
            || !crate::census::sieve::valid_script_name(&ownership.target)
        {
            return Err(Error::Unsupported);
        }
        Ok(SieveWritePermit {
            source: self,
            ownership,
        })
    }
}
pub(super) struct SieveReadPermit<'a>(&'a InstalledSource);

enum Stream {
    Plain(TcpStream),
    Secure(TlsStream<TcpStream>),
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(bytes),
            Self::Secure(s) => s.read(bytes),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(bytes),
            Self::Secure(s) => s.write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Secure(s) => s.flush(),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Token {
    String(Vec<u8>),
    Atom(Vec<u8>),
    Open,
    Close,
}

pub(super) struct Session {
    source_identity: String,
    stream: Stream,
    cancel: Cancellation,
    end: Instant,
    command_end: Instant,
    timeout: Duration,
    total: usize,
    written: usize,
    remaining: usize,
    buffer: Vec<u8>,
    cursor: usize,
}
fn check(cancel: &Cancellation, end: Instant) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= end {
        return Err(Error::Deadline);
    }
    Ok(())
}
fn pending(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}
impl Session {
    pub(super) fn connect(
        permit: SieveReadPermit<'_>,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<(Self, Capabilities), Error> {
        let source = permit.0;
        let end = Instant::now() + source.attempt_timeout.min(Duration::from_secs(60));
        check(&cancel, end)?;
        if login.is_empty()
            || login.len() > 1024
            || login.contains('\0')
            || password.bytes().is_empty()
            || password.bytes().len() > 4096
            || password.bytes().contains(&0)
            || std::str::from_utf8(password.bytes()).is_err()
        {
            return Err(Error::Unsupported);
        }
        let tcp = TcpStream::connect_timeout(
            &source.address,
            Duration::from_millis(250).min(end.saturating_duration_since(Instant::now())),
        )
        .map_err(|_error| {
            #[cfg(test)]
            eprintln!("sieve.transport.connect.io_kind={:?}", _error.kind());
            Error::Connect
        })?;
        tcp.set_nonblocking(true).map_err(|_| Error::Connect)?;
        let mut session = Self {
            source_identity: source.identity(),
            stream: Stream::Plain(tcp),
            cancel,
            end,
            command_end: end,
            timeout: source.command_timeout.min(Duration::from_secs(15)),
            total: 0,
            written: 0,
            remaining: RESPONSE,
            buffer: vec![],
            cursor: 0,
        };
        session.begin(RESPONSE);
        let greeting = session.capability_response()?;
        if !greeting.contains_key("STARTTLS") {
            return Err(Error::Unsupported);
        }
        session.command(b"STARTTLS\r\n", RESPONSE)?;
        session.status_only()?;
        if session.cursor != session.buffer.len() {
            return Err(Error::Protocol);
        }
        session.begin(RESPONSE);
        let mut builder = TlsConnector::builder();
        builder.min_protocol_version(Some(native_tls::Protocol::Tlsv12));
        builder.disable_built_in_roots(true);
        builder.add_root_certificate(
            native_tls::Certificate::from_der(&source.root).map_err(|_| Error::Tls)?,
        );
        let connector = builder.build().map_err(|_| Error::Tls)?;
        let Stream::Plain(tcp) = session.stream else {
            unreachable!()
        };
        let mut handshake = connector.connect(&source.tls_name, tcp);
        let tls = loop {
            check(&session.cancel, session.command_end.min(session.end))?;
            match handshake {
                Ok(tls) => break tls,
                Err(HandshakeError::WouldBlock(mid)) => {
                    std::thread::sleep(POLL);
                    handshake = mid.handshake();
                }
                Err(_) => return Err(Error::Tls),
            }
        };
        session.stream = Stream::Secure(tls);
        session.buffer.clear();
        session.cursor = 0;
        let secured = session.capability_response()?;
        if secured.contains_key("STARTTLS") {
            return Err(Error::Protocol);
        }
        if !secured
            .get("SASL")
            .is_some_and(|v| v.split_ascii_whitespace().any(|s| s == "PLAIN"))
        {
            return Err(Error::Unsupported);
        }
        let mut plain =
            Zeroizing::new(Vec::with_capacity(login.len() + password.bytes().len() + 2));
        plain.push(0);
        plain.extend_from_slice(login.as_bytes());
        plain.push(0);
        plain.extend_from_slice(password.bytes());
        let encoded = Zeroizing::new(data_encoding::BASE64.encode(&plain));
        let command = Zeroizing::new(format!(
            "AUTHENTICATE \"PLAIN\" \"{}\"\r\n",
            encoded.as_str()
        ));
        session.command(command.as_bytes(), RESPONSE)?;
        session.status_only()?;
        let capabilities = session.capabilities()?;
        Ok((session, capabilities))
    }
    fn begin(&mut self, limit: usize) {
        self.command_end = (Instant::now() + self.timeout).min(self.end);
        self.remaining = limit;
    }
    fn guard(&self) -> Result<(), Error> {
        check(&self.cancel, self.command_end.min(self.end))
    }
    fn command(&mut self, bytes: &[u8], limit: usize) -> Result<(), Error> {
        if bytes.len() > ATTEMPT - self.written {
            return Err(Error::Limit);
        }
        self.written += bytes.len();
        self.begin(limit);
        let mut offset = 0;
        while offset < bytes.len() {
            self.guard()?;
            match self.stream.write(&bytes[offset..]) {
                Ok(0) => return Err(Error::Eof),
                Ok(n) => offset += n,
                Err(e) if pending(&e) => std::thread::sleep(POLL),
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("sieve.transport.write.io_kind={:?}", _error.kind());
                    return Err(Error::Eof);
                }
            }
        }
        loop {
            self.guard()?;
            match self.stream.flush() {
                Ok(()) => return Ok(()),
                Err(e) if pending(&e) => std::thread::sleep(POLL),
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("sieve.transport.flush.io_kind={:?}", _error.kind());
                    return Err(Error::Eof);
                }
            }
        }
    }
    fn byte(&mut self) -> Result<u8, Error> {
        self.guard()?;
        if self.remaining == 0 || self.total >= ATTEMPT {
            return Err(Error::Limit);
        }
        if self.cursor == self.buffer.len() {
            self.buffer.resize(4096, 0);
            loop {
                self.guard()?;
                match self.stream.read(&mut self.buffer) {
                    Ok(0) => {
                        #[cfg(test)]
                        eprintln!("sieve.transport.read.peer_closed");
                        return Err(Error::Eof);
                    }
                    Ok(n) => {
                        self.buffer.truncate(n);
                        self.cursor = 0;
                        break;
                    }
                    Err(e) if pending(&e) => std::thread::sleep(POLL),
                    Err(_error) => {
                        #[cfg(test)]
                        eprintln!("sieve.transport.read.io_kind={:?}", _error.kind());
                        return Err(Error::Eof);
                    }
                }
            }
        }
        let byte = self.buffer[self.cursor];
        self.cursor += 1;
        self.total += 1;
        self.remaining -= 1;
        Ok(byte)
    }
    fn crlf(&mut self) -> Result<(), Error> {
        if self.byte()? != b'\r' || self.byte()? != b'\n' {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    fn row(&mut self) -> Result<Vec<Token>, Error> {
        let mut tokens = Vec::new();
        let mut next = self.byte()?;
        loop {
            if tokens.len() >= 1024 {
                return Err(Error::Limit);
            }
            let token = match next {
                b'"' => {
                    let mut value = vec![];
                    let mut quoted_octets = 0;
                    loop {
                        let b = self.byte()?;
                        if b == b'"' {
                            break;
                        }
                        quoted_octets += 1;
                        let b = if b == b'\\' {
                            let b = self.byte()?;
                            quoted_octets += 1;
                            if !matches!(b, b'"' | b'\\') {
                                return Err(Error::Protocol);
                            }
                            b
                        } else {
                            b
                        };
                        if b == 0 || b == b'\r' || b == b'\n' {
                            return Err(Error::Protocol);
                        }
                        value.push(b);
                        if quoted_octets > 1024 {
                            return Err(Error::Limit);
                        }
                    }
                    std::str::from_utf8(&value).map_err(|_| Error::Protocol)?;
                    Token::String(value)
                }
                b'{' => {
                    let mut number = Vec::new();
                    loop {
                        let b = self.byte()?;
                        if b == b'}' {
                            break;
                        }
                        if !b.is_ascii_digit() || number.len() >= 10 {
                            return Err(Error::Protocol);
                        }
                        number.push(b);
                    }
                    if number.is_empty() || (number.len() > 1 && number[0] == b'0') {
                        return Err(Error::Protocol);
                    }
                    let n: u32 = std::str::from_utf8(&number)
                        .map_err(|_| Error::Protocol)?
                        .parse()
                        .map_err(|_| Error::Limit)?;
                    let n = n as usize;
                    if n > SCRIPT || n > self.remaining || n > ATTEMPT - self.total {
                        return Err(Error::Limit);
                    }
                    self.crlf()?;
                    let mut value = Vec::with_capacity(n);
                    for _ in 0..n {
                        value.push(self.byte()?);
                    }
                    Token::String(value)
                }
                b'(' => Token::Open,
                b')' => Token::Close,
                b'\r' if tokens.is_empty() => return Err(Error::Protocol),
                b if b.is_ascii_graphic() && !matches!(b, b'\\' | b'{' | b'"') => {
                    let mut atom = vec![b];
                    loop {
                        let b = self.byte()?;
                        if matches!(b, b' ' | b'\r' | b')') {
                            next = b;
                            break;
                        }
                        if !b.is_ascii_graphic()
                            || matches!(b, b'(' | b'{' | b'"' | b'\\')
                            || atom.len() >= 128
                        {
                            return Err(Error::Protocol);
                        }
                        atom.push(b);
                    }
                    if atom.iter().all(u8::is_ascii_digit)
                        && (atom.len() > 10
                            || (atom.len() > 1 && atom[0] == b'0')
                            || std::str::from_utf8(&atom)
                                .ok()
                                .and_then(|s| s.parse::<u32>().ok())
                                .is_none())
                    {
                        return Err(Error::Protocol);
                    }
                    tokens.push(Token::Atom(atom));
                    if next == b')' {
                        continue;
                    }
                    if next == b'\r' {
                        if self.byte()? != b'\n' {
                            return Err(Error::Protocol);
                        }
                        return Ok(tokens);
                    }
                    next = self.byte()?;
                    continue;
                }
                _ => return Err(Error::Protocol),
            };
            let open = token == Token::Open;
            tokens.push(token);
            next = self.byte()?;
            if next == b'\r' {
                if self.byte()? != b'\n' {
                    return Err(Error::Protocol);
                }
                return Ok(tokens);
            }
            if next == b' ' {
                next = self.byte()?;
            } else if next != b')' && !open {
                return Err(Error::Protocol);
            }
        }
    }
    fn terminal(row: &[Token]) -> Result<Option<()>, Error> {
        let Some(Token::Atom(atom)) = row.first() else {
            return Ok(None);
        };
        let mut outcome = if atom.eq_ignore_ascii_case(b"OK") {
            Ok(Some(()))
        } else if atom.eq_ignore_ascii_case(b"NO") {
            Err(Error::Refused)
        } else if atom.eq_ignore_ascii_case(b"BYE") {
            Err(Error::Bye)
        } else {
            return Err(Error::Protocol);
        };
        let mut index = 1;
        if row.get(index) == Some(&Token::Open) {
            index += 1;
            if !matches!(row.get(index), Some(Token::Atom(_))) {
                return Err(Error::Protocol);
            }
            if let Some(Token::Atom(code)) = row.get(index) {
                if code.starts_with(b"/") {
                    return Err(Error::Protocol);
                }
                let parent = code.split(|b| *b == b'/').next().unwrap_or_default();
                if atom.eq_ignore_ascii_case(b"NO") {
                    if parent.eq_ignore_ascii_case(b"QUOTA") {
                        outcome = Err(Error::Quota);
                    }
                    if parent.eq_ignore_ascii_case(b"NONEXISTENT") {
                        outcome = Err(Error::Missing);
                    }
                    if parent.eq_ignore_ascii_case(b"REFERRAL") {
                        outcome = Err(Error::Referral);
                    }
                }
            }
            let mut depth = 1;
            while index < row.len() && depth > 0 {
                match row[index] {
                    Token::Open => depth += 1,
                    Token::Close => depth -= 1,
                    _ => {}
                }
                if depth > 8 {
                    return Err(Error::Limit);
                }
                index += 1;
            }
            if depth != 0 {
                return Err(Error::Protocol);
            }
        }
        if index < row.len() {
            if !matches!(row.get(index), Some(Token::String(_))) || index + 1 != row.len() {
                return Err(Error::Protocol);
            }
        }
        outcome
    }
    fn status_only(&mut self) -> Result<(), Error> {
        Self::terminal(&self.row()?)?.ok_or(Error::Protocol)
    }
    fn capability_response(&mut self) -> Result<Capabilities, Error> {
        let mut caps = BTreeMap::new();
        loop {
            let row = self.row()?;
            if Self::terminal(&row)?.is_some() {
                break;
            }
            let [Token::String(name), rest @ ..] = row.as_slice() else {
                return Err(Error::Protocol);
            };
            let name = std::str::from_utf8(name)
                .map_err(|_| Error::Protocol)?
                .to_ascii_uppercase();
            if name.is_empty() || name.len() > 128 || !name.bytes().all(|c| c.is_ascii_graphic()) {
                return Err(Error::Protocol);
            }
            if matches!(
                name.as_str(),
                "IMPLEMENTATION" | "SIEVE" | "VERSION" | "SASL"
            ) && rest.is_empty()
            {
                return Err(Error::Protocol);
            }
            let value = match rest {
                [] => String::new(),
                [Token::String(v)] => String::from_utf8(v.clone()).map_err(|_| Error::Protocol)?,
                _ => return Err(Error::Protocol),
            };
            if value.len() > 8192 || caps.len() >= 256 {
                return Err(Error::Limit);
            }
            if caps.insert(name, value).is_some() {
                return Err(Error::Protocol);
            }
        }
        if !caps.contains_key("SIEVE")
            || !caps.contains_key("IMPLEMENTATION")
            || caps.get("VERSION").map(String::as_str) != Some("1.0")
        {
            return Err(Error::Unsupported);
        }
        Ok(caps)
    }
    pub(super) fn capabilities(&mut self) -> Result<Capabilities, Error> {
        self.command(b"CAPABILITY\r\n", RESPONSE)?;
        self.capability_response()
    }
    pub(super) fn list(&mut self) -> Result<Vec<ScriptName>, Error> {
        self.command(b"LISTSCRIPTS\r\n", RESPONSE)?;
        let mut scripts: Vec<ScriptName> = vec![];
        loop {
            let row = self.row()?;
            if Self::terminal(&row)?.is_some() {
                break;
            }
            let [Token::String(name), rest @ ..] = row.as_slice() else {
                return Err(Error::Protocol);
            };
            let name = String::from_utf8(name.clone()).map_err(|_| Error::Protocol)?;
            if !crate::census::sieve::valid_script_name(&name) {
                return Err(Error::Protocol);
            }
            let active = match rest {
                [] => false,
                [Token::Atom(v)] if v.eq_ignore_ascii_case(b"ACTIVE") => true,
                _ => return Err(Error::Protocol),
            };
            if scripts.len() >= 256 {
                return Err(Error::Limit);
            }
            if scripts
                .iter()
                .any(|s| s.name == name || (s.active && active))
            {
                return Err(Error::Protocol);
            }
            scripts.push(ScriptName { name, active });
        }
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(scripts)
    }
    pub(super) fn get(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        if !crate::census::sieve::valid_script_name(name) {
            return Err(Error::Protocol);
        }
        let command = format!("GETSCRIPT {{{}+}}\r\n{}\r\n", name.len(), name);
        self.command(command.as_bytes(), SCRIPT + RESPONSE)?;
        let row = self.row()?;
        if Self::terminal(&row)?.is_some() {
            return Err(Error::Protocol);
        }
        let [Token::String(bytes)] = row.as_slice() else {
            return Err(Error::Protocol);
        };
        let bytes = bytes.clone();
        self.status_only()?;
        Ok(bytes)
    }
    fn write_authorized(&self, permit: &SieveWritePermit<'_>) -> Result<(), Error> {
        if self.source_identity != permit.source.identity() {
            return Err(Error::Unsupported);
        }
        Ok(())
    }
    pub(super) fn check_script(
        &mut self,
        permit: &SieveWritePermit<'_>,
        bytes: &[u8],
    ) -> Result<(), Error> {
        self.write_authorized(permit)?;
        if self.capabilities()?.get("VERSION").map(String::as_str) != Some("1.0") {
            return Err(Error::Unsupported);
        }
        if bytes.len() > SCRIPT {
            return Err(Error::Limit);
        }
        let command = Zeroizing::new(
            [
                format!("CHECKSCRIPT {{{}+}}\r\n", bytes.len()).as_bytes(),
                bytes,
                b"\r\n",
            ]
            .concat(),
        );
        self.command(&command, RESPONSE)?;
        self.status_only()
    }
    pub(super) fn have_space(
        &mut self,
        permit: &SieveWritePermit<'_>,
        bytes: usize,
    ) -> Result<(), Error> {
        self.write_authorized(permit)?;
        if bytes > SCRIPT {
            return Err(Error::Limit);
        }
        let name = &permit.ownership.target;
        let command = format!("HAVESPACE {{{}+}}\r\n{} {}\r\n", name.len(), name, bytes);
        self.command(command.as_bytes(), RESPONSE)?;
        self.status_only()
    }
    pub(super) fn put_script(
        &mut self,
        permit: &SieveWritePermit<'_>,
        bytes: &[u8],
    ) -> Result<(), Error> {
        self.write_authorized(permit)?;
        if bytes.len() > SCRIPT {
            return Err(Error::Limit);
        }
        let name = &permit.ownership.target;
        let command = Zeroizing::new(
            [
                format!(
                    "PUTSCRIPT {{{}+}}\r\n{} {{{}+}}\r\n",
                    name.len(),
                    name,
                    bytes.len()
                )
                .as_bytes(),
                bytes,
                b"\r\n",
            ]
            .concat(),
        );
        self.command(&command, RESPONSE)?;
        self.status_only()
    }
    pub(super) fn set_active(
        &mut self,
        permit: &SieveWritePermit<'_>,
        enabled: bool,
    ) -> Result<(), Error> {
        self.write_authorized(permit)?;
        let name = if enabled {
            permit.ownership.target.as_str()
        } else {
            ""
        };
        let command = format!("SETACTIVE {{{}+}}\r\n{}\r\n", name.len(), name);
        self.command(command.as_bytes(), RESPONSE)?;
        self.status_only()
    }
    pub(super) fn logout(&mut self) -> Result<(), Error> {
        self.command(b"LOGOUT\r\n", RESPONSE)?;
        match self.status_only() {
            Ok(()) | Err(Error::Bye) => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
