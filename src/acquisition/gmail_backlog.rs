use super::{
    Account, Accounts, AcquisitionError, Cancellation,
    folder_provisioning::transport::{
        gmail_label::{Connection, InstalledSource, LabelPermit, all_mail},
        message_move::RuntimeBinding,
    },
    imap::{MailRead, ReadOnlyImap},
};
use crate::{
    backlog::{
        Options,
        gmail_journal::{self as model, MissingLabel, Preview, Reverse, Run, UndoPreview},
        gmail_label::{GmailId, INBOX, LabelBatch, LabelIntent, Outcome, Snapshot, StepKind},
        imap_move::{Completion, MailboxIdentity, Purpose, Uid, UidValidity},
        protocol::{Binding, ClosePreview, CloseReason, Closure, SavedPlan, Status, same_binding},
    },
    census::{
        AccountId, AccountState, Census, EncryptedStore,
        folder_provisioning::{Failure, Inventory},
    },
    rules::{proof, workspace},
};
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;

const PROFILE: &str = "loopback-gmail-all-mail-labels/v1";

fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn failure<T: std::fmt::Debug>(_error: T) -> AcquisitionError {
    #[cfg(test)]
    eprintln!("gmail_backlog.failure={}", super::diagnostic_code(&_error));
    AcquisitionError::Protocol
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    store.save_state(state).map_err(Into::into)
}
fn kind_name(kind: StepKind) -> &'static str {
    match kind {
        StepKind::AddLabel => "add_label",
        StepKind::RemoveInbox => "remove_inbox",
        StepKind::RestoreInbox => "restore_inbox",
        StepKind::RemoveLabel => "remove_label",
    }
}
fn store_batch(saved: &mut AccountState, run: usize, reverse: bool, n: usize, batch: &LabelBatch) {
    let r = &mut saved.gmail_backlog.runs[run];
    if reverse {
        r.reverse[n].batch = batch.clone();
    } else {
        r.batches[n] = batch.clone();
    }
}
/// The parts of the approved topology execution depends on: capabilities,
/// namespaces, All Mail and every destination label. Labels another client
/// creates or removes elsewhere do not invalidate the run.
fn compatible(run: &Run, current: &Inventory) -> bool {
    let approved = &run.plan.topology;
    let same = |name: &str| approved.get(name).is_some() && approved.get(name) == current.get(name);
    approved.capabilities == current.capabilities
        && approved.namespaces == current.namespaces
        && all_mail(current).is_ok_and(|m| same(&m.name.decoded))
        && run
            .batches
            .iter()
            .all(|b| same(&b.intent().destination().decoded))
}
/// Every attempted step of the batch read back as verified for every member.
fn resolved(batch: &LabelBatch) -> bool {
    (0..batch.steps().len()).all(|i| {
        !batch.steps()[i].attempted() || batch.outcomes(i).values().all(|o| *o == Outcome::Verified)
    })
}
fn snapshot(m: &crate::census::MessageRecord) -> Result<Snapshot, AcquisitionError> {
    Ok(Snapshot {
        gmail_id: GmailId::new(m.gmail_message_id.as_deref().ok_or_else(invalid)?)
            .ok_or_else(invalid)?,
        message_id_hash: m.headers.message_id_hash.clone().ok_or_else(invalid)?,
        size: u32::try_from(m.size).map_err(failure)?,
        internal_date: m.internal_date.clone(),
        flags: m.flags.clone(),
        labels: m.gmail_labels.clone(),
    })
}

struct Filing {
    saved_proof: workspace::ProofPreview,
    topology: Inventory,
    observed: Census,
    execution: proof::Proof,
    plan: crate::backlog::Plan,
    missing: Vec<MissingLabel>,
    connection: Connection,
}

/// What label provisioning starts from: the saved artifact and proof it is
/// bound to, the observed topology and the labels the plan lacks.
pub(super) struct MissingLabels {
    pub artifact: String,
    pub saved_proof: String,
    pub topology: Inventory,
    pub missing: Vec<MissingLabel>,
    pub connection: Connection,
}

