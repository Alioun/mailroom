use super::{
    Account, Accounts, AcquisitionError, Cancellation,
    managesieve::{Error, Session, SieveWritePermit},
};
use crate::{
    census::{
        AccountState, EncryptedStore,
        sieve::{Body, ScriptName},
        sieve_transaction::{
            self as model, Composition, Effect, Image, Outcome, Purpose, Status, Step, Transaction,
        },
    },
    rules::composition::{self, Placement, Precedence, Refusal},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const COMPENSATE: &[u8] = b"mailroom/managesieve-conditional-compensation/v1";
const COMPOSED: &[u8] = b"mailroom/managesieve-composed-deploy/v1";
const REMOVE: &[u8] = b"mailroom/managesieve-composed-removal/v1";
pub const LIMITATIONS: &str = "Synthetic installation only. Existing rules run first and stay byte-identical; Mailroom writes only its own marked content after them. Syntax validation is distinct from proof of delivery or coexistence. The saved-account artifact retains conservative recipient authority. Replacing an active script changes future mail processing immediately. HAVESPACE reserves nothing. ManageSieve has no CAS or multi-command transaction. Another editor can race the final read and write. Compensation cannot reverse forwards or mail processed while a script was active. Removal leaves Mailroom's own included script in place because DELETE is not implemented. No automatic retry, continuation, compensation or disable. Production, native confirmation, the placement decision, OX editor behavior and independent Sieve-engine verification remain unavailable.";

pub(crate) enum Request<'a> {
    Deploy,
    Compensation(&'a str),
    Removal,
}
impl Request<'_> {
    fn compensates(&self) -> Option<&str> {
        match self {
            Self::Compensation(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Preview {
    pub transaction: Transaction,
    pub receipt: String,
    pub proof: serde_json::Value,
    pub precedence: Option<Precedence>,
    pub effects: Vec<Effect>,
    pub limitations: &'static str,
    #[serde(skip)]
    deployment_artifact: Option<crate::rules::CompiledArtifact>,
}
#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum Previewed {
    Ready(Box<Preview>),
    Refused {
        refusal: Refusal,
        precedence: Option<Precedence>,
        limitations: &'static str,
    },
}
fn invalid() -> AcquisitionError {
    AcquisitionError::InvalidInput
}
fn protocol(step: &'static str) -> impl Fn(Error) -> AcquisitionError {
    move |_error| {
        #[cfg(test)]
        eprintln!("sieve.transaction.{step}.failure={_error:?}");
        let _ = step;
        AcquisitionError::Protocol
    }
}
fn sha256(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}
fn save(store: &EncryptedStore, state: &mut AccountState) -> Result<(), AcquisitionError> {
    store.save_state(state).map_err(Into::into)
}

fn persist_confirmed_intent(
    store: &EncryptedStore,
    saved: &mut AccountState,
    preview: Preview,
) -> Result<(), AcquisitionError> {
    reserve_history(saved, &preview.transaction)?;
    let deploy = preview
        .transaction
        .composition
        .as_ref()
        .is_some_and(|c| c.purpose == Purpose::Deploy);
    match (deploy, preview.deployment_artifact) {
        (true, Some(artifact)) => saved
            .deployment_artifacts
            .insert(&preview.transaction, artifact)?,
        (false, None) => (),
        _ => return Err(invalid()),
    }
    saved.sieve_transactions.history.push(preview.transaction);
    save(store, saved)
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
/// Copies one script's body from `desired`, creating it inactive when it did not exist.
fn with_body(image: &Image, desired: &Image, name: &str) -> Image {
    let mut out = image.clone();
    out.bodies.retain(|b| b.name != name);
    out.bodies.extend(desired.body(name).cloned());
    if !out.names.iter().any(|n| n.name == name) {
        out.names.push(ScriptName {
            name: name.into(),
            active: false,
        });
    }
    out.names.sort_by(|a, b| a.name.cmp(&b.name));
    out.bodies.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
fn effects(before: &Image, after: &Image, t: &Transaction) -> Vec<Effect> {
    let owned = t.composition.as_ref().and_then(|c| c.owned.as_deref());
    let owned_changed = owned.is_some_and(|o| before.body(o) != after.body(o));
    let owned_first = t
        .composition
        .as_ref()
        .is_some_and(|c| c.purpose == Purpose::Deploy);
    let mut result = vec![];
    if owned_first && owned_changed {
        result.push(Effect::UploadOwned);
    }
    if before.body(&t.target) != after.body(&t.target) {
        result.push(Effect::Upload);
    }
    if !owned_first && owned_changed {
        result.push(Effect::UploadOwned);
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

fn effect_image(
    before: &Image,
    transaction: &Transaction,
    effect: Effect,
) -> Result<Image, AcquisitionError> {
    let desired = &transaction.after;
    Ok(match effect {
        Effect::Upload => with_body(before, desired, &transaction.target),
        Effect::UploadOwned => with_body(
            before,
            desired,
            transaction
                .composition
                .as_ref()
                .and_then(|c| c.owned.as_deref())
                .ok_or_else(invalid)?,
        ),
        Effect::Activate | Effect::Disable => Image {
            names: desired.names.clone(),
            ..before.clone()
        },
    })
}

fn reserve_history(
    saved: &AccountState,
    transaction: &Transaction,
) -> Result<(), AcquisitionError> {
    let mut completed = transaction.clone();
    let mut image = completed.before.clone();
    completed.steps.clear();
    for effect in effects(&completed.before, &completed.after, &completed) {
        let after = effect_image(&image, &completed, effect)?;
        completed.steps.push(Step {
            effect,
            before: image,
            after: after.clone(),
            outcome: Outcome::Observed,
            acknowledgment: true,
            failure: None,
        });
        image = after;
    }
    completed.status = Status::Complete;
    completed.validation = Some(Err(Error::CredentialUnavailable));
    completed.space = Some(Err(Error::CredentialUnavailable));
    let mut projected = saved.sieve_transactions.clone();
    projected.history.push(completed);
    projected.validate(&saved.census.account)?;
    // Error/status forms can grow after admission; this reserve stays in the byte budget.
    if serde_json::to_vec(&projected).map_err(|_| invalid())?.len() > model::JOURNAL_BYTES - 1024 {
        return Err(invalid());
    }
    Ok(())
}
struct Planned {
    after: Image,
    artifact: String,
    proof_binding: String,
    proof: serde_json::Value,
    precedence: Option<Precedence>,
    composition: Option<Composition>,
    deployment_artifact: Option<crate::rules::CompiledArtifact>,
}
enum Plan {
    Ready(Box<Planned>),
    Refused(Refusal, Option<Precedence>),
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
        .map_err(protocol("connect"))
    }
    /// Records the owner's placement choice. There is no default; composed deploys refuse
    /// until one is recorded.
    pub(crate) fn record_placement(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        placement: Option<Placement>,
    ) -> Result<(), AcquisitionError> {
        let permit = self.write_permit(account)?;
        if store.recovery_pending()
            || placement
                .as_ref()
                .is_some_and(|p| !p.valid_for(&permit.ownership.target))
        {
            return Err(invalid());
        }
        if saved.sieve_placement != placement {
            saved.sieve_placement = placement;
            save(store, saved)?;
        }
        Ok(())
    }
    fn compensation_plan(
        &self,
        saved: &AccountState,
        context: &crate::census::sieve::Context,
        permit: &SieveWritePermit<'_>,
        before: &Image,
        id: &str,
    ) -> Result<Planned, AcquisitionError> {
        let target = &permit.ownership.target;
        let original = saved
            .sieve_transactions
            .history
            .iter()
            .find(|t| t.id == id && t.compensates.is_none())
            .ok_or_else(invalid)?;
        if original.context != *context
            || original.ownership_epoch != permit.ownership.epoch
            || original.target != *target
            || original
                .composition
                .as_ref()
                .is_some_and(|c| c.purpose != Purpose::Deploy)
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
            ) || prior.context != *context
            {
                return Err(invalid());
            }
            match prior.steps.last() {
                Some(s) if s.outcome == Outcome::Observed => &s.after,
                Some(s) if matches!(s.outcome, Outcome::Preimage | Outcome::Refused) => &s.before,
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
        if eligible != before {
            return Err(invalid());
        }
        let mut after = original.before.clone();
        let composition = original.composition.as_ref().map(|c| Composition {
            purpose: Purpose::Compensation,
            precedence: None,
            ..c.clone()
        });
        if let Some(owned) = composition.as_ref().and_then(|c| c.owned.as_deref()) {
            if after.body(owned).is_none() {
                after = with_body(&after, before, owned);
            }
        }
        Ok(Planned {
            after,
            artifact: original.artifact.clone(),
            proof_binding: original.proof_binding.clone(),
            proof: serde_json::json!({"compensates": id, "fresh_provider_match":true}),
            precedence: None,
            composition,
            deployment_artifact: None,
        })
    }
    fn composed_plan(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        target: &str,
        before: &Image,
        removal: bool,
    ) -> Result<Plan, AcquisitionError> {
        let refuse = |r| Ok(Plan::Refused(r, None));
        let Some(host) = before.body(target) else {
            return refuse(Refusal::HostMissing {
                script: target.into(),
            });
        };
        if before.active() != Some(target) {
            return refuse(Refusal::HostInactive {
                script: target.into(),
                active: before.active().map(Into::into),
            });
        }
        let (foreign, found) = match composition::strip(&host.bytes, target) {
            Ok(v) => v,
            Err(r) => return refuse(r),
        };
        let parsed = composition::parse_foreign(&foreign, target);
        let existing_rules = |rules: &[composition::ForeignRule]| {
            rules
                .iter()
                .take(256)
                .map(|r| r.name.chars().take(128).collect())
                .collect::<Vec<String>>()
        };
        if removal {
            if found == Default::default() {
                return refuse(Refusal::NothingToRemove);
            }
            let names = parsed
                .as_ref()
                .ok()
                .and_then(|p| p.analyze(target).ok())
                .map(|r| existing_rules(&r))
                .unwrap_or_default();
            let mut after = before.clone();
            after = with_body(
                &after,
                &Image {
                    bodies: vec![Body::new(target.into(), foreign.clone().into_bytes())],
                    ..before.clone()
                },
                target,
            );
            return Ok(Plan::Ready(Box::new(Planned {
                after,
                artifact: sha256(&host.bytes),
                proof_binding: sha256(foreign.as_bytes()),
                proof: serde_json::json!({"removal": composition::SEMANTICS, "found": {"require": found.require, "copy": found.copy, "block": found.block, "include": found.include}}),
                precedence: None,
                deployment_artifact: None,
                composition: Some(Composition {
                    purpose: Purpose::Removal,
                    semantics: composition::SEMANTICS.into(),
                    placement: None,
                    owned: None,
                    foreign_sha256: sha256(foreign.as_bytes()),
                    existing_rules: names,
                    precedence: None,
                }),
            })));
        }
        let Some(placement) = saved.sieve_placement.clone() else {
            return refuse(Refusal::PlacementUnchosen);
        };
        if !placement.valid_for(target) {
            return refuse(Refusal::InvalidPlacement);
        }
        if let Some(owned) = placement.owned() {
            if before
                .body(owned)
                .is_some_and(|b| !b.bytes.starts_with(composition::OWNED.as_bytes()))
            {
                return refuse(Refusal::OwnedNameForeign {
                    script: owned.into(),
                });
            }
        }
        let script = match parsed {
            Ok(s) => s,
            Err(r) => return refuse(r),
        };
        let p = crate::rules::workspace::preview(
            saved,
            &self.workspace_context(account, store)?,
            store.keys(),
        )
        .map_err(|_| invalid())?;
        if !p.download_available {
            return Err(invalid());
        }
        let executable = p.proof.artifact.executable();
        if executable.len() > 32 * 1024 {
            return Err(invalid());
        }
        let precedence =
            match composition::precedence(&p.proof, &saved.census, &script, target, &placement) {
                Ok(v) => v,
                Err(r) => return refuse(r),
            };
        let composed = match composition::compose(&foreign, &script, &placement, executable, target)
        {
            Ok(v) => v,
            Err(r) => return refuse(r),
        };
        if precedence.gate == "blocked" {
            return Ok(Plan::Refused(Refusal::ProofBlocked, Some(precedence)));
        }
        let mut desired = before.clone();
        desired.bodies = vec![Body::new(target.into(), composed.host)];
        let mut after = with_body(before, &desired, target);
        if let Some((name, bytes)) = composed.owned {
            desired.bodies = vec![Body::new(name.clone(), bytes)];
            after = with_body(&after, &desired, &name);
        }
        let binding = zeroize::Zeroizing::new(
            serde_json::to_vec(&(&p.receipt, &precedence)).map_err(|_| invalid())?,
        );
        Ok(Plan::Ready(Box::new(Planned {
            after,
            artifact: p.proof.artifact.identity().to_owned(),
            deployment_artifact: Some(p.proof.artifact.clone()),
            proof_binding: store
                .keys()
                .binding_for(b"mailroom/managesieve-composed-proof/v1", &binding),
            proof: serde_json::json!({"gate":p.proof.gate,"semantics":p.proof.semantics,"composition":composition::SEMANTICS,"composed_gate":precedence.gate,"pins":precedence.pins,"warnings":p.proof.warnings.iter().chain(&precedence.warnings).collect::<Vec<_>>(),"runtime_cross_check":p.proof.runtime_cross_check}),
            composition: Some(Composition {
                purpose: Purpose::Deploy,
                semantics: composition::SEMANTICS.into(),
                owned: placement.owned().map(Into::into),
                placement: Some(placement),
                foreign_sha256: sha256(foreign.as_bytes()),
                existing_rules: existing_rules(&precedence.foreign_rules),
                precedence: Some(precedence.counts.clone()),
            }),
            precedence: Some(precedence),
        })))
    }
    pub(crate) fn transaction_review(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        request: Request<'_>,
        cancel: Cancellation,
    ) -> Result<Previewed, AcquisitionError> {
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
        let before = observe(&mut session).map_err(protocol("review_observe"))?;
        if !before.validate() {
            return Err(invalid());
        }
        let planned = match request {
            Request::Compensation(id) => {
                if before.body(target).is_none()
                    || before.active().is_some_and(|name| name != target)
                {
                    return Err(invalid());
                }
                self.compensation_plan(saved, &context, &permit, &before, id)?
            }
            Request::Deploy | Request::Removal => {
                match self.composed_plan(
                    account,
                    store,
                    saved,
                    target,
                    &before,
                    matches!(request, Request::Removal),
                )? {
                    Plan::Ready(p) => *p,
                    Plan::Refused(refusal, precedence) => {
                        session.logout().map_err(protocol("refused_logout"))?;
                        return Ok(Previewed::Refused {
                            refusal,
                            precedence,
                            limitations: LIMITATIONS,
                        });
                    }
                }
            }
        };
        if !planned.after.validate() {
            return Err(invalid());
        }
        let compensates = request.compensates();
        let mut transaction = Transaction {
            id: String::new(),
            compensates: compensates.map(String::from),
            context,
            target: target.clone(),
            ownership_epoch: permit.ownership.epoch.clone(),
            artifact: planned.artifact,
            proof_binding: planned.proof_binding,
            receipt: String::new(),
            before,
            after: planned.after,
            status: Status::Running,
            validation: None,
            space: None,
            steps: vec![],
            composition: planned.composition,
        };
        let proposed = effects(&transaction.before, &transaction.after, &transaction);
        if proposed.is_empty() {
            return Err(invalid());
        }
        let purpose = match (&request, &transaction.composition) {
            (Request::Compensation(_), _) => COMPENSATE,
            (Request::Removal, _) => REMOVE,
            (Request::Deploy, _) => COMPOSED,
        };
        let payload = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                saved,
                &transaction,
                &planned.proof,
                &planned.precedence,
                &proposed,
                compensates,
            ))
            .map_err(|_| invalid())?,
        );
        let receipt = store.keys().binding_for(purpose, &payload);
        transaction.id = store
            .keys()
            .binding_for(b"mailroom/managesieve-operation/v1", receipt.as_bytes());
        transaction.receipt = receipt.clone();
        reserve_history(saved, &transaction)?;
        session.logout().map_err(protocol("review_logout"))?;
        Ok(Previewed::Ready(Box::new(Preview {
            transaction,
            receipt,
            proof: planned.proof,
            precedence: planned.precedence,
            effects: proposed,
            limitations: LIMITATIONS,
            deployment_artifact: planned.deployment_artifact,
        })))
    }
    pub(crate) fn transaction_preview(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &AccountState,
        request: Request<'_>,
        cancel: Cancellation,
    ) -> Result<Preview, AcquisitionError> {
        match self.transaction_review(account, store, saved, request, cancel)? {
            Previewed::Ready(p) => Ok(*p),
            Previewed::Refused { .. } => Err(invalid()),
        }
    }
    pub(crate) fn transaction_confirm(
        &self,
        account: &Account,
        store: &EncryptedStore,
        saved: &mut AccountState,
        request: Request<'_>,
        receipt: &str,
        cancel: Cancellation,
        fresh: impl Fn(&AccountState) -> Result<(), String>,
    ) -> Result<(), AcquisitionError> {
        let preview = self.transaction_preview(account, store, saved, request, cancel.clone())?;
        if !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
            return Err(invalid());
        }
        fresh(saved).map_err(|_| invalid())?;
        persist_confirmed_intent(store, saved, preview)?;
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
        let uploads: Vec<(String, Vec<u8>)> = effects(&t.before, &t.after, t)
            .into_iter()
            .filter_map(|e| match e {
                Effect::Upload => Some(t.target.clone()),
                Effect::UploadOwned => t.composition.as_ref()?.owned.clone(),
                _ => None,
            })
            .map(|name| {
                let bytes = t
                    .after
                    .body(&name)
                    .map(|b| b.bytes.clone())
                    .unwrap_or_default();
                (name, bytes)
            })
            .collect();
        let refused = |error: Error| {
            if matches!(error, Error::Refused | Error::Quota | Error::Unsupported) {
                Status::Refused
            } else {
                Status::Stopped
            }
        };
        let check = uploads
            .iter()
            .try_for_each(|(_, bytes)| session.check_script(&permit, bytes));
        saved.sieve_transactions.history[index].validation = Some(check);
        if let Err(error) = check {
            saved.sieve_transactions.history[index].status = refused(error);
        }
        save(store, saved)?;
        check.map_err(protocol("check"))?;
        let space = uploads
            .iter()
            .try_for_each(|(name, bytes)| session.have_space(&permit, name, bytes.len()));
        saved.sieve_transactions.history[index].space = Some(space);
        if let Err(error) = space {
            saved.sieve_transactions.history[index].status = refused(error);
        }
        save(store, saved)?;
        space.map_err(protocol("space"))?;
        let t = saved.sieve_transactions.history[index].clone();
        let mut expected = t.before.clone();
        let desired = t.after.clone();
        let target = t.target.clone();
        let owned = t.composition.as_ref().and_then(|c| c.owned.clone());
        let compensation = t.compensates.is_some();
        for effect in effects(&expected, &desired, &t) {
            if cancel.is_cancelled() {
                return Err(AcquisitionError::Cancelled);
            }
            if fresh(saved).is_err() {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            let observed = observe(&mut session).map_err(protocol("run_observe"))?;
            let t = &saved.sieve_transactions.history[index];
            if observed != expected
                || self.sieve_context(account, store)? != t.context
                || permit.ownership.epoch != t.ownership_epoch
            {
                saved.sieve_transactions.history[index].status = Status::Conflict;
                save(store, saved)?;
                return Err(invalid());
            }
            let after = effect_image(&expected, &saved.sieve_transactions.history[index], effect)?;
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
                (_, Effect::UploadOwned) => "owned_upload_intent",
                (false, Effect::Upload) => "upload_intent",
                (false, _) => "activation_intent",
                (true, Effect::Upload) => "compensation_upload_intent",
                (true, _) => "compensation_activation_intent",
            });
            let result = match effect {
                Effect::Upload | Effect::UploadOwned => {
                    let name = if effect == Effect::Upload {
                        &target
                    } else {
                        owned.as_ref().ok_or_else(invalid)?
                    };
                    session.put_script(&permit, name, &after.body(name).ok_or_else(invalid)?.bytes)
                }
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
                return Err(protocol("effect")(error));
            }
            step.acknowledgment = true;
            step.outcome = Outcome::Acknowledged;
            save(store, saved)?;
            boundary(match (compensation, effect) {
                (_, Effect::UploadOwned) => "owned_upload_acknowledged",
                (false, Effect::Upload) => "upload_acknowledged",
                (false, _) => "activation_acknowledged",
                (true, Effect::Upload) => "compensation_upload_acknowledged",
                (true, _) => "compensation_activation_acknowledged",
            });
            let observed = observe(&mut session).map_err(protocol("run_readback"))?;
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
                (_, Effect::UploadOwned) => "owned_upload_observed",
                (false, Effect::Upload) => "upload_observed",
                (false, _) => "activation_observed",
                (true, Effect::Upload) => "compensation_upload_observed",
                (true, _) => "compensation_activation_observed",
            });
        }
        saved.sieve_transactions.history[index].status = Status::Complete;
        record_active_observation(
            saved,
            index,
            crate::census::deployment::ObservationVia::Readback,
        )?;
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
        let observed = observe(&mut session).map_err(protocol("reconcile_observe"))?;
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
            let index = saved
                .sieve_transactions
                .history
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(invalid)?;
            if saved.sieve_transactions.history[index].status == Status::Complete {
                record_active_observation(
                    saved,
                    index,
                    crate::census::deployment::ObservationVia::Reconciliation,
                )?;
            }
            save(store, saved)?;
        }
        let _ = session.logout();
        Ok(())
    }
}

fn record_active_observation(
    saved: &mut AccountState,
    index: usize,
    via: crate::census::deployment::ObservationVia,
) -> Result<(), AcquisitionError> {
    saved
        .deployment_artifacts
        .observe_active(
            &saved.sieve_transactions.history[index],
            crate::census::deployment::ActiveObservation {
                at: i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| invalid())?
                        .as_secs(),
                )
                .map_err(|_| invalid())?,
                via,
            },
        )
        .map_err(Into::into)
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
pub(crate) mod capacity_tests;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
pub(crate) mod tests;
