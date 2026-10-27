//! SMTP submission of one exact message over a verified TLS connection.
//!
//! The protocol never retries. A failure before message data is written means
//! the server cannot have accepted the message. After that, only a final reply
//! decides the outcome; anything else is uncertain.
use super::smtp_secret;
use crate::acquisition::CredentialBackend;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// TLS from the first byte (port 465).
    ImplicitTls,
    /// STARTTLS before anything else is sent (port 587). A server that does not
    /// offer it is refused, never used in plain text.
    StartTls,
}

/// Where and how to submit. The certificate must be valid for `tls_name`.
pub(crate) struct Server {
    pub address: SocketAddr,
    pub tls_name: String,
    pub security: Security,
    pub tls: native_tls::TlsConnector,
}

#[derive(Clone, Copy)]
pub(crate) struct Timeouts {
    /// Each read and write before the final reply.
    pub command: Duration,
    /// The final reply after the message's end; RFC 5321 section 4.5.3.2.6 allows 10 minutes.
    pub final_reply: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            command: Duration::from_secs(60),
            final_reply: Duration::from_secs(600),
        }
    }
}

/// The account's login and where its password is kept. Only `smtp_secret` reads the password.
pub(crate) struct Login<'a> {
    pub user: &'a str,
    pub backend: &'a dyn CredentialBackend,
    pub reference: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Input,
    Connect,
    Tls,
    Greeting,
    Ehlo,
    StartTls,
    Auth,
    Mail,
    Rcpt,
    Data,
    Message,
}

/// What one submission established. Reply text is never kept: a server may echo
/// anything it received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    /// Refused before any message data was written, so nothing was submitted.
    NotSent { stage: Stage, code: Option<u16> },
    /// The server took responsibility for the message (2xx after its end).
    Accepted { code: u16, enhanced: Option<String> },
    /// The server refused the complete message.
    Rejected { code: u16, enhanced: Option<String> },
    /// Message data was written but no final reply arrived.
    Uncertain { stage: Stage },
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}
impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}
impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => s,
            Self::Tls(s) => s.get_ref(),
        }
    }
}

pub(super) struct Reply {
    pub code: u16,
    pub lines: Vec<String>,
}
impl Reply {
    fn enhanced(&self) -> Option<String> {
        let first = self.lines.first()?;
        let candidate = first.split_whitespace().next()?;
        let parts: Vec<_> = candidate.split('.').collect();
        (parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
            && candidate.starts_with(char::from(b'0' + (self.code / 100) as u8)))
        .then(|| candidate.to_owned())
    }
}

const LINE_LIMIT: usize = 1024;
const REPLY_LINES: usize = 64;

/// One SMTP conversation. It writes only what its caller asks for.
pub(crate) struct Session {
    io: BufReader<Stream>,
    pub(super) extensions: BTreeSet<String>,
}
impl Session {
    pub(super) fn reply(&mut self) -> io::Result<Reply> {
        let malformed = || io::Error::other("Malformed SMTP reply");
        let mut first = None;
        let mut lines = vec![];
        loop {
            let mut line = vec![];
            let read = (&mut self.io)
                .take(LINE_LIMIT as u64)
                .read_until(b'\n', &mut line)?;
            if read == 0 || !line.ends_with(b"\r\n") || line.len() < 5 || lines.len() >= REPLY_LINES
            {
                return Err(malformed());
            }
            let code = std::str::from_utf8(&line[..3])
                .ok()
                .filter(|c| c.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|c| c.parse::<u16>().ok())
                .filter(|c| (200..600).contains(c))
                .ok_or_else(malformed)?;
            if *first.get_or_insert(code) != code {
                return Err(malformed());
            }
            lines.push(String::from_utf8_lossy(&line[4..line.len() - 2]).into_owned());
            match line[3] {
                b' ' => return Ok(Reply { code, lines }),
                b'-' => {}
                _ => return Err(malformed()),
            }
        }
    }
    /// Writes one command line. Commands never carry secrets; `smtp_secret` writes its own.
    fn command(&mut self, line: &str) -> io::Result<Reply> {
        if line.contains(['\r', '\n']) {
            return Err(io::Error::other("Command line break"));
        }
        let stream = self.io.get_mut();
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
        self.reply()
    }
    /// Writes bytes that `smtp_secret` built, without keeping a copy.
    pub(super) fn write_line(&mut self, bytes: &[u8]) -> io::Result<()> {
        let stream = self.io.get_mut();
        stream.write_all(bytes)?;
        stream.flush()
    }
    fn ehlo(&mut self) -> io::Result<Reply> {
        let reply = self.command("EHLO mailroom.localhost")?;
        if reply.code == 250 {
            self.extensions = reply
                .lines
                .iter()
                .skip(1)
                .filter_map(|l| l.split_whitespace().next())
                .map(|k| k.to_ascii_uppercase())
                .collect();
            if let Some(auth) = reply
                .lines
                .iter()
                .skip(1)
                .find(|l| l.to_ascii_uppercase().starts_with("AUTH "))
            {
                self.extensions.extend(
                    auth.split_whitespace()
                        .skip(1)
                        .map(|m| format!("AUTH={}", m.to_ascii_uppercase())),
                );
            }
        }
        Ok(reply)
    }
    fn set_timeout(&self, timeout: Duration) {
        let tcp = self.io.get_ref().tcp();
        let _ = tcp.set_read_timeout(Some(timeout));
        let _ = tcp.set_write_timeout(Some(timeout));
    }
}

fn not_sent(stage: Stage, code: Option<u16>) -> Outcome {
    Outcome::NotSent { stage, code }
}