/// Destinations of `plan` that `topology` lacks as selectable labels of
/// exactly that name, with the messages each would receive. A label is
/// blocked when creating it would need a case variant of an existing label,
/// a reserved name or a parent that cannot hold it; Mailroom then neither
/// creates, renames nor reuses anything.
fn missing_labels(plan: &crate::backlog::Plan, topology: &Inventory) -> Vec<MissingLabel> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &plan.changes {
        if topology
            .get(&c.destination)
            .is_none_or(|m| !m.selectable() || m.name.decoded != c.destination)
        {
            *counts.entry(&c.destination).or_default() += 1;
        }
    }
    let system = all_mail(topology).ok().and_then(|m| {
        m.name
            .decoded
            .split_once('/')
            .map(|(root, _)| root.to_owned())
    });
    counts
        .into_iter()
        .map(|(name, messages)| MissingLabel {
            name: name.into(),
            messages,
            blocked: creatable(name, topology, system.as_deref()).err(),
        })
        .collect()
}

/// Whether one missing label can be created on its own: a user label outside
/// the system root whose path builds into an exact plan.
pub(super) fn creatable(
    name: &str,
    topology: &Inventory,
    system: Option<&str>,
) -> Result<(), Failure> {
    let exact = crate::census::folder_provisioning::ExactName::new(name)?;
    if !crate::backlog::gmail_label::user_label(&exact)
        || system.is_none_or(|root| name.split('/').next() == Some(root))
    {
        return Err(Failure::Unsupported);
    }
    crate::census::folder_provisioning::FolderPlan::build(&[name.to_owned()].into(), topology)
        .map(|_| ())
}

