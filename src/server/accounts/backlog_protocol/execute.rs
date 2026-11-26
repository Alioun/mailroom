use super::*;

pub(super) fn execute(
    state: &Arc<AppState>,
    request: &Request,
    cancel: Cancellation,
    kind: Kind,
) -> Result<serde_json::Value, AcquisitionError> {
    let repo = state.accounts.repository()?;
    let account = repo.load(&request.account)?;
    let store = repo.store(&account)?;
    let mut saved = store.load_state()?;
    let journal = |saved: &crate::census::AccountState| match kind {
        Kind::Move => saved.protocol_backlog.history_view(),
        Kind::Gmail => json!(saved.gmail_backlog),
    };
    let deadlines = |saved: &crate::census::AccountState| match kind {
        Kind::Move => saved.protocol_backlog.deadlines(repo.now()),
        Kind::Gmail => saved.gmail_backlog.deadlines(repo.now()),
    };
    let report = |saved: &crate::census::AccountState, mut value: serde_json::Value| {
        if kind == Kind::Gmail {
            value["outcomes"] = gmail_journal::outcomes(&saved.gmail_backlog);
        }
        value
    };
    let report = |saved: &crate::census::AccountState, mut value: serde_json::Value| {
        if kind == Kind::Move {
            value["copy_workflow"] = json!({
                "installed":false,
                "revision":saved.revision,
                "targets":acquisition::copy_undo_candidates(saved, repo.now()),
            });
        }
        value["closable"] = json!(match kind {
            Kind::Move => saved
                .protocol_backlog
                .runs
                .iter()
                .filter(|r| r.closable())
                .map(|r| &r.id)
                .collect::<Vec<_>>(),
            Kind::Gmail => saved
                .gmail_backlog
                .runs
                .iter()
                .filter(|r| r.closable())
                .map(|r| &r.id)
                .collect(),
        });
        if kind == Kind::Gmail {
            value["labels"] = json!(saved.label_provisioning);
            value["label_limitations"] =
                json!(crate::acquisition::folder_provisioning::LABEL_LIMITATIONS);
        }
        report(saved, value)
    };
    if matches!(request.command, Action::Status {}) {
        return Ok(report(
            &saved,
            json!({"installed":kind.installed(&repo, Some(&account.id)),"recovery_hold":store.recovery_pending(),"journal":journal(&saved),"undo_deadlines":deadlines(&saved),"undo_window_days":crate::backlog::retention::WINDOW_SECS / 86_400,"limitations":match kind { Kind::Move => protocol::LIMITATIONS, Kind::Gmail => gmail_journal::LIMITATIONS }}),
        ));
    }
    if store.recovery_pending() {
        return Err(AcquisitionError::LifecyclePending);
    }
    repo.retain_backlog(&store, &mut saved)?;
    if matches!(request.command, Action::Cancel {}) {
        let runs: Vec<(&mut Status, &mut bool)> = match kind {
            Kind::Move => saved
                .protocol_backlog
                .runs
                .iter_mut()
                .map(|r| (&mut r.status, &mut r.stop_requested))
                .collect(),
            Kind::Gmail => saved
                .gmail_backlog
                .runs
                .iter_mut()
                .map(|r| (&mut r.status, &mut r.stop_requested))
                .collect(),
        };
        for (status, stop_requested) in runs {
            if !matches!(status, Status::Complete | Status::Undone) {
                *stop_requested = true;
                if *status == Status::Prepared {
                    *status = Status::Stopped;
                }
            }
        }
        store.save_state(&mut saved)?;
        return Ok(report(
            &saved,
            json!({"cancel_requested":true,"journal":journal(&saved)}),
        ));
    }
    let guard = state
        .accounts
        .ledger()?
        .map(|l| l.lock())
        .transpose()
        .map_err(|_| AcquisitionError::Storage)?;
    sync_ledger(
        &mut saved,
        &store,
        &repo.workspace_context(&account, &store)?,
        guard.as_ref(),
    )
    .map_err(|_| AcquisitionError::InvalidInput)?;
    ledger_fresh(&saved, guard.as_ref()).map_err(|_| AcquisitionError::InvalidInput)?;
    let fresh = |s: &crate::census::AccountState| {
        ledger_fresh(s, guard.as_ref())?;
        state.accounts.update_job_if(
            |j| j.backlog_account.as_ref() == Some(&request.account),
            |job| {
                if kind == Kind::Gmail {
                    if let Some(run) = s.gmail_backlog.runs.last() {
                        let entries = run.entries(run.undo_receipt.is_some());
                        job.progress.saved = entries
                            .iter()
                            .filter(|b| b.verified())
                            .map(|b| b.intent().messages().len())
                            .sum();
                        job.progress.folders = entries.len();
                    }
                } else if let Some(run) = s.protocol_backlog.runs.last() {
                    let entries: Vec<_> = if run.undo_receipt.is_some() {
                        run.reverse.iter().map(|r| &r.evidence).collect()
                    } else {
                        run.batches.iter().collect()
                    };
                    job.progress.saved = entries
                        .iter()
                        .filter(|e| protocol::verified(e))
                        .map(|e| e.command().messages().len())
                        .sum();
                    job.progress.folders = entries.len();
                    if !run.compensation.is_empty() {
                        let counts = run.compensation_counts();
                        job.progress.saved = counts.restored + counts.copies_removed;
                        job.progress.folders = run.compensation.len();
                    }
                }
            },
        );
        Ok(())
    };
    match &request.command {
        Action::CopyConfirm {
            options,
            receipt,
            confirmed,
        } => {
            if kind != Kind::Move || !confirmed || other_effects_pending(&saved) {
                return Err(AcquisitionError::InvalidInput);
            }
            mailbox_consent(
                state,
                &account.id,
                if options.intent == crate::backlog::Intent::ArchiveUnmatched {
                    "Archive the previewed old unmatched Inbox messages"
                } else {
                    "File the previewed backlog messages and remove their originals"
                },
                receipt,
            )?;
            repo.backlog_confirm_copy(
                &account,
                &store,
                &mut saved,
                options.clone(),
                receipt,
                cancel,
                fresh,
            )?;
            return Ok(report(&saved, json!({"journal":journal(&saved)})));
        }
        Action::CopyContinue { run, work } => {
            if kind != Kind::Move {
                return Err(AcquisitionError::InvalidInput);
            }
            repo.copy_continuation(&account, &store, &mut saved, run, *work, &fresh)?
                .connected(cancel)?;
            return Ok(report(&saved, json!({"journal":journal(&saved)})));
        }
        Action::CopyUndoPreview { target } => {
            if kind != Kind::Move {
                return Err(AcquisitionError::InvalidInput);
            }
            return Ok(json!(
                repo.copy_undo_workflow(&account, &store, &mut saved, target.target(), &fresh)?
                    .connected_preview(cancel)?
            ));
        }
        Action::CopyUndoConfirm {
            target,
            receipt,
            confirmed,
        } => {
            if kind != Kind::Move || !confirmed || other_effects_pending(&saved) {
                return Err(AcquisitionError::InvalidInput);
            }
            let workflow =
                repo.copy_undo_workflow(&account, &store, &mut saved, target.target(), &fresh)?;
            mailbox_consent(
                state,
                &account.id,
                match target {
                    CopyUndoSelection::Initial { .. } => "Apply the previewed COPY Undo actions",
                    CopyUndoSelection::AfterClear { .. } => {
                        "Remove the previewed extra copies after clearing Deleted"
                    }
                },
                receipt,
            )?;
            workflow.connected_confirm(receipt, cancel)?;
            return Ok(report(&saved, json!({"journal":journal(&saved)})));
        }
        Action::ClosePreview { run } => {
            return Ok(json!(match kind {
                Kind::Move => repo.backlog_close_preview(&account, &store, &saved, run, cancel)?,
                Kind::Gmail => repo.gmail_close_preview(&account, &store, &saved, run, cancel)?,
            }));
        }
        Action::Close {
            run,
            receipt,
            confirmed,
        } => {
            if !confirmed {
                return Err(AcquisitionError::InvalidInput);
            }
            mailbox_consent(
                state,
                &account.id,
                "Close the conflicted sorting run without changing mail",
                receipt,
            )?;
            match kind {
                Kind::Move => {
                    repo.backlog_close(&account, &store, &mut saved, run, receipt, cancel)?
                }
                Kind::Gmail => {
                    repo.gmail_close(&account, &store, &mut saved, run, receipt, cancel)?
                }
            }
            return Ok(report(&saved, json!({"journal":journal(&saved)})));
        }
        Action::LabelPreview { .. }
        | Action::LabelConfirm { .. }
        | Action::LabelReconcile { .. }
            if kind == Kind::Move =>
        {
            return Err(AcquisitionError::InvalidInput);
        }
        _ => {}
    }
    if kind == Kind::Gmail {
        match &request.command {
            Action::LabelPreview { options } => {
                return Ok(json!(repo.label_preview(
                    &account,
                    &store,
                    &saved,
                    options.clone(),
                    cancel
                )?));
            }
            Action::LabelConfirm {
                options,
                receipt,
                confirmed,
            } => {
                if !confirmed {
                    return Err(AcquisitionError::InvalidInput);
                }
                mailbox_consent(
                    state,
                    &account.id,
                    "Create the previewed Gmail labels",
                    receipt,
                )?;
                repo.label_confirm(
                    &account,
                    &store,
                    &mut saved,
                    options.clone(),
                    receipt,
                    cancel,
                    |s| ledger_fresh(s, guard.as_ref()),
                )?;
            }
            Action::LabelReconcile { operation } => {
                repo.label_reconcile(&account, &store, &mut saved, operation, cancel)?
            }
            Action::Preview { options } => {
                return Ok(json!(repo.gmail_preview(
                    &account,
                    &store,
                    &saved,
                    options.clone(),
                    cancel
                )?));
            }
            Action::Confirm {
                options,
                receipt,
                confirmed,
            } => {
                if !confirmed || other_effects_pending(&saved) {
                    return Err(AcquisitionError::InvalidInput);
                }
                mailbox_consent(
                    state,
                    &account.id,
                    if options.intent == crate::backlog::Intent::ArchiveUnmatched {
                        "Archive the previewed old unmatched Inbox messages"
                    } else {
                        "Apply the previewed Gmail label changes"
                    },
                    receipt,
                )?;
                repo.gmail_confirm(
                    &account,
                    &store,
                    &mut saved,
                    options.clone(),
                    receipt,
                    cancel,
                    fresh,
                )?;
            }
            Action::Reconcile { run } => {
                repo.gmail_reconcile(&account, &store, &mut saved, run, cancel)?
            }
            Action::Resume { run } => {
                repo.gmail_run(&account, &store, &mut saved, run, false, cancel, fresh)?
            }
            Action::UndoPreview { run } => {
                return Ok(json!(
                    repo.gmail_undo_preview(&account, &store, &saved, run, cancel)?
                ));
            }
            Action::Undo {
                run,
                receipt,
                confirmed,
            } => {
                if !confirmed {
                    return Err(AcquisitionError::InvalidInput);
                }
                mailbox_consent(
                    state,
                    &account.id,
                    "Undo the previewed Gmail label changes",
                    receipt,
                )?;
                repo.gmail_undo(&account, &store, &mut saved, run, receipt, cancel, fresh)?;
            }
            Action::ResumeUndo { run } => {
                repo.gmail_run(&account, &store, &mut saved, run, true, cancel, fresh)?
            }
            Action::Status {}
            | Action::Cancel {}
            | Action::CopyConfirm { .. }
            | Action::CopyContinue { .. }
            | Action::CopyUndoPreview { .. }
            | Action::CopyUndoConfirm { .. }
            | Action::ClosePreview { .. }
            | Action::Close { .. } => unreachable!(),
        }
        return Ok(report(&saved, json!({"journal":saved.gmail_backlog})));
    }
    match &request.command {
        Action::Preview { options } => {
            return Ok(json!(repo.backlog_preview(
                &account,
                &store,
                &saved,
                options.clone(),
                cancel
            )?));
        }
        Action::Confirm {
            options,
            receipt,
            confirmed,
        } => {
            if !confirmed || other_effects_pending(&saved) {
                return Err(AcquisitionError::InvalidInput);
            }
            mailbox_consent(
                state,
                &account.id,
                if options.intent == crate::backlog::Intent::ArchiveUnmatched {
                    "Archive the previewed old unmatched Inbox messages"
                } else {
                    "Move the previewed backlog messages"
                },
                receipt,
            )?;
            repo.backlog_confirm(
                &account,
                &store,
                &mut saved,
                options.clone(),
                receipt,
                cancel,
                fresh,
            )?;
        }
        Action::Reconcile { run } => {
            repo.backlog_reconcile(&account, &store, &mut saved, run, cancel)?
        }
        Action::Resume { run } => {
            repo.backlog_run(&account, &store, &mut saved, run, false, cancel, fresh)?
        }
        Action::UndoPreview { run } => {
            return Ok(json!(
                repo.backlog_undo_preview(&account, &store, &saved, run, cancel)?
            ));
        }
        Action::Undo {
            run,
            receipt,
            confirmed,
        } => {
            if !confirmed {
                return Err(AcquisitionError::InvalidInput);
            }
            mailbox_consent(
                state,
                &account.id,
                "Undo the previewed backlog moves",
                receipt,
            )?;
            repo.backlog_undo(&account, &store, &mut saved, run, receipt, cancel, fresh)?;
        }
        Action::ResumeUndo { run } => {
            repo.backlog_run(&account, &store, &mut saved, run, true, cancel, fresh)?
        }
        Action::Status {}
        | Action::Cancel {}
        | Action::CopyConfirm { .. }
        | Action::CopyContinue { .. }
        | Action::CopyUndoPreview { .. }
        | Action::CopyUndoConfirm { .. }
        | Action::ClosePreview { .. }
        | Action::Close { .. }
        | Action::LabelPreview { .. }
        | Action::LabelConfirm { .. }
        | Action::LabelReconcile { .. } => unreachable!(),
    }
    Ok(json!({"journal":saved.protocol_backlog.history_view()}))
}
