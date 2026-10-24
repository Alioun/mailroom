use super::{AcquisitionError, Provider, SecretValue, scan::Cancellation};
use crate::{
    census::{Direction, HEADER_FIELDS, MessageRecord, parse_headers},
    policy::ReadPermit,
};
use imap_proto::{
    AttributeValue, MailboxDatum, MessageSection, NameAttribute, Response, SectionPath,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) const BATCH: usize = 25;
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(super) struct Folder {
    pub roles: BTreeSet<String>,
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
}

pub(super) trait MailRead {
    fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError>;
    fn inventory(&mut self, folder: &Folder) -> Result<(u32, BTreeSet<u32>), AcquisitionError>;
    fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError>;
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError>;
}

struct Budget {
    remaining: usize,
    deadline: Instant,
}
struct LimitedTls {
    tls: native_tls::TlsStream<TcpStream>,
    budget: Arc<Mutex<Budget>>,
    cancel: Cancellation,
}
impl Read for LimitedTls {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut budget = self.budget.lock().expect("response budget");
        if self.cancel.is_cancelled() || budget.remaining == 0 || Instant::now() > budget.deadline {
            return Err(io::Error::other("Read limit reached"));
        }
        let limit = buf.len().min(budget.remaining);
        let read = self.tls.read(&mut buf[..limit])?;
        budget.remaining -= read;
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

pub(super) struct ReadOnlyImap {
    session: imap::Session<LimitedTls>,
    budget: Arc<Mutex<Budget>>,
    cancel: Cancellation,
    gmail: bool,
    special_use: bool,
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
        let tls = connector
            .connect(hostname, tcp)
            .map_err(|_| AcquisitionError::Tls)?;
        let budget = Arc::new(Mutex::new(Budget {
            remaining: RESPONSE_LIMIT,
            deadline: Instant::now() + TIMEOUT,
        }));
        let mut client = imap::Client::new(LimitedTls {
            tls,
            budget: budget.clone(),
            cancel: cancel.clone(),
        });
        client
            .read_greeting()
            .map_err(|_| AcquisitionError::Protocol)?;
        let session = client
            .login(login, password.text()?)
            .map_err(|_| AcquisitionError::Authentication)?;
        let mut result = Self {
            session,
            budget,
            cancel,
            gmail,
            special_use: false,
        };
        result.reset()?;
        let capabilities = result
            .session
            .capabilities()
            .map_err(|_| AcquisitionError::Protocol)?;
        if !capabilities.has_str("IMAP4rev1") || (gmail && !capabilities.has_str("X-GM-EXT-1")) {
            return Err(AcquisitionError::Protocol);
        }
        result.special_use = capabilities.has_str("SPECIAL-USE");
        Ok(result)
    }
    fn reset(&self) -> Result<(), AcquisitionError> {
        self.cancel.check()?;
        *self.budget.lock().expect("response budget") = Budget {
            remaining: RESPONSE_LIMIT,
            deadline: Instant::now() + TIMEOUT,
        };
        Ok(())
    }
    fn fetch(&mut self, uids: &[u32], headers: bool) -> Result<Vec<u8>, AcquisitionError> {
        if uids.is_empty() || uids.len() > BATCH || uids.contains(&0) {
            return Err(AcquisitionError::InvalidInput);
        }
        self.reset()?;
        let sequence = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let fields = if headers {
            format!(
                "BODY.PEEK[HEADER.FIELDS ({})]<0.65537>",
                HEADER_FIELDS.join(" ").to_ascii_uppercase()
            )
        } else if self.gmail {
            "RFC822.SIZE INTERNALDATE FLAGS X-GM-MSGID X-GM-THRID X-GM-LABELS".to_owned()
        } else {
            "RFC822.SIZE INTERNALDATE FLAGS".to_owned()
        };
        self.session
            .run_command_and_read_response(format!("UID FETCH {sequence} (UID {fields})"))
            .map_err(|_| AcquisitionError::Protocol)
    }
}
impl MailRead for ReadOnlyImap {
    fn folders(&mut self) -> Result<Vec<Folder>, AcquisitionError> {
        self.reset()?;
        if !self.special_use && !self.gmail {
            return Err(AcquisitionError::Protocol);
        }
        let command = if self.special_use {
            "LIST \"\" \"*\" RETURN (SPECIAL-USE)"
        } else {
            "LIST \"\" *"
        };
        let raw = self
            .session
            .run_command_and_read_response(command)
            .map_err(|_| AcquisitionError::Protocol)?;
        let mut rest = raw.as_slice();
        let mut folders = Vec::new();
        while !rest.is_empty() {
            let (remaining, response) =
                imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
            if let Response::MailboxData(MailboxDatum::List {
                name_attributes: attrs,
                name,
                ..
            }) = response
            {
                folders.push(Folder {
                    roles: attrs
                        .iter()
                        .filter_map(|a| match a {
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
                    wire: name.to_string(),
                    name: decode_name(&name)?,
                    sent: attrs.contains(&NameAttribute::Sent),
                    all: attrs.contains(&NameAttribute::All),
                    excluded: attrs.iter().any(|a| {
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
    fn inventory(&mut self, folder: &Folder) -> Result<(u32, BTreeSet<u32>), AcquisitionError> {
        self.reset()?;
        let mailbox = self
            .session
            .examine(&folder.wire)
            .map_err(|_| AcquisitionError::Protocol)?;
        if !mailbox.is_read_only {
            return Err(AcquisitionError::Protocol);
        }
        let validity = mailbox
            .uid_validity
            .filter(|v| *v != 0)
            .ok_or(AcquisitionError::Incomplete)?;
        self.reset()?;
        let uids: BTreeSet<_> = self
            .session
            .uid_search("ALL")
            .map_err(|_| AcquisitionError::Protocol)?
            .into_iter()
            .collect();
        if uids.len() != mailbox.exists as usize || uids.len() > 250_000 || uids.contains(&0) {
            return Err(AcquisitionError::Incomplete);
        }
        Ok((validity, uids))
    }
    fn metadata(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Metadata>, AcquisitionError> {
        let raw = self.fetch(uids, false)?;
        let mut result = BTreeMap::new();
        parse_fetches(&raw, |attrs| {
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
                || (self.gmail && (item.gmail_id.is_none() || item.thread_id.is_none() || !labels))
                || result.insert(uid, item).is_some()
            {
                return Err(AcquisitionError::Incomplete);
            }
            Ok(())
        })?;
        validate_uids(uids, result.keys().copied())?;
        Ok(result)
    }
    fn headers(&mut self, uids: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>, AcquisitionError> {
        let raw = self.fetch(uids, true)?;
        let mut result = BTreeMap::new();
        parse_fetches(&raw, |attrs| {
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
            if header.len() > 65536 || result.insert(uid, header.to_vec()).is_some() {
                return Err(AcquisitionError::Incomplete);
            }
            Ok(())
        })?;
        validate_uids(uids, result.keys().copied())?;
        Ok(result)
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
    Ok(result)
}

pub(super) fn record(
    metadata: &Metadata,
    folder: &Folder,
    raw: &[u8],
) -> Result<MessageRecord, AcquisitionError> {
    let mut record = MessageRecord {
        uid: metadata.uid,
        size: 0,
        internal_date: String::new(),
        flags: BTreeSet::new(),
        gmail_message_id: None,
        gmail_thread_id: None,
        gmail_labels: BTreeSet::new(),
        headers: parse_headers(raw, Direction::Received).map_err(|_| AcquisitionError::Protocol)?,
    };
    metadata.apply(&mut record, folder);
    Ok(record)
}

#[cfg(test)]
mod tests;