impl Accounts {
    pub(crate) fn labels_installed(&self, id: Option<&AccountId>) -> bool {
        self.label_source
            .as_ref()
            .is_some_and(|s| s.installed() && id.is_none_or(|id| *id == s.account))
    }
    fn label_source(&self, account: &Account) -> Result<&InstalledSource, AcquisitionError> {
        let s = self.label_source.as_deref().ok_or_else(invalid)?;
        if s.account != account.id
            || s.login != account.login
            || account.provider != super::Provider::Gmail
            || !s.installed()
        {
            return Err(invalid());
        }
        Ok(s)
    }
    fn gmail_binding(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
    ) -> Result<Binding, AcquisitionError> {
        let material = model::material(saved, store.keys()).map_err(failure)?;
        self.gmail_binding_with(account, store, material)
    }
    /// The binding with an already computed material digest. The digest covers
    /// the account state a run relies on, so a request computes it once and a
    /// run reuses it while it changes only its own journal.
    fn gmail_binding_with(
        &self,
        account: &Account,
        store: &EncryptedStore,
        material: String,
    ) -> Result<Binding, AcquisitionError> {
        let s = self.label_source(account)?;
        if store.recovery_pending() {
            return Err(AcquisitionError::LifecyclePending);
        }
        Ok(Binding {
            account: account.id.clone(),
            source: s.identity(),
            authority: self.sieve_authority(account, store)?,
            workspace: self.workspace_context(account, store)?.binding().into(),
            material,
        })
    }
    fn gmail_runtime(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        b: &Binding,
    ) -> Result<RuntimeBinding, AcquisitionError> {
        Ok(RuntimeBinding {
            account: b.account.clone(),
            provider: account.provider,
            login: account.login.clone(),
            source: b.source.clone(),
            credential_key_recovery: b.authority.clone(),
            workspace: b.workspace.clone(),
            topology: store.keys().binding_for(
                b"mailroom/gmail-topology-profile/v1",
                &serde_json::to_vec(&(PROFILE, &saved.census.folder_roles)).map_err(failure)?,
            ),
        })
    }
    /// The label source's permit under the account's current authority and
    /// workspace, for label provisioning.
    pub(super) fn label_permit(
        &self,
        account: &Account,
        store: &EncryptedStore,
    ) -> Result<LabelPermit<'_>, AcquisitionError> {
        let source = self.label_source(account)?;
        if store.recovery_pending() {
            return Err(AcquisitionError::LifecyclePending);
        }
        source
            .runtime_permit(&RuntimeBinding {
                account: account.id.clone(),
                provider: account.provider,
                login: account.login.clone(),
                source: source.identity(),
                credential_key_recovery: self.sieve_authority(account, store)?,
                workspace: self.workspace_context(account, store)?.binding().into(),
                topology: PROFILE.into(),
            })
            .map_err(failure)
    }
    fn gmail_connect(
        &self,
        account: &Account,
        runtime: &RuntimeBinding,
        cancel: Cancellation,
    ) -> Result<Connection, AcquisitionError> {
        let permit = self
            .label_source(account)?
            .runtime_permit(runtime)
            .map_err(failure)?;
        Connection::connect(&permit, &self.password(account)?, cancel).map_err(failure)
    }
    /// Reads All Mail once over the installed source. Other label mailboxes are
    /// topology only; X-GM-MSGID is the message identity. The label session
    /// stays open for the request.
    fn gmail_observe(
        &self,
        account: &Account,
        saved: &AccountState,
        runtime: &RuntimeBinding,
        cancel: Cancellation,
    ) -> Result<(Inventory, Census, Connection), AcquisitionError> {
        let mut connection = self.gmail_connect(account, runtime, cancel.clone())?;
        let before = connection.inventory().map_err(failure)?;
        let all = all_mail(&before).map_err(failure)?.name.clone();
        let read = crate::policy::read_permit(account.provider)?;
        let permit = self
            .label_source(account)?
            .runtime_permit(runtime)
            .map_err(failure)?;
        let mut reader = ReadOnlyImap::loopback_gmail(
            &permit,
            &read,
            &account.login,
            &self.password(account)?,
            cancel.clone(),
        )?;
        let folders = reader.folders()?;
        if folders.len() != before.mailboxes.len() {
            return Err(invalid());
        }
        for folder in &folders {
            if before
                .get(&folder.name)
                .is_none_or(|m| m.name.wire != folder.wire)
            {
                return Err(invalid());
            }
        }
        let selected: Vec<_> = folders
            .iter()
            .filter(|f| f.name == all.decoded)
            .cloned()
            .collect();
        let observed = reader.observe(&selected, &saved.census)?;
        drop(reader);
        for folder in observed.values() {
            let mut ids = BTreeSet::new();
            for record in folder.messages.values() {
                if !ids.insert(record.gmail_message_id.clone().ok_or_else(invalid)?) {
                    return Err(invalid());
                }
            }
        }
        let census = saved.census.observed(
            folders
                .iter()
                .map(|f| (f.name.clone(), f.roles.clone()))
                .collect(),
            observed,
        );
        if census.folders.len() != 1 {
            return Err(invalid());
        }
        if connection.inventory().map_err(failure)? != before {
            return Err(invalid());
        }
        Ok((before, census, connection))
    }

    pub(crate) fn gmail_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: Options,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        self.gmail_prepare(account, store, saved, options, cancel)
            .map(|(preview, _)| preview)
    }
    /// The saved and fresh proofs, the topology, All Mail as observed, and
    /// the filing plan with every destination it needs, labels missing from
    /// the topology included. The label session stays open.
    fn gmail_filing(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        binding: &Binding,
        options: Options,
        cancel: Cancellation,
    ) -> Result<Filing, AcquisitionError> {
        let context = self.workspace_context(account, store)?;
        let saved_proof = workspace::preview(saved, &context, store.keys()).map_err(failure)?;
        if saved_proof.proof.gate == "blocked" {
            return Err(invalid());
        }
        let runtime = self.gmail_runtime(account, store, saved, binding)?;
        let (topology, observed, connection) =
            self.gmail_observe(account, saved, &runtime, cancel)?;
        let input = &saved.workspace.as_ref().ok_or_else(invalid)?.input;
        let baseline = if input.baseline == proof::Baseline::Previous {
            Some(&saved.exports.last().ok_or_else(invalid)?.artifact)
        } else {
            None
        };
        let execution = proof::prove(
            saved_proof.proof.artifact.clone(),
            &observed,
            input.as_of,
            &input.pins,
            input.baseline,
            baseline,
        )
        .map_err(failure)?;
        if execution.gate == "blocked" {
            return Err(invalid());
        }
        let plan = model::project(saved, &observed, &execution, options).map_err(failure)?;
        let missing = missing_labels(&plan, &topology);
        Ok(Filing {
            saved_proof,
            topology,
            observed,
            execution,
            plan,
            missing,
            connection,
        })
    }

    /// The labels a Gmail filing plan for `options` needs and the topology
    /// lacks, with the label session they were observed through.
    pub(super) fn gmail_missing_labels(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: Options,
        cancel: Cancellation,
    ) -> Result<MissingLabels, AcquisitionError> {
        let binding = self.gmail_binding(account, store, saved)?;
        let f = self.gmail_filing(account, store, saved, &binding, options, cancel)?;
        Ok(MissingLabels {
            artifact: f.saved_proof.proof.artifact.identity().into(),
            saved_proof: f.saved_proof.receipt,
            topology: f.topology,
            missing: f.missing,
            connection: f.connection,
        })
    }

    /// The preview and the label session it observed through, which a
    /// confirm goes on to use.
    fn gmail_prepare(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: Options,
        cancel: Cancellation,
    ) -> Result<(Preview, Connection), AcquisitionError> {
        let binding = self.gmail_binding(account, store, saved)?;
        if saved.gmail_backlog.runs.len() >= model::HISTORY
            || saved.gmail_backlog.runs.iter().any(|r| !r.settled())
        {
            return Err(invalid());
        }
        let Filing {
            saved_proof,
            topology,
            observed,
            execution,
            plan: mut filing,
            missing,
            connection,
        } = self.gmail_filing(account, store, saved, &binding, options, cancel)?;
        let all = all_mail(&topology).map_err(failure)?.name.clone();
        filing
            .changes
            .retain(|c| !missing.iter().any(|m| m.name == c.destination));
        for m in &missing {
            *filing.excluded.entry("missing_label".into()).or_default() += m.messages;
            filing.diagnostics.push(match m.blocked {
                None => format!("Missing destination label {}: create it with label provisioning first. No label store creates labels.", m.name),
                Some(_) => format!("Destination label {} cannot be created: a label differing only in case, a reserved name or the system root is in the way. Mailroom never renames or reuses a label.", m.name),
            });
        }
        let folder = observed.folders.get(&all.decoded).ok_or_else(invalid)?;
        let identity = MailboxIdentity {
            name: all.clone(),
            uidvalidity: UidValidity::new(folder.uidvalidity).map_err(failure)?,
        };
        let mut grouped: BTreeMap<String, BTreeMap<Uid, Snapshot>> = BTreeMap::new();
        for c in &filing.changes {
            if c.source.folder != all.decoded {
                return Err(invalid());
            }
            let m = folder.messages.get(&c.source.uid).ok_or_else(invalid)?;
            grouped
                .entry(c.destination.clone())
                .or_default()
                .insert(Uid::new(m.uid).map_err(failure)?, snapshot(m)?);
        }
        let mut batches = vec![];
        for (destination, messages) in grouped {
            let label = topology.get(&destination).ok_or_else(invalid)?.name.clone();
            let entries: Vec<_> = messages.into_iter().collect();
            for chunk in entries.chunks(crate::backlog::BATCH_LIMIT) {
                let purpose = Purpose::Filing {
                    protect_flagged: filing.options.protect_flagged,
                };
                let messages = chunk.iter().cloned().collect();
                let intent = if filing.options.intent == crate::backlog::Intent::ArchiveUnmatched {
                    LabelIntent::archive(purpose, identity.clone(), messages)
                } else {
                    LabelIntent::new(purpose, identity.clone(), label.clone(), messages)
                }
                .ok_or_else(invalid)?;
                batches.push(LabelBatch::forward(intent).map_err(failure)?);
            }
        }
        if batches.is_empty() || batches.len() > model::BATCHES {
            return Err(invalid());
        }
        let plan = SavedPlan {
            filing,
            binding,
            topology,
            saved_proof: saved_proof.receipt.clone(),
            execution_proof: store.keys().binding_for(
                b"mailroom/gmail-execution-proof/v1",
                &serde_json::to_vec(&(&observed, &execution)).map_err(failure)?,
            ),
        };
        let receipt = store.keys().binding_for(
            plan.filing.options.intent.purpose(
                b"mailroom/gmail-backlog-start/v2",
                b"mailroom/gmail-backlog-archive/v1",
            ),
            &crate::census::binding::bound(|| serde_json::to_vec(&(saved, &plan, &batches)))
                .map_err(failure)?,
        );
        let id = store
            .keys()
            .binding_for(b"mailroom/gmail-backlog-id/v1", receipt.as_bytes());
        Ok((
            Preview {
                operation: Run {
                    id,
                    receipt: receipt.clone(),
                    plan,
                    status: Status::Prepared,
                    stop_requested: false,
                    batches,
                    reverse: vec![],
                    undo_receipt: None,
                    undo_conflicts: 0,
                    completed_at: None,
                    closed: None,
                },
                receipt,
                saved: saved_proof,
                execution,
                missing_labels: missing,
                limitations: model::LIMITATIONS,
            },
            connection,
        ))
    }

    pub(crate) fn gmail_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        options: Options,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let (p, connection) = self.gmail_prepare(account, store, saved, options, cancel.clone())?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes()))
            || saved
                .gmail_backlog
                .runs
                .iter()
                .any(|r| r.id == p.operation.id)
        {
            return Err(invalid());
        }
        fresh(saved).map_err(failure)?;
        let id = p.operation.id.clone();
        saved.gmail_backlog.runs.push(p.operation);
        save(store, saved)?;
        boundary("gmail_consumed");
        self.gmail_execute(
            account,
            store,
            saved,
            &id,
            false,
            cancel,
            fresh,
            Some(connection),
        )
    }

    pub(crate) fn gmail_run(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        reverse: bool,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        self.gmail_execute(account, store, saved, id, reverse, cancel, fresh, None)
    }
    /// Runs the steps over `session` when the request already holds one for
    /// this source, or over a new one.
    #[allow(clippy::too_many_arguments)]
    fn gmail_execute(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        reverse: bool,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
        session: Option<Connection>,
    ) -> Result<(), AcquisitionError> {
        let i = saved
            .gmail_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        let mut binding = self.gmail_binding(account, store, saved)?;
        if !same_binding(&binding, &saved.gmail_backlog.runs[i].plan.binding, || {
            model::material_legacy(saved, store.keys())
        }) {
            saved.gmail_backlog.runs[i].status = Status::Conflict;
            save(store, saved)?;
            return Err(invalid());
        }
        // Reused for every step, in the form the run was saved with.
        binding.material = saved.gmail_backlog.runs[i].plan.binding.material.clone();
        let run = &saved.gmail_backlog.runs[i];
        if matches!(
            run.status,
            Status::Complete | Status::Undone | Status::Conflict
        ) && !(reverse && run.status == Status::Complete && run.undo_receipt.is_some())
            || reverse != run.undo_receipt.is_some()
            || run.entries(reverse).iter().any(|b| !resolved(b))
        {
            return Err(invalid());
        }
        saved.gmail_backlog.runs[i].stop_requested = false;
        saved.gmail_backlog.runs[i].status = if reverse {
            Status::UndoRunning
        } else {
            Status::Running
        };
        save(store, saved)?;
        let runtime = self.gmail_runtime(account, store, saved, &binding)?;
        let source = self.label_source(account)?;
        let permit = source.runtime_permit(&runtime).map_err(failure)?;
        let password = self.password(account)?;
        let result = (|| {
            let mut connection = match session {
                Some(connection) => connection,
                None => Connection::connect(&permit, &password, cancel.clone()).map_err(failure)?,
            };
            if !compatible(
                &saved.gmail_backlog.runs[i],
                &connection.inventory().map_err(failure)?,
            ) {
                return Err(invalid());
            }
            let count = saved.gmail_backlog.runs[i].entries(reverse).len();
            for n in 0..count {
                let steps = saved.gmail_backlog.runs[i].entries(reverse)[n]
                    .steps()
                    .len();
                for index in 0..steps {
                    let mut batch = saved.gmail_backlog.runs[i].entries(reverse)[n].clone();
                    if batch.steps()[index].attempted() {
                        continue;
                    }
                    if cancel.is_cancelled() {
                        return Err(AcquisitionError::Cancelled);
                    }
                    fresh(saved).map_err(failure)?;
                    if !same_binding(
                        &self.gmail_binding_with(account, store, binding.material.clone())?,
                        &saved.gmail_backlog.runs[i].plan.binding,
                        Vec::new,
                    ) {
                        return Err(invalid());
                    }
                    let kind = kind_name(batch.steps()[index].kind);
                    boundary(&format!("gmail_before_intent_{kind}"));
                    let result = connection.store(&permit, &mut batch, index, |b| {
                        store_batch(saved, i, reverse, n, b);
                        save(store, saved).map_err(|_| Failure::Limit)?;
                        boundary(&format!(
                            "gmail_{}_{kind}",
                            if b.steps()[index].completion().is_some() {
                                "completion"
                            } else {
                                "attempted"
                            }
                        ));
                        Ok(())
                    });
                    if result.is_ok() {
                        let uids = batch.steps()[index].uids.clone();
                        let readback = connection
                            .readback(batch.intent().context(), &uids)
                            .map_err(failure)?;
                        batch.observe(index, readback).map_err(failure)?;
                        store_batch(saved, i, reverse, n, &batch);
                        save(store, saved)?;
                        fresh(saved).map_err(failure)?;
                        boundary(&format!("gmail_saved_readback_{kind}"));
                    }
                    result.map_err(failure)?;
                    if batch
                        .outcomes(index)
                        .values()
                        .any(|o| *o != Outcome::Verified)
                    {
                        return Err(invalid());
                    }
                }
                boundary("gmail_between_batches");
            }
            Ok(())
        })();
        let run = &mut saved.gmail_backlog.runs[i];
        run.status = model::outcome(run, reverse);
        run.stop_requested |= cancel.is_cancelled();
        if result.is_err() && run.status == Status::Stopped && !cancel.is_cancelled() {
            run.status = Status::Conflict;
        }
        run.stamp(self.now());
        save(store, saved)?;
        result
    }

    /// Observes only attempted steps without saved readback. Saved readback is
    /// never replaced, so identical repeated observations write nothing.
    pub(crate) fn gmail_reconcile(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let binding = self.gmail_binding(account, store, saved)?;
        let i = saved
            .gmail_backlog
            .runs
            .iter()
            .position(|r| r.id == id && r.closed.is_none())
            .ok_or_else(invalid)?;
        let conflict = |saved: &mut AccountState| -> Result<(), AcquisitionError> {
            if saved.gmail_backlog.runs[i].status != Status::Conflict {
                saved.gmail_backlog.runs[i].status = Status::Conflict;
                save(store, saved)?;
            }
            Ok(())
        };
        if !same_binding(&binding, &saved.gmail_backlog.runs[i].plan.binding, || {
            model::material_legacy(saved, store.keys())
        }) {
            conflict(saved)?;
            return Err(invalid());
        }
        let before = saved.gmail_backlog.clone();
        let runtime = self.gmail_runtime(account, store, saved, &binding)?;
        let mut connection = self.gmail_connect(account, &runtime, cancel)?;
        if !compatible(
            &saved.gmail_backlog.runs[i],
            &connection.inventory().map_err(failure)?,
        ) {
            conflict(saved)?;
            return Err(invalid());
        }
        let run = &mut saved.gmail_backlog.runs[i];
        let reverse = run.undo_receipt.is_some();
        let mut unstable = false;
        for batch in run.entries_mut(reverse) {
            for index in batch.unobserved() {
                if batch.steps()[index].completion().is_none() {
                    batch
                        .record_completion(index, Completion::Lost)
                        .map_err(failure)?;
                }
                let uids = batch.steps()[index].uids.clone();
                match connection.readback(batch.intent().context(), &uids) {
                    Ok(readback) => batch.observe(index, readback).map_err(failure)?,
                    Err(Failure::Unstable) => unstable = true,
                    Err(error) => return Err(failure(error)),
                }
            }
        }
        if unstable {
            conflict(saved)?;
            return Err(invalid());
        }
        let run = &mut saved.gmail_backlog.runs[i];
        run.status = model::outcome(run, reverse);
        run.stamp(self.now());
        if saved.gmail_backlog != before {
            save(store, saved)?;
        }
        Ok(())
    }

    pub(crate) fn gmail_undo_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<UndoPreview, AcquisitionError> {
        let binding = self.gmail_binding(account, store, saved)?;
        let run = saved
            .gmail_backlog
            .runs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(invalid)?;
        if !same_binding(&binding, &run.plan.binding, || {
            model::material_legacy(saved, store.keys())
        }) || run.undo_receipt.is_some()
            || run.closed.is_some()
            || run.batches.iter().any(|b| !b.observed())
            || !crate::backlog::retention::undo_open(run.completed_at, self.now())
        {
            return Err(invalid());
        }
        let runtime = self.gmail_runtime(account, store, saved, &binding)?;
        let mut connection = self.gmail_connect(account, &runtime, cancel)?;
        let compatible = compatible(run, &connection.inventory().map_err(failure)?);
        let mut reverse = vec![];
        let mut conflicts = 0;
        for (forward, b) in run.batches.iter().enumerate() {
            if !b.attempted() {
                continue;
            }
            let outcomes: Vec<_> = (0..b.steps().len())
                .map(|i| (b.steps()[i].kind, b.outcomes(i)))
                .collect();
            let applied = |kind: StepKind, uid: &Uid| {
                outcomes
                    .iter()
                    .any(|(k, o)| *k == kind && o.get(uid).is_some_and(|o| o.applied()))
            };
            let mut candidates = BTreeSet::new();
            for uid in b.intent().messages().keys() {
                let all: Vec<_> = outcomes.iter().filter_map(|(_, o)| o.get(uid)).collect();
                if all
                    .iter()
                    .any(|o| matches!(o, Outcome::Uncertain | Outcome::Conflict))
                {
                    conflicts += 1;
                } else if all.iter().any(|o| o.applied()) {
                    candidates.insert(*uid);
                }
            }
            if candidates.is_empty() {
                continue;
            }
            if !compatible {
                conflicts += candidates.len();
                continue;
            }
            let current = match connection.readback(b.intent().context(), &candidates) {
                Ok(value) => value,
                Err(Failure::Unstable) => {
                    conflicts += candidates.len();
                    continue;
                }
                Err(error) => return Err(failure(error)),
            };
            let label = &b.intent().destination().decoded;
            let (mut messages, mut restore, mut remove) =
                (BTreeMap::new(), BTreeSet::new(), BTreeSet::new());
            for uid in &candidates {
                let Some(now) = current
                    .messages
                    .get(uid)
                    .filter(|m| m.same_message(&b.intent().messages()[uid]))
                else {
                    conflicts += 1;
                    continue;
                };
                let back = applied(StepKind::RemoveInbox, uid) && !now.labels.contains(INBOX);
                let off = applied(StepKind::AddLabel, uid) && now.labels.contains(label);
                if !back && !off {
                    conflicts += 1;
                    continue;
                }
                if back {
                    restore.insert(*uid);
                }
                if off {
                    remove.insert(*uid);
                }
                messages.insert(*uid, now.clone());
            }
            if messages.is_empty() {
                continue;
            }
            let intent = if b.intent().archive_only() {
                LabelIntent::archive(
                    Purpose::ConditionalUndo,
                    b.intent().context().clone(),
                    messages,
                )
            } else {
                LabelIntent::new(
                    Purpose::ConditionalUndo,
                    b.intent().context().clone(),
                    b.intent().destination().clone(),
                    messages,
                )
            }
            .ok_or_else(invalid)?;
            reverse.push(Reverse {
                forward,
                batch: LabelBatch::reverse(intent, restore, remove).map_err(failure)?,
            });
        }
        let receipt = store.keys().binding_for(
            b"mailroom/gmail-backlog-undo/v2",
            &crate::census::binding::bound(|| {
                serde_json::to_vec(&(saved, &binding, &reverse, conflicts))
            })
            .map_err(failure)?,
        );
        Ok(UndoPreview {
            run: id.into(),
            reverse,
            conflicts,
            receipt,
            limitations: model::LIMITATIONS,
        })
    }

    pub(crate) fn gmail_undo(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let p = self.gmail_undo_preview(account, store, saved, id, cancel.clone())?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes())) || p.reverse.is_empty() {
            return Err(invalid());
        }
        let run = saved
            .gmail_backlog
            .runs
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(invalid)?;
        run.reverse = p.reverse;
        run.undo_receipt = Some(p.receipt);
        run.undo_conflicts = p.conflicts;
        if run.status != Status::Complete {
            run.status = model::outcome(run, true);
        }
        run.stamp(self.now());
        save(store, saved)?;
        boundary("gmail_undo_consumed");
        self.gmail_run(account, store, saved, id, true, cancel, fresh)
    }
}

