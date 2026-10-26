use super::{message_move::RuntimeBinding, *};
use crate::{
    acquisition::Provider,
    backlog::{
        gmail_label::{GmailId, INBOX, LabelBatch, LabelReadback, Snapshot},
        imap_move::{Completion, MailboxIdentity, TaggedStatus, Uid, UidValidity},
    },
};
use std::collections::BTreeMap;

pub(in crate::acquisition) const AUTHORITY: &str = "fixture-gmail-labels/v1";

#[derive(Clone)]
pub(in crate::acquisition) struct InstalledSource {
    pub account: AccountId,
    pub login: String,
    pub address: SocketAddr,
    pub tls_name: String,
    pub root: Vec<u8>,
    pub epoch: String,
    pub authority: String,
    pub command_timeout: Duration,
    pub operation_timeout: Duration,
}

pub(in crate::acquisition) struct LabelPermit<'a>(&'a InstalledSource);

impl LabelPermit<'_> {
    pub(in crate::acquisition) fn source(&self) -> &InstalledSource {
        self.0
    }
}

impl InstalledSource {
    pub(in crate::acquisition) fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            "loopback-imap-gmail-labels/v1",
            &self.account,
            &self.login,
            self.address,
            &self.tls_name,
            &self.root,
            &self.epoch,
            &self.authority,
        ))
        .expect("installed Gmail label source");
        data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
    }

    pub(in crate::acquisition) fn installed(&self) -> bool {
        self.authority == AUTHORITY
            && !self.epoch.is_empty()
            && self.address.ip().is_loopback()
            && !self.login.is_empty()
            && !self.root.is_empty()
            && self.root.len() <= 8192
            && !self.tls_name.is_empty()
    }

    pub(in crate::acquisition) fn runtime_permit<'a>(
        &'a self,
        binding: &RuntimeBinding,
    ) -> Result<LabelPermit<'a>, Error> {
        if binding.account != self.account
            || binding.provider != Provider::Gmail
            || binding.login != self.login
            || binding.source != self.identity()
            || binding.credential_key_recovery.is_empty()
            || binding.workspace.is_empty()
            || binding.topology.is_empty()
            || !self.installed()
        {
            return Err(Error::Unsupported);
        }
        Ok(LabelPermit(self))
    }
}

pub(in crate::acquisition) struct Connection {
    io: Session,
    capabilities: BTreeSet<String>,
}

/// The discovered Gmail topology: the single selectable All Mail mailbox and
/// the label mailboxes LIST reports.
pub(in crate::acquisition) fn all_mail(inventory: &Inventory) -> Result<&Mailbox, Error> {
    let mut all = inventory
        .mailboxes
        .iter()
        .filter(|m| m.attributes.contains("\\ALL"));
    match (all.next(), all.next()) {
        (Some(m), None) if m.selectable() => Ok(m),
        _ => Err(Error::Unsupported),
    }
}

fn label_wire(label: &str) -> Result<Vec<u8>, Error> {
    if label == INBOX {
        return Ok(INBOX.as_bytes().to_vec());
    }
    let name = ExactName::new(label)?;
    quoted(name.wire.as_bytes())
}

fn decode_label(token: &Token) -> Result<String, Error> {
    match token {
        Token::Atom(a) if a.starts_with('\\') => Ok(a.clone()),
        Token::String(s) if s.first() == Some(&b'\\') => {
            String::from_utf8(s.clone()).map_err(|_| Error::Protocol)
        }
        Token::Atom(_) | Token::String(_) => Ok(ExactName::from_wire(&token.text()?)?.decoded),
        _ => Err(Error::Protocol),
    }
}

