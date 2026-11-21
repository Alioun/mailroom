//! Exact local consent is independent of provider-write authority.
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub(crate) const CANCELLED: &str = "Confirmation cancelled";
pub(crate) const CHANGED_AFTER: &str = "Plan changed after confirmation";
pub(crate) const MALFORMED: &str = "Malformed confirmation outcome";
pub(crate) const UNAUTHENTICATED: &str = "Unauthenticated confirmation outcome";
pub(crate) const OVERSIZED: &str = "Oversized confirmation outcome";
const LIFETIME: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub(crate) enum ExactAction {
    AgentSummary {
        account: String,
        report_version: String,
        content_digest: String,
        clear_domain_count: usize,
    },
    LocalAccount {
        account: String,
        operation: LocalAccountOperation,
        provider: crate::acquisition::Provider,
        login: String,
        plan_receipt: String,
    },
    Recovery {
        account: String,
        operation: String,
        manifest_digest: String,
        consequences: String,
    },
    InvoiceSend {
        account: String,
        revision: u64,
        report_version: String,
        target_name: String,
        target: String,
        sending_identity: String,
        source: String,
        content_digest: String,
    },
    UnsubscribeLocalMail {
        account: String,
        revision: u64,
        workspace_revision: Option<u64>,
        subscription: String,
        target: String,
        sending_identity: String,
        receiving_identity: String,
        subject: String,
        content_digest: String,
        source_digest: String,
    },
    MailboxChange {
        account: String,
        operation: String,
        plan_receipt: String,
    },
    SubscriptionAction {
        account: String,
        revision: u64,
        workspace_revision: Option<u64>,
        subscription: String,
        method: crate::unsubscribe::Method,
        receiving_identity: String,
        host: Option<String>,
        content_digest: String,
        source_digest: String,
        effect: String,
        destination: Option<String>,
    },
    /// One SMTP submission of exact message bytes through the account's server.
    SmtpSend {
        account: String,
        operation: String,
        target: String,
        sending_identity: String,
        content_digest: String,
    },
    /// One native HTTPS POST of `List-Unsubscribe=One-Click`; `target` is the URI host.
    OneClickPost {
        account: String,
        operation: String,
        target: String,
        sending_identity: String,
        content_digest: String,
    },
    /// Opens a new message in Mail from the confirmed sender. Sends nothing, but no app
    /// opens unconfirmed.
    MailAppHandoff {
        account: String,
        operation: String,
        target: String,
        sending_identity: String,
        content_digest: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalAccountOperation {
    Create,
    ReplacePassword,
}
impl LocalAccountOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Create => "Create a local account",
            Self::ReplacePassword => "Replace the stored password",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub actions: Vec<ExactAction>,
    pub exact_receipt: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct Request {
    request_id: String,
    protocol: &'static str,
    actions: Vec<ExactAction>,
    plan_digest: String,
    display: String,
    expires_after_seconds: u64,
    synthetic: bool,
}

fn digest(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}
fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}
fn display(actions: &[ExactAction]) -> String {
    actions.iter().map(|a| match a {
        ExactAction::AgentSummary { account, content_digest, clear_domain_count, .. } => format!("Export this census summary for account {account} as census-summary.md for agent review. {clear_domain_count} approved organisational domains may appear in clear text. Exact summary SHA-256 {content_digest}. Exported copies are outside local account forgetting."),
        ExactAction::LocalAccount {account, operation, provider, login, plan_receipt} => format!("{} for {login} at {}, account {account}. Store the credential privately. No provider connection or setting change. Exact local plan receipt {plan_receipt}.", operation.label(), match provider { crate::acquisition::Provider::Gmail => "Gmail", crate::acquisition::Provider::MailboxOrg => "mailbox.org" }),
        ExactAction::Recovery {account, operation, manifest_digest, consequences} => format!("{operation} for account {account}. Exact recovery plan SHA-256 {manifest_digest}. {consequences}"),
        ExactAction::InvoiceSend {account, target_name, target, sending_identity, content_digest, ..} => format!("Send this invoice for account {account} to {target_name} <{target}> from {sending_identity}. Exact content SHA-256 {content_digest}. No Undo."),
        ExactAction::UnsubscribeLocalMail {account, target, sending_identity, receiving_identity, subject, content_digest, ..} => format!("Send this unsubscribe message for account {account} to {target} from {sending_identity}, received at {receiving_identity}. Subject {subject}. Exact content SHA-256 {content_digest}. Success unknown. No Undo."),
        ExactAction::MailboxChange {account, operation, plan_receipt} => format!("{operation} for account {account}. Exact previewed plan receipt {plan_receipt}. Mailbox changes reach the provider."),
        ExactAction::SubscriptionAction {account, subscription, method, receiving_identity, host, content_digest, effect, ..} => format!("Apply synthetic {} for subscription {subscription}, account {account}, received at {receiving_identity}, host {}. Payload SHA-256 {content_digest}. {effect}", serde_json::to_string(method).expect("Method encoding"), host.as_deref().unwrap_or("none")),
        ExactAction::SmtpSend {account, operation, target, sending_identity, content_digest} => format!("{operation} for account {account}. Send by SMTP to {target} from {sending_identity}. Exact content SHA-256 {content_digest}. Sent mail has no Undo."),
        ExactAction::OneClickPost {account, operation, target, sending_identity, content_digest} => format!("{operation} for account {account}. One HTTPS POST of List-Unsubscribe=One-Click to {target} for mail received at {sending_identity}. Exact URI SHA-256 {content_digest}. Success unknown. No Undo."),
        ExactAction::MailAppHandoff {account, operation, target, sending_identity, content_digest} => format!("{operation} for account {account}. Open a new message in Mail to {target} from {sending_identity} with the confirmed subject and body. Fields SHA-256 {content_digest}. Nothing is sent unless you send it in Mail."),
    }).collect::<Vec<_>>().join("\n")
}

struct Pending {
    request: Request,
    plan: Plan,
    created: Instant,
}
/// Issues single-use requests and consumes them through one backend chosen at startup.
pub(crate) struct Boundary {
    #[cfg(test)]
    pub(crate) accesses: usize,
    pending: BTreeMap<String, Pending>,
    backend: Box<dyn Backend>,
}

/// This capability is produced only after consuming and revalidating consent.
pub(crate) struct Confirmed {
    exact_receipt: String,
}
impl Confirmed {
    pub fn receipt(&self) -> &str {
        &self.exact_receipt
    }
    #[cfg(test)]
    pub(crate) fn for_test(exact_receipt: &str) -> Self {
        Self {
            exact_receipt: exact_receipt.into(),
        }
    }
}

impl Boundary {
    pub fn synthetic() -> Self {
        Self::with_backend(Box::new(SyntheticBackend))
    }
    /// The production boundary. It issues no request unless a helper was pinned into this build.
    pub fn native() -> Self {
        Self::with_backend(Box::new(NativeBackend {
            helper: crate::native::installed(),
        }))
    }
    /// Browser-confirmed boundary for `--local-vault` mode.
    pub fn browser() -> Self {
        Self::with_backend(Box::new(BrowserBackend))
    }
    pub(crate) fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            #[cfg(test)]
            accesses: 0,
            pending: BTreeMap::new(),
            backend,
        }
    }
    pub fn offer(&mut self, plan: Plan) -> Result<(String, Request), String> {
        #[cfg(test)]
        {
            self.accesses += 1;
        }
        self.backend.available()?;
        self.pending.retain(|_, p| p.created.elapsed() < LIFETIME);
        if self.pending.len() >= 128
            || plan.actions.is_empty()
            || plan.actions.len() > 64
            || plan.exact_receipt.len() != 64
        {
            return Err("Confirmation request limit".into());
        }
        let request_id = data_encoding::HEXLOWER.encode(&random());
        let request = Request {
            request_id: request_id.clone(),
            protocol: "mailroom-exact-action-v1",
            display: display(&plan.actions),
            actions: plan.actions.clone(),
            plan_digest: digest(
                &serde_json::to_vec(&(&plan.actions, &plan.exact_receipt))
                    .map_err(|_| "Plan encoding")?,
            ),
            expires_after_seconds: LIFETIME.as_secs(),
            synthetic: self.backend.synthetic(),
        };
        self.pending.insert(
            request_id.clone(),
            Pending {
                request: request.clone(),
                plan,
                created: Instant::now(),
            },
        );
        Ok((request_id, request))
    }

    /// Consumes the request whatever happens, asks the backend, authenticates its answer and
    /// revalidates the plan. Only an approved, authenticated, unchanged plan yields `Confirmed`.
    pub fn confirm(
        &mut self,
        handle: &str,
        current: &Plan,
        revalidate: impl FnOnce() -> Result<Plan, String>,
    ) -> Result<Confirmed, String> {
        let pending = self
            .pending
            .remove(handle)
            .ok_or("Unknown or consumed confirmation request")?;
        if pending.created.elapsed() >= LIFETIME || pending.plan != *current {
            return Err("Expired or changed confirmation plan".into());
        }
        let request = serde_json::to_vec(&pending.request).map_err(|_| "Request encoding")?;
        let challenge = Zeroizing::new(random());
        let answer = self.backend.exchange(&request, &challenge)?;
        let outcome = authenticate(&pending.request.request_id, &request, &challenge, &answer)?;
        if outcome != Outcome::Approved {
            return Err(CANCELLED.into());
        }
        if pending.created.elapsed() >= LIFETIME || revalidate()? != pending.plan {
            return Err(CHANGED_AFTER.into());
        }
        Ok(Confirmed {
            exact_receipt: pending.plan.exact_receipt,
        })
    }
}

