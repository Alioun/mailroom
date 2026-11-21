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
pub(crate) fn recommend_sieve_placement(saved: &AccountState) -> serde_json::Value {
    let Some(baseline) = &saved.sieve.last_successful else {
        return serde_json::json!({
            "recommended": "block",
            "include_supported": false,
            "has_open_xchange": false,
            "discovered": false,
            "reason": "Run ManageSieve discovery first to inspect server capabilities and Open-Xchange rules. Block mode is the safe default when capabilities are unknown."
        });
    };
    let include_supported = baseline.capabilities.get("SIEVE").is_some_and(|exts| {
        exts.split_whitespace()
            .any(|ext| ext.eq_ignore_ascii_case("include"))
    });
    let has_open_xchange = baseline.names.iter().any(|n| n.name == "Open-Xchange")
        || baseline.bodies.iter().any(|b| b.name == "Open-Xchange");
    let recommended = if include_supported {
        "include"
    } else {
        "block"
    };
    let reason = if include_supported {
        "Server advertises the Sieve include extension; Include keeps Mailroom rules in a dedicated script referenced by one marked line in Open-Xchange. Block is also available if you prefer keeping rules inline in Open-Xchange."
    } else {
        "Server does not advertise the Sieve include extension; Block appends Mailroom rules inside a marked OX-compatible section after existing rules in Open-Xchange."
    };
    serde_json::json!({
        "recommended": recommended,
        "include_supported": include_supported,
        "has_open_xchange": has_open_xchange,
        "discovered": true,
        "reason": reason
    })
}

impl Accounts {
    pub(crate) fn sieve_installed(&self) -> bool {
        self.sieve_source.is_some() || self.live_transports
    }
    pub(super) fn resolve_sieve_source(
        &self,
        account: &super::Account,
    ) -> Result<std::borrow::Cow<'_, super::managesieve::InstalledSource>, AcquisitionError> {
        if let Some(source) = self.sieve_source.as_deref() {
            return Ok(std::borrow::Cow::Borrowed(source));
        }
        if self.live_transports && account.provider == super::Provider::MailboxOrg {
            return super::managesieve::InstalledSource::real_mailbox_org()
                .map(std::borrow::Cow::Owned)
                .map_err(|_| AcquisitionError::Protocol);
        }
        Err(AcquisitionError::InvalidInput)
    }
    pub(crate) fn sieve_context(
        &self,
        account: &super::Account,
        store: &EncryptedStore,
    ) -> Result<Context, AcquisitionError> {
        let (profile, source_epoch, tls_identity) = if let Some(source) = self.sieve_source.as_ref()
        {
            (
                "installed-loopback-starttls-plain".into(),
                source.identity(),
                source.tls_name.clone(),
            )
        } else if self.live_transports && account.provider == super::Provider::MailboxOrg {
            (
                "live-mailbox-org-starttls-plain".into(),
                super::managesieve::InstalledSource::live_mailbox_org_identity(),
                super::managesieve::MAILBOX_ORG_SIEVE_HOST.to_owned(),
            )
        } else {
            return Err(AcquisitionError::InvalidInput);
        };
        Ok(Context {
            account: account.id.clone(),
            provider: account.provider,
            profile,
            source_epoch,
            tls_identity,
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
        let account = self.load(id)?;
        let source = self.resolve_sieve_source(&account)?;
        let permit = source
            .permit()
            .map_err(|_| AcquisitionError::InvalidInput)?;
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
