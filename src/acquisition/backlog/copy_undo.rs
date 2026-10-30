use super::compensation_observer::{
    CompensationReadTransport, ObservedCopyCompensation, ObservedDependentCleanup,
};
use super::compensation_review::{CopyCompensationActions, CopyCompensationReview};
use super::compensation_worker::CompensationTransport;
use super::dependent_cleanup::{DependentCleanupReview, DependentCleanupTarget};
use super::*;
use crate::acquisition::{
    CopyCompensationWork, folder_provisioning::transport::message_move::BacklogPermit,
};
use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CopyUndoTarget<'a> {
    Initial {
        run: &'a str,
    },
    AfterClear {
        run: &'a str,
        forward: usize,
        clear: usize,
    },
}

pub(crate) fn copy_undo_candidates(saved: &AccountState, now: u64) -> Vec<CopyUndoTarget<'_>> {
    let mut targets = vec![];
    for run in &saved.protocol_backlog.runs {
        if run.closed.is_some()
            || run.plan.filing.semantics != model::COPY_SEMANTICS
            || run.batches.iter().any(|batch| !batch.is_copy())
            || !crate::backlog::retention::undo_open(run.completed_at, now)
        {
            continue;
        }
        if run.undo_receipt.is_none() {
            if matches!(
                run.status,
                Status::Complete | Status::Stopped | Status::Conflict
            ) && run.batches.iter().any(|batch| batch.attempted())
                && run
                    .batches
                    .iter()
                    .all(|batch| !batch.attempted() || batch.observed())
            {
                targets.push(CopyUndoTarget::Initial { run: &run.id });
            }
        } else if matches!(run.status, Status::Stopped | Status::Conflict)
            && model::outcome(run, true) != Status::Uncertain
        {
            for entry in &run.compensation {
                for clear in 0..entry.evidence.preview().clear().len() {
                    if entry.evidence.clear(clear).is_some_and(|effect| {
                        effect.result().is_some()
                            && effect.readback().is_some()
                            && !effect.verified().is_empty()
                    }) && entry.evidence.dependent_plan(clear).is_none()
                    {
                        targets.push(CopyUndoTarget::AfterClear {
                            run: &run.id,
                            forward: entry.forward,
                            clear,
                        });
                    }
                }
            }
        }
    }
    targets
}

impl<'a> CopyUndoTarget<'a> {
    fn run(self) -> &'a str {
        match self {
            Self::Initial { run } | Self::AfterClear { run, .. } => run,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "review", rename_all = "snake_case")]
pub(crate) enum CopyUndoReview {
    Initial(CopyCompensationReview),
    AfterClear(DependentCleanupReview),
}

impl CopyUndoReview {
    pub(in crate::acquisition) fn receipt(&self) -> &str {
        match self {
            Self::Initial(review) => review.receipt(),
            Self::AfterClear(review) => review.receipt(),
        }
    }

    pub(in crate::acquisition) fn actions(&self) -> CopyCompensationActions {
        match self {
            Self::Initial(review) => review.actions(),
            Self::AfterClear(review) => review.actions(),
        }
    }
}

enum Observed {
    Initial(ObservedCopyCompensation),
    AfterClear(ObservedDependentCleanup),
}

pub(crate) struct CopyUndoWorkflow<'a> {
    repo: &'a Accounts,
    account: &'a Account,
    store: &'a EncryptedStore,
    saved: &'a mut AccountState,
    target: CopyUndoTarget<'a>,
    fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
}

impl Accounts {
    pub(crate) fn copy_undo_workflow<'a>(
        &'a self,
        account: &'a Account,
        store: &'a EncryptedStore,
        saved: &'a mut AccountState,
        target: CopyUndoTarget<'a>,
        fresh: &'a dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<CopyUndoWorkflow<'a>, AcquisitionError> {
        let workflow = CopyUndoWorkflow {
            repo: self,
            account,
            store,
            saved,
            target,
            fresh,
        };
        workflow.admit()?;
        Ok(workflow)
    }
}

