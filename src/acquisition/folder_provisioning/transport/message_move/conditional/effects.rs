use super::*;

impl<I: Commands> Conditional<'_, I> {
    fn pair(
        &mut self,
        source: &MailboxIdentity,
        source_uids: &BTreeSet<Uid>,
        destination: &MailboxIdentity,
        destination_uids: &BTreeSet<Uid>,
    ) -> Result<(Readback, Readback), Error> {
        let mut found = self
            .readbacks(&[(source, source_uids), (destination, destination_uids)])?
            .into_iter();
        Ok((
            found.next().ok_or(Error::Protocol)??,
            found.next().ok_or(Error::Protocol)??,
        ))
    }

    pub(super) fn copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        if evidence.copy().attempted() {
            return Err(Error::Unsupported);
        }
        let intent = evidence.intent().clone();
        let requested = intent.messages().keys().copied().collect();
        let empty = BTreeSet::new();
        let mut found = self
            .readbacks(&[
                (intent.destination(), &empty),
                (intent.source(), &requested),
            ])?
            .into_iter();
        found.next().ok_or(Error::Protocol)??;
        let fresh = found.next().ok_or(Error::Protocol)??;
        self.select_source(&fresh)?;
        let command = evidence.begin_copy(&fresh).map_err(|_| Error::Unstable)?;
        persist(evidence)?;
        let result = self.exchange(command.into_bytes(), |row, tagged| {
            let status = row.get(1).and_then(Token::atom);
            if !matches!(status, Some("OK" | "NO" | "BAD")) {
                return if tagged { Err(Error::Protocol) } else { Ok(()) };
            }
            let code = row.get(2).and_then(Token::atom);
            if let Some(code) = code {
                if code.starts_with("[COPYUID ") {
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
                } else if code == "[UIDNOTSTICKY]" {
                    evidence.reject_mapping();
                    persist(evidence)?;
                    return Err(Error::Unsupported);
                } else {
                    return Err(Error::UnknownCode);
                }
            }
            if tagged {
                let status = match status {
                    Some("OK") => TaggedStatus::Ok,
                    Some("NO") => TaggedStatus::No,
                    Some("BAD") => TaggedStatus::Bad,
                    _ => return Err(Error::Protocol),
                };
                evidence
                    .record_copy_completion(Completion::Tagged {
                        status,
                        code: code.map(str::to_owned),
                    })
                    .map_err(|_| Error::Protocol)?;
                persist(evidence)?;
            }
            Ok(())
        });
        if evidence.copy().completion().is_none() {
            evidence
                .record_copy_completion(Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        result
    }

    pub(super) fn observe_copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        if !evidence.copy().attempted() {
            return Err(Error::Unsupported);
        }
        if evidence.copy().completion().is_none() {
            evidence
                .record_copy_completion(Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        let intent = evidence.intent().clone();
        let mapping = evidence.copy().mapping().ok_or(Error::Unsupported)?;
        let source = intent.messages().keys().copied().collect();
        let destination = mapping.pairs().values().copied().collect();
        let (source, destination) =
            self.pair(intent.source(), &source, intent.destination(), &destination)?;
        evidence
            .observe_copy(source, destination)
            .map_err(|_| Error::Unstable)?;
        persist(evidence)
    }

    pub(super) fn observe_store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.deletions().nth(index).ok_or(Error::Unsupported)?;
        if deletion.expunge_before().is_some() {
            return Err(Error::Unsupported);
        }
        let before = deletion.intent().before().clone();
        if deletion.flag().result().is_none() {
            evidence
                .record_store_result(index, StoreResult::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        let uids = before.messages().keys().copied().collect();
        let found = self
            .readbacks(&[(before.identity(), &uids)])?
            .pop()
            .ok_or(Error::Protocol)??;
        evidence
            .observe_store(index, found)
            .map_err(|_| Error::Unstable)?;
        persist(evidence)
    }

    pub(super) fn expunge(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.deletions().nth(index).ok_or(Error::Unsupported)?;
        let owned = deletion.flag().verified();
        if deletion.expunge_before().is_some() || owned.is_empty() {
            return Err(Error::Unsupported);
        }
        let source = owned.keys().copied().collect();
        let mapping = evidence.copy().mapping().ok_or(Error::Unsupported)?;
        let destination = deletion
            .intent()
            .before()
            .messages()
            .keys()
            .map(|uid| mapping.pairs()[uid])
            .collect();
        let intent = evidence.intent().clone();
        let (source, destination) =
            self.pair(intent.source(), &source, intent.destination(), &destination)?;
        self.select_source(&source)?;
        let command = evidence
            .begin_expunge(index, source, destination)
            .map_err(|_| Error::Unstable)?;
        persist(evidence)?;
        let result = self.exchange(command.into_bytes(), |row, tagged| {
            if !tagged {
                return if matches!(
                    row.get(1).and_then(Token::atom),
                    Some("OK" | "NO" | "BAD" | "BYE")
                ) {
                    Session::status(row, 1)
                } else {
                    Ok(())
                };
            }
            let status = match row.get(1).and_then(Token::atom) {
                Some("OK") => TaggedStatus::Ok,
                Some("NO") => TaggedStatus::No,
                Some("BAD") => TaggedStatus::Bad,
                _ => return Err(Error::Protocol),
            };
            if row.get(2).is_some() {
                return Err(Error::UnknownCode);
            }
            evidence
                .record_expunge_completion(index, Completion::Tagged { status, code: None })
                .map_err(|_| Error::Protocol)?;
            persist(evidence)
        });
        if evidence
            .deletions()
            .nth(index)
            .ok_or(Error::Protocol)?
            .expunge_completion()
            .is_none()
        {
            evidence
                .record_expunge_completion(index, Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        result
    }

    pub(super) fn observe_expunge(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        mut persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.authorize(permit)?;
        let deletion = evidence.deletions().nth(index).ok_or(Error::Unsupported)?;
        let before = deletion.expunge_before().ok_or(Error::Unsupported)?;
        let source = before.messages().keys().copied().collect();
        let mapping = evidence.copy().mapping().ok_or(Error::Unsupported)?;
        let destination = deletion
            .intent()
            .before()
            .messages()
            .keys()
            .map(|uid| mapping.pairs()[uid])
            .collect();
        if deletion.expunge_completion().is_none() {
            evidence
                .record_expunge_completion(index, Completion::Lost)
                .map_err(|_| Error::Protocol)?;
            persist(evidence)?;
        }
        let intent = evidence.intent().clone();
        let (source, destination) =
            self.pair(intent.source(), &source, intent.destination(), &destination)?;
        evidence
            .observe_final(index, source, destination)
            .map_err(|_| Error::Unstable)?;
        persist(evidence)
    }
}

impl Connection {
    pub(in crate::acquisition) fn copy_messages(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .copy(permit, evidence, persist)
    }
    pub(in crate::acquisition) fn observe_copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .observe_copy(permit, evidence, persist)
    }
    pub(in crate::acquisition) fn observe_copy_store(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .observe_store(permit, evidence, index, persist)
    }
    pub(in crate::acquisition) fn expunge_copy(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .expunge(permit, evidence, index, persist)
    }
    pub(in crate::acquisition) fn observe_copy_expunge(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        index: usize,
        persist: impl FnMut(&CopyEvidence) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Conditional {
            io: &mut self.io,
            capabilities: &self.capabilities,
        }
        .observe_expunge(permit, evidence, index, persist)
    }
}
