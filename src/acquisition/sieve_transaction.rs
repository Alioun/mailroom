use super::{
    Account, Accounts, AcquisitionError, Cancellation,
    managesieve::{Error, Session, SieveWritePermit},
};
use crate::census::{
    AccountState, EncryptedStore,
    sieve::Body,
    sieve_transaction::{self as model, Effect, Image, Outcome, Status, Step, Transaction},
};
use serde::Serialize;
use subtle::ConstantTimeEq;

const DEPLOY: &[u8] = b"mailroom/managesieve-whole-owned-replacement/v1";
const COMPENSATE: &[u8] = b"mailroom/managesieve-conditional-compensation/v1";
pub const LIMITATIONS: &str = "Synthetic installation only. Syntax validation is distinct from proof of delivery or coexistence. The saved-account artifact retains conservative recipient authority. Replacing an active script changes future mail processing immediately. HAVESPACE reserves nothing. ManageSieve has no CAS or multi-command transaction. Another editor can race the final read and write. Compensation cannot reverse forwards or mail processed while a script was active. No automatic retry, continuation, compensation or disable. Production, native confirmation, foreign composition and independent Sieve-engine verification remain unavailable.";

#[derive(Serialize)]
pub(crate) struct Preview {
    pub transaction: Transaction,
    pub receipt: String,
    pub proof: serde_json::Value,
    pub effects: Vec<Effect>,
    pub limitations: &'static str,
}
fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    store.save_state(state).map_err(Into::into)
}
fn observe(session: &mut Session) -> Result<Image, Error> {
    let capabilities = session.capabilities()?;
    let names = session.list()?;
    if names.len() > 8 {
        return Err(Error::Limit);
    }
    let mut bodies = vec![];
    let mut total = 0;
    for name in &names {
        let bytes = session.get(&name.name)?;
        total += bytes.len();
        if total > model::IMAGE_BYTES {
            return Err(Error::Limit);
        }
        bodies.push(Body::new(name.name.clone(), bytes));
    }
    if session.list()? != names {
        return Err(Error::Unstable);
    }
    Ok(Image {
        capabilities,
        names,
        bodies,
    })
}
fn effects(before: &Image, after: &Image, target: &str) -> Vec<Effect> {
    let mut result = vec![];
    if before.body(target) != after.body(target) {
        result.push(Effect::Upload);
    }
    if before.active() != after.active() {
        result.push(if after.active().is_some() {
            Effect::Activate
        } else {
            Effect::Disable
        });
    }
    result
}
impl Accounts {
    pub(crate) fn sieve_write_installed(&self) -> bool {
        self.sieve_write.is_some() && self.sieve_source.is_some()
    }
    pub(crate) fn sieve_write_account(&self, id: &crate::census::AccountId) -> bool {
        self.sieve_write
            .as_ref()
            .is_some_and(|ownership| ownership.account == *id)
    }
    fn write_permit(&self, account: &Account) -> Result<SieveWritePermit<'_>, AcquisitionError> {
        self.sieve_source
            .as_ref()
            .ok_or_else(invalid)?
            .write_permit(self.sieve_write.as_ref().ok_or_else(invalid)?, &account.id)
            .map_err(|_| invalid())
    }
    fn transaction_session(
        &self,
        account: &Account,
        cancel: Cancellation,
    ) -> Result<Session, AcquisitionError> {
        self.write_permit(account)?;
        let password = self.password(account)?;
        Session::connect(
            self.sieve_source
                .as_ref()
                .ok_or_else(invalid)?
                .permit()
                .map_err(|_| invalid())?,
            &account.login,
            &password,
            cancel,
        )
        .map(|s| s.0)
        .map_err(|_| AcquisitionError::Protocol)
    }
    pub(crate) fn transaction_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        compensates: Option<&str>,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        if store.recovery_pending()
            || saved.sieve_transactions.history.len() >= model::HISTORY
            || saved
                .sieve_transactions
                .history
                .iter()
                .any(|t| matches!(t.status, Status::Running | Status::Uncertain))
        {
            return Err(invalid());
        }
        let permit = self.write_permit(account)?;
        let target = &permit.ownership.target;
        let context = self.sieve_context(account, store)?;
        let mut session = self.transaction_session(account, cancel)?;
        let before = observe(&mut session).map_err(|_| AcquisitionError::Protocol)?;
        if !before.validate()
            || before.body(target).is_none()
            || before.active().is_some_and(|name| name != target)
        {
            return Err(invalid());
        }
        let mut after = before.clone();
        let (artifact, proof_binding, proof) = if let Some(id) = compensates {
            let original = saved
                .sieve_transactions
                .history
                .iter()
                .find(|t| t.id == id && t.compensates.is_none())
                .ok_or_else(invalid)?;
            if original.context != context
                || original.ownership_epoch != permit.ownership.epoch
                || original.target != *target
            {
                return Err(invalid());
            }
            let prior = saved
                .sieve_transactions
                .history
                .iter()
                .rev()
                .find(|t| t.compensates.as_deref() == Some(id));
            let eligible = if let Some(prior) = prior {
                if !matches!(
                    prior.status,
                    Status::FreshReview | Status::Stopped | Status::Refused
                ) || prior.context != context
                {
                    return Err(invalid());
                }
                match prior.steps.last() {
                    Some(s) if s.outcome == Outcome::Observed => &s.after,
                    Some(s) if matches!(s.outcome, Outcome::Preimage | Outcome::Refused) => {
                        &s.before
                    }
                    None => &prior.before,
                    _ => return Err(invalid()),
                }
            } else {
                &original
                    .steps
                    .last()
                    .filter(|s| s.outcome == Outcome::Observed)
                    .ok_or_else(invalid)?
                    .after
            };
            if *eligible != before {
                return Err(invalid());
            }
            after = original.before.clone();
            (
                original.artifact.clone(),
                original.proof_binding.clone(),
                serde_json::json!({"compensates": id, "fresh_provider_match":true}),
            )
        } else {
            let p = crate::rules::workspace::preview(saved, account.provider, store.keys())
                .map_err(|_| invalid())?;
            if !p.download_available || p.proof.gate == "blocked" {
                return Err(invalid());
            }
            let bytes = p.proof.artifact.executable().as_bytes().to_vec();
            if bytes.len() > 32 * 1024 {
                return Err(invalid());
            }
            *after
                .bodies
                .iter_mut()
                .find(|b| b.name == *target)
                .ok_or_else(invalid)? = Body::new(target.clone(), bytes);
            for n in &mut after.names {
                n.active = n.name == *target;
            }
            (
                p.proof.artifact.identity().to_owned(),
                p.receipt.clone(),
                serde_json::json!({"gate":p.proof.gate,"semantics":p.proof.semantics,"pins":p.proof.pins,"warnings":p.proof.warnings,"runtime_cross_check":p.proof.runtime_cross_check}),
            )
        };
        if !after.validate() {
            return Err(invalid());
        }
        let proposed = effects(&before, &after, target);
        if proposed.is_empty() {
            return Err(invalid());
        }
        let purpose = if compensates.is_some() {
            COMPENSATE
        } else {
            DEPLOY
        };
        let payload = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                saved,
                &context,
                target,
                &permit.ownership.epoch,
                &before,
                &after,
                &artifact,
                &proof_binding,
                &proof,
                &proposed,
                compensates,
            ))
            .map_err(|_| invalid())?,
        );
        let receipt = store.keys().binding_for(purpose, &payload);
        let id = store
            .keys()
            .binding_for(b"mailroom/managesieve-operation/v1", receipt.as_bytes());
        session.logout().map_err(|_| AcquisitionError::Protocol)?;
        Ok(Preview {
            transaction: Transaction {
                id,
                compensates: compensates.map(String::from),
                context,
                target: target.clone(),
                ownership_epoch: permit.ownership.epoch.clone(),
                artifact,
                proof_binding,
                receipt: receipt.clone(),
                before,
                after,
                status: Status::Running,
                validation: None,
                space: None,
                steps: vec![],
            },
            receipt,
            proof,
            effects: proposed,
            limitations: LIMITATIONS,
        })
    }
    pub(crate) fn transaction_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        compensates: Option<&str>,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let preview =
            self.transaction_preview(account, store, saved, compensates, cancel.clone())?;
        if !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        fresh(saved).map_err(|_| invalid())?;
        saved.sieve_transactions.history.push(preview.transaction);
        save(store, saved)?;
        boundary("transaction_consumed");
        let index = saved.sieve_transactions.history.len() - 1;
        let execution = self.transaction_run(account, store, saved, index, cancel.clone(), &fresh);
        if execution.is_err() {
            let t = &mut saved.sieve_transactions.history[index];
            if t.status == Status::Running {
                t.status = if t.steps.is_empty() {
                    Status::Stopped
                } else {
                    Status::Uncertain
                };
            }
            if let Some(s) = t.steps.last_mut() {
                if matches!(s.outcome, Outcome::Intent | Outcome::Acknowledged) {
                    s.outcome = Outcome::Uncertain;
                }
            }
            save(store, saved)?;
        }
        execution
    }
    fn transaction_run(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        index: usize,
        cancel: Cancellation,
        fresh: &impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let permit = self.write_permit(account)?;
        let mut session = self.transaction_session(account, cancel.clone())?;
        let t = &saved.sieve_transactions.history[index];
        let bytes = t.after.body(&t.target).ok_or_else(invalid)?.bytes.clone();
        let check = session.check_script(&permit, &bytes);
        saved.sieve_transactions.history[index].validation = Some(check);
        if let Err(error) = check {
            saved.sieve_transactions.history[index].status =
                if matches!(error, Error::Refused | Error::Quota | Error::Unsupported) {
                    Status::Refused
                } else {
                    Status::Stopped
                };
        }
        save(store, saved)?;
        check.map_err(|_| AcquisitionError::Protocol)?;
        let space = session.have_space(&permit, bytes.len());
        saved.sieve_transactions.history[index].space = Some(space);
        if let Err(error) = space {
            saved.sieve_transactions.history[index].status =
                if matches!(error, Error::Refused | Error::Quota | Error::Unsupported) {
                    Status::Refused
                } else {
                    Status::Stopped
                };
        }
        save(store, saved)?;
        space.map_err(|_| AcquisitionError::Protocol)?;
        let t = &saved.sieve_transactions.history[index];
        let mut expected = t.before.clone();
        let desired = t.after.clone();
        let target = t.target.clone();
        let compensation = t.compensates.is_some();
        for effect in effects(&expected, &desired, &target) {
            if cancel.is_cancelled() {
                return Err(AcquisitionError::Cancelled);
            }
            if fresh(saved).is_err() {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            let observed = observe(&mut session).map_err(|_| AcquisitionError::Protocol)?;
            let t = &saved.sieve_transactions.history[index];
            if observed != expected
                || self.sieve_context(account, store)? != t.context
                || permit.ownership.epoch != t.ownership_epoch
            {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            let mut after = expected.clone();
            match effect {
                Effect::Upload => after.bodies = desired.bodies.clone(),
                Effect::Activate | Effect::Disable => after.names = desired.names.clone(),
            }
            if fresh(saved).is_err() {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            if saved.sieve.latest.is_some() {
                saved.sieve.invalidated_by_write =
                    Some(saved.sieve_transactions.history[index].id.clone());
            }
            saved.sieve_transactions.history[index].steps.push(Step {
                effect,
                before: expected.clone(),
                after: after.clone(),
                outcome: Outcome::Intent,
                acknowledgment: false,
                failure: None,
            });
            save(store, saved)?;
            boundary(match (compensation, effect) {
                (false, Effect::Upload) => "upload_intent",
                (false, _) => "activation_intent",
                (true, Effect::Upload) => "compensation_upload_intent",
                (true, _) => "compensation_activation_intent",
            });
            let result = match effect {
                Effect::Upload => session.put_script(&permit, &bytes),
                Effect::Activate => session.set_active(&permit, true),
                Effect::Disable => session.set_active(&permit, false),
            };
            let step = saved.sieve_transactions.history[index]
                .steps
                .last_mut()
                .unwrap();
            if let Err(error) = result {
                step.failure = Some(error);
                step.outcome = if matches!(error, Error::Refused | Error::Quota | Error::Missing) {
                    Outcome::Refused
                } else {
                    Outcome::Uncertain
                };
                saved.sieve_transactions.history[index].status =
                    if matches!(error, Error::Refused | Error::Quota | Error::Missing) {
                        Status::Refused
                    } else {
                        Status::Uncertain
                    };
                save(store, saved)?;
                return Err(AcquisitionError::Protocol);
            }
            step.acknowledgment = true;
            step.outcome = Outcome::Acknowledged;
            save(store, saved)?;
            boundary(match (compensation, effect) {
                (false, Effect::Upload) => "upload_acknowledged",
                (false, _) => "activation_acknowledged",
                (true, Effect::Upload) => "compensation_upload_acknowledged",
                (true, _) => "compensation_activation_acknowledged",
            });
            let observed = observe(&mut session).map_err(|_| AcquisitionError::Protocol)?;
            let step = saved.sieve_transactions.history[index]
                .steps
                .last_mut()
                .unwrap();
            step.outcome = if observed == after {
                Outcome::Observed
            } else {
                Outcome::Conflict
            };
            if observed != after {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            save(store, saved)?;
            expected = after;
            boundary(match (compensation, effect) {
                (false, Effect::Upload) => "upload_observed",
                (false, _) => "activation_observed",
                (true, Effect::Upload) => "compensation_upload_observed",
                (true, _) => "compensation_activation_observed",
            });
        }
        saved.sieve_transactions.history[index].status = Status::Complete;
        save(store, saved)?;
        let _ = session.logout();
        Ok(())
    }
    pub(crate) fn transaction_reconcile(
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
        let context = self.sieve_context(account, store)?;
        let permit = self.write_permit(account)?;
        let t = saved
            .sieve_transactions
            .history
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(invalid)?;
        if t.context != context
            || t.target != permit.ownership.target
            || t.ownership_epoch != permit.ownership.epoch
        {
            if t.status != Status::Conflict {
                t.status = Status::Conflict;
                save(store, saved)?;
            }
            return Err(invalid());
        }
        if !matches!(
            t.status,
            Status::Uncertain | Status::Stopped | Status::FreshReview
        ) {
            return Ok(());
        }
        let mut session = self.transaction_session(account, cancel)?;
        let observed = observe(&mut session).map_err(|_| AcquisitionError::Protocol)?;
        let previous = t.clone();
        if let Some(s) = t.steps.last_mut() {
            if observed == s.after {
                s.outcome = Outcome::Observed;
                t.status = if observed == t.after {
                    Status::Complete
                } else {
                    Status::FreshReview
                };
            } else if observed == s.before {
                s.outcome = Outcome::Preimage;
                t.status = Status::FreshReview;
            } else {
                s.outcome = Outcome::Conflict;
                t.status = Status::Conflict;
            }
        } else {
            t.status = if observed == t.before {
                Status::FreshReview
            } else {
                Status::Conflict
            };
        }
        if *t != previous {
            save(store, saved)?;
        }
        let _ = session.logout();
        Ok(())
    }
}
pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MAILROOM_SIEVE_TRANSACTIONS_FIXTURE") {
        let root = std::path::Path::new(&root);
        if std::fs::read_to_string(root.join("transaction-boundary")).is_ok_and(|s| s == name) {
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
