//! Read-only acquisition of one original message.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OriginalError {
    /// The folder is gone or its UIDVALIDITY changed, so the UID names another message.
    Changed,
    Missing,
    TooLarge,
    Transport(AcquisitionError),
}
impl From<AcquisitionError> for OriginalError {
    fn from(error: AcquisitionError) -> Self {
        Self::Transport(error)
    }
}

impl ReadOnlyImap {
    /// Reads the exact bytes of the message `uid` in the folder named `name`, which
    /// must still have `uidvalidity`. EXAMINE and BODY.PEEK leave every flag,
    /// `\Seen` included, unchanged. Messages over `limit` bytes are refused
    /// before their content is requested.
    pub(crate) fn fetch_original(
        &mut self,
        name: &str,
        uidvalidity: u32,
        uid: u32,
        limit: usize,
    ) -> Result<Vec<u8>, OriginalError> {
        if uid == 0 || uidvalidity == 0 {
            return Err(OriginalError::Missing);
        }
        let folder = self
            .folders()?
            .into_iter()
            .find(|f| f.name == name && f.selectable)
            .ok_or(OriginalError::Changed)?;
        let examined = self
            .run(&self.examine_command(&folder)?, RESPONSE_LIMIT)?
            .map_err(protocol("original.examine"))?;
        if examined_validity(&examined)? != uidvalidity {
            return Err(OriginalError::Changed);
        }
        let sized = self
            .run(
                &format!("UID FETCH {uid} (UID RFC822.SIZE)"),
                RESPONSE_LIMIT,
            )?
            .map_err(protocol("original.size"))?;
        let mut size = None;
        parse_fetches(&sized.data, |attrs| {
            if super::uid(attrs)? == uid {
                size = attrs.iter().find_map(|a| match a {
                    AttributeValue::Rfc822Size(v) => Some(*v as usize),
                    _ => None,
                });
            }
            Ok(())
        })?;
        match size {
            None => return Err(OriginalError::Missing),
            Some(size) if size > limit => return Err(OriginalError::TooLarge),
            Some(_) => {}
        }
        let fetched = self
            .run(
                &format!("UID FETCH {uid} (UID BODY.PEEK[])"),
                limit + ROW_OVERHEAD,
            )?
            .map_err(protocol("original.body"))?;
        let mut body = None;
        parse_fetches(&fetched.data, |attrs| {
            if super::uid(attrs)? != uid {
                return Ok(());
            }
            for attr in attrs {
                if let AttributeValue::BodySection {
                    section: None,
                    index: None,
                    data: Some(data),
                } = attr
                {
                    if body.is_some() {
                        return Err(AcquisitionError::Incomplete);
                    }
                    body = Some(data.to_vec());
                }
            }
            Ok(())
        })?;
        let body = body.ok_or(OriginalError::Missing)?;
        if body.len() > limit {
            return Err(OriginalError::TooLarge);
        }
        Ok(body)
    }
}

impl super::super::Accounts {
    /// Reads the census message `folder/uidvalidity/uid` read-only through the
    /// account's IMAP login, as a census does, and parses it. The password goes
    /// only to the approved IMAP LOGIN and is dropped once the session is open.
    pub(crate) fn acquire_original(
        &self,
        account: &super::super::Account,
        folder: &str,
        uidvalidity: u32,
        uid: u32,
    ) -> Result<crate::original::Original, String> {
        let permit = crate::policy::read_permit(account.provider).map_err(|e| e.to_string())?;
        let password = self.password(account).map_err(|e| e.to_string())?;
        let session =
            self.connect_read(&permit, &account.login, &password, Cancellation::default());
        drop(password);
        let raw = session
            .map_err(|e| e.to_string())?
            .fetch_original(folder, uidvalidity, uid, crate::original::LIMIT)
            .map_err(|e| match e {
                OriginalError::Changed => "Folder or UIDVALIDITY changed".to_owned(),
                OriginalError::Missing => "Message no longer on the server".to_owned(),
                OriginalError::TooLarge => "Original exceeds the size limit".to_owned(),
                OriginalError::Transport(e) => e.to_string(),
            })?;
        crate::original::Original::parse(raw).map_err(|_| "Original cannot be parsed".into())
    }
}

