use super::*;
use crate::{
    acquisition::Provider,
    backlog::imap_move::{
        Completion, CopyUidMapping, MailboxIdentity, MailboxReadback, Metadata, MoveEvidence,
        TaggedStatus, Uid, UidValidity,
    },
};
use std::collections::BTreeMap;

#[cfg(test)]
mod codec_tests;
mod conditional;
mod metadata;
mod selection;

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

pub(in crate::acquisition) struct RuntimeBinding {
    pub account: AccountId,
    pub provider: Provider,
    pub login: String,
    pub source: String,
    pub credential_key_recovery: String,
    pub workspace: String,
    pub topology: String,
}

pub(in crate::acquisition) struct BacklogPermit<'a>(&'a InstalledSource);

impl BacklogPermit<'_> {
    pub(in crate::acquisition) fn source(&self) -> &InstalledSource {
        self.0
    }
}

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

    pub(in crate::acquisition) fn runtime_permit<'a>(
        &'a self,
        binding: &RuntimeBinding,
    ) -> Result<BacklogPermit<'a>, Error> {
        if binding.account != self.account
            || binding.provider != Provider::MailboxOrg
            || binding.login != self.login
            || binding.source != self.identity()
            || binding.credential_key_recovery.is_empty()
            || binding.workspace.is_empty()
            || binding.topology.is_empty()
            || self.authority != "fixture-message-move/v1"
            || self.epoch.is_empty()
            || !self.address.ip().is_loopback()
            || self.login.is_empty()
            || self.root.is_empty()
            || self.root.len() > 8192
            || self.tls_name.is_empty()
        {
            return Err(Error::Unsupported);
        }
        Ok(BacklogPermit(self))
    }
    #[cfg(test)]
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