impl<'a> CopyUndoWorkflow<'a> {
    fn admit(&self) -> Result<&Run, AcquisitionError> {
        match self.target {
            CopyUndoTarget::Initial { run } => self.repo.copy_compensation_admit(
                self.account,
                self.store,
                self.saved,
                run,
                self.fresh,
            ),
            CopyUndoTarget::AfterClear {
                run,
                forward,
                clear,
            } => self
                .repo
                .dependent_cleanup_admit(
                    self.account,
                    self.store,
                    self.saved,
                    &DependentCleanupTarget {
                        run,
                        forward,
                        clear,
                    },
                    self.fresh,
                )
                .map(|(run, _)| run),
        }
    }

    fn observe(
        &self,
        transport: &mut impl CompensationReadTransport,
        permit: &BacklogPermit<'_>,
        cancel: &Cancellation,
    ) -> Result<Observed, AcquisitionError> {
        self.admit()?;
        match self.target {
            CopyUndoTarget::Initial { run } => self
                .repo
                .observe_copy_compensation(
                    self.account,
                    self.store,
                    self.saved,
                    run,
                    transport,
                    permit,
                    cancel,
                    self.fresh,
                )
                .map(Observed::Initial),
            CopyUndoTarget::AfterClear {
                run,
                forward,
                clear,
            } => self
                .repo
                .observe_dependent_cleanup(
                    self.account,
                    self.store,
                    self.saved,
                    DependentCleanupTarget {
                        run,
                        forward,
                        clear,
                    },
                    transport,
                    permit,
                    cancel,
                    self.fresh,
                )
                .map(Observed::AfterClear),
        }
    }

    pub(in crate::acquisition) fn preview_with(
        self,
        transport: &mut impl CompensationReadTransport,
        permit: &BacklogPermit<'_>,
        cancel: &Cancellation,
    ) -> Result<CopyUndoReview, AcquisitionError> {
        Ok(match self.observe(transport, permit, cancel)? {
            Observed::Initial(observed) => CopyUndoReview::Initial(observed.into_review()),
            Observed::AfterClear(observed) => CopyUndoReview::AfterClear(observed.into_review()),
        })
    }

    pub(in crate::acquisition) fn confirm_with(
        self,
        transport: &mut (impl CompensationReadTransport + CompensationTransport),
        permit: &BacklogPermit<'_>,
        receipt: &str,
        cancel: &Cancellation,
    ) -> Result<(), AcquisitionError> {
        let observed = self.observe(transport, permit, cancel)?;
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        match observed {
            Observed::Initial(observed) => observed.confirm(
                self.repo,
                self.account,
                self.store,
                self.saved,
                receipt,
                self.fresh,
            )?,
            Observed::AfterClear(observed) => observed.confirm(
                self.repo,
                self.account,
                self.store,
                self.saved,
                receipt,
                self.fresh,
            )?,
        }
        self.repo
            .compensation_worker(
                self.account,
                self.store,
                self.saved,
                self.target.run(),
                CopyCompensationWork::Execute,
                self.fresh,
            )?
            .drive(transport, permit, cancel)
    }

    fn connect(
        &self,
        cancel: Cancellation,
    ) -> Result<(Connection, BacklogPermit<'a>), AcquisitionError> {
        let run = self.admit()?;
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        let repo: &'a Accounts = self.repo;
        let runtime =
            repo.backlog_runtime(self.account, self.store, self.saved, &run.plan.binding)?;
        let permit = repo
            .backlog_source(self.account)?
            .runtime_permit(&runtime)
            .map_err(failure)?;
        let connection =
            Connection::connect(&permit, &repo.password(self.account)?, cancel).map_err(failure)?;
        Ok((connection, permit))
    }

    pub(crate) fn connected_preview(
        self,
        cancel: Cancellation,
    ) -> Result<CopyUndoReview, AcquisitionError> {
        let (mut connection, permit) = self.connect(cancel.clone())?;
        self.preview_with(&mut connection, &permit, &cancel)
    }

    pub(crate) fn connected_confirm(
        self,
        receipt: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let (mut connection, permit) = self.connect(cancel.clone())?;
        self.confirm_with(&mut connection, &permit, receipt, &cancel)
    }
}
