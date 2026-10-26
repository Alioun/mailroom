use super::{
    Accounts, AcquisitionError, Cancellation,
    managesieve::{Error, Session},
};
use crate::census::{
    AccountId, AccountState, EncryptedStore,
    sieve::{self, Attempt, Baseline, Body, Context, Outcome},
};
use std::time::{SystemTime, UNIX_EPOCH};
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl Accounts {
    pub(crate) fn sieve_installed(&self) -> bool {
        self.sieve_source.is_some()
    }
    pub(crate) fn sieve_context(
        &self,
        account: &super::Account,
        store: &EncryptedStore,
    ) -> Result<Context, AcquisitionError> {
        let source = self
            .sieve_source
            .as_ref()
            .ok_or(AcquisitionError::InvalidInput)?;
        Ok(Context {
            account: account.id.clone(),
            provider: account.provider,
            profile: "installed-loopback-starttls-plain".into(),
            source_epoch: source.identity(),
            tls_identity: source.tls_name.clone(),
            authority: store.keys().binding_for(
                b"mailroom/sieve-workspace-authority/v1",
                &serde_json::to_vec(&(
                    self.sieve_authority(account, store)?,
                    self.workspace_context(account, store)?.binding(),
                ))
                .map_err(|_| AcquisitionError::Storage)?,
            ),
            adapter: sieve::ADAPTER.into(),
        })
    }
    pub(crate) fn discover_sieve(
        &self,
        id: &AccountId,
        cancel: Cancellation,
    ) -> Result<(), AcquisitionError> {
        let source = self
            .sieve_source
            .as_ref()
            .ok_or(AcquisitionError::InvalidInput)?;
        let permit = source
            .permit()
            .map_err(|_| AcquisitionError::InvalidInput)?;
        let account = self.load(id)?;
        let store = self.store(&account)?;
        if store.recovery_pending() {
            return Err(AcquisitionError::LifecyclePending);
        }
        let mut saved = store.load_state()?;
        let context = self.sieve_context(&account, &store)?;
        saved.sieve.version = 1;
        saved.sieve.latest = Some(Attempt {
            id: format!("{:032x}", rand::random::<u128>()),
            started_at: now(),
            finished_at: None,
            context,
            outcome: Outcome::Running,
            failure: None,
            capabilities: None,
            names: None,
            bodies: vec![],
        });
        store.save_state(&mut saved)?;
        let result = (|| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let password = self
                .password(&account)
                .map_err(|_| Error::CredentialUnavailable)?;
            let (mut session, capabilities) =
                Session::connect(permit, &account.login, &password, cancel.clone())?;
            saved.sieve.latest.as_mut().unwrap().capabilities = Some(capabilities.clone());
            checkpoint(&store, &mut saved)?;
            let names = session.list()?;
            saved.sieve.latest.as_mut().unwrap().names = Some(names.clone());
            checkpoint(&store, &mut saved)?;
            for name in names
                .iter()
                .filter(|n| n.active || n.name == "Open-Xchange")
            {
                let bytes = session.get(&name.name)?;
                saved
                    .sieve
                    .latest
                    .as_mut()
                    .unwrap()
                    .bodies
                    .push(Body::new(name.name.clone(), bytes));
                checkpoint(&store, &mut saved)?;
            }
            if session.capabilities()? != capabilities || session.list()? != names {
                return Err(Error::Unstable);
            }
            for body in &saved.sieve.latest.as_ref().unwrap().bodies {
                if session.get(&body.name)? != body.bytes {
                    return Err(Error::Unstable);
                }
            }
            session.logout()?;
            Ok(())
        })();
        let a = saved.sieve.latest.as_mut().unwrap();
        a.finished_at = Some(now());
        a.outcome = match result {
            Ok(()) => Outcome::Complete,
            Err(Error::Cancelled) => Outcome::Cancelled,
            Err(_) => Outcome::Failed,
        };
        a.failure = result.err();
        if result.is_ok() {
            saved.sieve.invalidated_by_write = None;
            saved.sieve.last_successful = Some(Baseline {
                attempt: a.id.clone(),
                observed_at: now(),
                context: a.context.clone(),
                capabilities: a.capabilities.clone().unwrap(),
                names: a.names.clone().unwrap(),
                bodies: a.bodies.clone(),
            });
        }
        store.save_state(&mut saved)?;
        result.map_err(|e| {
            #[cfg(test)]
            tests::report_failure(e);
            if e == Error::Cancelled {
                AcquisitionError::Cancelled
            } else {
                AcquisitionError::Protocol
            }
        })
    }
}
fn checkpoint(store: &EncryptedStore, saved: &mut AccountState) -> Result<(), Error> {
    store.save_state(saved).map_err(|_| Error::Limit)
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
impl Accounts {
    pub(crate) fn install_sieve_fixture(&mut self, fake: &super::SieveFake) {
        let source = &fake.source;
        self.sieve_source = Some(std::sync::Arc::new(super::managesieve::InstalledSource {
            address: source.address,
            tls_name: source.tls_name.clone(),
            root: source.root.clone(),
            epoch: source.epoch.clone(),
            command_timeout: source.command_timeout,
            attempt_timeout: source.attempt_timeout,
        }));
    }
}
