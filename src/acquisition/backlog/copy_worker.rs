use super::copy_checkpoints::CopySlot;
use super::*;
use crate::{
    acquisition::folder_provisioning::transport::message_move::{BacklogPermit, CopyCheckpoints},
    backlog::imap_copy::CopyEvidence,
    census::folder_provisioning::Failure,
};

pub(in crate::acquisition) trait CopyTransport {
    fn inventory(&mut self) -> Result<Inventory, Failure>;
    fn file(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Failure>;
    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Failure>;
}

impl CopyTransport for Connection {
    fn inventory(&mut self) -> Result<Inventory, Failure> {
        Connection::inventory(self)
    }
    fn file(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Failure> {
        self.file_copy(permit, evidence, cancel, checkpoints)
    }
    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CopyEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CopyCheckpoints,
    ) -> Result<(), Failure> {
        self.reconcile_copy(permit, evidence, cancel, checkpoints)
    }
}

#[derive(Clone, Copy)]
pub(in crate::acquisition) enum CopyWork {
    File,
    Reconcile,
}

pub(in crate::acquisition) struct CopyWorker<'a> {
    repo: &'a Accounts,
    account: &'a Account,
    store: &'a EncryptedStore,
    saved: &'a mut AccountState,
    run: usize,
    work: CopyWork,
    fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
}

impl Accounts {
    pub(in crate::acquisition) fn copy_worker<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        id: &str,
        work: CopyWork,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<CopyWorker<'a>, AcquisitionError> {
        saved.protocol_backlog.validate(&account.id)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        let worker = CopyWorker {
            repo: self,
            account,
            store,
            saved,
            run,
            work,
            fresh,
        };
        worker.admit()?;
        Ok(worker)
    }
}

impl CopyWorker<'_> {
    fn admit(&self) -> Result<(), AcquisitionError> {
        let run = &self.saved.protocol_backlog.runs[self.run];
        if run.plan.filing.semantics != model::COPY_SEMANTICS
            || run.batches.iter().any(|b| !b.is_copy())
            || run.undo_receipt.is_some()
            || run.closed.is_some()
        {
            return Err(invalid());
        }
        match self.work {
            CopyWork::File
                if !matches!(
                    run.status,
                    Status::Prepared | Status::Running | Status::Stopped
                ) || run.batches.iter().any(|b| !b.resumable()) =>
            {
                return Err(invalid());
            }
            CopyWork::Reconcile
                if !matches!(
                    run.status,
                    Status::Uncertain | Status::Stopped | Status::Conflict
                ) =>
            {
                return Err(invalid());
            }
            _ => (),
        }
        self.repo.validate_copy_authority(
            self.account,
            self.store,
            self.saved,
            &run.plan.binding,
            self.fresh,
        )
    }

    pub(super) fn connected(
        self,
        session: Option<Connection>,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        self.admit()?;
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        let repo = self.repo;
        let runtime = repo.backlog_runtime(
            self.account,
            self.store,
            self.saved,
            &self.saved.protocol_backlog.runs[self.run].plan.binding,
        )?;
        let permit = repo
            .backlog_source(self.account)?
            .runtime_permit(&runtime)
            .map_err(failure)?;
        let mut transport = match session {
            Some(connection) => connection,
            None => Connection::connect(&permit, &repo.password(self.account)?, cancel.clone())
                .map_err(failure)?,
        };
        self.drive(&mut transport, &permit, &cancel)
    }

    pub(in crate::acquisition) fn drive(
        self,
        transport: &mut impl CopyTransport,
        permit: &BacklogPermit<'_>,
        cancel: &Cancellation,
    ) -> Result<(), AcquisitionError> {
        self.admit()?;
        if permit.source().identity()
            != self.saved.protocol_backlog.runs[self.run]
                .plan
                .binding
                .source
        {
            return Err(invalid());
        }
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        if matches!(self.work, CopyWork::File) {
            let mut staged = self.saved.clone();
            let run = &mut staged.protocol_backlog.runs[self.run];
            run.status = Status::Running;
            run.stop_requested = false;
            self.store.save_state(&mut staged)?;
            *self.saved = staged;
        }
        let result = (|| {
            let id = self.saved.protocol_backlog.runs[self.run].id.clone();
            for batch in 0..self.saved.protocol_backlog.runs[self.run].batches.len() {
                let original = &self.saved.protocol_backlog.runs[self.run].batches[batch];
                let needed = match self.work {
                    CopyWork::File => !model::verified(original),
                    CopyWork::Reconcile => original
                        .as_copy()
                        .ok_or_else(invalid)?
                        .next_action()
                        .needs_observation(),
                };
                if !needed {
                    continue;
                }
                if cancel.is_cancelled() {
                    return Err(AcquisitionError::Cancelled);
                }
                let topology = transport.inventory().map_err(failure)?;
                if topology != self.saved.protocol_backlog.runs[self.run].plan.topology {
                    return Err(invalid());
                }
                let mut evidence = original.as_copy().ok_or_else(invalid)?.clone();
                let slot = match self.work {
                    CopyWork::File => CopySlot::Filing { run: &id, batch },
                    CopyWork::Reconcile => CopySlot::Reconcile { run: &id, batch },
                };
                let mut checkpoints = self.repo.copy_checkpoints(
                    self.account,
                    self.store,
                    self.saved,
                    slot,
                    &topology,
                    self.fresh,
                )?;
                let result = match self.work {
                    CopyWork::File => {
                        transport.file(permit, &mut evidence, cancel, &mut checkpoints)
                    }
                    CopyWork::Reconcile => {
                        transport.reconcile(permit, &mut evidence, cancel, &mut checkpoints)
                    }
                };
                result.map_err(|e| {
                    if e == Failure::Cancelled {
                        AcquisitionError::Cancelled
                    } else {
                        failure(e)
                    }
                })?;
                if matches!(self.work, CopyWork::File)
                    && !model::verified(&model::Batch::copy(evidence))
                {
                    return Err(invalid());
                }
            }
            Ok(())
        })();
        let mut staged = self.saved.clone();
        let run = &mut staged.protocol_backlog.runs[self.run];
        run.status = model::outcome(run, false);
        run.stop_requested |= cancel.is_cancelled();
        if matches!(self.work, CopyWork::File)
            && result.is_err()
            && run.status == Status::Stopped
            && !cancel.is_cancelled()
        {
            run.status = Status::Conflict;
        }
        run.stamp(self.repo.now());
        self.store.save_state(&mut staged)?;
        *self.saved = staged;
        result
    }
}
