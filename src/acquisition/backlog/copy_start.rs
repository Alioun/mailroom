use super::*;

struct ObservedCopyStart {
    operation: Run,
}

impl ObservedCopyStart {
    fn confirm(
        self,
        repo: &Accounts,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        receipt: &str,
        cancel: &Cancellation,
        fresh: &dyn Fn(&AccountState) -> Result<(), String>,
    ) -> Result<String, AcquisitionError> {
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        let operation = self.operation;
        if operation.status != Status::Prepared
            || operation.stop_requested
            || operation.closed.is_some()
            || operation.undo_receipt.is_some()
            || operation.undo_conflicts != 0
            || operation.completed_at.is_some()
            || !operation.reverse.is_empty()
            || !operation.compensation.is_empty()
            || operation.plan.filing.semantics != model::COPY_SEMANTICS
            || operation.batches.is_empty()
            || operation
                .batches
                .iter()
                .any(|b| !b.is_copy() || b.attempted())
            || saved.protocol_backlog.runs.len() >= model::HISTORY
            || saved.protocol_backlog.runs.iter().any(|r| !r.settled())
            || saved
                .protocol_backlog
                .runs
                .iter()
                .any(|r| r.id == operation.id)
            || saved
                .protocol_backlog
                .summaries
                .iter()
                .any(|r| r.id == operation.id)
        {
            return Err(invalid());
        }
        repo.validate_copy_authority(account, store, saved, &operation.plan.binding, fresh)?;
        if !store.checkpoint_matches(saved, crate::census::ProtocolStateView::Persisted)? {
            return Err(invalid());
        }
        let expected = start_receipt(store, saved, &operation.plan, &operation.batches)?;
        if !bool::from(expected.as_bytes().ct_eq(receipt.as_bytes()))
            || operation.receipt != expected
            || operation.id
                != store
                    .keys()
                    .binding_for(b"mailroom/imap-backlog-id/v1", expected.as_bytes())
        {
            return Err(invalid());
        }
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        let id = operation.id.clone();
        let mut staged = saved.clone();
        staged.protocol_backlog.runs.push(operation);
        save(store, &mut staged)?;
        *saved = staged;
        Ok(id)
    }
}

impl Accounts {
    pub(crate) fn backlog_confirm_copy(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        options: Options,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        if self.backlog_source(account)?.profile().map_err(failure)? != Profile::Copy {
            return Err(invalid());
        }
        if cancel.is_cancelled() {
            return Err(AcquisitionError::Cancelled);
        }
        let (preview, connection) =
            self.backlog_prepare(account, store, saved, options, cancel.clone())?;
        let id = ObservedCopyStart {
            operation: preview.operation,
        }
        .confirm(self, account, store, saved, receipt, &cancel, &fresh)?;
        boundary("copy_consumed");
        self.copy_worker(
            account,
            store,
            saved,
            &id,
            copy_worker::CopyWork::File,
            &fresh,
        )?
        .connected(Some(connection), cancel)
    }
}

#[cfg(test)]
pub(in crate::acquisition) mod tests;
