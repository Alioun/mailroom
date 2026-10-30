use super::compensation_worker::CompensationWorker;
use super::copy_worker::{CopyWork, CopyWorker};
use super::*;
use crate::acquisition::CopyCompensationWork;
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CopyContinuationWork {
    Resume,
    ResumeUndo,
    Reconcile,
}

pub(in crate::acquisition) enum Worker<'a> {
    Forward(CopyWorker<'a>),
    Compensation(CompensationWorker<'a>),
}

pub(crate) struct CopyContinuation<'a> {
    pub(in crate::acquisition) worker: Worker<'a>,
}

impl Accounts {
    pub(crate) fn copy_continuation<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        id: &str,
        work: CopyContinuationWork,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<CopyContinuation<'a>, AcquisitionError> {
        let undo = saved
            .protocol_backlog
            .runs
            .iter()
            .find(|run| run.id == id)
            .ok_or_else(invalid)?
            .undo_receipt
            .is_some();
        let worker = match work {
            CopyContinuationWork::Resume => Worker::Forward(self.copy_worker(
                account,
                store,
                saved,
                id,
                CopyWork::File,
                fresh,
            )?),
            CopyContinuationWork::ResumeUndo => Worker::Compensation(self.compensation_worker(
                account,
                store,
                saved,
                id,
                CopyCompensationWork::Execute,
                fresh,
            )?),
            CopyContinuationWork::Reconcile if undo => {
                Worker::Compensation(self.compensation_worker(
                    account,
                    store,
                    saved,
                    id,
                    CopyCompensationWork::Reconcile,
                    fresh,
                )?)
            }
            CopyContinuationWork::Reconcile => Worker::Forward(self.copy_worker(
                account,
                store,
                saved,
                id,
                CopyWork::Reconcile,
                fresh,
            )?),
        };
        Ok(CopyContinuation { worker })
    }
}

impl CopyContinuation<'_> {
    pub(crate) fn connected(self, cancel: Cancellation) -> Result<(), AcquisitionError> {
        match self.worker {
            Worker::Forward(worker) => worker.connected(None, cancel),
            Worker::Compensation(worker) => worker.connected(None, cancel),
        }
    }
}
