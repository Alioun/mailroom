use super::{
    Account, Accounts, AcquisitionError, Cancellation,
    folder_provisioning::transport::message_move::{Connection, InstalledSource, RuntimeBinding},
    imap::{MailRead, ReadOnlyImap},
};
use crate::{
    backlog::{
        Options,
        imap_move::*,
        protocol::{self as model, Binding, Preview, Reverse, Run, SavedPlan, Status, UndoPreview},
    },
    census::{AccountId, AccountState, Census, EncryptedStore, folder_provisioning::Inventory},
    rules::{proof, workspace},
};
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;

fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn failure<T: std::fmt::Debug>(_error: T) -> AcquisitionError {
    #[cfg(test)]
    eprintln!("backlog.failure={}", super::diagnostic_code(&_error));
    AcquisitionError::Protocol
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    store.save_state(state).map_err(Into::into)
}
impl Accounts {
    pub(crate) fn backlog_installed(&self, id: Option<&AccountId>) -> bool {
        self.backlog_source.as_ref().is_some_and(|s| {
            s.authority == "fixture-message-move/v1"
                && s.address.ip().is_loopback()
                && id.is_none_or(|id| *id == s.account)
        })
    }
    fn backlog_source(&self, account: &Account) -> Result<&InstalledSource, AcquisitionError> {
        let s = self.backlog_source.as_deref().ok_or_else(invalid)?;
        if s.account != account.id
            || s.login != account.login
            || account.provider != super::Provider::MailboxOrg
        {
            return Err(invalid());
        }
        Ok(s)
    }
    fn backlog_binding(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
    ) -> Result<Binding, AcquisitionError> {
        let material = model::material(saved, store.keys()).map_err(failure)?;
        self.backlog_binding_with(account, store, material)
    }
    /// The binding with an already computed material digest. The digest covers
    /// the whole account state, census included, so a run computes it once and
    /// reuses it while it changes only its own journal.
    fn backlog_binding_with(
        &self,
        account: &Account,
        store: &EncryptedStore,
        material: String,
    ) -> Result<Binding, AcquisitionError> {
        let s = self.backlog_source(account)?;
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
    fn backlog_runtime(
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
                b"mailroom/imap-topology-profile/v1",
                &serde_json::to_vec(&(
                    crate::census::folder_provisioning::PROFILE,
                    &saved.census.folder_roles,
                ))
                .map_err(failure)?,
            ),
        })
    }
    /// Observes every folder over one MOVE session, which stays open for
    /// the request, and one read-only session. The topology must not change
    /// while the folders are read, and every folder must still be writable
    /// under the UIDVALIDITY it was read with.
    fn backlog_observe(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        binding: &Binding,
        cancel: Cancellation,
    ) -> Result<(Inventory, Census, Connection), AcquisitionError> {
        let runtime = self.backlog_runtime(account, store, saved, binding)?;
        let source = self.backlog_source(account)?;
        let permit = source.runtime_permit(&runtime).map_err(failure)?;
        let password = self.password(account)?;
        let mut connection =
            Connection::connect(&permit, &password, cancel.clone()).map_err(failure)?;
        let before = connection.inventory().map_err(failure)?;
        let read = crate::policy::read_permit(account.provider)?;
        let mut reader =
            ReadOnlyImap::loopback(&permit, &read, &account.login, &password, cancel.clone())?;
        let folders = reader.folders()?;
        for folder in &folders {
            if !folder.selectable
                || before
                    .get(&folder.name)
                    .is_none_or(|m| m.name.wire != folder.wire)
            {
                return Err(invalid());
            }
        }
        let observed = reader.observe(&folders, &saved.census)?;
        drop(reader);
        let census = saved.census.observed(
            folders
                .iter()
                .map(|f| (f.name.clone(), f.roles.clone()))
                .collect(),
            observed,
        );
        if census.folders.len() != before.mailboxes.len() {
            return Err(invalid());
        }
        if connection.inventory().map_err(failure)? != before {
            return Err(invalid());
        }
        let identities = census
            .folders
            .iter()
            .map(|(name, folder)| {
                Ok(MailboxIdentity {
                    name: before.get(name).ok_or_else(invalid)?.name.clone(),
                    uidvalidity: UidValidity::new(folder.uidvalidity).map_err(failure)?,
                })
            })
            .collect::<Result<Vec<_>, AcquisitionError>>()?;
        let none = BTreeSet::new();
        let requests: Vec<_> = identities.iter().map(|i| (i, &none)).collect();
        for readback in connection.readbacks(&requests).map_err(failure)? {
            readback.map_err(failure)?;
        }
        Ok((before, census, connection))
    }
    pub(crate) fn backlog_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: Options,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        self.backlog_prepare(account, store, saved, options, cancel)
            .map(|(preview, _)| preview)
    }
    /// The preview and the MOVE session it observed through, which a confirm
    /// goes on to use.
    fn backlog_prepare(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        options: Options,
        cancel: Cancellation,
    ) -> Result<(Preview, Connection), AcquisitionError> {
        let binding = self.backlog_binding(account, store, saved)?;
        if saved.protocol_backlog.runs.len() >= model::HISTORY
            || saved.protocol_backlog.runs.iter().any(|r| !r.finished())
        {
            return Err(invalid());
        }
        let context = self.workspace_context(account, store)?;
        let saved_proof = workspace::preview(saved, &context, store.keys()).map_err(failure)?;
        if !saved_proof.download_available {
            return Err(invalid());
        }
        let (topology, observed, connection) =
            self.backlog_observe(account, store, saved, &binding, cancel)?;
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
        let filing = model::project(saved, &observed, &execution, options).map_err(failure)?;
        #[cfg(test)]
        if let Some(root) = std::env::var_os("MAILROOM_MOVE_FIXTURE") {
            std::fs::write(
                std::path::Path::new(&root).join("move-preview-debug.json"),
                serde_json::to_vec(
                    &serde_json::json!({"topology":topology,"execution":execution,"filing":filing}),
                )
                .unwrap(),
            )
            .unwrap();
        }
        let mut grouped: BTreeMap<(String, String), BTreeMap<Uid, Metadata>> = BTreeMap::new();
        for c in &filing.changes {
            let m = &observed.folders[&c.source.folder].messages[&c.source.uid];
            grouped
                .entry((c.source.folder.clone(), c.destination.clone()))
                .or_default()
                .insert(
                    Uid::new(m.uid).map_err(failure)?,
                    Metadata {
                        message_id_hash: c.source.message_id_hash.clone(),
                        size: u32::try_from(m.size).map_err(failure)?,
                        internal_date: m.internal_date.clone(),
                        flags: m.flags.clone(),
                    },
                );
        }
        let mut batches = vec![];
        for ((src, dst), messages) in grouped {
            let identity = |name: &str| -> Result<MailboxIdentity, AcquisitionError> {
                Ok(MailboxIdentity {
                    name: topology
                        .get(name)
                        .filter(|m| m.selectable())
                        .ok_or_else(invalid)?
                        .name
                        .clone(),
                    uidvalidity: UidValidity::new(
                        observed.folders.get(name).ok_or_else(invalid)?.uidvalidity,
                    )
                    .map_err(failure)?,
                })
            };
            let entries: Vec<_> = messages.into_iter().collect();
            for chunk in entries.chunks(crate::backlog::BATCH_LIMIT) {
                batches.push(MoveEvidence::intent(
                    MoveIntent::new(
                        Purpose::Filing {
                            protect_flagged: filing.options.protect_flagged,
                        },
                        identity(&src)?,
                        identity(&dst)?,
                        chunk.iter().cloned().collect(),
                    )
                    .map_err(failure)?,
                ));
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
                b"mailroom/imap-execution-proof/v1",
                &serde_json::to_vec(&(&observed, &execution)).map_err(failure)?,
            ),
        };
        let receipt = store.keys().binding_for(
            b"mailroom/imap-backlog-start/v2",
            &crate::census::binding::bound(|| serde_json::to_vec(&(saved, &plan, &batches)))
                .map_err(failure)?,
        );
        let id = store
            .keys()
            .binding_for(b"mailroom/imap-backlog-id/v1", receipt.as_bytes());
        let operation = Run {
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
        };
        Ok((
            Preview {
                operation,
                receipt,
                saved: saved_proof,
                execution,
                limitations: model::LIMITATIONS,
            },
            connection,
        ))
    }
    pub(crate) fn backlog_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        options: Options,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let (p, connection) =
            self.backlog_prepare(account, store, saved, options, cancel.clone())?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes()))
            || saved
                .protocol_backlog
                .runs
                .iter()
                .any(|r| r.id == p.operation.id)
        {
            return Err(invalid());
        }
        fresh(saved).map_err(failure)?;
        let id = p.operation.id.clone();
        saved.protocol_backlog.runs.push(p.operation);
        save(store, saved)?;
        boundary("move_consumed");
        self.backlog_execute(
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
    pub(crate) fn backlog_run(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        reverse: bool,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        self.backlog_execute(account, store, saved, id, reverse, cancel, fresh, None)
    }
    /// Runs the batches over `session` when the request already holds one
    /// for this source, or over a new one.
    #[allow(clippy::too_many_arguments)]
    fn backlog_execute(
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
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        let mut binding = self.backlog_binding(account, store, saved)?;
        if !model::same_binding(
            &binding,
            &saved.protocol_backlog.runs[i].plan.binding,
            || model::material_v1(saved, store.keys()),
        ) {
            saved.protocol_backlog.runs[i].status = Status::Conflict;
            save(store, saved)?;
            return Err(invalid());
        }
        // Reused for every batch, in the form the run was saved with.
        binding.material = saved.protocol_backlog.runs[i].plan.binding.material.clone();
        if matches!(
            saved.protocol_backlog.runs[i].status,
            Status::Complete | Status::Undone | Status::Conflict
        ) && !(reverse
            && saved.protocol_backlog.runs[i].status == Status::Complete
            && saved.protocol_backlog.runs[i].undo_receipt.is_some())
            || (!reverse && saved.protocol_backlog.runs[i].undo_receipt.is_some())
        {
            return Err(invalid());
        }
        let entries: Vec<_> = if reverse {
            saved.protocol_backlog.runs[i]
                .reverse
                .iter()
                .map(|b| &b.evidence)
                .collect()
        } else {
            saved.protocol_backlog.runs[i].batches.iter().collect()
        };
        if entries.iter().any(|b| b.attempted() && !model::verified(b))
            || (reverse && saved.protocol_backlog.runs[i].undo_receipt.is_none())
        {
            return Err(invalid());
        }
        saved.protocol_backlog.runs[i].stop_requested = false;
        saved.protocol_backlog.runs[i].status = if reverse {
            Status::UndoRunning
        } else {
            Status::Running
        };
        save(store, saved)?;
        let runtime = self.backlog_runtime(account, store, saved, &binding)?;
        let source = self.backlog_source(account)?;
        let permit = source.runtime_permit(&runtime).map_err(failure)?;
        let password = self.password(account)?;
        let result = (|| {
            let mut connection = match session {
                Some(connection) => connection,
                None => Connection::connect(&permit, &password, cancel.clone()).map_err(failure)?,
            };
            if connection.inventory().map_err(failure)?
                != saved.protocol_backlog.runs[i].plan.topology
            {
                return Err(invalid());
            }
            let count = if reverse {
                saved.protocol_backlog.runs[i].reverse.len()
            } else {
                saved.protocol_backlog.runs[i].batches.len()
            };
            for n in 0..count {
                let mut evidence = if reverse {
                    saved.protocol_backlog.runs[i].reverse[n].evidence.clone()
                } else {
                    saved.protocol_backlog.runs[i].batches[n].clone()
                };
                if evidence.attempted() {
                    continue;
                }
                if cancel.is_cancelled() {
                    return Err(AcquisitionError::Cancelled);
                }
                fresh(saved).map_err(failure)?;
                if !model::same_binding(
                    &self.backlog_binding_with(account, store, binding.material.clone())?,
                    &saved.protocol_backlog.runs[i].plan.binding,
                    || None,
                ) {
                    return Err(invalid());
                }
                boundary("move_before_intent");
                let result = connection.move_messages(&permit, &mut evidence, |e| {
                    if e.mapping().is_some() && e.completion().is_none() {
                        boundary("move_parsed_mapping");
                    }
                    if reverse {
                        saved.protocol_backlog.runs[i].reverse[n].evidence = e.clone();
                    } else {
                        saved.protocol_backlog.runs[i].batches[n] = e.clone();
                    }
                    save(store, saved)
                        .map_err(|_| crate::census::folder_provisioning::Failure::Limit)?;
                    boundary(if e.mapping().is_some() && e.completion().is_none() {
                        "move_saved_mapping"
                    } else if e.completion().is_some() {
                        "move_completion"
                    } else {
                        "move_attempted"
                    });
                    Ok(())
                });
                if result.is_ok() {
                    observe(&mut connection, &mut evidence).map_err(failure)?;
                    boundary("move_readback");
                    if reverse {
                        saved.protocol_backlog.runs[i].reverse[n].evidence = evidence.clone();
                    } else {
                        saved.protocol_backlog.runs[i].batches[n] = evidence.clone();
                    }
                    save(store, saved)?;
                    fresh(saved).map_err(failure)?;
                    boundary("move_saved_readback");
                }
                result.map_err(failure)?;
                if !model::verified(&evidence) {
                    return Err(invalid());
                }
                boundary("move_between_batches");
            }
            Ok(())
        })();
        let run = &mut saved.protocol_backlog.runs[i];
        run.status = model::outcome(run, reverse);
        run.stop_requested |= cancel.is_cancelled();
        if result.is_err() && run.status == Status::Stopped && !cancel.is_cancelled() {
            run.status = Status::Conflict;
        }
        run.stamp(self.now());
        save(store, saved)?;
        result
    }
    pub(crate) fn backlog_reconcile(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let binding = self.backlog_binding(account, store, saved)?;
        let i = saved
            .protocol_backlog
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(invalid)?;
        if !model::same_binding(
            &binding,
            &saved.protocol_backlog.runs[i].plan.binding,
            || model::material_v1(saved, store.keys()),
        ) {
            if saved.protocol_backlog.runs[i].status != Status::Conflict {
                saved.protocol_backlog.runs[i].status = Status::Conflict;
                save(store, saved)?;
            }
            return Err(invalid());
        }
        let before = saved.protocol_backlog.clone();
        let runtime = self.backlog_runtime(account, store, saved, &binding)?;
        let permit = self
            .backlog_source(account)?
            .runtime_permit(&runtime)
            .map_err(failure)?;
        let mut connection =
            Connection::connect(&permit, &self.password(account)?, cancel).map_err(failure)?;
        if connection.inventory().map_err(failure)? != saved.protocol_backlog.runs[i].plan.topology
        {
            if saved.protocol_backlog.runs[i].status != Status::Conflict {
                saved.protocol_backlog.runs[i].status = Status::Conflict;
                save(store, saved)?;
            }
            return Err(invalid());
        }
        let run = &mut saved.protocol_backlog.runs[i];
        let entries: Vec<_> = if run.undo_receipt.is_some() {
            run.reverse.iter_mut().map(|r| &mut r.evidence).collect()
        } else {
            run.batches.iter_mut().collect()
        };
        for e in entries {
            if e.attempted() && !e.observed() {
                if e.completion().is_none() {
                    e.record_completion(Completion::Lost).map_err(failure)?;
                }
                if let Err(error) = observe(&mut connection, e) {
                    if error == crate::census::folder_provisioning::Failure::Unstable
                        && run.status != Status::Conflict
                    {
                        run.status = Status::Conflict;
                        save(store, saved)?;
                    }
                    return Err(failure(error));
                }
            }
        }
        run.status = model::outcome(run, run.undo_receipt.is_some());
        run.stamp(self.now());
        if saved.protocol_backlog != before {
            save(store, saved)?;
        }
        Ok(())
    }
    pub(crate) fn backlog_undo_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        id: &str,
        cancel: Cancellation,
    ) -> Result<UndoPreview, AcquisitionError> {
        let binding = self.backlog_binding(account, store, saved)?;
        let run = saved
            .protocol_backlog
            .runs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(invalid)?;
        if !model::same_binding(&binding, &run.plan.binding, || {
            model::material_v1(saved, store.keys())
        }) || run.undo_receipt.is_some()
            || run.batches.iter().any(|b| b.attempted() && !b.observed())
            || !crate::backlog::retention::undo_open(run.completed_at, self.now())
        {
            return Err(invalid());
        }
        let runtime = self.backlog_runtime(account, store, saved, &binding)?;
        let permit = self
            .backlog_source(account)?
            .runtime_permit(&runtime)
            .map_err(failure)?;
        let mut connection =
            Connection::connect(&permit, &self.password(account)?, cancel).map_err(failure)?;
        let compatible_topology = connection.inventory().map_err(failure)? == run.plan.topology;
        let mut reverse = vec![];
        let mut conflicts = 0;
        // Every batch's current destination and source identity, read in
        // one flight.
        let mut candidates = vec![];
        for (forward, e) in run.batches.iter().enumerate() {
            if !e.attempted() {
                continue;
            }
            let verified: BTreeMap<Uid, Uid> = e
                .resolutions()
                .into_iter()
                .filter_map(|(uid, r)| match r {
                    Resolution::Verified { destination } => Some((uid, destination)),
                    _ => None,
                })
                .collect();
            if !compatible_topology || verified.is_empty() {
                conflicts += e.command().messages().len();
                continue;
            }
            let uids: BTreeSet<Uid> = verified.values().copied().collect();
            candidates.push((forward, e, verified, uids));
        }
        let none = BTreeSet::new();
        let requests: Vec<_> = candidates
            .iter()
            .flat_map(|(_, e, _, uids)| {
                [
                    (e.command().destination(), uids),
                    (e.command().source(), &none),
                ]
            })
            .collect();
        let mut readbacks = connection
            .readbacks(&requests)
            .map_err(failure)?
            .into_iter();
        for (forward, e, verified, _) in &candidates {
            let (current, original) = (readbacks.next(), readbacks.next());
            let unstable = crate::census::folder_provisioning::Failure::Unstable;
            let current = match current.ok_or_else(invalid)? {
                Ok(value) => value,
                Err(error) if error == unstable => {
                    conflicts += e.command().messages().len();
                    continue;
                }
                Err(error) => return Err(failure(error)),
            };
            let original = match original.ok_or_else(invalid)? {
                Ok(value) => value,
                Err(error) if error == unstable => {
                    conflicts += e.command().messages().len();
                    continue;
                }
                Err(error) => return Err(failure(error)),
            };
            if original.identity != *e.command().source() {
                conflicts += e.command().messages().len();
                continue;
            }
            let messages = verified
                .iter()
                .filter_map(|(a, b)| {
                    current
                        .messages
                        .get(b)
                        .filter(|m| {
                            m.message_id_hash == e.command().messages()[a].message_id_hash
                                && m.size == e.command().messages()[a].size
                                && m.internal_date == e.command().messages()[a].internal_date
                        })
                        .map(|m| (*b, m.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            conflicts += e.command().messages().len() - messages.len();
            if messages.is_empty() {
                continue;
            }
            reverse.push(Reverse {
                forward: *forward,
                evidence: MoveEvidence::intent(
                    MoveIntent::new(
                        Purpose::ConditionalUndo,
                        current.identity,
                        original.identity,
                        messages,
                    )
                    .map_err(failure)?,
                ),
            });
        }
        let receipt = store.keys().binding_for(
            b"mailroom/imap-backlog-undo/v2",
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
    pub(crate) fn backlog_undo(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        id: &str,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let p = self.backlog_undo_preview(account, store, saved, id, cancel.clone())?;
        if !bool::from(p.receipt.as_bytes().ct_eq(receipt.as_bytes())) || p.reverse.is_empty() {
            return Err(invalid());
        }
        let run = saved
            .protocol_backlog
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
        boundary("move_undo_consumed");
        self.backlog_run(account, store, saved, id, true, cancel, fresh)
    }
}
fn observe(
    connection: &mut Connection,
    e: &mut MoveEvidence,
) -> Result<(), crate::census::folder_provisioning::Failure> {
    use crate::census::folder_provisioning::Failure;
    let requested = e.command().messages().keys().copied().collect();
    let mapped = e
        .mapping()
        .map(|m| m.pairs().values().copied().collect())
        .unwrap_or_default();
    let mut readbacks = connection
        .readbacks(&[
            (e.command().source(), &requested),
            (e.command().destination(), &mapped),
        ])?
        .into_iter();
    let source = readbacks.next().unwrap_or(Err(Failure::Protocol))?;
    let destination = readbacks.next().unwrap_or(Err(Failure::Protocol))?;
    e.observe(source, destination)
        .map_err(|_| Failure::Protocol)
}
#[cfg(test)]
thread_local! {
    /// A measured personal-scale run stops between batches after this time,
    /// the way Cancel stops it, so one process stays within its time limit and
    /// the next resumes the remainder.
    pub(crate) static STOP_BETWEEN_BATCHES: std::cell::RefCell<Option<(std::time::Instant, Cancellation)>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn stop_between_batches() {
    STOP_BETWEEN_BATCHES.with(|stop| {
        if let Some((deadline, cancel)) = stop.borrow().as_ref() {
            if std::time::Instant::now() > *deadline {
                cancel.cancel();
            }
        }
    });
}
pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if name == "move_between_batches" {
        stop_between_batches();
    }
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MAILROOM_MOVE_FIXTURE") {
        let root = std::path::Path::new(&root);
        if std::fs::read_to_string(root.join("move-boundary")).is_ok_and(|s| s == name) {
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
    super::folder_provisioning::transport::message_move::tests::Fake,
) {
    super::folder_provisioning::transport::message_move::tests::workflow_install(root)
}

/// A second repository over the same fixture installation whose installed
/// authority differs from the original in exactly one leaf.
#[cfg(test)]
pub(crate) fn fixture_mutated(root: &std::path::Path, original: &Accounts, leaf: &str) -> Accounts {
    let mut repo = super::persistent_test_accounts(root);
    let mut source = (**original.backlog_source.as_ref().unwrap()).clone();
    let account = original.load(&source.account).unwrap();
    let mut epoch = std::fs::read_to_string(root.join("workspace-authority")).unwrap();
    match leaf {
        "none" => {}
        "account" => source.account = AccountId::new("mutated-account").unwrap(),
        "login" => source.login.push_str(".mutated"),
        "address_remote" => source.address = "192.0.2.10:993".parse().unwrap(),
        "address_port" => {
            let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            source.address = unused.local_addr().unwrap();
        }
        "tls_name" => source.tls_name = "mutated.example.test".into(),
        "root" => {
            let key = rcgen::KeyPair::generate().unwrap();
            source.root = rcgen::CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .self_signed(&key)
                .unwrap()
                .der()
                .to_vec();
        }
        "epoch" => source.epoch.push_str("-mutated"),
        "authority" => source.authority = "fixture-message-move/v2".into(),
        "workspace_epoch" => epoch.push_str("-mutated"),
        "workspace_absent" => epoch.clear(),
        _ => unreachable!("{leaf}"),
    }
    repo.backlog_source = Some(std::sync::Arc::new(source));
    if !epoch.is_empty() {
        repo.install_synthetic_workspace(&account, &epoch);
    }
    repo
}

#[cfg(test)]
mod tests;
#[cfg(test)]
#[test]
fn workflow_observer() {
    let Some(root) = std::env::var_os("MAILROOM_MOVE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let repo = super::persistent_test_accounts(root);
    let account = if let Ok(id) = std::fs::read_to_string(root.join("move-account-id")) {
        repo.load(&AccountId::new(&id).unwrap()).unwrap()
    } else {
        repo.list().unwrap().remove(0)
    };
    let action = std::env::var("MAILROOM_MOVE_ACTION").unwrap_or_default();
    let external = repo.fixture_sieve_store(&account.id, root).unwrap();
    let mut provider = external.load_state().unwrap();
    if action == "later_flag" {
        provider
            .census
            .folders
            .get_mut("Personal/Studio")
            .unwrap()
            .messages
            .values_mut()
            .find(|m| m.uid != 40)
            .unwrap()
            .flags
            .insert("$later-client".into());
        external.save_state(&mut provider).unwrap();
    }
    if action == "epoch" {
        provider
            .synthetic_folders
            .as_mut()
            .unwrap()
            .epoch
            .push_str("-changed");
        external.save_state(&mut provider).unwrap();
    }
    if action == "moved_away" {
        let folder = provider.census.folders.get_mut("Personal/Studio").unwrap();
        folder.live_uids.remove(&60);
        folder.messages.remove(&60);
        external.save_state(&mut provider).unwrap();
    }
    if action == "original_validity" {
        provider
            .census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .uidvalidity += 1;
        external.save_state(&mut provider).unwrap();
    }
    if action == "destination_validity" {
        provider
            .census
            .folders
            .get_mut("Personal/Studio")
            .unwrap()
            .uidvalidity += 1;
        external.save_state(&mut provider).unwrap();
    }
    if matches!(action.as_str(), "no_move" | "no_uidplus") {
        provider
            .synthetic_folders
            .as_mut()
            .unwrap()
            .inventory
            .capabilities
            .remove(if action == "no_move" {
                "MOVE"
            } else {
                "UIDPLUS"
            });
        external.save_state(&mut provider).unwrap();
    }
    if action == "missing_destination" {
        provider
            .synthetic_folders
            .as_mut()
            .unwrap()
            .inventory
            .mailboxes
            .retain(|m| m.name.decoded != "Personal/Studio");
        provider.census.folders.remove("Personal/Studio");
        external.save_state(&mut provider).unwrap();
    }
    if action == "special_destination" {
        let name = "Personal/Ü & \"quotes\" \\backslash";
        let folder = provider.census.folders.remove("Personal/Studio").unwrap();
        provider.census.folders.insert(name.into(), folder);
        let next = provider
            .synthetic_uid_next
            .remove("Personal/Studio")
            .unwrap();
        provider.synthetic_uid_next.insert(name.into(), next);
        provider
            .synthetic_folders
            .as_mut()
            .unwrap()
            .inventory
            .mailboxes
            .iter_mut()
            .find(|m| m.name.decoded == "Personal/Studio")
            .unwrap()
            .name = crate::census::folder_provisioning::ExactName::new(name).unwrap();
        external.save_state(&mut provider).unwrap();
    }
    if action == "scope_mailboxes" {
        let message = provider.census.folders["INBOX"].messages[&4].clone();
        for (index, (name, roles)) in [
            ("Archive evidenced", vec!["\\Archive"]),
            ("Sent observed", vec!["\\Sent"]),
            ("Drafts observed", vec!["\\Drafts"]),
            ("Spam observed", vec!["\\Junk"]),
            ("Trash observed", vec!["\\Trash"]),
            ("Mixed observed", vec!["\\Archive", "\\Sent"]),
            ("Other observed", vec![]),
        ]
        .into_iter()
        .enumerate()
        {
            provider.census.folders.insert(
                name.into(),
                crate::census::FolderCensus {
                    uidvalidity: 300 + index as u32,
                    state: crate::census::ScanState::Complete,
                    live_uids: [4].into(),
                    messages: [(4, message.clone())].into(),
                    highestmodseq: None,
                },
            );
            provider.synthetic_uid_next.insert(name.into(), 1004);
            provider
                .synthetic_folders
                .as_mut()
                .unwrap()
                .inventory
                .mailboxes
                .push(crate::census::folder_provisioning::Mailbox {
                    name: crate::census::folder_provisioning::ExactName::new(name).unwrap(),
                    delimiter: Some("/".into()),
                    attributes: roles.into_iter().map(str::to_owned).collect(),
                });
        }
        let inbox = provider.census.folders.get_mut("INBOX").unwrap();
        inbox
            .messages
            .get_mut(&8)
            .unwrap()
            .flags
            .insert("\\Flagged".into());
        inbox
            .messages
            .get_mut(&19)
            .unwrap()
            .flags
            .insert("\\Deleted".into());
        provider
            .synthetic_folders
            .as_mut()
            .unwrap()
            .inventory
            .mailboxes
            .sort_by(|a, b| a.name.decoded.cmp(&b.name.decoded));
        external.save_state(&mut provider).unwrap();
    }
    let digest =
        |value: &serde_json::Value| crate::unsubscribe::digest(&serde_json::to_vec(value).unwrap());
    let folders = provider.census.folders.iter().map(|(name,f)| (name.clone(), serde_json::json!({"uidvalidity":f.uidvalidity,"uids":f.live_uids,"messages":f.messages.iter().map(|(uid,m)| (uid.to_string(), serde_json::json!({"flags":m.flags,"date":m.internal_date,"size":m.size,"hash":m.headers.message_id_hash}))).collect::<BTreeMap<_,_>>()}))).collect::<BTreeMap<_,_>>();
    let mut independent = serde_json::to_value(&provider).unwrap();
    for f in ["revision", "census", "synthetic_uid_next"] {
        independent.as_object_mut().unwrap().remove(f);
    }
    independent["synthetic_folders"]
        .as_object_mut()
        .unwrap()
        .remove("commands");
    let mut value = serde_json::json!({"account":account.id,"folders":folders,"independent":digest(&independent),"uid_next":provider.synthetic_uid_next,"commands":provider.synthetic_folders.as_ref().unwrap().commands});
    drop(external);
    if action != "provider_only" {
        let primary = std::env::var_os("MAILROOM_MOVE_INSTALLATION")
            .map(|p| super::persistent_test_accounts(std::path::Path::new(&p)));
        let primary = primary.as_ref().unwrap_or(&repo);
        let restored_account = primary.load(&account.id).unwrap();
        let store = primary.store(&restored_account).unwrap();
        let saved = store.load_state().unwrap();
        value["primary"] = digest(&serde_json::to_value(&saved).unwrap()).into();
        value["ciphertext"] =
            crate::unsubscribe::digest(&std::fs::read(store.path()).unwrap()).into();
        let mut canonical = serde_json::to_value(&saved).unwrap();
        for f in ["revision", "protocol_backlog"] {
            canonical.as_object_mut().unwrap().remove(f);
        }
        value["unrelated"] = digest(&canonical).into();
        canonical.as_object_mut().unwrap().remove("census");
        value["non_census"] = digest(&canonical).into();
        value["exports"] = serde_json::to_value(&saved.exports).unwrap();
        value["recovery_hold"] = store.recovery_pending().into();
        value["revision"] = saved.revision.into();
        value["independent_effects"] = serde_json::to_value(
            super::sieve_transaction::tests::independent_effects(root, &store),
        )
        .unwrap();
        value["populated"] = serde_json::json!({"backlog":!saved.backlog.is_empty(),"exports":!saved.exports.is_empty(),"proposals":!saved.proposals.history.is_empty(),"ledger":saved.ledger.document.as_ref().is_some_and(|d| !d.entries.is_empty()),"winddown":!saved.winddown.history.is_empty(),"invoice":!saved.invoices.sends.is_empty(),"unsubscribe":!saved.unsubscribe.history.is_empty(),"sieve":!saved.sieve_transactions.history.is_empty(),"protocol":!saved.protocol_backlog.runs.is_empty()});
        value["schema"] = store.schema_version().unwrap().into();
        value["journal"] = serde_json::to_value(&saved.protocol_backlog).unwrap();
        value["saved_census"] = serde_json::to_value(
            &saved
                .census
                .folders
                .iter()
                .map(|(n, f)| (n, &f.live_uids))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("move-observations.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}