/// The message as written after DATA: dot-stuffed, ending in CRLF.CRLF. The
/// message must use CRLF line ends only and end with one, so the server stores
/// exactly `message`.
fn data(message: &[u8]) -> Option<Vec<u8>> {
    if message.is_empty() || !message.ends_with(b"\r\n") {
        return None;
    }
    let mut out = Vec::with_capacity(message.len() + message.len() / 64 + 5);
    let mut start = true;
    for (i, &b) in message.iter().enumerate() {
        if (b == b'\n' && (i == 0 || message[i - 1] != b'\r'))
            || (b == b'\r' && message.get(i + 1) != Some(&b'\n'))
        {
            return None;
        }
        if start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        start = b == b'\n';
    }
    out.extend_from_slice(b".\r\n");
    Some(out)
}

/// Submits `message` from `sender` to exactly `recipient`. The caller records
/// its intent durably before calling and records the outcome after.
pub(crate) fn submit(
    server: &Server,
    login: &Login<'_>,
    sender: &str,
    recipient: &str,
    message: &[u8],
    timeouts: Timeouts,
) -> Outcome {
    if !crate::rules::config::mailbox(sender) || !crate::rules::config::mailbox(recipient) {
        return not_sent(Stage::Input, None);
    }
    let Some(data) = data(message) else {
        return not_sent(Stage::Input, None);
    };
    let eight_bit = !message.is_ascii();
    let Ok(tcp) = TcpStream::connect_timeout(&server.address, timeouts.command) else {
        return not_sent(Stage::Connect, None);
    };
    let _ = tcp.set_nodelay(true);
    let mut session = match server.security {
        Security::ImplicitTls => match server.tls.connect(&server.tls_name, tcp) {
            Ok(tls) => Session {
                io: BufReader::new(Stream::Tls(Box::new(tls))),
                extensions: BTreeSet::new(),
            },
            Err(_) => return not_sent(Stage::Tls, None),
        },
        Security::StartTls => Session {
            io: BufReader::new(Stream::Plain(tcp)),
            extensions: BTreeSet::new(),
        },
    };
    session.set_timeout(timeouts.command);
    match session.reply() {
        Ok(r) if r.code == 220 => {}
        Ok(r) => return not_sent(Stage::Greeting, Some(r.code)),
        Err(_) => return not_sent(Stage::Greeting, None),
    }
    match session.ehlo() {
        Ok(r) if r.code == 250 => {}
        Ok(r) => return not_sent(Stage::Ehlo, Some(r.code)),
        Err(_) => return not_sent(Stage::Ehlo, None),
    }
    if server.security == Security::StartTls {
        if !session.extensions.contains("STARTTLS") {
            return not_sent(Stage::StartTls, None);
        }
        match session.command("STARTTLS") {
            Ok(r) if r.code == 220 => {}
            Ok(r) => return not_sent(Stage::StartTls, Some(r.code)),
            Err(_) => return not_sent(Stage::StartTls, None),
        }
        // Bytes the server sent after its 220 arrived before TLS and could be an
        // injected reply, so they end the session (RFC 3207 section 4.2).
        if !session.io.buffer().is_empty() {
            return not_sent(Stage::StartTls, None);
        }
        let Stream::Plain(tcp) = session.io.into_inner() else {
            return not_sent(Stage::StartTls, None);
        };
        session = match server.tls.connect(&server.tls_name, tcp) {
            Ok(tls) => Session {
                io: BufReader::new(Stream::Tls(Box::new(tls))),
                extensions: BTreeSet::new(),
            },
            Err(_) => return not_sent(Stage::Tls, None),
        };
        session.set_timeout(timeouts.command);
        match session.ehlo() {
            Ok(r) if r.code == 250 => {}
            Ok(r) => return not_sent(Stage::Ehlo, Some(r.code)),
            Err(_) => return not_sent(Stage::Ehlo, None),
        }
    }
    if eight_bit && !session.extensions.contains("8BITMIME") {
        return not_sent(Stage::Mail, None);
    }
    if let Err(code) = smtp_secret::authenticate(&mut session, login) {
        return not_sent(Stage::Auth, code);
    }
    let body = if eight_bit { " BODY=8BITMIME" } else { "" };
    for (stage, line, expected) in [
        (Stage::Mail, format!("MAIL FROM:<{sender}>{body}"), 250),
        (Stage::Rcpt, format!("RCPT TO:<{recipient}>"), 250),
        (Stage::Data, "DATA".to_owned(), 354),
    ] {
        match session.command(&line) {
            Ok(r) if r.code == expected || (stage == Stage::Rcpt && r.code == 251) => {}
            Ok(r) => return not_sent(stage, Some(r.code)),
            Err(_) => return not_sent(stage, None),
        }
    }
    if session.write_line(&data).is_err() {
        return Outcome::Uncertain {
            stage: Stage::Message,
        };
    }
    session.set_timeout(timeouts.final_reply);
    let outcome = match session.reply() {
        Ok(r) if (200..300).contains(&r.code) => Outcome::Accepted {
            code: r.code,
            enhanced: r.enhanced(),
        },
        Ok(r) => Outcome::Rejected {
            code: r.code,
            enhanced: r.enhanced(),
        },
        Err(_) => Outcome::Uncertain {
            stage: Stage::Message,
        },
    };
    session.set_timeout(timeouts.command);
    let _ = session.command("QUIT");
    outcome
}

#[cfg(test)]
#[path = "smtp_tests.rs"]
pub(crate) mod tests;
