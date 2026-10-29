use super::*;
use crate::backlog::imap_copy::{
    CopyEvidence, FlagIntent, ModSeq, PreparedDelete, Readback, StoreResult, VersionedMetadata,
};
use std::cell::RefCell;

mod effects;
mod filing;

trait Commands {
    fn source(&self) -> &str;
    fn flight(&mut self, steps: Vec<Step<'_>>) -> Result<Vec<Result<(), Error>>, Error>;
}
impl Commands for Session {
    fn source(&self) -> &str {
        &self.source
    }
    fn flight(&mut self, steps: Vec<Step<'_>>) -> Result<Vec<Result<(), Error>>, Error> {
        Session::flight(self, steps)
    }
}
struct Conditional<'a, I> {
    io: &'a mut I,
    capabilities: &'a BTreeSet<String>,
}

pub(super) fn select<'a>(
    mailbox: &'a MailboxIdentity,
    highest: &'a RefCell<Option<ModSeq>>,
    unit: usize,
) -> Result<Step<'a>, Error> {
    let name = quoted(mailbox.name.wire.as_bytes())?;
    let mut selected = selection::Selection::new(mailbox);
    Ok(Step::new(
        [b"SELECT ".as_slice(), &name, b" (CONDSTORE)"].concat(),
        move |row, tagged| {
            selected.observe(row, tagged)?;
            if tagged {
                *highest.borrow_mut() = Some(selected.persistent()?);
            }
            Ok(())
        },
    )
    .unit(unit))
}

pub(super) fn steps<'a>(
    mailbox: &'a MailboxIdentity,
    uids: &'a BTreeSet<Uid>,
    highest: &'a RefCell<Option<ModSeq>>,
    messages: &'a RefCell<BTreeMap<Uid, VersionedMetadata>>,
    unit: usize,
) -> Result<Vec<Step<'a>>, Error> {
    if uids.len() > crate::backlog::BATCH_LIMIT {
        return Err(Error::Limit);
    }
    let mut steps = vec![select(mailbox, highest, unit)?];
    for set in crate::acquisition::imap::uid_sets(uids.iter().map(|u| u.get())) {
        steps.push(Step::new(format!("UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)").into_bytes(), move |row, tagged| {
            if tagged { return Session::status(row, 1); }
            if row.get(2).and_then(Token::atom) != Some("FETCH") { return Ok(()); }
            let Some(uid) = fetch_uid(row)? else { return Ok(()); };
            if !uids.contains(&uid) { return Ok(()); }
            let (uid, metadata, version) = metadata::parse(row)?;
            let modseq = version.ok_or(Error::Unsupported)?;
            let highest = highest.borrow().ok_or(Error::Protocol)?;
            if modseq > highest { return Err(Error::Unstable); }
            if messages.borrow_mut().insert(uid, VersionedMetadata { metadata, modseq }).is_some() { return Err(Error::Unstable); }
            Ok(())
        }).unit(unit));
    }
    Ok(steps)
}

pub(super) struct StoreReply<'a> {
    intent: &'a FlagIntent,
    updated: BTreeMap<Uid, ModSeq>,
    completed: bool,
}
impl<'a> StoreReply<'a> {
    pub(super) fn new(intent: &'a FlagIntent) -> Self {
        Self {
            intent,
            updated: BTreeMap::new(),
            completed: false,
        }
    }
    pub(super) fn observe(
        &mut self,
        row: &[Token],
        tagged: bool,
    ) -> Result<Option<StoreResult>, Error> {
        if self.completed || row.len() < 2 {
            return Err(Error::Protocol);
        }
        if tagged {
            let status = match row.get(1).and_then(Token::atom) {
                Some("OK") => TaggedStatus::Ok,
                Some("NO") => TaggedStatus::No,
                Some("BAD") => TaggedStatus::Bad,
                _ => return Err(Error::Protocol),
            };
            let code = row.get(2).and_then(Token::atom);
            if code.is_some_and(|c| !c.starts_with("[MODIFIED ")) {
                return Err(Error::UnknownCode);
            }
            let result = StoreResult::tagged(status, code, self.updated.clone(), self.intent)
                .map_err(|_| Error::Protocol)?;
            self.completed = true;
            return Ok(Some(result));
        }
        if row.first().and_then(Token::atom) != Some("*") {
            return Err(Error::Protocol);
        }
        if matches!(
            row.get(1).and_then(Token::atom),
            Some("OK" | "NO" | "BAD" | "BYE")
        ) {
            return Session::status(row, 1).map(|()| None);
        }
        if row.get(2).and_then(Token::atom) != Some("FETCH") {
            return Ok(None);
        }
        if row
            .get(1)
            .and_then(Token::atom)
            .and_then(|value| value.parse::<u32>().ok())
            .is_none_or(|value| value == 0)
        {
            return Err(Error::Protocol);
        }
        let Some(uid) = fetch_uid(row)? else {
            return Ok(None);
        };
        if !self.intent.before().messages().contains_key(&uid) {
            return Ok(None);
        }
        let mut at = 4;
        let mut got_uid = false;
        let mut got_flags = false;
        let mut version = None;
        while at + 1 < row.len() {
            let key = row[at].atom().ok_or(Error::Protocol)?;
            at += 1;
            match key {
                "UID" => {
                    if got_uid
                        || row
                            .get(at)
                            .and_then(Token::atom)
                            .and_then(|s| s.parse::<u32>().ok())
                            != Some(uid.get())
                    {
                        return Err(Error::Protocol);
                    }
                    got_uid = true;
                    at += 1;
                }
                "MODSEQ" => {
                    if version.is_some()
                        || row.get(at) != Some(&Token::Open)
                        || row.get(at + 2) != Some(&Token::Close)
                    {
                        return Err(Error::Protocol);
                    }
                    version = Some(metadata::sequence(
                        row.get(at + 1)
                            .and_then(Token::atom)
                            .ok_or(Error::Protocol)?,
                    )?);
                    at += 3;
                }
                "FLAGS" => {
                    if got_flags || row.get(at) != Some(&Token::Open) {
                        return Err(Error::Protocol);
                    }
                    got_flags = true;
                    at += 1;
                    let mut flags = BTreeSet::new();
                    while row.get(at) != Some(&Token::Close) {
                        let flag = row.get(at).and_then(Token::atom).ok_or(Error::Protocol)?;
                        if flags.len() == 64 || !flags.insert(flag) {
                            return Err(Error::Limit);
                        }
                        at += 1;
                    }
                    at += 1;
                }
                _ => return Err(Error::Unsupported),
            }
        }
        if at != row.len() - 1 || !got_uid {
            return Err(Error::Protocol);
        }
        let version = version.ok_or(Error::Unsupported)?;
        if self.updated.insert(uid, version).is_some() {
            return Err(Error::Unstable);
        }
        Ok(None)
    }
}

