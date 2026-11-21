use super::compensation_checkpoints::CopyCompensationWork;
use super::*;
use crate::{
    acquisition::{
        CopyCompensationSlot,
        folder_provisioning::transport::message_move::{BacklogPermit, CompensationCheckpoints},
    },
    backlog::imap_copy::CompensationEvidence,
    census::folder_provisioning::Failure,
};

pub(in crate::acquisition) trait CompensationTransport {
    fn inventory(&mut self) -> Result<Inventory, Failure>;
    fn execute(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Failure>;
    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Failure>;
}

impl CompensationTransport for Connection {
    fn inventory(&mut self) -> Result<Inventory, Failure> {
        Connection::inventory(self)
    }
    fn execute(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Failure> {
        self.execute_compensation(permit, evidence, cancel, checkpoints)
    }
    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Failure> {
        self.reconcile_compensation(permit, evidence, cancel, checkpoints)
    }
}

pub(in crate::acquisition) struct CompensationWorker<'a> {
    repo: &'a Accounts,
    account: &'a Account,
    store: &'a EncryptedStore,
    saved: &'a mut AccountState,
    run: usize,
    work: CopyCompensationWork,
    fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
}

impl Accounts {
    pub(in crate::acquisition) fn compensation_worker<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        id: &str,
        work: CopyCompensationWork,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<CompensationWorker<'a>, AcquisitionError> {
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        let worker = CompensationWorker {
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

impl CompensationWorker<'_> {
    fn admit(&self) -> Result<(), AcquisitionError> {
        self.saved.protocol_backlog.validate(&self.account.id)?;
        let run = &self.saved.protocol_backlog.runs[self.run];
        if run.plan.filing.semantics != model::COPY_SEMANTICS
            || run.undo_receipt.is_none()
            || run.closed.is_some()
            || run.compensation.is_empty()
        {
            return Err(invalid());
        }
        let view = match self.work {
            CopyCompensationWork::Execute => {
                if !matches!(run.status, Status::Stopped | Status::Conflict)
                    || model::outcome(run, true) == Status::Uncertain
                    || !crate::backlog::retention::undo_open(run.completed_at, self.repo.now())
                {
                    return Err(invalid());
                }
                crate::census::ProtocolStateView::Persisted
            }
            CopyCompensationWork::Reconcile => {
                if !matches!(
                    run.status,
                    Status::Uncertain | Status::Stopped | Status::Conflict
                ) {
                    return Err(invalid());
                }
                crate::census::ProtocolStateView::Interrupted
            }
        };
        if !self.store.checkpoint_matches(self.saved, view)? {
            return Err(invalid());
        }
        super::dependent_cleanup::approved_dependencies(self.store, run)?;
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
            .map(|p| p.into_owned())
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
        transport: &mut impl CompensationTransport,
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
        if self.work == CopyCompensationWork::Execute {
            let mut staged = self.saved.clone();
            staged.protocol_backlog.runs[self.run].status = Status::UndoRunning;
            staged.protocol_backlog.runs[self.run].stop_requested = false;
            self.store.save_state(&mut staged)?;
            *self.saved = staged;
        }
        let result = (|| {
            let id = self.saved.protocol_backlog.runs[self.run].id.clone();
            for entry in 0..self.saved.protocol_backlog.runs[self.run]
                .compensation
                .len()
            {
                if cancel.is_cancelled() {
                    return Err(AcquisitionError::Cancelled);
                }
                let topology = transport.inventory().map_err(failure)?;
                if topology != self.saved.protocol_backlog.runs[self.run].plan.topology {
                    return Err(invalid());
                }
                let current = &self.saved.protocol_backlog.runs[self.run].compensation[entry];
                let mut evidence = current.evidence.clone();
                let forward = current.forward;
                let mut checkpoints = self.repo.copy_compensation_checkpoints(
                    self.account,
                    self.store,
                    self.saved,
                    CopyCompensationSlot {
                        run: &id,
                        forward,
                        work: self.work,
                    },
                    &topology,
                    self.fresh,
                )?;
                let result = match self.work {
                    CopyCompensationWork::Execute => {
                        transport.execute(permit, &mut evidence, cancel, &mut checkpoints)
                    }
                    CopyCompensationWork::Reconcile => {
                        transport.reconcile(permit, &mut evidence, cancel, &mut checkpoints)
                    }
                };
                result.map_err(|error| {
                    if error == Failure::Cancelled {
                        AcquisitionError::Cancelled
                    } else {
                        failure(error)
                    }
                })?;
            }
            Ok(())
        })();
        let mut staged = self.saved.clone();
        let run = &mut staged.protocol_backlog.runs[self.run];
        run.status = model::outcome(run, true);
        run.stop_requested = true;
        run.stamp(self.repo.now());
        self.store.save_state(&mut staged)?;
        *self.saved = staged;
        result
    }
}