fn examined_validity(examined: &Reply) -> Result<u32, AcquisitionError> {
    let read_only = matches!(
        imap_proto::parser::parse_response(&examined.done),
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
    let mut validity = None;
    let mut rest = examined.data.as_slice();
    while !rest.is_empty() {
        let (remaining, response) =
            imap_proto::parser::parse_response(rest).map_err(|_| AcquisitionError::Protocol)?;
        if let Response::Data {
            code: Some(ResponseCode::UidValidity(v)),
            ..
        } = response
        {
            validity = Some(v);
        }
        rest = remaining;
    }
    validity.ok_or(AcquisitionError::Incomplete)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{collections::BTreeSet, net::TcpListener, thread};

    const MESSAGE: &[u8] = b"From: billing@shop.example\r\nTo: books@work.example.test\r\nSubject: Rechnung\r\nMessage-ID: <m1@shop.example>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nA999 OK inside literal\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=r.pdf\r\n\r\n%PDF\r\n--b--\r\n";

    /// A one-connection IMAP fake holding one message whose flags it changes
    /// exactly as a server would: a non-PEEK body fetch or a STORE sets `\Seen`.
    pub(crate) struct Fake {
        pub(crate) address: std::net::SocketAddr,
        pub(crate) identity: crate::fake_tls::Identity,
        pub(crate) log: Arc<Mutex<(Vec<String>, BTreeSet<String>)>>,
        worker: thread::JoinHandle<()>,
    }
    impl Fake {
        fn start(validity: u32, size: usize) -> Self {
            Self::serve(
                validity,
                BTreeMap::from([(7, MESSAGE.to_vec())]),
                Some(size),
            )
        }
        /// Serves each of `messages` under its UID in INBOX and `Geschäftlich`.
        /// `size` overrides the reported RFC822.SIZE.
        pub(crate) fn serve(
            validity: u32,
            messages: BTreeMap<u32, Vec<u8>>,
            size: Option<usize>,
        ) -> Self {
            let identity = crate::fake_tls::identity("localhost");
            let config = identity.clone();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let log = Arc::new(Mutex::new((vec![], BTreeSet::new())));
            let state = log.clone();
            let worker = thread::spawn(move || {
                let (tcp, _) = listener.accept().unwrap();
                let mut io = BufReader::new(config.accept(tcp));
                io.get_mut().write_all(b"* OK fake\r\n").unwrap();
                let mut line = String::new();
                while matches!(io.read_line(&mut line), Ok(n) if n > 0) {
                    let (tag, command) = line.trim_end().split_once(' ').unwrap();
                    let (tag, command) = (tag.to_owned(), command.to_owned());
                    line.clear();
                    let mut s = state.lock().unwrap();
                    s.0.push(if command.starts_with("LOGIN") {
                        "LOGIN [REDACTED]".into()
                    } else {
                        command.clone()
                    });
                    let reply: Vec<u8> = if command.starts_with("LOGIN") {
                        format!("{tag} OK in\r\n").into()
                    } else if command == "CAPABILITY" {
                        format!("* CAPABILITY IMAP4rev1 SPECIAL-USE\r\n{tag} OK\r\n").into()
                    } else if command.starts_with("LIST") {
                        format!("* LIST () \"/\" \"INBOX\"\r\n* LIST () \"/\" \"Gesch&AOQ-ftlich\"\r\n{tag} OK\r\n").into()
                    } else if command.starts_with("EXAMINE") {
                        format!("* 1 EXISTS\r\n* OK [UIDVALIDITY {validity}] v\r\n{tag} OK [READ-ONLY] done\r\n").into()
                    } else if let Some((uid, message)) = command
                        .strip_prefix("UID FETCH ")
                        .and_then(|rest| rest.split_once(' '))
                        .and_then(|(uid, _)| uid.parse::<u32>().ok())
                        .and_then(|uid| messages.get(&uid).map(|m| (uid, m)))
                        .filter(|_| command.ends_with("(UID RFC822.SIZE)"))
                    {
                        let size = size.unwrap_or(message.len());
                        format!("* 1 FETCH (UID {uid} RFC822.SIZE {size})\r\n{tag} OK\r\n").into()
                    } else if let Some((uid, message)) = command
                        .strip_prefix("UID FETCH ")
                        .and_then(|rest| rest.split_once(' '))
                        .and_then(|(uid, _)| uid.parse::<u32>().ok())
                        .and_then(|uid| messages.get(&uid).map(|m| (uid, m)))
                        .filter(|_| command.contains("(UID BODY"))
                    {
                        if !command.contains("PEEK") {
                            s.1.insert("\\Seen".into());
                        }
                        let mut r =
                            format!("* 1 FETCH (UID {uid} BODY[] {{{}}}\r\n", message.len())
                                .into_bytes();
                        r.extend_from_slice(&message);
                        r.extend_from_slice(format!(")\r\n{tag} OK\r\n").as_bytes());
                        r
                    } else if command.starts_with("UID FETCH") {
                        format!("{tag} OK no such message\r\n").into()
                    } else {
                        s.1.insert("\\Seen".into());
                        format!("{tag} BAD unexpected\r\n").into()
                    };
                    drop(s);
                    if io
                        .get_mut()
                        .write_all(&reply)
                        .and_then(|_| io.get_mut().flush())
                        .is_err()
                    {
                        return;
                    }
                }
            });
            Self {
                address,
                identity,
                log,
                worker,
            }
        }
        fn session(&self) -> ReadOnlyImap {
            ReadOnlyImap::open(
                TcpStream::connect(self.address).unwrap(),
                "localhost",
                self.identity.connector(),
                "owner@example.test",
                &SecretValue::new(b"dummy-imap-password".to_vec()),
                Cancellation::default(),
                false,
            )
            .unwrap()
        }
        pub(crate) fn finish(self) -> (Vec<String>, BTreeSet<String>) {
            self.worker.join().unwrap();
            let log = self.log.lock().unwrap();
            (log.0.clone(), log.1.clone())
        }
    }

    /// The commands and flag changes of every connection an installed factory opened.
    pub(crate) type Logs = Arc<Mutex<Vec<Arc<Mutex<(Vec<String>, BTreeSet<String>)>>>>>;

    /// Installs an IMAP factory that opens a fresh fake serving `messages` from
    /// INBOX at `validity` for each connection.
    pub(crate) fn install(
        repo: &mut super::super::super::Accounts,
        validity: u32,
        messages: BTreeMap<u32, Vec<u8>>,
    ) -> Logs {
        let logs: Logs = Arc::default();
        let record = logs.clone();
        repo.imap_factory = Some(Arc::new(move |permit, login, password, cancel| {
            let fake = Fake::serve(validity, messages.clone(), None);
            record.lock().unwrap().push(fake.log.clone());
            ReadOnlyImap::open(
                TcpStream::connect(fake.address).unwrap(),
                "localhost",
                fake.identity.connector(),
                login,
                password,
                cancel,
                permit.provider() == Provider::Gmail,
            )
        }));
        logs
    }

    #[test]
    fn fetch_reads_exact_bytes_read_only_without_setting_seen() {
        let fake = Fake::start(9, MESSAGE.len());
        let mut session = fake.session();
        let raw = session.fetch_original("Geschäftlich", 9, 7, 1024).unwrap();
        assert_eq!(
            raw, MESSAGE,
            "literal with an embedded tagged-looking line is exact"
        );
        let original = crate::original::Original::parse(raw).unwrap();
        assert_eq!(original.attachments[0].media_type, "application/pdf");
        drop(session);
        let (commands, flags) = fake.finish();
        assert!(flags.is_empty(), "no flag changed");
        assert!(commands.iter().any(|c| c == "EXAMINE \"Gesch&AOQ-ftlich\""));
        assert!(
            commands.iter().all(|c| !c.starts_with("SELECT")
                && !c.starts_with("STORE")
                && !c.contains("BODY[]"))
        );
    }

    #[test]
    fn changed_validity_missing_uid_and_oversized_messages_are_refused_before_content() {
        let fake = Fake::start(10, MESSAGE.len());
        let mut session = fake.session();
        assert_eq!(
            session.fetch_original("INBOX", 9, 7, 1024),
            Err(OriginalError::Changed)
        );
        assert_eq!(
            session.fetch_original("Gone", 10, 7, 1024),
            Err(OriginalError::Changed)
        );
        assert_eq!(
            session.fetch_original("INBOX", 10, 8, 1024),
            Err(OriginalError::Missing)
        );
        assert_eq!(
            session.fetch_original("INBOX", 10, 7, 16),
            Err(OriginalError::TooLarge)
        );
        drop(session);
        let (commands, flags) = fake.finish();
        assert!(flags.is_empty());
        assert!(
            commands.iter().all(|c| !c.contains("BODY.PEEK")),
            "no content was requested"
        );
    }
}