impl Accounts {
    /// See `backlog_close_preview`.
    pub(crate) fn gmail_close_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<ClosePreview, AcquisitionError> {
        let binding = self.gmail_binding(account, store, saved)?;
        let run = saved
            .gmail_backlog
            .runs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(invalid)?;
        if !run.closable() {
            return Err(invalid());
        }
        let reason = if run.undo_receipt.is_some() {
            CloseReason::UndoEnded
        } else if !same_binding(&binding, &run.plan.binding, || {
            model::material_legacy(saved, store.keys())
        }) || !crate::backlog::retention::undo_open(run.completed_at, self.now())
        {
            CloseReason::UndoUnavailable
        } else if self
            .gmail_undo_preview(account, store, saved, id, cancel)?
            .reverse
            .is_empty()
        {
            CloseReason::NothingToUndo
        } else {
            return Err(invalid());
        };
        super::backlog::close_preview(store, saved, "gmail-labels", id, reason)
    }
    pub(crate) fn gmail_close(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        receipt: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let p = self.gmail_close_preview(account, store, saved, id, cancel)?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        let run = saved
            .gmail_backlog
            .runs
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(invalid)?;
        run.closed = Some(Closure {
            at: self.now(),
            receipt: p.receipt,
        });
        save(store, saved)
    }
}

