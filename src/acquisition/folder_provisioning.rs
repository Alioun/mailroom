pub(super) mod transport;
use super::{Account, Accounts, AcquisitionError, Cancellation};
use crate::census::{
    AccountState, EncryptedStore,
    folder_provisioning::{
        self as model, Context, Failure, FolderPlan, Operation, Outcome, Status, Step,
    },
};
use serde::Serialize;
use subtle::ConstantTimeEq;
use transport::{InstalledSource, Session};

const PURPOSE: &[u8] = b"mailroom/exact-imap-folder-create/v1";
pub const LIMITATIONS: &str = "Synthetic loopback installation only. Folder candidates come from the emitted artifact and need not be reachable under conservative saved-account recipient authority. Empty personal prefix and slash hierarchy only. Combining sequences and Jamo names are unsupported; names are never normalized. Visible folders prove neither attribution nor delivery. No Sieve deployment or native confirmation. CREATE has no compare-and-swap. Another editor can remove a parent after the final read; a server may recreate parents or subscribe folders implicitly. No automatic retry, continuation, cleanup or undo. Uncertainty requires explicit observation and fresh exact approval. Actual provider mapping and personal readiness remain unverified.";
#[derive(Serialize)]
pub(crate) struct Preview {
    pub operation: Operation,
    pub receipt: String,
    pub proof: serde_json::Value,
    pub limitations: &'static str,
}
fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn protocol(_: Failure) -> AcquisitionError {
    AcquisitionError::Protocol
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    store.save_state(state)?;
    #[cfg(test)]
    tests::progress_evidence(state);
    Ok(())
}
impl Accounts {
    pub(crate) fn folders_installed(&self) -> bool {
        self.folder_source
            .as_ref()
            .is_some_and(|s| s.permit(&s.account).is_ok())
    }
    pub(crate) fn folders_account(&self, id: &crate::census::AccountId) -> bool {
        self.folder_source
            .as_ref()
            .is_some_and(|s| s.account == *id)
    }
    fn folder_source(&self, account: &Account) -> Result<&InstalledSource, AcquisitionError> {
        let source = self.folder_source.as_deref().ok_or_else(invalid)?;
        source.permit(&account.id).map_err(protocol)?;
        Ok(source)
    }
    fn folder_context(
        &self,
        account: &Account,
        store: &EncryptedStore,
    ) -> Result<Context, AcquisitionError> {
        let source = self.folder_source(account)?;
        let authority = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                &account.login,
                self.workspace_context(account, store)?.binding(),
                self.sieve_authority(account, store)?,
            ))
            .map_err(|_| invalid())?,
        );
        Ok(Context {
            account: account.id.clone(),
            source: source.identity(),
            authority: store
                .keys()
                .binding_for(b"mailroom/folder-authority/v1", &authority),
            profile: model::PROFILE.into(),
        })
    }
    fn folder_session(
        &self,
        account: &Account,
        cancel: Cancellation,
    ) -> Result<Session, AcquisitionError> {
        let source = self.folder_source(account)?;
        let permit = source.permit(&account.id).map_err(protocol)?;
        let password = self.password(account)?;
        Session::connect(&permit, &account.login, &password, cancel).map_err(protocol)
    }
    pub(crate) fn folder_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        self.folder_source(account)?;
        if store.recovery_pending()
            || saved.folder_provisioning.history.len() >= model::HISTORY
            || saved.folder_provisioning.history.iter().any(|op| {
                matches!(
                    op.status,
                    Status::Running | Status::Uncertain | Status::Stopped | Status::Refused
                )
            })
        {
            return Err(invalid());
        }
        let p = crate::rules::workspace::preview(
            saved,
            &self.workspace_context(account, store)?,
            store.keys(),
        )
        .map_err(|_| invalid())?;
        if !p.download_available || p.proof.gate == "blocked" {
            return Err(invalid());
        }
        let context = self.folder_context(account, store)?;
        let mut session = self.folder_session(account, cancel)?;
        let before = session.observe().map_err(protocol)?;
        let plan = FolderPlan::build(p.proof.artifact.folders(), &before).map_err(protocol)?;
        let proof = serde_json::json!({"gate":p.proof.gate,"semantics":p.proof.semantics,"pins":p.proof.pins,"warnings":p.proof.warnings,"runtime_cross_check":p.proof.runtime_cross_check,"recipient_evidence":p.proof.artifact.context().recipient_evidence,"capabilities":p.proof.artifact.context().capabilities,"context_binding":self.workspace_context(account, store)?.binding()});
        let payload = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                saved,
                &context,
                &before,
                &plan,
                p.proof.artifact.identity(),
                &p.receipt,
                &proof,
            ))
            .map_err(|_| invalid())?,
        );
        let receipt = store.keys().binding_for(PURPOSE, &payload);
        let id = store
            .keys()
            .binding_for(b"mailroom/folder-operation/v1", receipt.as_bytes());
        session.logout();
        Ok(Preview {
            operation: Operation {
                id,
                context,
                artifact: p.proof.artifact.identity().into(),
                proof_binding: p.receipt,
                receipt: receipt.clone(),
                before,
                plan,
                status: Status::Running,
                steps: vec![],
            },
            receipt,
            proof,
            limitations: LIMITATIONS,
        })
    }
    pub(crate) fn folder_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let p = self.folder_preview(account, store, saved, cancel.clone())?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        fresh(saved).map_err(|_| invalid())?;
        saved.folder_provisioning.history.push(p.operation);
        save(store, saved)?;
        boundary("folder_consumed");
        let index = saved.folder_provisioning.history.len() - 1;
        let result = self.folder_run(account, store, saved, index, cancel, &fresh);
        if result.is_err() {
            let op = &mut saved.folder_provisioning.history[index];
            if op.status == Status::Running {
                op.status =
                    if op.steps.last().is_some_and(|s| {
                        matches!(s.outcome, Outcome::Intent | Outcome::Acknowledged)
                    }) {
                        Status::Uncertain
                    } else {
                        Status::Stopped
                    };
                if let Some(step) = op.steps.last_mut() {
                    if matches!(step.outcome, Outcome::Intent | Outcome::Acknowledged) {
                        step.outcome = Outcome::Uncertain;
                    }
                }
            }
            save(store, saved)?;
        }
        result
    }
    fn folder_run(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        index: usize,
        cancel: Cancellation,
        fresh: &impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let source = self.folder_source(account)?;
        let permit = source.permit(&account.id).map_err(protocol)?;
        let mut session = self.folder_session(account, cancel.clone())?;
        let op = &saved.folder_provisioning.history[index];
        let entries: Vec<_> = op
            .plan
            .entries
            .iter()
            .filter(|e| e.existing.is_none())
            .cloned()
            .collect();
        let mut expected = op.before.clone();
        for (position, entry) in entries.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(AcquisitionError::Cancelled);
            }
            if fresh(saved).is_err()
                || self.folder_context(account, store)?
                    != saved.folder_provisioning.history[index].context
                || session.observe().map_err(protocol)? != expected
                || FolderPlan::build(
                    &saved.folder_provisioning.history[index].plan.logical_leaves,
                    &expected,
                )
                .is_err()
            {
                saved.folder_provisioning.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            fresh(saved).map_err(|_| invalid())?;
            saved.folder_provisioning.history[index].steps.push(Step {
                name: entry.name.clone(),
                outcome: Outcome::Intent,
                acknowledgment: false,
                failure: None,
                before: expected.clone(),
                observed: None,
            });
            save(store, saved)?;
            boundary(if position == 0 {
                "folder_first_intent"
            } else {
                "folder_next_intent"
            });
            if entry.role == model::Role::Leaf {
                boundary("folder_leaf_intent");
            }
            let result = session.create(&permit, &entry.name);
            let step = saved.folder_provisioning.history[index]
                .steps
                .last_mut()
                .unwrap();
            if let Err(e) = result {
                step.failure = Some(e);
                step.outcome = if matches!(e, Failure::No | Failure::Bad | Failure::Quota) {
                    Outcome::Refused
                } else {
                    Outcome::Uncertain
                };
                saved.folder_provisioning.history[index].status =
                    if matches!(e, Failure::No | Failure::Bad | Failure::Quota) {
                        Status::Refused
                    } else {
                        Status::Uncertain
                    };
                save(store, saved)?;
                if matches!(e, Failure::No | Failure::Bad | Failure::Quota) {
                    if let Ok(observed) = session.observe() {
                        if observed != expected {
                            saved.folder_provisioning.history[index].status = Status::Conflict;
                        }
                        saved.folder_provisioning.history[index]
                            .steps
                            .last_mut()
                            .unwrap()
                            .observed = Some(observed);
                        save(store, saved)?;
                    }
                }
                return Err(protocol(e));
            }
            step.acknowledgment = true;
            step.outcome = Outcome::Acknowledged;
            save(store, saved)?;
            boundary("folder_acknowledged");
            let observed = session.observe().map_err(protocol)?;
            let matches = model::desired_delta(&expected, &observed, &entry.name);
            let step = saved.folder_provisioning.history[index]
                .steps
                .last_mut()
                .unwrap();
            step.observed = Some(observed.clone());
            step.outcome = if matches {
                Outcome::Observed
            } else {
                Outcome::Conflict
            };
            if !matches {
                saved.folder_provisioning.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            save(store, saved)?;
            expected = observed;
            boundary("folder_observed");
        }
        if entries.is_empty() && session.observe().map_err(protocol)? != expected {
            saved.folder_provisioning.history[index].status = Status::Conflict;
            save(store, saved)?;
            return Err(invalid());
        }
        saved.folder_provisioning.history[index].status = Status::Complete;
        save(store, saved)?;
        session.logout();
        Ok(())
    }
    pub(crate) fn folder_reconcile(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        if store.recovery_pending() {
            return Err(invalid());
        }
        let context = self.folder_context(account, store)?;
        let op = saved
            .folder_provisioning
            .history
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or_else(invalid)?;
        if op.context != context {
            if op.status != Status::Conflict {
                op.status = Status::Conflict;
                save(store, saved)?;
            }
            return Err(invalid());
        }
        if op.status == Status::Conflict {
            return Ok(());
        }
        let mut session = self.folder_session(account, cancel)?;
        let observed = match session.observe() {
            Ok(observed) => observed,
            Err(Failure::Unsupported | Failure::UnsupportedName) => {
                if op.status != Status::Conflict {
                    op.status = Status::Conflict;
                    save(store, saved)?;
                }
                return Err(AcquisitionError::Protocol);
            }
            Err(error) => return Err(protocol(error)),
        };
        let previous = op.clone();
        if let Some(step) = op.steps.last_mut() {
            if model::desired_delta(&step.before, &observed, &step.name) {
                step.outcome = Outcome::Observed;
                step.observed = Some(observed);
                op.status = if op.steps.len()
                    == op
                        .plan
                        .entries
                        .iter()
                        .filter(|e| e.existing.is_none())
                        .count()
                {
                    Status::Complete
                } else {
                    Status::FreshReview
                };
            } else if observed == step.before {
                step.outcome = Outcome::Absent;
                step.observed = Some(observed);
                op.status = Status::FreshReview;
            } else {
                step.outcome = Outcome::Conflict;
                step.observed = Some(observed);
                op.status = Status::Conflict;
            }
        } else {
            op.status = if observed == op.before {
                if previous.status == Status::Complete {
                    Status::Complete
                } else {
                    Status::FreshReview
                }
            } else {
                Status::Conflict
            };
        }
        if *op != previous {
            save(store, saved)?;
        }
        session.logout();
        Ok(())
    }
}
pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MAILROOM_FOLDERS_FIXTURE") {
        let root = std::path::Path::new(&root);
        if std::fs::read_to_string(root.join("folder-boundary")).is_ok_and(|s| s == name) {
            std::fs::write(root.join(format!("reached-{name}")), b"reached").unwrap();
            for _ in 0..1500 {
                if root.join(format!("continue-{name}")).exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    let _ = name;
}
#[cfg(test)]
pub(crate) mod tests;