/// Shows one exact request to a person and returns the raw, still unauthenticated answer.
pub(crate) trait Backend: Send {
    fn available(&self) -> Result<(), String>;
    fn synthetic(&self) -> bool;
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String>;
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Approved,
    Cancelled,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireOutcome {
    request_id: String,
    request_digest: String,
    outcome: Outcome,
    authentication: String,
}
fn tag(key: &[u8; 32], id: &str, request_digest: &str, outcome: Outcome) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key length");
    mac.update(b"mailroom-exact-action-outcome-v1\0");
    mac.update(&serde_json::to_vec(&(id, request_digest, outcome)).expect("Outcome encoding"));
    data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
}
fn authenticate(id: &str, request: &[u8], key: &[u8; 32], bytes: &[u8]) -> Result<Outcome, String> {
    if bytes.len() > 4096 {
        return Err(OVERSIZED.into());
    }
    let wire: WireOutcome = serde_json::from_slice(bytes).map_err(|_| MALFORMED)?;
    let request_digest = digest(request);
    let expected = tag(key, id, &request_digest, wire.outcome);
    if wire.request_id != id
        || wire.request_digest != request_digest
        || wire.authentication.len() != 64
        || !bool::from(expected.as_bytes().ct_eq(wire.authentication.as_bytes()))
    {
        return Err(UNAUTHENTICATED.into());
    }
    Ok(wire.outcome)
}
fn answer(request: &[u8], key: &[u8; 32], outcome: Outcome) -> Result<Vec<u8>, String> {
    #[derive(Deserialize)]
    struct Id {
        request_id: String,
    }
    let id: Id = serde_json::from_slice(request).map_err(|_| "Request encoding")?;
    let request_digest = digest(request);
    serde_json::to_vec(&WireOutcome {
        authentication: tag(key, &id.request_id, &request_digest, outcome),
        request_id: id.request_id,
        request_digest,
        outcome,
    })
    .map_err(|_| "Outcome encoding".into())
}

