use super::{AcquisitionError, Provider, SecretValue, scan::Cancellation};
use crate::{
    census::{
        Census, Direction, FolderCensus, HEADER_FIELDS, MessageRecord, ScanState,
        acquisition::{CountObservation, CountSource, UnknownCount, now},
        parse_headers,
    },
    policy::ReadPermit,
};
use imap_proto::{
    AttributeValue, MailboxDatum, MessageSection, NameAttribute, Response, ResponseCode,
    SectionPath, Status,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// A header fetch asks for at most this many bytes of each message's header,
/// so its response is bounded by the count it asks for.
const HEADER_BYTES: usize = 65_537;
/// Bytes a FETCH response row adds around its data.
const ROW_OVERHEAD: usize = 1024;
/// Header fetches aim at responses of about this size, from the mean header
/// seen so far, so a connection carries few large commands instead of many
/// small ones.
const HEADER_TARGET: usize = 1024 * 1024;
const HEADER_COMMAND_MAX: usize = 500;
/// Metadata rows are small; one command carries up to this many.
const METADATA_COMMAND_MAX: usize = 5_000;
const METADATA_ROW_LIMIT: usize = 4 * 1024;
/// UID sets stay under this length so command lines stay well inside the
/// 8,000-octet limit RFC 7162 section 4 asks clients to respect.
const SET_LENGTH: usize = 4_000;
/// Commands in flight at once. A flight is written before its responses are
/// read, so its commands must fit the socket buffers: a server blocked
/// writing a response it cannot send stops reading commands.
const WINDOW_COMMANDS: usize = 32;
const WINDOW_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(super) struct Folder {
    pub roles: BTreeSet<String>,
    pub attributes: BTreeSet<String>,
    pub delimiter: Option<String>,
    pub selectable: bool,
    pub wire: String,
    pub name: String,
    pub sent: bool,
    pub all: bool,
    pub excluded: bool,
}
#[derive(Clone)]
pub(super) struct Metadata {
    pub uid: u32,
    pub size: u64,
    pub date: String,
    pub flags: BTreeSet<String>,
    pub gmail_id: Option<String>,
    pub thread_id: Option<String>,
    pub labels: BTreeSet<String>,
}
impl Metadata {
    pub fn excluded(&self) -> bool {
        self.labels.iter().any(|s| {
            ["\\Trash", "\\Spam", "\\Junk", "\\Draft", "\\Drafts"]
                .iter()
                .any(|v| s.eq_ignore_ascii_case(v))
        })
    }
    pub fn apply(&self, record: &mut MessageRecord, folder: &Folder) {
        record.size = self.size;
        record.internal_date = self.date.clone();
        record.flags_observed = true;
        record.flags = self.flags.clone();
        record.gmail_message_id = self.gmail_id.clone();
        record.gmail_thread_id = self.thread_id.clone();
        record.gmail_labels = self.labels.clone();
        record.headers.direction =
            if folder.sent || self.labels.iter().any(|l| l.eq_ignore_ascii_case("\\Sent")) {
                Direction::Sent
            } else {
                Direction::Received
            };
    }
    /// The metadata a saved record holds.
    pub fn saved(record: &MessageRecord) -> Self {
        Self {
            uid: record.uid,
            size: record.size,
            date: record.internal_date.clone(),
            flags: record.flags.clone(),
            gmail_id: record.gmail_message_id.clone(),
            thread_id: record.gmail_thread_id.clone(),
            labels: record.gmail_labels.clone(),
        }
    }
}

/// One folder as EXAMINE and UID SEARCH ALL report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listing {
    pub validity: u32,
    pub uids: BTreeSet<u32>,
    /// HIGHESTMODSEQ, when the server keeps mod-sequences for the folder
    /// (RFC 7162) and this session asked for them.
    pub modseq: Option<u64>,
}

pub(super) trait MailRead {
    fn adapter(&self) -> &'static str;
    fn capabilities(&self) -> BTreeSet<String>;
    fn count(&mut self, folder: &Folder) -> CountObservation;
    fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError>;
    /// Examines `folder` and lists its UIDs. With `since`, also the metadata
    /// of every message whose mod-sequence exceeds it, or `None` when the
    /// folder cannot answer that.
    fn inventory(
        &mut self,
        folder: &Folder,
        since: Option<u64>,
    ) -> Result<(Listing, Option<BTreeMap<u32, Metadata>>), AcquisitionError>;
    /// Metadata of exactly `uids` in the folder `inventory` last examined.
    fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError>;
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError>;
    /// How many headers one `headers` call should ask for.
    fn header_batch(&self) -> usize;
}

/// Compact UID sets for `uids`: runs of three or more consecutive UIDs become
/// ranges, and each set stays under `SET_LENGTH`. Every set names exactly
/// the given UIDs.
pub(in crate::acquisition) fn uid_sets(uids: impl IntoIterator<Item = u32>) -> Vec<String> {
    let mut runs: Vec<(u32, u32)> = vec![];
    let mut sorted: Vec<u32> = uids.into_iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    for uid in sorted {
        match runs.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(uid) => *end = uid,
            _ => runs.push((uid, uid)),
        }
    }
    let mut sets = vec![];
    let mut current = String::new();
    for (start, end) in runs {
        let item = match end - start {
            0 => start.to_string(),
            1 => format!("{start},{end}"),
            _ => format!("{start}:{end}"),
        };
        if !current.is_empty() && current.len() + 1 + item.len() > SET_LENGTH {
            sets.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(',');
        }
        current.push_str(&item);
    }
    if !current.is_empty() {
        sets.push(current);
    }
    sets
}