impl Connection {
    pub(in crate::acquisition) fn connect(
        permit: &LabelPermit<'_>,
        password: &SecretValue,
        cancel: Cancellation,
    ) -> Result<Self, Error> {
        let source = permit.0;
        let mut io = Session::open(
            Endpoint {
                address: source.address,
                tls_name: &source.tls_name,
                root: &source.root,
                identity: source.identity(),
                command_timeout: source.command_timeout,
                operation_timeout: source.operation_timeout,
            },
            &source.login,
            password,
            cancel,
        )?;
        io.code_limit = 16 * 1024;
        let capabilities = io.capabilities()?;
        if !capabilities.contains("X-GM-EXT-1") {
            return Err(Error::Unsupported);
        }
        Ok(Self { io, capabilities })
    }

    pub(in crate::acquisition) fn inventory(&mut self) -> Result<Inventory, Error> {
        let inventory = self.io.observe()?;
        if inventory.capabilities != self.capabilities {
            return Err(Error::Unstable);
        }
        all_mail(&inventory)?;
        Ok(inventory)
    }

    fn exchange(
        &mut self,
        command: &[u8],
        mut row: impl FnMut(&[Token], bool) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let tag = self.io.begin(command)?;
        for _ in 0..2048 {
            let response = self.io.row()?;
            let first = response
                .first()
                .and_then(Token::atom)
                .ok_or(Error::Protocol)?;
            let tagged = first == tag;
            if !tagged && first != "*" {
                return Err(Error::Protocol);
            }
            if tagged {
                self.io.completed();
            }
            if response.get(1).and_then(Token::atom) == Some("BYE") {
                return Err(Error::Bye);
            }
            row(&response, tagged)?;
            if tagged {
                return Ok(());
            }
        }
        Err(Error::Limit)
    }

    fn select(&mut self, mailbox: &MailboxIdentity) -> Result<(), Error> {
        let name = quoted(mailbox.name.wire.as_bytes())?;
        let mut validity = None;
        let mut writable = false;
        self.exchange(&[b"SELECT ".as_slice(), &name].concat(), |row, tagged| {
            if let Some(code) = row.get(2).and_then(Token::atom) {
                if code == "[UIDNOTSTICKY]" || code == "[READ-ONLY]" {
                    return Err(Error::Unsupported);
                }
                if let Some(v) = code
                    .strip_prefix("[UIDVALIDITY ")
                    .and_then(|s| s.strip_suffix(']'))
                {
                    let v = UidValidity::new(v.parse().map_err(|_| Error::Protocol)?)
                        .map_err(|_| Error::Protocol)?;
                    if validity.replace(v).is_some() {
                        return Err(Error::Protocol);
                    }
                } else if code == "[READ-WRITE]" {
                    writable = true;
                } else if code.starts_with('[')
                    && !code.starts_with("[PERMANENTFLAGS ")
                    && !code.starts_with("[UIDNEXT ")
                    && !code.starts_with("[UNSEEN ")
                    && !code.starts_with("[HIGHESTMODSEQ ")
                {
                    return Err(Error::UnknownCode);
                }
            }
            if tagged && row.get(1).and_then(Token::atom) != Some("OK") {
                return Err(Error::No);
            }
            Ok(())
        })?;
        if validity != Some(mailbox.uidvalidity) || !writable {
            return Err(Error::Unstable);
        }
        Ok(())
    }

