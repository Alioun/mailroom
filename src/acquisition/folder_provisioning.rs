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
use transport::{
    InstalledSource, Session,
    gmail_label::{Connection as LabelConnection, LabelPermit},
};

const PURPOSE: &[u8] = b"mailroom/exact-imap-folder-create/v1";
const LABEL_PURPOSE: &[u8] = b"mailroom/exact-gmail-label-create/v1";
pub const LABEL_LIMITATIONS: &str = "Synthetic loopback Gmail X-GM-EXT-1 source only. Labels come from the destinations the Gmail sorting preview lacks for the same options; every missing parent is its own explicit CREATE, top down, and each is read back through LIST before the next. Names are created in the exact case and modified UTF-7 the plan holds and never normalized. A label differing only in case, a reserved name or one under the system root is refused, never renamed or reused. Mailroom never renames or deletes a label and never creates one through a label store. CREATE has no compare-and-swap: another client can create, rename or remove a label after the final read, and Gmail may create parents implicitly or store a different case; Mailroom then records a conflict and retries nothing. No automatic retry, continuation, cleanup or undo. Gmail's label creation semantics are unverified fixture assumptions.";
pub const LIMITATIONS: &str = "Synthetic loopback installation only. Folder candidates come from the emitted artifact and need not be reachable under conservative saved-account recipient authority. Empty personal prefix and slash hierarchy only. Combining sequences and Jamo names are unsupported; names are never normalized. Visible folders prove neither attribution nor delivery. No Sieve deployment or native confirmation. CREATE has no compare-and-swap. Another editor can remove a parent after the final read; a server may recreate parents or subscribe folders implicitly. No automatic retry, continuation, cleanup or undo. Uncertainty requires explicit observation and fresh exact approval. Actual provider mapping and personal readiness remain unverified.";
#[derive(Debug, Serialize)]
pub(crate) struct Preview {
    pub operation: Operation,
    pub receipt: String,
    pub proof: serde_json::Value,
    pub limitations: &'static str,
}
fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn protocol(_failure: Failure) -> AcquisitionError {
    #[cfg(test)]
    tests::report_failure(_failure);
    AcquisitionError::Protocol
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    #[cfg(test)]
    let began = std::time::Instant::now();
    store.save_state(state)?;
    #[cfg(test)]
    tests::SAVE_TIME.with(|t| t.set(t.get() + began.elapsed()));
    #[cfg(test)]
    tests::progress_evidence(state);
    Ok(())
}
/// Which provisioning journal and source an operation uses: mailbox.org
/// folders over the create source, or Gmail labels over the X-GM-EXT-1 label
/// source. Both share the plan, journal, crash and reconcile rules.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Folders,
    Labels,
}
impl Target {
    fn journal(self, state: &mut AccountState) -> &mut model::FolderJournal {
        match self {
            Self::Folders => &mut state.folder_provisioning,
            Self::Labels => &mut state.label_provisioning,
        }
    }
    fn boundary(self, name: &str) {
        match self {
            Self::Folders => boundary(name),
            Self::Labels => super::gmail_backlog::boundary(&name.replacen("folder_", "label_", 1)),
        }
    }
}
/// An open session that observes the topology and creates exact names.
enum Creator<'a> {
    Folders(Session, transport::CreatePermit<'a>),
    Labels(LabelConnection, LabelPermit<'a>),
}
impl Creator<'_> {
    fn observe(&mut self) -> Result<model::Inventory, Failure> {
        match self {
            Self::Folders(session, _) => session.observe(),
            Self::Labels(connection, _) => connection.observe(),
        }
    }
    fn create(&mut self, name: &model::ExactName) -> Result<(), Failure> {
        match self {
            Self::Folders(session, permit) => session.create(permit, name),
            Self::Labels(connection, permit) => connection.create(permit, name),
        }
    }
    fn logout(&mut self) {
        match self {
            Self::Folders(session, _) => session.logout(),
            Self::Labels(connection, _) => connection.logout(),
        }
    }
}
/// The operation an exact approval would start, before any check against the
/// journal it goes into.
fn blocked(journal: &model::FolderJournal) -> bool {
    journal.history.len() >= model::HISTORY
        || journal.history.iter().any(|op| {
            matches!(
                op.status,
                Status::Running | Status::Uncertain | Status::Stopped | Status::Refused
            )
        })
}
fn missing(op: &Operation) -> Vec<model::Entry> {
    op.plan
        .entries
        .iter()
        .filter(|e| e.existing.is_none())
        .cloned()
        .collect()
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
    fn provision_context(
        &self,
        target: Target,
        account: &Account,
        store: &EncryptedStore,
    ) -> Result<Context, AcquisitionError> {
        let (source, purpose, profile) = match target {
            Target::Folders => (
                self.folder_source(account)?.identity(),
                b"mailroom/folder-authority/v1".as_slice(),
                model::PROFILE,
            ),
            Target::Labels => (
                self.label_permit(account, store)?.source().identity(),
                b"mailroom/gmail-label-authority/v1".as_slice(),
                model::GMAIL_PROFILE,
            ),
        };
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
            source,
            authority: store.keys().binding_for(purpose, &authority),
            profile: profile.into(),
        })
    }
    fn creator(
        &self,
        target: Target,
        account: &Account,
        store: &EncryptedStore,
        cancel: Cancellation,
    ) -> Result<Creator<'_>, AcquisitionError> {
        Ok(match target {
            Target::Folders => {
                let permit = self
                    .folder_source(account)?
                    .permit(&account.id)
                    .map_err(protocol)?;
                let password = self.password(account)?;
                let session = Session::connect(&permit, &account.login, &password, cancel)
                    .map_err(protocol)?;
                Creator::Folders(session, permit)
            }
            Target::Labels => {
                let permit = self.label_permit(account, store)?;
                let password = self.password(account)?;
                let connection =
                    LabelConnection::connect(&permit, &password, cancel).map_err(protocol)?;
                Creator::Labels(connection, permit)
            }
        })
    }
    pub(crate) fn folder_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        self.folder_source(account)?;
        if store.recovery_pending() || blocked(&saved.folder_provisioning) {
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
        let context = self.provision_context(Target::Folders, account, store)?;
        let mut session = self.creator(Target::Folders, account, store, cancel)?;
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
        session.logout();
        Ok(prepared(
            store,
            PURPOSE,
            &payload,
            context,
            p.proof.artifact.identity().into(),
            p.receipt,
            before,
            plan,
            proof,
            LIMITATIONS,
        ))
    }
    /// The exact labels a Gmail filing plan for `options` lacks, as a
    /// provisioning operation: every missing ancestor and leaf, top down.
    /// Blocked labels (case variants, reserved names, the system root) are
    /// left out and stay excluded from sorting.
    pub(crate) fn label_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: crate::backlog::Options,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        if store.recovery_pending() || blocked(&saved.label_provisioning) {
            return Err(invalid());
        }
        let context = self.provision_context(Target::Labels, account, store)?;
        let mut found =
            self.gmail_missing_labels(account, store, saved, options.clone(), cancel)?;
        let leaves: std::collections::BTreeSet<String> = found
            .missing
            .iter()
            .filter(|m| m.blocked.is_none())
            .map(|m| m.name.clone())
            .collect();
        if leaves.is_empty() {
            return Err(invalid());
        }
        let before = found.topology;
        let plan = FolderPlan::build(&leaves, &before).map_err(protocol)?;
        let proof = serde_json::json!({"saved_proof":found.saved_proof,"options":options,"missing":found.missing});
        let payload = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                saved,
                &context,
                &before,
                &plan,
                &found.artifact,
                &found.saved_proof,
                &proof,
            ))
            .map_err(|_| invalid())?,
        );
        found.connection.logout();
        Ok(prepared(
            store,
            LABEL_PURPOSE,
            &payload,
            context,
            found.artifact,
            found.saved_proof,
            before,
            plan,
            proof,
            LABEL_LIMITATIONS,
        ))
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
        self.provision_confirm(
            Target::Folders,
            account,
            store,
            saved,
            p,
            receipt,
            cancel,
            &fresh,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn label_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        options: crate::backlog::Options,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let p = self.label_preview(account, store, saved, options, cancel.clone())?;
        self.provision_confirm(
            Target::Labels,
            account,
            store,
            saved,
            p,
            receipt,
            cancel,
            &fresh,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn provision_confirm(
        &self,
        target: Target,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        p: Preview,
        receipt: &str,
        cancel: Cancellation,
        fresh: &impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        fresh(saved).map_err(|_| invalid())?;
        target.journal(saved).history.push(p.operation);
        save(store, saved)?;
        target.boundary("folder_consumed");
        let index = target.journal(saved).history.len() - 1;
        #[cfg(test)]
        let timing = tests::RunTiming::start();
        let result = self.provision_run(target, account, store, saved, index, cancel, fresh);
        #[cfg(test)]
        timing.report(target.journal(saved).history[index].steps.len());
        if result.is_err() {
            let op = &mut target.journal(saved).history[index];
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
    #[cfg(test)]
    fn folder_run(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        index: usize,
        cancel: Cancellation,
        fresh: &impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        self.provision_run(Target::Folders, account, store, saved, index, cancel, fresh)
    }
    #[allow(clippy::too_many_arguments)]
    fn provision_run(
        &self,
        target: Target,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        index: usize,
        cancel: Cancellation,
        fresh: &impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let mut session = self.creator(target, account, store, cancel.clone())?;
        let op = &target.journal(saved).history[index];
        let entries = missing(op);
        let mut expected = op.before.clone();
        for (position, entry) in entries.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(AcquisitionError::Cancelled);
            }
            if fresh(saved).is_err()
                || self.provision_context(target, account, store)?
                    != target.journal(saved).history[index].context
                || session.observe().map_err(protocol)? != expected
                || FolderPlan::build(
                    &target.journal(saved).history[index].plan.logical_leaves,
                    &expected,
                )
                .is_err()
            {
                target.journal(saved).history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            fresh(saved).map_err(|_| invalid())?;
            target.journal(saved).history[index].steps.push(Step {
                name: entry.name.clone(),
                outcome: Outcome::Intent,
                acknowledgment: false,
                failure: None,
                before: expected.clone(),
                observed: None,
            });
            save(store, saved)?;
            target.boundary(if position == 0 {
                "folder_first_intent"
            } else {
                "folder_next_intent"
            });
            if entry.role == model::Role::Leaf {
                target.boundary("folder_leaf_intent");
            }
            let result = session.create(&entry.name);
            let op = &mut target.journal(saved).history[index];
            let step = op.steps.last_mut().unwrap();
            if let Err(e) = result {
                let refused = matches!(e, Failure::No | Failure::Bad | Failure::Quota);
                step.failure = Some(e);
                step.outcome = if refused {
                    Outcome::Refused
                } else {
                    Outcome::Uncertain
                };
                op.status = if refused {
                    Status::Refused
                } else {
                    Status::Uncertain
                };
                save(store, saved)?;
                if refused {
                    if let Ok(observed) = session.observe() {
                        let op = &mut target.journal(saved).history[index];
                        if observed != expected {
                            op.status = Status::Conflict;
                        }
                        op.steps.last_mut().unwrap().observed = Some(observed);
                        save(store, saved)?;
                    }
                }
                return Err(protocol(e));
            }
            step.acknowledgment = true;
            step.outcome = Outcome::Acknowledged;
            save(store, saved)?;
            target.boundary("folder_acknowledged");
            let observed = session.observe().map_err(protocol)?;
            let matches = model::desired_delta(&expected, &observed, &entry.name);
            let op = &mut target.journal(saved).history[index];
            let step = op.steps.last_mut().unwrap();
            step.observed = Some(observed.clone());
            step.outcome = if matches {
                Outcome::Observed
            } else {
                Outcome::Conflict
            };
            if !matches {
                op.status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            save(store, saved)?;
            expected = observed;
            target.boundary("folder_observed");
        }
        if entries.is_empty() && session.observe().map_err(protocol)? != expected {
            target.journal(saved).history[index].status = Status::Conflict;
            save(store, saved)?;
            return Err(invalid());
        }
        target.journal(saved).history[index].status = Status::Complete;
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
        self.provision_reconcile(Target::Folders, account, store, saved, id, cancel)
    }
    pub(crate) fn label_reconcile(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        self.provision_reconcile(Target::Labels, account, store, saved, id, cancel)
    }
    fn provision_reconcile(
        &self,
        target: Target,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        if store.recovery_pending() {
            return Err(invalid());
        }
        let context = self.provision_context(target, account, store)?;
        let index = target
            .journal(saved)
            .history
            .iter()
            .position(|o| o.id == id)
            .ok_or_else(invalid)?;
        let op = &mut target.journal(saved).history[index];
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
        let mut session = self.creator(target, account, store, cancel)?;
        let observed = match session.observe() {
            Ok(observed) => observed,
            Err(Failure::Unsupported | Failure::UnsupportedName) => {
                let op = &mut target.journal(saved).history[index];
                if op.status != Status::Conflict {
                    op.status = Status::Conflict;
                    save(store, saved)?;
                }
                return Err(AcquisitionError::Protocol);
            }
            Err(error) => return Err(protocol(error)),
        };
        let op = &mut target.journal(saved).history[index];
        let previous = op.clone();
        let count = missing(op).len();
        if let Some(step) = op.steps.last_mut() {
            if model::desired_delta(&step.before, &observed, &step.name) {
                step.outcome = Outcome::Observed;
                step.observed = Some(observed);
                op.status = if op.steps.len() == count {
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
/// A preview of `operation`, identified by its exact-purpose receipt.
#[allow(clippy::too_many_arguments)]
fn prepared(
    store: &EncryptedStore,
    purpose: &[u8],
    payload: &[u8],
    context: Context,
    artifact: String,
    proof_binding: String,
    before: model::Inventory,
    plan: FolderPlan,
    proof: serde_json::Value,
    limitations: &'static str,
) -> Preview {
    let receipt = store.keys().binding_for(purpose, payload);
    let id = store
        .keys()
        .binding_for(b"mailroom/folder-operation/v1", receipt.as_bytes());
    Preview {
        operation: Operation {
            id,
            context,
            artifact,
            proof_binding,
            receipt: receipt.clone(),
            before,
            plan,
            status: Status::Running,
            steps: vec![],
        },
        receipt,
        proof,
        limitations,
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