/// The bytes and time one response may take. It fails after `idle` without
/// bytes, or once it falls below `rate` bytes per second on average after
/// its first `idle`: a slow healthy link completes, a stalled one fails
/// promptly, and a trickle cannot hold the session.
struct Budget {
    remaining: usize,
    started: Instant,
    last: Instant,
    received: usize,
    pace: Pace,
}
#[derive(Clone, Copy)]
struct Pace {
    idle: Duration,
    rate: usize,
}
const PACE: Pace = Pace {
    idle: TIMEOUT,
    rate: 32 * 1024,
};
impl Budget {
    fn new(remaining: usize, pace: Pace) -> Self {
        let now = Instant::now();
        Self {
            remaining,
            started: now,
            last: now,
            received: 0,
            pace,
        }
    }
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.last) > self.pace.idle
            || now
                > self.started
                    + self.pace.idle
                    + Duration::from_secs_f64(self.received as f64 / self.pace.rate as f64)
    }
}
struct LimitedTls {
    tls: native_tls::TlsStream<TcpStream>,
    budget: Arc<Mutex<Budget>>,
    cancel: Cancellation,
}
impl Read for LimitedTls {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut budget = self.budget.lock().expect("response budget");
        if self.cancel.is_cancelled() || budget.remaining == 0 || budget.expired(Instant::now()) {
            return Err(io::Error::other("Read limit reached"));
        }
        let limit = buf.len().min(budget.remaining);
        let read = self.tls.read(&mut buf[..limit])?;
        budget.remaining -= read;
        budget.received += read;
        budget.last = Instant::now();
        Ok(read)
    }
}
impl Write for LimitedTls {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("Cancelled"));
        }
        self.tls.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
    }
}

/// How the server refused one command. Transport failures are errors of
/// their own and end the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refused {
    No,
    Bad,
}
/// One command's untagged responses, and its tagged completion line.
struct Reply {
    data: Vec<u8>,
    done: Vec<u8>,
}
type Answer = Result<Reply, Refused>;