    /// Fresh state of the requested All Mail UIDs. An absent UID is omitted.
    pub(in crate::acquisition) fn readback(
        &mut self,
        mailbox: &MailboxIdentity,
        uids: &BTreeSet<Uid>,
    ) -> Result<LabelReadback, Error> {
        if uids.len() > crate::backlog::BATCH_LIMIT {
            return Err(Error::Limit);
        }
        self.select(mailbox)?;
        let mut messages = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for chunk in uids.iter().copied().collect::<Vec<_>>().chunks(25) {
            let set = chunk
                .iter()
                .map(|u| u.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let command = format!(
                "UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE X-GM-MSGID X-GM-LABELS BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"
            );
            self.exchange(command.as_bytes(), |row, tagged| {
                if tagged {
                    return if row.get(1).and_then(Token::atom) == Some("OK") {
                        Ok(())
                    } else {
                        Err(Error::No)
                    };
                }
                if row.get(2).and_then(Token::atom) != Some("FETCH") {
                    return Ok(());
                }
                let Some(uid) = super::message_move::fetch_uid(row)? else {
                    return Ok(());
                };
                if !chunk.contains(&uid) {
                    return Ok(());
                }
                let (uid, snapshot) = parse_snapshot(row)?;
                if !ids.insert(snapshot.gmail_id.clone())
                    || messages.insert(uid, snapshot).is_some()
                {
                    return Err(Error::Unstable);
                }
                Ok(())
            })?;
        }
        Ok(LabelReadback {
            identity: mailbox.clone(),
            messages,
        })
    }

    /// Issues one approved label step. The members must still match the
    /// approved state exactly. Evidence persists before transmission and
    /// after completion; untagged FETCH rows are not attribution.
    pub(in crate::acquisition) fn store(
        &mut self,
        permit: &LabelPermit<'_>,
        batch: &mut LabelBatch,
        index: usize,
        mut persist: impl FnMut(&LabelBatch) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let step = batch.steps().get(index).ok_or(Error::Unsupported)?.clone();
        if self.io.source != permit.0.identity() || step.attempted() {
            return Err(Error::Unsupported);
        }
        let intent = batch.intent().clone();
        let inventory = self.inventory()?;
        if all_mail(&inventory)?.name != intent.context().name
            || inventory
                .get(&intent.destination().decoded)
                .is_none_or(|m| m.name != *intent.destination())
        {
            return Err(Error::Unsupported);
        }
        let expected = batch.expected(index);
        let before = self.readback(intent.context(), &step.uids)?;
        if before.messages.len() != expected.len()
            || expected
                .iter()
                .any(|(uid, (want, _))| before.messages.get(uid) != Some(want))
        {
            return Err(Error::Unstable);
        }
        let set = step
            .uids
            .iter()
            .map(|u| u.get().to_string())
            .collect::<Vec<_>>()
            .join(",");
        let command = [
            b"UID STORE ".as_slice(),
            set.as_bytes(),
            if step.kind.adds() {
                b" +X-GM-LABELS ("
            } else {
                b" -X-GM-LABELS ("
            },
            &label_wire(step.kind.label(intent.destination()))?,
            b")",
        ]
        .concat();
        batch.begin_attempt(index).map_err(|_| Error::Unsupported)?;
        persist(batch)?;
        let mut completion = None;
        let result = self.exchange(&command, |row, tagged| {
            let status = row.get(1).and_then(Token::atom);
            if tagged {
                completion = Some(Completion::Tagged {
                    status: match status {
                        Some("OK") => TaggedStatus::Ok,
                        Some("NO") => TaggedStatus::No,
                        Some("BAD") => TaggedStatus::Bad,
                        _ => return Err(Error::Protocol),
                    },
                    code: row.get(2).and_then(Token::atom).map(str::to_owned),
                });
            } else if row.get(2).and_then(Token::atom) == Some("FETCH") {
                parse_store_fetch(row)?;
            }
            Ok(())
        });
        batch
            .record_completion(index, completion.unwrap_or(Completion::Lost))
            .map_err(|_| Error::Protocol)?;
        persist(batch)?;
        result
    }
}

/// An untagged FETCH during STORE must be well formed. Its labels are not
/// recorded: readback is the evidence.
fn parse_store_fetch(row: &[Token]) -> Result<(), Error> {
    if row.get(3) != Some(&Token::Open) || row.last() != Some(&Token::Close) {
        return Err(Error::Protocol);
    }
    Ok(())
}

fn parse_snapshot(row: &[Token]) -> Result<(Uid, Snapshot), Error> {
    if row.get(3) != Some(&Token::Open) || row.last() != Some(&Token::Close) {
        return Err(Error::Protocol);
    }
    let mut uid = None;
    let mut flags = None;
    let mut labels = None;
    let mut date = None;
    let mut size = None;
    let mut gmail = None;
    let mut hash = None;
    let mut at = 4;
    let list = |at: &mut usize, label: bool| -> Result<BTreeSet<String>, Error> {
        if row.get(*at) != Some(&Token::Open) {
            return Err(Error::Protocol);
        }
        *at += 1;
        let mut found = BTreeSet::new();
        while row.get(*at) != Some(&Token::Close) {
            let token = row.get(*at).ok_or(Error::Protocol)?;
            let value = if label {
                decode_label(token)?
            } else {
                token.atom().ok_or(Error::Protocol)?.to_owned()
            };
            if !label && value.eq_ignore_ascii_case("\\Recent") {
                *at += 1;
                continue;
            }
            if !found.insert(value) {
                return Err(Error::Protocol);
            }
            if found.len() > if label { 128 } else { 64 } {
                return Err(Error::Limit);
            }
            *at += 1;
        }
        *at += 1;
        Ok(found)
    };
    while at + 1 < row.len() {
        let key = row[at].atom().ok_or(Error::Protocol)?;
        at += 1;
        let number = |at: usize| -> Result<&str, Error> {
            row.get(at).and_then(Token::atom).ok_or(Error::Protocol)
        };
        match key {
            "UID" if uid.is_none() => {
                uid = Some(
                    Uid::new(number(at)?.parse().map_err(|_| Error::Protocol)?)
                        .map_err(|_| Error::Protocol)?,
                );
                at += 1;
            }
            "RFC822.SIZE" if size.is_none() => {
                size = Some(number(at)?.parse().map_err(|_| Error::Protocol)?);
                at += 1;
            }
            "X-GM-MSGID" if gmail.is_none() => {
                gmail = Some(GmailId::new(number(at)?).ok_or(Error::Protocol)?);
                at += 1;
            }
            "INTERNALDATE" if date.is_none() => {
                date = Some(row.get(at).ok_or(Error::Protocol)?.text()?);
                at += 1;
            }
            "FLAGS" if flags.is_none() => flags = Some(list(&mut at, false)?),
            "X-GM-LABELS" if labels.is_none() => labels = Some(list(&mut at, true)?),
            "BODY[HEADER.FIELDS" if hash.is_none() => {
                if row.get(at) != Some(&Token::Open)
                    || row.get(at + 1).and_then(Token::atom) != Some("MESSAGE-ID")
                    || row.get(at + 2) != Some(&Token::Close)
                    || row.get(at + 3).and_then(Token::atom) != Some("]")
                {
                    return Err(Error::Protocol);
                }
                let Some(Token::String(raw)) = row.get(at + 4) else {
                    return Err(Error::Protocol);
                };
                hash = Some(
                    crate::census::parse_headers(raw, crate::census::Direction::Received)
                        .map_err(|_| Error::Protocol)?
                        .message_id_hash
                        .ok_or(Error::Unsupported)?,
                );
                at += 5;
            }
            _ => return Err(Error::Protocol),
        }
    }
    if at != row.len() - 1 {
        return Err(Error::Protocol);
    }
    let snapshot = Snapshot {
        gmail_id: gmail.ok_or(Error::Protocol)?,
        message_id_hash: hash.ok_or(Error::Protocol)?,
        size: size.ok_or(Error::Protocol)?,
        internal_date: date.ok_or(Error::Protocol)?,
        flags: flags.ok_or(Error::Protocol)?,
        labels: labels.ok_or(Error::Protocol)?,
    };
    if !snapshot.validate() {
        return Err(Error::Limit);
    }
    Ok((uid.ok_or(Error::Protocol)?, snapshot))
}

#[cfg(test)]
pub(crate) mod tests;