#[cfg(test)]
thread_local! {
    /// A test's stand-in for a crash: runs at the named boundary on the
    /// thread that reaches it, for example to capture both stores there.
    pub(crate) static AT_BOUNDARY: std::cell::RefCell<Option<(String, Box<dyn FnMut()>)>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if name == "gmail_between_batches" {
        super::backlog::stop_between_batches();
    }
    #[cfg(test)]
    AT_BOUNDARY.with(|hook| {
        if let Some((at, action)) = hook.borrow_mut().as_mut() {
            if at == name {
                action();
            }
        }
    });
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MAILROOM_GMAIL_FIXTURE") {
        let root = std::path::Path::new(&root);
        if std::fs::read_to_string(root.join("gmail-boundary")).is_ok_and(|s| s == name) {
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
pub(crate) fn fixture_install(
    root: &std::path::Path,
) -> (
    Accounts,
    Account,
    super::folder_provisioning::transport::gmail_label::tests::Fake,
) {
    super::folder_provisioning::transport::gmail_label::tests::workflow_install(root)
}

#[cfg(test)]
mod tests;

/// Writes an independent view of the fake and the primary journal for the
/// HTTP and browser verifiers. An action first edits the fake as another
/// client would.
#[cfg(test)]
#[test]
fn gmail_workflow_observer() {
    use super::folder_provisioning::transport::gmail_label::tests::{self as fake, ALL};
    let Some(root) = std::env::var_os("MAILROOM_GMAIL_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let repo = super::persistent_test_accounts(root);
    let id =
        AccountId::new(&std::fs::read_to_string(root.join("gmail-account-id")).unwrap()).unwrap();
    let action = std::env::var("MAILROOM_GMAIL_ACTION").unwrap_or_default();
    if !action.is_empty() && action != "provider_only" {
        fake::other_client(&repo, &id, root, &action);
    }
    let (_, provider) = fake::external(&repo, &id, root);
    let digest =
        |value: &serde_json::Value| crate::unsubscribe::digest(&serde_json::to_vec(value).unwrap());
    let view = |name: &str| {
        provider.census.folders.get(name).map(|f| {
            serde_json::json!({"uidvalidity":f.uidvalidity,"messages":f.messages.iter().map(|(uid,m)| (m.gmail_message_id.clone().unwrap(), serde_json::json!({"uid":uid,"labels":m.gmail_labels,"flags":m.flags}))).collect::<BTreeMap<_,_>>()})
        })
    };
    let mut independent = serde_json::to_value(&provider).unwrap();
    for f in ["revision", "census", "synthetic_uid_next"] {
        independent.as_object_mut().unwrap().remove(f);
    }
    independent["synthetic_folders"]
        .as_object_mut()
        .unwrap()
        .remove("commands");
    let p = provider.synthetic_folders.as_ref().unwrap();
    let mut value = serde_json::json!({
        "account": id,
        "all_mail": view(ALL),
        "inbox": view("INBOX"),
        "mailboxes": p.inventory.mailboxes.iter().map(|m| &m.name.decoded).collect::<Vec<_>>(),
        "capabilities": p.inventory.capabilities,
        "logins": p.commands.iter().filter(|c| *c == "LOGIN").count(),
        "independent": digest(&independent),
        "uid_next": provider.synthetic_uid_next,
        "wires": std::fs::read_to_string(root.join("gmail-wire-exact.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).collect::<Vec<_>>(),
        "creates": std::fs::read_to_string(root.join("gmail-create-exact.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).collect::<Vec<_>>(),
        "forbidden": std::fs::read_to_string(root.join("gmail-forbidden")).ok(),
    });
    if action != "provider_only" {
        let primary = std::env::var_os("MAILROOM_GMAIL_INSTALLATION")
            .map(|p| super::persistent_test_accounts(std::path::Path::new(&p)));
        let primary = primary.as_ref().unwrap_or(&repo);
        let account = primary.load(&id).unwrap();
        let store = primary.store(&account).unwrap();
        let saved = store.load_state().unwrap();
        value["primary"] = digest(&serde_json::to_value(&saved).unwrap()).into();
        value["ciphertext"] =
            crate::unsubscribe::digest(&std::fs::read(store.path()).unwrap()).into();
        let mut canonical = serde_json::to_value(&saved).unwrap();
        for f in ["revision", "gmail_backlog", "label_provisioning"] {
            canonical.as_object_mut().unwrap().remove(f);
        }
        value["unrelated"] = digest(&canonical).into();
        canonical.as_object_mut().unwrap().remove("census");
        value["non_census"] = digest(&canonical).into();
        value["recovery_hold"] = store.recovery_pending().into();
        value["revision"] = saved.revision.into();
        value["schema"] = store.schema_version().unwrap().into();
        value["journal"] = serde_json::to_value(&saved.gmail_backlog).unwrap();
        value["labels"] = serde_json::to_value(&saved.label_provisioning).unwrap();
        value["exports"] = serde_json::to_value(&saved.exports).unwrap();
        value["saved_census"] = serde_json::to_value(
            saved
                .census
                .folders
                .get(ALL)
                .map(|f| {
                    f.messages
                        .values()
                        .map(|m| (m.gmail_message_id.clone().unwrap(), &m.gmail_labels))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default(),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("gmail-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}