fn quote(value: &str) -> Option<String> {
    if value.contains(['\r', '\n']) {
        return None;
    }
    Some(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn protocol(step: &'static str) -> impl Fn(Refused) -> AcquisitionError {
    move |_refused| {
        #[cfg(test)]
        eprintln!("imap.{step}.refused={_refused:?}");
        let _ = step;
        AcquisitionError::Protocol
    }
}

/// Read-only IMAP session. Commands are written and read by this client:
/// several read-only commands may be pipelined (RFC 3501 section 5.5), and
/// each response is read in order up to its tagged line.
pub(super) struct ReadOnlyImap {
    stream: BufReader<LimitedTls>,
    budget: Arc<Mutex<Budget>>,
    cancel: Cancellation,
    gmail: bool,
    special_use: bool,
    /// CONDSTORE (RFC 7162) is advertised; Gmail's labels are not covered
    /// by its mod-sequences, so it is never used there.
    condstore: bool,
    capabilities: BTreeSet<String>,
    tag: u32,
    /// A response was left unread or unparsed, so the stream is unusable.
    broken: bool,
    /// Mean bytes of each header row read so far, for sizing header fetches.
    header_mean: usize,
}
impl ReadOnlyImap {
    pub fn connect(
        permit: &ReadPermit,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, AcquisitionError> {
        let address = (permit.provider().host(), 993)
            .to_socket_addrs()
            .map_err(|_| AcquisitionError::Tls)?
            .next()
            .ok_or(AcquisitionError::Tls)?;
        let tcp =
            TcpStream::connect_timeout(&address, TIMEOUT).map_err(|_| AcquisitionError::Tls)?;
        let connector = native_tls::TlsConnector::new().map_err(|_| AcquisitionError::Tls)?;
        Self::open(
            tcp,
            permit.provider().host(),
            connector,
            login,
            password,
            cancel,
            permit.provider() == Provider::Gmail,
        )
    }
    pub(super) fn loopback(
        permit: &super::folder_provisioning::transport::message_move::BacklogPermit<'_>,
        read: &ReadPermit,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, AcquisitionError> {
        let source = permit.source();
        if read.provider() != Provider::MailboxOrg || source.login != login {
            return Err(AcquisitionError::InvalidInput);
        }
        Self::installed(
            source.address,
            &source.tls_name,
            &source.root,
            login,
            password,
            cancel,
            false,
        )
    }
    pub(super) fn loopback_gmail(
        permit: &super::folder_provisioning::transport::gmail_label::LabelPermit<'_>,
        read: &ReadPermit,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, AcquisitionError> {
        let source = permit.source();
        if read.provider() != Provider::Gmail || source.login != login {
            return Err(AcquisitionError::InvalidInput);
        }
        Self::installed(
            source.address,
            &source.tls_name,
            &source.root,
            login,
            password,
            cancel,
            true,
        )
    }
    fn installed(
        address: std::net::SocketAddr,
        tls_name: &str,
        root: &[u8],
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
        gmail: bool,
    ) -> Result<Self, AcquisitionError> {
        if !address.ip().is_loopback() {
            return Err(AcquisitionError::InvalidInput);
        }
        let tcp =
            TcpStream::connect_timeout(&address, TIMEOUT).map_err(|_| AcquisitionError::Tls)?;
        let root = native_tls::Certificate::from_der(root).map_err(|_| AcquisitionError::Tls)?;
        let connector = native_tls::TlsConnector::builder()
            .disable_built_in_roots(true)
            .add_root_certificate(root)
            .build()
            .map_err(|_| AcquisitionError::Tls)?;
        Self::open(tcp, tls_name, connector, login, password, cancel, gmail)
    }
    fn open(
        tcp: TcpStream,
        hostname: &str,
        connector: native_tls::TlsConnector,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
        gmail: bool,
    ) -> Result<Self, AcquisitionError> {
        tcp.set_read_timeout(Some(TIMEOUT))
            .and_then(|_| tcp.set_write_timeout(Some(TIMEOUT)))
            .map_err(|_| AcquisitionError::Tls)?;
        let _ = tcp.set_nodelay(true);
        let tls = connector
            .connect(hostname, tcp)
            .map_err(|_| AcquisitionError::Tls)?;
        let budget = Arc::new(Mutex::new(Budget::new(RESPONSE_LIMIT, PACE)));
        let mut result = Self {
            stream: BufReader::new(LimitedTls {
                tls,
                budget: budget.clone(),
                cancel: cancel.clone(),
            }),
            budget,
            cancel,
            gmail,
            special_use: false,
            condstore: false,
            capabilities: BTreeSet::new(),
            tag: 0,
            broken: false,
            header_mean: 4096,
        };
        let mut greeting = vec![];
        if !matches!(result.stream.read_until(b'\n', &mut greeting), Ok(n) if n > 0) {
            return Err(AcquisitionError::Protocol);
        }
        let command = Zeroizing::new(format!(
            "LOGIN {} {}",
            quote(login).ok_or(AcquisitionError::Authentication)?,
            *Zeroizing::new(quote(password.text()?).ok_or(AcquisitionError::Authentication)?)
        ));
        match result.run(&command, RESPONSE_LIMIT) {
            Ok(Ok(_)) => {}
            _ => return Err(AcquisitionError::Authentication),
        }
        let raw = result
            .run("CAPABILITY", RESPONSE_LIMIT)?
            .map_err(protocol("capability"))?
            .data;
        let mut rest = raw.as_slice();
        while !rest.is_empty() {
            let (remaining, response) =
                imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
            if let Response::Capabilities(values) = response {
                for value in values {
                    let value = match value {
                        imap_proto::Capability::Imap4rev1 => "IMAP4REV1".to_owned(),
                        imap_proto::Capability::Auth(v) => format!("AUTH={v}").to_ascii_uppercase(),
                        imap_proto::Capability::Atom(v) => v.to_ascii_uppercase(),
                    };
                    if value.len() > 256 || result.capabilities.len() >= 256 {
                        return Err(AcquisitionError::Incomplete);
                    }
                    result.capabilities.insert(value);
                }
            }
            rest = remaining;
        }
        if !result.capabilities.contains("IMAP4REV1")
            || (gmail && !result.capabilities.contains("X-GM-EXT-1"))
        {
            return Err(AcquisitionError::Protocol);
        }
        result.special_use = result.capabilities.contains("SPECIAL-USE");
        result.condstore = !gmail && result.capabilities.contains("CONDSTORE");
        Ok(result)
    }

    /// Shortens the response pace for tests of slow and stalled links.
    #[cfg(test)]
    pub(super) fn pace(&mut self, idle: Duration, rate: usize) {
        let _ = self
            .stream
            .get_ref()
            .tls
            .get_ref()
            .set_read_timeout(Some(idle));
        self.budget.lock().expect("response budget").pace = Pace { idle, rate };
    }
    fn transport(&mut self) -> AcquisitionError {
        self.broken = true;
        if self.cancel.is_cancelled() {
            AcquisitionError::Cancelled
        } else {
            AcquisitionError::Protocol
        }
    }
    /// Writes tagged commands and returns their tags.
    fn send(&mut self, commands: &[&str]) -> Result<Vec<String>, AcquisitionError> {
        self.cancel.check()?;
        if self.broken {
            return Err(AcquisitionError::Protocol);
        }
        let mut bytes = Zeroizing::new(vec![]);
        let mut tags = vec![];
        for command in commands {
            if command.contains(['\r', '\n']) {
                return Err(AcquisitionError::InvalidInput);
            }
            self.tag += 1;
            let tag = format!("A{}", self.tag);
            bytes.extend_from_slice(tag.as_bytes());
            bytes.push(b' ');
            bytes.extend_from_slice(command.as_bytes());
            bytes.extend_from_slice(b"\r\n");
            tags.push(tag);
        }
        let stream = self.stream.get_mut();
        if stream
            .write_all(&bytes)
            .and_then(|_| stream.flush())
            .is_err()
        {
            return Err(self.transport());
        }
        Ok(tags)
    }
    /// Reads one response, literals included, onto `data` and returns where
    /// it starts. Lines are read until the response parses, as the imap
    /// crate did: a literal's bytes may hold line ends of their own.
    fn response(&mut self, data: &mut Vec<u8>) -> Result<usize, AcquisitionError> {
        let start = data.len();
        loop {
            match self.stream.read_until(b'\n', data) {
                Ok(0) | Err(_) => return Err(self.transport()),
                Ok(_) => {}
            }
            match imap_proto::parser::parse_response(&data[start..]) {
                Ok((rest, _)) if rest.is_empty() => return Ok(start),
                Err(e) if e.is_incomplete() => {}
                _ => return Err(self.transport()),
            }
        }
    }
    /// Reads the response to `tag` within `limit` bytes.
    fn answer(&mut self, tag: &str, limit: usize) -> Result<Answer, AcquisitionError> {
        self.cancel.check()?;
        let mut budget = self.budget.lock().expect("response budget");
        *budget = Budget::new(limit, budget.pace);
        drop(budget);
        let mut data = vec![];
        loop {
            let start = self.response(&mut data)?;
            let status = match imap_proto::parser::parse_response(&data[start..]) {
                Ok((_, Response::Done { tag: t, status, .. })) if t.0 == tag => Some(status),
                Ok((_, Response::Done { .. } | Response::Continue { .. })) => {
                    return Err(self.transport());
                }
                Ok((
                    _,
                    Response::Data {
                        status: Status::Bye,
                        ..
                    },
                )) => {
                    return Err(self.transport());
                }
                Ok(_) => None,
                Err(_) => return Err(self.transport()),
            };
            if let Some(status) = status {
                let done = data.split_off(start);
                return Ok(match status {
                    Status::Ok => Ok(Reply { data, done }),
                    Status::No => Err(Refused::No),
                    Status::Bad => Err(Refused::Bad),
                    _ => return Err(self.transport()),
                });
            }
        }
    }
    fn run(&mut self, command: &str, limit: usize) -> Result<Answer, AcquisitionError> {
        let tag = self.send(&[command])?.remove(0);
        self.answer(&tag, limit)
    }
    /// Pipelines read-only commands, each with its response limit, and hands
    /// each answer to `visit` in order. At most `WINDOW_COMMANDS` commands
    /// and `WINDOW_BYTES` of them are outstanding at once.
    fn flight(
        &mut self,
        commands: &[(String, usize)],
        mut visit: impl FnMut(usize, Answer) -> Result<(), AcquisitionError>,
    ) -> Result<(), AcquisitionError> {
        let mut pending = std::collections::VecDeque::new();
        let mut next = 0;
        let mut done = 0;
        while done < commands.len() {
            let mut batch = vec![];
            let mut bytes: usize = pending.iter().map(|(_, len)| len).sum();
            while next < commands.len()
                && pending.len() + batch.len() < WINDOW_COMMANDS
                && (bytes + commands[next].0.len() <= WINDOW_BYTES
                    || pending.len() + batch.len() == 0)
            {
                bytes += commands[next].0.len() + 16;
                batch.push(next);
                next += 1;
            }
            if !batch.is_empty() {
                let lines: Vec<&str> = batch.iter().map(|i| commands[*i].0.as_str()).collect();
                for (i, tag) in batch.iter().zip(self.send(&lines)?) {
                    pending.push_back(((*i, tag), commands[*i].0.len() + 16));
                }
            }
            let ((i, tag), _) = pending.pop_front().expect("outstanding command");
            let answer = self.answer(&tag, commands[i].1)?;
            done += 1;
            if let Err(error) = visit(i, answer) {
                if done < commands.len() {
                    self.broken = true;
                }
                return Err(error);
            }
        }
        Ok(())
    }
    fn examine_command(&self, folder: &Folder) -> Result<String, AcquisitionError> {
        let name = quote(&folder.wire).ok_or(AcquisitionError::Protocol)?;
        Ok(if self.condstore {
            format!("EXAMINE {name} (CONDSTORE)")
        } else {
            format!("EXAMINE {name}")
        })
    }
    fn metadata_items(&self) -> &'static str {
        if self.gmail {
            "UID RFC822.SIZE INTERNALDATE FLAGS X-GM-MSGID X-GM-THRID X-GM-LABELS"
        } else {
            "UID RFC822.SIZE INTERNALDATE FLAGS"
        }
    }
    fn header_items() -> String {
        format!(
            "UID BODY.PEEK[HEADER.FIELDS ({})]<0.{HEADER_BYTES}>",
            HEADER_FIELDS.join(" ").to_ascii_uppercase()
        )
    }
    /// Metadata commands for `uids`, each with its response limit.
    fn metadata_commands(&self, uids: &[u32]) -> Vec<(String, usize)> {
        let mut commands = vec![];
        for chunk in uids.chunks(METADATA_COMMAND_MAX) {
            let limit = RESPONSE_LIMIT.max(chunk.len() * METADATA_ROW_LIMIT);
            for set in uid_sets(chunk.iter().copied()) {
                commands.push((
                    format!("UID FETCH {set} ({})", self.metadata_items()),
                    limit,
                ));
            }
        }
        commands
    }
    fn header_commands(&self, uids: &[u32]) -> Vec<(String, usize)> {
        let mut commands = vec![];
        for chunk in uids.chunks(self.header_batch()) {
            let limit = RESPONSE_LIMIT.max(chunk.len() * (HEADER_BYTES + ROW_OVERHEAD));
            for set in uid_sets(chunk.iter().copied()) {
                commands.push((format!("UID FETCH {set} ({})", Self::header_items()), limit));
            }
        }
        commands
    }
    fn parse_metadata(
        gmail: bool,
        raw: &[u8],
        into: &mut BTreeMap<u32, Metadata>,
    ) -> Result<(), AcquisitionError> {
        parse_fetches(raw, |attrs| {
            let uid = uid(attrs)?;
            let mut item = Metadata {
                uid,
                size: 0,
                date: String::new(),
                flags: BTreeSet::new(),
                gmail_id: None,
                thread_id: None,
                labels: BTreeSet::new(),
            };
            let (mut size, mut date, mut flags, mut labels) = (false, false, false, false);
            for attr in attrs {
                match attr {
                    AttributeValue::Rfc822Size(v) => {
                        item.size = *v as u64;
                        size = true;
                    }
                    AttributeValue::InternalDate(v) => {
                        item.date = v.to_string();
                        date = true;
                    }
                    AttributeValue::Flags(v) => {
                        item.flags = v.iter().map(|f| f.to_string()).collect();
                        flags = true;
                    }
                    AttributeValue::GmailMsgId(v) => item.gmail_id = Some(v.to_string()),
                    AttributeValue::GmailThrId(v) => item.thread_id = Some(v.to_string()),
                    AttributeValue::GmailLabels(v) => {
                        item.labels = v.iter().map(|l| decode_name(l)).collect::<Result<_, _>>()?;
                        labels = true;
                    }
                    _ => {}
                }
            }
            if !size
                || !date
                || !flags
                || (gmail && (item.gmail_id.is_none() || item.thread_id.is_none() || !labels))
                || into.insert(uid, item).is_some()
            {
                return Err(AcquisitionError::Incomplete);
            }
            Ok(())
        })
    }
    /// Parses header rows and folds their mean size into `mean`.
    fn parse_headers(
        mean: &mut usize,
        raw: &[u8],
        into: &mut BTreeMap<u32, Vec<u8>>,
    ) -> Result<(), AcquisitionError> {
        let mut rows = 0;
        parse_fetches(raw, |attrs| {
            let uid = uid(attrs)?;
            let header = attrs
                .iter()
                .find_map(|a| match a {
                    AttributeValue::BodySection {
                        section: Some(SectionPath::Full(MessageSection::Header)),
                        data: Some(data),
                        ..
                    } => Some(data.as_ref()),
                    _ => None,
                })
                .ok_or(AcquisitionError::Incomplete)?;
            if header.len() > 65536 || into.insert(uid, header.to_vec()).is_some() {
                return Err(AcquisitionError::Incomplete);
            }
            rows += 1;
            Ok(())
        })?;
        if rows > 0 {
            *mean = (*mean + raw.len() / rows).div_ceil(2).max(256);
        }
        Ok(())
    }
    /// Parses EXAMINE and UID SEARCH ALL answers into a listing.
    fn listing(examine: Answer, search: Answer) -> Result<Listing, AcquisitionError> {
        let examine = examine.map_err(protocol("examine"))?;
        let (mut exists, mut validity, mut modseq) = (None, None, None);
        let mut rest = examine.data.as_slice();
        while !rest.is_empty() {
            let (remaining, response) =
                imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
            match response {
                Response::MailboxData(MailboxDatum::Exists(n)) => exists = Some(n),
                Response::Data {
                    code: Some(ResponseCode::UidValidity(v)),
                    ..
                } => validity = Some(v),
                Response::Data {
                    code: Some(ResponseCode::HighestModSeq(v)),
                    ..
                } => modseq = Some(v),
                _ => {}
            }
            rest = remaining;
        }
        let read_only = matches!(
            imap_proto::parser::parse_response(&examine.done),
            Ok((
                _,
                Response::Done {
                    code: Some(ResponseCode::ReadOnly),
                    ..
                }
            ))
        );
        if !read_only {
            return Err(AcquisitionError::Protocol);
        }
        let validity = validity
            .filter(|v| *v != 0)
            .ok_or(AcquisitionError::Incomplete)?;
        let search = search.map_err(protocol("search"))?;
        let mut uids = BTreeSet::new();
        let mut rest = search.data.as_slice();
        while !rest.is_empty() {
            let (remaining, response) =
                imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
            if let Response::MailboxData(MailboxDatum::Search(values)) = response {
                uids.extend(values);
            }
            rest = remaining;
        }
        if Some(uids.len() as u32) != exists || uids.len() > 250_000 || uids.contains(&0) {
            return Err(AcquisitionError::Incomplete);
        }
        Ok(Listing {
            validity,
            uids,
            modseq: modseq.filter(|m| *m > 0),
        })
    }

    /// Reads `folders` for one request in three flights. The first examines
    /// every folder, lists its UIDs and fetches its metadata: with CONDSTORE
    /// and a saved census taken at a known HIGHESTMODSEQ under the same
    /// UIDVALIDITY, only what changed since. The second fetches the metadata
    /// and headers the saved census cannot supply. Headers are immutable for
    /// a UID under one UIDVALIDITY (RFC 3501 section 2.3.1.1), so a saved
    /// record's header evidence stands for the message. The third examines
    /// every folder again; each must still show the same UIDVALIDITY and
    /// UIDs.
    pub(super) fn observe(
        &mut self,
        folders: &[Folder],
        saved: &Census,
    ) -> Result<BTreeMap<String, FolderCensus>, AcquisitionError> {
        let cached = |folder: &Folder, validity: u32| {
            saved
                .folders
                .get(&folder.name)
                .filter(|f| f.uidvalidity == validity)
        };
        // Flight one: EXAMINE, UID SEARCH ALL and metadata per folder.
        let mut commands = vec![];
        for folder in folders {
            commands.push((self.examine_command(folder)?, RESPONSE_LIMIT));
            commands.push(("UID SEARCH ALL".to_owned(), RESPONSE_LIMIT));
            let since = saved
                .folders
                .get(&folder.name)
                .and_then(|f| f.highestmodseq)
                .filter(|_| self.condstore);
            commands.push((
                match since {
                    Some(m) => format!(
                        "UID FETCH 1:* ({}) (CHANGEDSINCE {m})",
                        self.metadata_items()
                    ),
                    None => format!("UID FETCH 1:* ({})", self.metadata_items()),
                },
                64 * 1024 * 1024,
            ));
        }
        let mut answers: Vec<Option<Answer>> = (0..commands.len()).map(|_| None).collect();
        self.flight(&commands, |i, answer| {
            answers[i] = Some(answer);
            Ok(())
        })?;
        let mut answers = answers.into_iter().map(|a| a.expect("answered"));
        struct Partial {
            listing: Listing,
            metadata: BTreeMap<u32, Metadata>,
            missing_metadata: Vec<u32>,
            missing_headers: Vec<u32>,
        }
        let mut partials = vec![];
        for folder in folders {
            let (examine, search, fetch) = (
                answers.next().expect("examine"),
                answers.next().expect("search"),
                answers.next().expect("fetch"),
            );
            let listing = Self::listing(examine, search)?;
            let saved_folder = cached(folder, listing.validity);
            let since = saved
                .folders
                .get(&folder.name)
                .and_then(|f| f.highestmodseq)
                .filter(|_| self.condstore);
            let mut fetched = BTreeMap::new();
            let usable = match fetch {
                Ok(reply) => {
                    Self::parse_metadata(self.gmail, &reply.data, &mut fetched)?;
                    // CHANGEDSINCE rows describe changes after the saved
                    // census only under its UIDVALIDITY and when the folder
                    // reports mod-sequences.
                    since.is_none() || (saved_folder.is_some() && listing.modseq.is_some())
                }
                Err(_) => false,
            };
            fetched.retain(|uid, _| listing.uids.contains(uid));
            let mut metadata = BTreeMap::new();
            if usable {
                if let (Some(_), Some(saved_folder)) = (since, saved_folder) {
                    for uid in &listing.uids {
                        if let Some(record) = saved_folder.messages.get(uid) {
                            metadata.insert(*uid, Metadata::saved(record));
                        }
                    }
                }
                metadata.extend(fetched);
            }
            let missing_metadata = listing
                .uids
                .iter()
                .filter(|u| !metadata.contains_key(u))
                .copied()
                .collect();
            let missing_headers = listing
                .uids
                .iter()
                .filter(|u| saved_folder.is_none_or(|f| !f.messages.contains_key(u)))
                .copied()
                .collect();
            partials.push(Partial {
                listing,
                metadata,
                missing_metadata,
                missing_headers,
            });
        }
        // Flight two: what the saved census cannot supply. Each folder is
        // examined again first, and must show the listing flight one saw.
        enum Need {
            Examine,
            Search(usize),
            Metadata(usize),
            Headers(usize),
        }
        let mut commands = vec![];
        let mut needs = vec![];
        for (n, (folder, p)) in folders.iter().zip(&partials).enumerate() {
            if p.missing_metadata.is_empty() && p.missing_headers.is_empty() {
                continue;
            }
            commands.push((self.examine_command(folder)?, RESPONSE_LIMIT));
            needs.push(Need::Examine);
            commands.push(("UID SEARCH ALL".to_owned(), RESPONSE_LIMIT));
            needs.push(Need::Search(n));
            for command in self.metadata_commands(&p.missing_metadata) {
                commands.push(command);
                needs.push(Need::Metadata(n));
            }
            for command in self.header_commands(&p.missing_headers) {
                commands.push(command);
                needs.push(Need::Headers(n));
            }
        }
        let mut headers: Vec<BTreeMap<u32, Vec<u8>>> =
            folders.iter().map(|_| BTreeMap::new()).collect();
        let gmail = self.gmail;
        let mut mean = self.header_mean;
        let mut examined = None;
        self.flight(&commands, |i, answer| {
            match needs[i] {
                Need::Examine => examined = Some(answer),
                Need::Search(n) => {
                    let listing =
                        Self::listing(examined.take().ok_or(AcquisitionError::Protocol)?, answer)?;
                    if (listing.validity, &listing.uids)
                        != (partials[n].listing.validity, &partials[n].listing.uids)
                    {
                        return Err(AcquisitionError::InvalidInput);
                    }
                }
                Need::Metadata(n) => {
                    let reply = answer.map_err(protocol("fetch"))?;
                    Self::parse_metadata(gmail, &reply.data, &mut partials[n].metadata)?;
                }
                Need::Headers(n) => {
                    let reply = answer.map_err(protocol("fetch"))?;
                    Self::parse_headers(&mut mean, &reply.data, &mut headers[n])?;
                }
            }
            Ok(())
        })?;
        self.header_mean = mean;
        // Flight three: every folder unchanged.
        let mut commands = vec![];
        for folder in folders {
            commands.push((self.examine_command(folder)?, RESPONSE_LIMIT));
            commands.push(("UID SEARCH ALL".to_owned(), RESPONSE_LIMIT));
        }
        let mut answers: Vec<Option<Answer>> = (0..commands.len()).map(|_| None).collect();
        self.flight(&commands, |i, answer| {
            answers[i] = Some(answer);
            Ok(())
        })?;
        let mut answers = answers.into_iter().map(|a| a.expect("answered"));
        let mut result = BTreeMap::new();
        for ((folder, p), headers) in folders.iter().zip(partials).zip(headers) {
            let again = Self::listing(
                answers.next().expect("examine"),
                answers.next().expect("search"),
            )?;
            if again.validity != p.listing.validity || again.uids != p.listing.uids {
                return Err(AcquisitionError::InvalidInput);
            }
            validate_uids(
                &p.listing.uids.iter().copied().collect::<Vec<_>>(),
                p.metadata.keys().copied(),
            )?;
            let saved_folder = cached(folder, p.listing.validity);
            let mut messages = BTreeMap::new();
            for uid in &p.listing.uids {
                let metadata = &p.metadata[uid];
                let record = match headers.get(uid) {
                    Some(raw) => record(metadata, folder, raw)?,
                    None => {
                        let saved = saved_folder
                            .and_then(|f| f.messages.get(uid))
                            .ok_or(AcquisitionError::Incomplete)?;
                        let mut record = MessageRecord {
                            delivery: Default::default(),
                            uid: *uid,
                            size: 0,
                            internal_date: String::new(),
                            flags_observed: true,
                            flags: BTreeSet::new(),
                            gmail_message_id: None,
                            gmail_thread_id: None,
                            gmail_labels: BTreeSet::new(),
                            headers: saved.headers.clone(),
                        };
                        metadata.apply(&mut record, folder);
                        record
                    }
                };
                messages.insert(*uid, record);
            }
            result.insert(
                folder.name.clone(),
                FolderCensus {
                    uidvalidity: p.listing.validity,
                    state: ScanState::Complete,
                    live_uids: p.listing.uids,
                    messages,
                    highestmodseq: None,
                },
            );
        }
        Ok(result)
    }
}
impl MailRead for ReadOnlyImap {
    fn adapter(&self) -> &'static str {
        crate::census::acquisition::ADAPTER
    }
    fn capabilities(&self) -> BTreeSet<String> {
        self.capabilities.clone()
    }
    fn count(&mut self, folder: &Folder) -> CountObservation {
        if !folder.selectable {
            return CountObservation::unknown(UnknownCount::NotSelectable);
        }
        if self.cancel.check().is_err() {
            return CountObservation::unknown(UnknownCount::Unavailable);
        }
        let Some(mailbox) = quote(&folder.wire) else {
            return CountObservation::unknown(UnknownCount::Unavailable);
        };
        match self.run(&format!("STATUS {mailbox} (MESSAGES)"), RESPONSE_LIMIT) {
            Ok(Ok(reply)) => parse_count(&reply.data, &folder.wire),
            Ok(Err(_)) => CountObservation::unknown(UnknownCount::Denied),
            Err(_) => CountObservation::unknown(UnknownCount::Unavailable),
        }
    }

    fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError> {
        self.cancel.check()?;
        if !self.special_use && !self.gmail {
            return Err(AcquisitionError::Protocol);
        }
        let command = if self.special_use {
            "LIST \"\" \"*\" RETURN (SPECIAL-USE)"
        } else {
            "LIST \"\" *"
        };
        let raw = self
            .run(command, RESPONSE_LIMIT)?
            .map_err(protocol("list"))?
            .data;
        let mut rest = raw.as_slice();
        let mut folders = Vec::new();
        while !rest.is_empty() {
            let (remaining, response) =
                imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
            if let Response::MailboxData(MailboxDatum::List {
                name_attributes: attrs,
                name,
                delimiter,
            }) = response
            {
                let name = list_wire_name(&rest[..rest.len() - remaining.len()], &name)?;
                if folders.len() >= crate::census::acquisition::MAX_FOLDERS
                    || attrs.len() > 128
                    || delimiter
                        .as_ref()
                        .is_some_and(|d| d.chars().count() != 1 || d.chars().any(char::is_control))
                {
                    return Err(AcquisitionError::Incomplete);
                }
                let attributes: BTreeSet<String> = attrs.iter().map(attribute_name).collect();
                if attributes.iter().any(|a| a.len() > 256) {
                    return Err(AcquisitionError::Incomplete);
                }
                let selectable = !attrs.contains(&NameAttribute::NoSelect)
                    && !attributes
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case("\\NonExistent"));
                folders.push(Folder {
                    attributes,
                    selectable,
                    delimiter: delimiter.map(|d| d.to_string()),
                    roles: attrs
                        .iter()
                        .filter_map(|a| match a {
                            NameAttribute::Flagged => Some("flagged".into()),
                            NameAttribute::Archive => Some("archive".into()),
                            NameAttribute::Sent => Some("sent".into()),
                            NameAttribute::All => Some("all".into()),
                            NameAttribute::Drafts => Some("drafts".into()),
                            NameAttribute::Junk => Some("junk".into()),
                            NameAttribute::Trash => Some("trash".into()),
                            NameAttribute::NoSelect => Some("noselect".into()),
                            _ => None,
                        })
                        .collect(),
                    wire: name.clone(),
                    name: decode_name(&name)?,
                    sent: attrs.contains(&NameAttribute::Sent),
                    all: attrs.contains(&NameAttribute::All),
                    excluded: !selectable
                        || attrs.iter().any(|a| {
                            matches!(
                                a,
                                NameAttribute::NoSelect
                                    | NameAttribute::Junk
                                    | NameAttribute::Trash
                                    | NameAttribute::Drafts
                            )
                        }),
                });
            }
            rest = remaining;
        }
        if folders.is_empty() || folders.len() > 10_000 {
            return Err(AcquisitionError::Incomplete);
        }
        let unique: BTreeSet<_> = folders.iter().map(|f| &f.name).collect();
        if unique.len() != folders.len() {
            return Err(AcquisitionError::Protocol);
        }
        Ok(folders)
    }
    /// EXAMINE, UID SEARCH ALL and, with `since`, CHANGEDSINCE, pipelined.
    fn inventory(
        &mut self,
        folder: &Folder,
        since: Option<u64>,
    ) -> Result<(Listing, Option<BTreeMap<u32, Metadata>>), AcquisitionError> {
        let since = since.filter(|_| self.condstore);
        let mut commands = vec![
            (self.examine_command(folder)?, RESPONSE_LIMIT),
            ("UID SEARCH ALL".to_owned(), RESPONSE_LIMIT),
        ];
        if let Some(m) = since {
            commands.push((
                format!(
                    "UID FETCH 1:* ({}) (CHANGEDSINCE {m})",
                    self.metadata_items()
                ),
                64 * 1024 * 1024,
            ));
        }
        let mut answers = vec![];
        self.flight(&commands, |_, answer| {
            answers.push(answer);
            Ok(())
        })?;
        let mut answers = answers.into_iter();
        let listing = Self::listing(
            answers.next().ok_or(AcquisitionError::Protocol)?,
            answers.next().ok_or(AcquisitionError::Protocol)?,
        )?;
        let changed = match answers.next() {
            Some(Ok(reply)) if listing.modseq.is_some() => {
                let mut changed = BTreeMap::new();
                Self::parse_metadata(self.gmail, &reply.data, &mut changed)?;
                changed.retain(|uid, _| listing.uids.contains(uid));
                Some(changed)
            }
            _ => None,
        };
        Ok((listing, changed))
    }
    fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError> {
        if uids.is_empty() || uids.contains(&0) {
            return Err(AcquisitionError::InvalidInput);
        }
        let commands = self.metadata_commands(uids);
        let gmail = self.gmail;
        let mut result = BTreeMap::new();
        self.flight(&commands, |_, answer| {
            let reply = answer.map_err(protocol("fetch"))?;
            Self::parse_metadata(gmail, &reply.data, &mut result)
        })?;
        validate_uids(uids, result.keys().copied())?;
        Ok(result)
    }
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError> {
        if uids.is_empty() || uids.contains(&0) {
            return Err(AcquisitionError::InvalidInput);
        }
        let commands = self.header_commands(uids);
        let mut mean = self.header_mean;
        let mut result = BTreeMap::new();
        self.flight(&commands, |_, answer| {
            let reply = answer.map_err(protocol("fetch"))?;
            Self::parse_headers(&mut mean, &reply.data, &mut result)
        })?;
        self.header_mean = mean;
        validate_uids(uids, result.keys().copied())?;
        Ok(result)
    }
    fn header_batch(&self) -> usize {
        (HEADER_TARGET / self.header_mean).clamp(25, HEADER_COMMAND_MAX)
    }
}
fn uid(attrs: &[AttributeValue<'_>]) -> Result<u32, AcquisitionError> {
    attrs
        .iter()
        .find_map(|a| {
            if let AttributeValue::Uid(v) = a {
                Some(*v)
            } else {
                None
            }
        })
        .filter(|u| *u != 0)
        .ok_or(AcquisitionError::Incomplete)
}
fn validate_uids(
    expected: &[u32],
    actual: impl Iterator<Item = u32>,
) -> Result<(), AcquisitionError> {
    if expected.iter().copied().collect::<BTreeSet<_>>() != actual.collect() {
        return Err(AcquisitionError::Incomplete);
    }
    Ok(())
}
fn parse_fetches(
    raw: &[u8],
    mut visit: impl FnMut(&[AttributeValue<'_>]) -> Result<(), AcquisitionError>,
) -> Result<(), AcquisitionError> {
    let mut rest = raw;
    while !rest.is_empty() {
        let (remaining, response) =
            imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
        if let Response::Fetch(_, attrs) = response {
            visit(&attrs)?;
        }
        rest = remaining;
    }
    Ok(())
}

fn list_wire_name(row: &[u8], name: &str) -> Result<String, AcquisitionError> {
    // imap-proto borrows quoted contents without removing IMAP escapes. Literals are exact bytes.
    let quoted = row
        .strip_suffix(b"\"\r\n")
        .and_then(|prefix| prefix.strip_suffix(name.as_bytes()))
        .is_some_and(|prefix| prefix.ends_with(b"\""));
    if !quoted {
        return Ok(name.into());
    }
    let mut value = String::new();
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        value.push(if c == '\\' {
            match chars.next() {
                Some(c @ ('\\' | '"')) => c,
                _ => return Err(AcquisitionError::Protocol),
            }
        } else {
            c
        });
    }
    Ok(value)
}

fn decode_name(wire: &str) -> Result<String, AcquisitionError> {
    if wire.len() > 4096 || wire.chars().any(char::is_control) {
        return Err(AcquisitionError::Protocol);
    }
    let mut result = String::new();
    let mut rest = wire;
    while let Some(index) = rest.find('&') {
        result.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        let end = rest.find('-').ok_or(AcquisitionError::Protocol)?;
        if end == 0 {
            result.push('&');
        } else {
            let encoded = rest[..end].replace(',', "/");
            let bytes = data_encoding::BASE64_NOPAD
                .decode(encoded.as_bytes())
                .map_err(|_| AcquisitionError::Protocol)?;
            if bytes.len() % 2 != 0 {
                return Err(AcquisitionError::Protocol);
            }
            let units: Vec<_> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect();
            result.push_str(&String::from_utf16(&units).map_err(|_| AcquisitionError::Protocol)?);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    if result.len() > 4096 || result.chars().any(char::is_control) {
        return Err(AcquisitionError::Protocol);
    }
    Ok(result)
}

pub(super) fn record(
    metadata: &Metadata,
    folder: &Folder,
    raw: &[u8],
) -> Result<MessageRecord, AcquisitionError> {
    let mut record = MessageRecord {
        delivery: Default::default(),
        uid: metadata.uid,
        size: 0,
        internal_date: String::new(),
        flags_observed: true,
        flags: BTreeSet::new(),
        gmail_message_id: None,
        gmail_thread_id: None,
        gmail_labels: BTreeSet::new(),
        headers: parse_headers(raw, Direction::Received).map_err(|_| AcquisitionError::Protocol)?,
    };
    metadata.apply(&mut record, folder);
    Ok(record)
}

pub(crate) mod original;
#[cfg(test)]
pub(crate) mod tests;

fn attribute_name(attribute: &NameAttribute<'_>) -> String {
    let name = match attribute {
        NameAttribute::NoInferiors => "NoInferiors",
        NameAttribute::NoSelect => "Noselect",
        NameAttribute::Marked => "Marked",
        NameAttribute::Unmarked => "Unmarked",
        NameAttribute::All => "All",
        NameAttribute::Archive => "Archive",
        NameAttribute::Drafts => "Drafts",
        NameAttribute::Flagged => "Flagged",
        NameAttribute::Junk => "Junk",
        NameAttribute::Sent => "Sent",
        NameAttribute::Trash => "Trash",
        NameAttribute::Extension(v) => return v.to_string(),
        _ => "Unknown",
    };
    format!("\\{name}")
}
fn parse_count(raw: &[u8], wire: &str) -> CountObservation {
    let mut rest = raw;
    let mut values = Vec::new();
    let mut responses = 0;
    while !rest.is_empty() {
        let Ok((remaining, response)) = imap_proto::parser::parse_response(rest) else {
            return CountObservation::unknown(UnknownCount::Malformed);
        };
        if let Response::MailboxData(MailboxDatum::Status { mailbox, status }) = response {
            if mailbox != wire {
                return CountObservation::unknown(UnknownCount::Ambiguous);
            }
            responses += 1;
            for v in status {
                if let imap_proto::StatusAttribute::Messages(n) = v {
                    values.push(n);
                }
            }
        }
        rest = remaining;
    }
    match values.as_slice() {
        [messages] if responses == 1 => CountObservation::Observed {
            messages: *messages,
            observed_at: now(),
            source: CountSource::StatusMessages,
        },
        [] => CountObservation::unknown(UnknownCount::Missing),
        _ => CountObservation::unknown(UnknownCount::Ambiguous),
    }
}

#[cfg(test)]
pub(super) type TestConnection = Arc<
    dyn Fn(&ReadPermit, &str, &SecretValue, Cancellation) -> Result<ReadOnlyImap, AcquisitionError>
        + Send
        + Sync,
>;
impl super::Accounts {
    pub(super) fn connect_read(
        &self,
        permit: &ReadPermit,
        login: &str,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<ReadOnlyImap, AcquisitionError> {
        #[cfg(test)]
        if let Some(factory) = &self.imap_factory {
            return factory(permit, login, password, cancel);
        }
        ReadOnlyImap::connect(permit, login, password, cancel)
    }
    pub(crate) fn installed_test_transport(&self) -> bool {
        #[cfg(test)]
        {
            self.imap_factory.is_some()
        }
        #[cfg(not(test))]
        {
            false
        }
    }
}