pub(in crate::acquisition) trait CopyCheckpoints {
    fn validate_intent(
        &mut self,
        evidence: &crate::backlog::imap_copy::CopyEvidence,
    ) -> Result<(), Error>;
    fn save(&mut self, evidence: &crate::backlog::imap_copy::CopyEvidence) -> Result<(), Error>;
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
        row: impl FnMut(&[Token], bool) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.io
            .flight(vec![Step::new(command.to_vec(), row)])?
            .pop()
            .unwrap_or(Ok(()))
    }

    /// SELECT as a pipelined step. Its tagged line fails the step unless the
    /// mailbox is writable under the expected UIDVALIDITY, so the unit's
    /// later steps never see rows from another mailbox.
    fn select<'a>(mailbox: &'a MailboxIdentity, unit: usize) -> Result<Step<'a>, Error> {
        let name = quoted(mailbox.name.wire.as_bytes())?;
        let mut selection = selection::Selection::new(mailbox);
        Ok(Step::new(
            [b"SELECT ".as_slice(), &name].concat(),
            move |row, tagged| selection.observe(row, tagged),
        )
        .unit(unit))
    }

    /// The steps reading `uids` in `mailbox`: SELECT, then UID FETCH in
    /// sets of consecutive ranges. Rows for other UIDs are ignored.
    fn readback_steps<'a>(
        mailbox: &'a MailboxIdentity,
        uids: &'a BTreeSet<Uid>,
        messages: &'a std::cell::RefCell<BTreeMap<Uid, Metadata>>,
        unit: usize,
    ) -> Result<Vec<Step<'a>>, Error> {
        if uids.len() > crate::backlog::BATCH_LIMIT {
            return Err(Error::Limit);
        }
        let mut steps = vec![Self::select(mailbox, unit)?];
        for set in crate::acquisition::imap::uid_sets(uids.iter().map(|u| u.get())) {
            let command = format!(
                "UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"
            );
            steps.push(
                Step::new(command.into_bytes(), move |row, tagged| {
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
                    let Some(uid) = fetch_uid(row)? else {
                        return Ok(());
                    };
                    if !uids.contains(&uid) {
                        return Ok(());
                    }
                    let (uid, metadata) = parse_metadata(row)?;
                    if messages.borrow_mut().insert(uid, metadata).is_some() {
                        return Err(Error::Unstable);
                    }
                    Ok(())
                })
                .unit(unit),
            );
        }
        Ok(steps)
    }

    #[cfg(test)]
    pub(in crate::acquisition) fn readback(
        &mut self,
        mailbox: &MailboxIdentity,
        uids: &BTreeSet<Uid>,
    ) -> Result<MailboxReadback, Error> {
        self.readbacks(&[(mailbox, uids)])?
            .pop()
            .unwrap_or(Err(Error::Protocol))
    }

    /// Several independent readbacks in one flight. Each fails on its own.
    pub(in crate::acquisition) fn readbacks(
        &mut self,
        requests: &[(&MailboxIdentity, &BTreeSet<Uid>)],
    ) -> Result<Vec<Result<MailboxReadback, Error>>, Error> {
        let found: Vec<_> = requests
            .iter()
            .map(|_| std::cell::RefCell::new(BTreeMap::new()))
            .collect();
        let mut steps = vec![];
        for (unit, ((mailbox, uids), messages)) in requests.iter().zip(&found).enumerate() {
            steps.extend(Self::readback_steps(mailbox, uids, messages, unit)?);
        }
        let results = self.io.flight(steps)?;
        Ok(requests
            .iter()
            .zip(found)
            .zip(results)
            .map(|(((mailbox, _), messages), result)| {
                result.map(|()| MailboxReadback {
                    identity: (*mailbox).clone(),
                    messages: messages.into_inner(),
                })
            })
            .collect())
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
        // Every check before the effect travels in one flight: topology, the
        // destination's identity, the sources' exact state, and the source
        // selected again for the MOVE. The MOVE leaves only after all passed.
        let mut observed = InventoryRows::default();
        let before = std::cell::RefCell::new(BTreeMap::new());
        let requested = intent.messages().keys().copied().collect();
        let mut steps = observed.steps();
        steps.push(Self::select(intent.destination(), 0)?);
        steps.extend(Self::readback_steps(
            intent.source(),
            &requested,
            &before,
            0,
        )?);
        steps.push(Self::select(intent.source(), 0)?);
        let result = self.io.flight(steps)?.pop().unwrap_or(Ok(()));
        let inventory = match observed.finish(&self.capabilities) {
            Ok(inventory) => inventory,
            Err(error) => return Err(result.err().unwrap_or(error)),
        };
        if [intent.source(), intent.destination()].iter().any(|m| {
            inventory
                .get(&m.name.decoded)
                .is_none_or(|found| found.name != m.name || !found.selectable())
        }) {
            return Err(Error::Unsupported);
        }
        result?;
        if before.into_inner() != *intent.messages() {
            return Err(Error::Unstable);
        }
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

pub(super) fn fetch_uid(row: &[Token]) -> Result<Option<Uid>, Error> {
    if row.get(3) != Some(&Token::Open) || row.last() != Some(&Token::Close) {
        return Err(Error::Protocol);
    }
    let mut depth = 0usize;
    let mut uid = None;
    for (at, token) in row.iter().enumerate().skip(3) {
        match token {
            Token::Open => depth += 1,
            Token::Close => depth = depth.checked_sub(1).ok_or(Error::Protocol)?,
            Token::Atom(key) if depth == 1 && key == "UID" => {
                let value = row
                    .get(at + 1)
                    .and_then(Token::atom)
                    .ok_or(Error::Protocol)?;
                let value = Uid::new(value.parse().map_err(|_| Error::Protocol)?)
                    .map_err(|_| Error::Protocol)?;
                if uid.replace(value).is_some() {
                    return Err(Error::Protocol);
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(Error::Protocol);
    }
    Ok(uid)
}

fn parse_metadata(row: &[Token]) -> Result<(Uid, Metadata), Error> {
    metadata::parse(row).map(|(uid, metadata, _)| (uid, metadata))
}

#[cfg(test)]
pub(crate) mod tests;
