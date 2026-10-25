use super::*;
use crate::{
    acquisition::Provider,
    backlog::imap_move::{
        Completion, CopyUidMapping, MailboxIdentity, MailboxReadback, Metadata, MoveEvidence,
        TaggedStatus, Uid, UidValidity,
    },
};
use std::collections::BTreeMap;

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

pub(in crate::acquisition) struct BacklogPermit<'a>(&'a InstalledSource);

impl InstalledSource {
    pub(in crate::acquisition) fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            "loopback-imap-move-uidplus/v1",
            &self.account,
            &self.login,
            self.address,
            &self.tls_name,
            &self.root,
            &self.epoch,
            &self.authority,
        ))
        .expect("installed MOVE source");
        data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
    }

    pub(in crate::acquisition) fn permit(
        &self,
        account: &AccountId,
        provider: Provider,
        login: &str,
        authority: &str,
    ) -> Result<BacklogPermit<'_>, Error> {
        if self.account != *account
            || provider != Provider::MailboxOrg
            || self.login != login
            || login.is_empty()
            || self.authority != authority
            || authority.is_empty()
            || self.epoch.is_empty()
            || !self.address.ip().is_loopback()
            || self.root.is_empty()
            || self.root.len() > 8192
            || self.tls_name.is_empty()
        {
            return Err(Error::Unsupported);
        }
        Ok(BacklogPermit(self))
    }
}

pub(in crate::acquisition) struct Connection {
    io: Session,
    capabilities: BTreeSet<String>,
}

impl Connection {
    pub(in crate::acquisition) fn connect(
        permit: &BacklogPermit<'_>,
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
        if !capabilities.contains("MOVE") || !capabilities.contains("UIDPLUS") {
            return Err(Error::Unsupported);
        }
        Ok(Self { io, capabilities })
    }