impl<I: Commands> Conditional<'_, I> {
    fn authorize(&self, permit: &BacklogPermit<'_>) -> Result<(), Error> {
        if self.io.source() != permit.0.identity()
            || permit.0.authority != "fixture-message-copy/v1"
            || self.capabilities.contains("MOVE")
            || !self.capabilities.contains("UIDPLUS")
            || !self.capabilities.contains("CONDSTORE")
        {
            return Err(Error::Unsupported);
        }
        Ok(())
    }

    fn select_source(&mut self, before: &Readback) -> Result<(), Error> {
        let highest = RefCell::new(None);
        self.io
            .flight(vec![select(before.identity(), &highest, 0)?])?
            .pop()
            .ok_or(Error::Protocol)??;
        if highest
            .into_inner()
            .is_none_or(|value| value < before.highest_modseq())
        {
            return Err(Error::Unstable);
        }
        Ok(())
    }

    fn exchange(
        &mut self,
        command: Vec<u8>,
        handler: impl FnMut(&[Token], bool) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.io
            .flight(vec![Step::new(command, handler)])?
            .pop()
            .ok_or(Error::Protocol)?
    }

    fn readbacks(
        &mut self,
        requests: &[(&MailboxIdentity, &BTreeSet<Uid>)],
    ) -> Result<Vec<Result<Readback, Error>>, Error> {
        if !self.capabilities.contains("CONDSTORE") || !self.capabilities.contains("UIDPLUS") {
            return Err(Error::Unsupported);
        }
        let highest: Vec<_> = requests.iter().map(|_| RefCell::new(None)).collect();
        let found: Vec<_> = requests
            .iter()
            .map(|_| RefCell::new(BTreeMap::new()))
            .collect();
        let mut commands = vec![];
        for (unit, ((mailbox, uids), (highest, messages))) in
            requests.iter().zip(highest.iter().zip(&found)).enumerate()
        {
            commands.extend(steps(mailbox, uids, highest, messages, unit)?);
        }
        let results = self.io.flight(commands)?;
        Ok(requests
            .iter()
            .zip(highest)
            .zip(found)
            .zip(results)
            .map(|((((mailbox, _), highest), messages), result)| {
                result.and_then(|()| {
                    Readback::new(
                        (*mailbox).clone(),
                        highest.into_inner().ok_or(Error::Unsupported)?,
                        messages.into_inner(),
                    )
                    .map_err(|_| Error::Unstable)
                })
            })
            .collect())
    }

    fn store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        prepared: PreparedDelete,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        self.authorize(permit)?;
        let intent = prepared.intent().clone();
        let source: BTreeSet<_> = intent.before().messages().keys().copied().collect();
        let destination: BTreeSet<_> = prepared.destination().messages().keys().copied().collect();
        let mut observed = self
            .readbacks(&[
                (intent.before().identity(), &source),
                (prepared.destination().identity(), &destination),
            ])?
            .into_iter();
        if observed.next().ok_or(Error::Protocol)?? != *intent.before()
            || observed.next().ok_or(Error::Protocol)?? != *prepared.destination()
        {
            return Err(Error::Unstable);
        }
        self.select_source(intent.before())?;
        let index = evidence
            .begin_delete(prepared)
            .map_err(|_| Error::Unsupported)?;
        persist(evidence)?;
        let mut reply = StoreReply::new(&intent);
        let result = self.exchange(intent.command().into_bytes(), |row, tagged| {
            if let Some(result) = reply.observe(row, tagged)? {
                evidence
                    .record_store_result(index, result)
                    .map_err(|_| Error::Protocol)?;
                persist(evidence)?;
            }
            Ok(())
        });
        if evidence
            .deletions()
            .nth(index)
            .ok_or(Error::Protocol)?
            .flag()
            .result()
            .is_none()
        {
            let result = match &result {
                Err(
                    Error::Protocol
                    | Error::UnknownCode
                    | Error::Unsupported
                    | Error::Unstable
                    | Error::Limit,
                ) => StoreResult::Rejected,
                _ => StoreResult::Lost,
            };
            evidence
                .record_store_result(index, result)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        result.map(|()| index)
    }
}

impl Connection {
    pub(in crate::acquisition) fn conditional_readbacks(
        &mut self,
        requests: &[(&MailboxIdentity, &BTreeSet<Uid>)],
    ) -> Result<Vec<Result<Readback, Error>>, Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .readbacks(requests)
    }

    pub(in crate::acquisition) fn copy_store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        prepared: PreparedDelete,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .store(permit, evidence, prepared, persist)
    }
}

#[cfg(test)]
mod tests;
