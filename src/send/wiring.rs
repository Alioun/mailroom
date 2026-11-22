//! The real adapters behind production routes.
//!
//! A route reaches an SMTP server, the one-click network or the mail app only
//! through a `policy::WritePermit`, so every consequential route stays denied
//! while `policy::authorize` denies account writes. Tests install fake
//! endpoints instead; that variant and its authorization exist only in test
//! builds, so a release build has no way to authorize a write.
use super::{handoff, smtp};
use crate::{
    acquisition::Provider,
    policy::{self, Operation, WritePermit},
    unsubscribe::one_click,
};
use std::{net::ToSocketAddrs, time::Duration};

/// How long one one-click POST may take.
const ONE_CLICK_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) enum Endpoints {
    /// The provider's submission server, the system resolver and Apple Mail.
    Real,
    #[cfg(test)]
    Fake(Fake),
}

/// Loopback fakes a test installed in place of every real endpoint.
#[cfg(test)]
pub(crate) struct Fake {
    pub smtp: Option<smtp::Server>,
    pub network: one_click::tests::Fixture,
    pub https: std::net::SocketAddr,
    pub tls: native_tls::TlsConnector,
    pub launcher: handoff::Recorder,
    pub authserv: Option<String>,
    pub timeouts: smtp::Timeouts,
}

/// The submission server of each provider, with TLS from the first byte.
fn submission(provider: Provider) -> (&'static str, u16) {
    match provider {
        Provider::MailboxOrg => ("smtp.mailbox.org", 465),
        Provider::Gmail => ("smtp.gmail.com", 465),
    }
}

/// Refuses a permit issued for another kind of write.
fn allows(permit: &WritePermit, operations: &[Operation]) -> Result<(), String> {
    let granted = permit.operation() as u8;
    if operations.iter().any(|o| *o as u8 == granted) {
        Ok(())
    } else {
        Err("Write permit is for another operation".into())
    }
}

impl Endpoints {
    /// Whether a test installed fakes in place of the real endpoints.
    pub(crate) fn fake(&self) -> bool {
        match self {
            Self::Real => false,
            #[cfg(test)]
            Self::Fake(_) => true,
        }
    }

    /// Asks the policy for `operation`. Routes call this before any repository,
    /// credential or network access.
    pub(crate) fn permit_with_mode(
        &self,
        operation: Operation,
        live_writes: bool,
    ) -> Result<WritePermit, &'static str> {
        match self {
            Self::Real => policy::write_permit_with_mode(operation, live_writes),
            #[cfg(test)]
            Self::Fake(fake) => {
                let mut endpoints = vec![fake.https];
                endpoints.extend(fake.smtp.as_ref().map(|s| s.address));
                policy::fake_write_permit(operation, &endpoints)
            }
        }
    }

    /// The account's submission server. Resolves its address only after the permit.
    pub(crate) fn smtp(
        &self,
        permit: &WritePermit,
        provider: Provider,
    ) -> Result<smtp::Server, String> {
        allows(permit, &[Operation::SendMessage, Operation::Unsubscribe])?;
        match self {
            Self::Real => {
                let (host, port) = submission(provider);
                let address = (host, port)
                    .to_socket_addrs()
                    .map_err(|_| "Submission server unresolved")?
                    .next()
                    .ok_or("Submission server unresolved")?;
                Ok(smtp::Server {
                    address,
                    tls_name: host.into(),
                    security: smtp::Security::ImplicitTls,
                    tls: native_tls::TlsConnector::new().map_err(|_| "TLS unavailable")?,
                })
            }
            #[cfg(test)]
            Self::Fake(fake) => {
                let server = fake.smtp.as_ref().ok_or("No fake SMTP server")?;
                Ok(smtp::Server {
                    address: server.address,
                    tls_name: server.tls_name.clone(),
                    security: server.security,
                    tls: server.tls.clone(),
                })
            }
        }
    }

    pub(crate) fn timeouts(&self) -> smtp::Timeouts {
        match self {
            Self::Real => smtp::Timeouts::default(),
            #[cfg(test)]
            Self::Fake(fake) => fake.timeouts,
        }
    }

    /// The network, certificate verification and deadline of a one-click POST.
    pub(crate) fn one_click(
        &self,
        permit: &WritePermit,
    ) -> Result<(&dyn one_click::Network, native_tls::TlsConnector, Duration), String> {
        allows(permit, &[Operation::Unsubscribe])?;
        match self {
            Self::Real => Ok((
                &one_click::System,
                native_tls::TlsConnector::new().map_err(|_| "TLS unavailable")?,
                ONE_CLICK_TIMEOUT,
            )),
            #[cfg(test)]
            Self::Fake(fake) => Ok((&fake.network, fake.tls.clone(), fake.timeouts.command)),
        }
    }

    pub(crate) fn launcher(&self, permit: &WritePermit) -> Result<&dyn handoff::Launcher, String> {
        allows(permit, &[Operation::SendMessage, Operation::Unsubscribe])?;
        Ok(match self {
            Self::Real => &handoff::Osascript,
            #[cfg(test)]
            Self::Fake(fake) => &fake.launcher,
        })
    }

    /// The authserv-id whose Authentication-Results this provider adds, when it is
    /// established. Only results under it can authorize a one-click POST.
    pub(crate) fn authserv(&self, provider: Provider) -> Option<&str> {
        match self {
            Self::Real => match provider {
                Provider::Gmail => Some("mx.google.com"),
                // Not yet established by the owner's read-only check, so mailbox.org mail
                // offers no one-click POST.
                Provider::MailboxOrg => None,
            },
            #[cfg(test)]
            Self::Fake(fake) => fake.authserv.as_deref(),
        }
    }
}