    pub(in crate::acquisition) fn inventory(&mut self) -> Result<Inventory, Error> {
        let inventory = self.io.observe()?;
        if inventory.capabilities != self.capabilities {
            return Err(Error::Unstable);
        }
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

    pub(in crate::acquisition) fn readback(
        &mut self,
        mailbox: &MailboxIdentity,
        uids: &BTreeSet<Uid>,
    ) -> Result<MailboxReadback, Error> {
        if uids.len() > crate::backlog::BATCH_LIMIT {
            return Err(Error::Limit);
        }
        self.select(mailbox)?;
        let mut messages = BTreeMap::new();
        for chunk in uids.iter().copied().collect::<Vec<_>>().chunks(25) {
            let set = chunk
                .iter()
                .map(|u| u.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let command = format!(
                "UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"
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
                let (uid, metadata) = parse_metadata(row)?;
                if !chunk.contains(&uid) {
                    return Ok(());
                }
                if messages.insert(uid, metadata).is_some() {
                    return Err(Error::Unstable);
                }
                Ok(())
            })?;
        }
        Ok(MailboxReadback {
            identity: mailbox.clone(),
            messages,
        })
    }

    pub(in crate::acquisition) fn move_messages(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut MoveEvidence,
        mut persist: impl FnMut(&MoveEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.io.source != permit.0.identity()
            || evidence.attempted()
            || evidence.completion().is_some()
            || evidence.mapping().is_some()
        {
            return Err(Error::Unsupported);
        }
        let intent = evidence.command().clone();
        let inventory = self.inventory()?;
        if [intent.source(), intent.destination()].iter().any(|m| {
            inventory
                .get(&m.name.decoded)
                .is_none_or(|found| found.name != m.name || !found.selectable())
        }) {
            return Err(Error::Unsupported);
        }
        self.select(intent.destination())?;
        let before = self.readback(
            intent.source(),
            &intent.messages().keys().copied().collect(),
        )?;
        if &before.messages != intent.messages() {
            return Err(Error::Unstable);
        }
        self.select(intent.source())?;
        let destination = quoted(intent.destination().name.wire.as_bytes())?;
        let command = [
            b"UID MOVE ".as_slice(),
            intent.wire_uids().as_bytes(),
            b" ",
            &destination,
        ]
        .concat();
        evidence.begin_attempt().map_err(|_| Error::Unsupported)?;
        persist(evidence)?;
        let result = self.exchange(&command, |row, tagged| {
            let status = row.get(1).and_then(Token::atom);
            if matches!(status, Some("OK" | "NO" | "BAD")) {
                let code = row.get(2).and_then(Token::atom);
                if let Some(code) = code.filter(|c| c.starts_with("[COPYUID ")) {
                    match CopyUidMapping::parse(code, &intent) {
                        Ok(mapping) => {
                            if evidence.record_mapping(mapping).is_err() {
                                persist(evidence)?;
                                return Err(Error::Protocol);
                            }
                            persist(evidence)?;
                        }
                        Err(_) => {
                            evidence.reject_mapping();
                            persist(evidence)?;
                            return Err(Error::Protocol);
                        }
                    }
                } else if code == Some("[UIDNOTSTICKY]") {
                    evidence.reject_mapping();
                    persist(evidence)?;
                    return Err(Error::Unsupported);
                }
                if tagged {
                    let status = match status {
                        Some("OK") => TaggedStatus::Ok,
                        Some("NO") => TaggedStatus::No,
                        Some("BAD") => TaggedStatus::Bad,
                        _ => return Err(Error::Protocol),
                    };
                    evidence
                        .record_completion(Completion::Tagged {
                            status,
                            code: code.map(str::to_owned),
                        })
                        .map_err(|_| Error::Protocol)?;
                    persist(evidence)?;
                }
            } else if tagged {
                return Err(Error::Protocol);
            }
            Ok(())
        });
        if evidence.completion().is_none() {
            evidence
                .record_completion(Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        result
    }
}

fn parse_metadata(row: &[Token]) -> Result<(Uid, Metadata), Error> {
    if row.get(3) != Some(&Token::Open) || row.last() != Some(&Token::Close) {
        return Err(Error::Protocol);
    }
    let mut uid = None;
    let mut flags = None;
    let mut date = None;
    let mut size = None;
    let mut hash = None;
    let mut at = 4;
    while at + 1 < row.len() {
        let key = row[at].atom().ok_or(Error::Protocol)?;
        at += 1;
        match key {
            "UID" => {
                if uid.is_some() {
                    return Err(Error::Protocol);
                }
                uid = Some(
                    Uid::new(
                        row.get(at)
                            .and_then(Token::atom)
                            .ok_or(Error::Protocol)?
                            .parse()
                            .map_err(|_| Error::Protocol)?,
                    )
                    .map_err(|_| Error::Protocol)?,
                );
                at += 1;
            }
            "RFC822.SIZE" => {
                if size.is_some() {
                    return Err(Error::Protocol);
                }
                size = Some(
                    row.get(at)
                        .and_then(Token::atom)
                        .ok_or(Error::Protocol)?
                        .parse()
                        .map_err(|_| Error::Protocol)?,
                );
                at += 1;
            }
            "INTERNALDATE" => {
                if date.is_some() {
                    return Err(Error::Protocol);
                }
                date = Some(row.get(at).ok_or(Error::Protocol)?.text()?);
                at += 1;
            }
            "FLAGS" => {
                if flags.is_some() || row.get(at) != Some(&Token::Open) {
                    return Err(Error::Protocol);
                }
                at += 1;
                let mut found = BTreeSet::new();
                while row.get(at) != Some(&Token::Close) {
                    let flag = row.get(at).and_then(Token::atom).ok_or(Error::Protocol)?;
                    if !flag.eq_ignore_ascii_case("\\Recent") && !found.insert(flag.into()) {
                        return Err(Error::Protocol);
                    }
                    if found.len() > 64 {
                        return Err(Error::Limit);
                    }
                    at += 1;
                }
                flags = Some(found);
                at += 1;
            }
            "BODY[HEADER.FIELDS" => {
                if hash.is_some()
                    || row.get(at) != Some(&Token::Open)
                    || row.get(at + 1).and_then(Token::atom) != Some("MESSAGE-ID")
                    || row.get(at + 2) != Some(&Token::Close)
                    || row.get(at + 3).and_then(Token::atom) != Some("]")
                {
                    return Err(Error::Protocol);
                }
                let Some(Token::String(raw)) = row.get(at + 4) else {
                    return Err(Error::Protocol);
                };
                hash = crate::census::parse_headers(raw, crate::census::Direction::Received)
                    .map_err(|_| Error::Protocol)?
                    .message_id_hash;
                at += 5;
            }
            _ => return Err(Error::Unsupported),
        }
    }
    if at != row.len() - 1 {
        return Err(Error::Protocol);
    }
    Ok((
        uid.ok_or(Error::Protocol)?,
        Metadata {
            message_id_hash: hash.ok_or(Error::Unsupported)?,
            size: size.ok_or(Error::Protocol)?,
            internal_date: date.ok_or(Error::Protocol)?,
            flags: flags.ok_or(Error::Protocol)?,
        },
    ))
}

#[cfg(test)]
mod tests;