/// Approves in process. Used only by the synthetic fixture server, whose requests say so.
pub(crate) struct SyntheticBackend;
impl Backend for SyntheticBackend {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        true
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        answer(request, challenge, Outcome::Approved)
    }
}

/// Approves after explicit browser review of the exact plan and receipt in `--local-vault` mode.
pub(crate) struct BrowserBackend;
impl Backend for BrowserBackend {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        answer(request, challenge, Outcome::Approved)
    }
}

/// Runs the pinned `mailroom-confirm` helper. The request and one-time challenge travel only
/// on its private stdin pipe.
pub(crate) struct NativeBackend {
    helper: Result<crate::native::Helper, crate::native::LaunchError>,
}
impl Backend for NativeBackend {
    fn available(&self) -> Result<(), String> {
        self.helper.as_ref().map(|_| ()).map_err(|e| e.to_string())
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn exchange(&mut self, request: &[u8], challenge: &[u8; 32]) -> Result<Vec<u8>, String> {
        let helper = self.helper.as_ref().map_err(|e| e.to_string())?;
        let mut envelope = Zeroizing::new(String::with_capacity(request.len() * 2 + 128));
        envelope.push_str("{\"request\":\"");
        data_encoding::BASE64.encode_append(request, &mut envelope);
        envelope.push_str("\",\"challenge\":\"");
        data_encoding::BASE64.encode_append(challenge, &mut envelope);
        envelope.push_str("\"}");
        crate::native::exchange(helper, envelope.as_bytes(), 4096, LIFETIME)
            .map(|answer| answer.to_vec())
            .map_err(|e| e.to_string())
    }
}

/// Why production confirmation is or is not available in this build.
pub(crate) fn native_status() -> String {
    match crate::native::installed() {
        Ok(_) => "Native confirmation helper pinned into this build".into(),
        Err(e) => e.to_string(),
    }
}

#[cfg(test)]
mod tests;
