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

const LIFETIME: Duration = Duration::from_secs(300);
pub const NATIVE_GAP: &str = "Native invocation denied: owner-selected signing identity, protected executable continuity and authenticated launch channel are not established. Credential trust and per-access Keychain approval also require owner-run verification.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub(crate) enum ExactAction {
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
        ExactAction::InvoiceSend {account, target_name, target, sending_identity, content_digest, ..} => format!("Send this invoice for account {account} to {target_name} <{target}> from {sending_identity}. Exact content SHA-256 {content_digest}. No Undo."),
        ExactAction::UnsubscribeLocalMail {account, target, sending_identity, receiving_identity, subject, content_digest, ..} => format!("Send this unsubscribe message for account {account} to {target} from {sending_identity}, received at {receiving_identity}. Subject {subject}. Exact content SHA-256 {content_digest}. Success unknown. No Undo."),
        ExactAction::SubscriptionAction {account, subscription, method, receiving_identity, host, content_digest, effect, ..} => format!("Apply synthetic {} for subscription {subscription}, account {account}, received at {receiving_identity}, host {}. Payload SHA-256 {content_digest}. {effect}", serde_json::to_string(method).expect("Method encoding"), host.as_deref().unwrap_or("none")),
    }).collect::<Vec<_>>().join("\n")
}

struct Pending {
    request: Request,
    plan: Plan,
    created: Instant,
}
pub(crate) struct Boundary {
    pending: BTreeMap<String, Pending>,
    expected_helper: Option<ExpectedHelper>,
}
#[derive(Clone)]
struct ExpectedHelper {
    path: String,
    signing_requirement: String,
    owner: u32,
}
impl Default for Boundary {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            expected_helper: None,
        }
    }
}

/// This capability is produced only after consuming and revalidating consent.
pub(crate) struct Confirmed {
    exact_receipt: String,
}
impl Confirmed {
    pub fn receipt(&self) -> &str {
        &self.exact_receipt
    }
}

impl Boundary {
    pub fn synthetic() -> Self {
        Self {
            pending: BTreeMap::new(),
            expected_helper: Some(ExpectedHelper {
                path: "in-process synthetic helper".into(),
                signing_requirement: "synthetic fixture identity only".into(),
                owner: 0,
            }),
        }
    }
    pub fn offer(&mut self, plan: Plan) -> Result<(String, Request), String> {
        if self.expected_helper.is_none() {
            return Err(NATIVE_GAP.into());
        }
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
            synthetic: true,
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

    pub fn confirm(
        &mut self,
        handle: &str,
        current: &Plan,
        backend: &mut impl Backend,
        revalidate: impl FnOnce() -> Result<Plan, String>,
    ) -> Result<Confirmed, String> {
        let pending = self
            .pending
            .remove(handle)
            .ok_or("Unknown or consumed confirmation request")?;
        if pending.created.elapsed() >= LIFETIME || pending.plan != *current {
            return Err("Expired or changed confirmation plan".into());
        }
        let executable = backend.verify_expected_identity()?;
        let expected = self.expected_helper.as_ref().ok_or(NATIVE_GAP)?;
        if executable.path != expected.path
            || executable.signing_requirement != expected.signing_requirement
            || executable.owner != expected.owner
            || executable.file_identity.is_empty()
        {
            return Err("Unexpected helper identity, location or ownership".into());
        }
        let challenge = Zeroizing::new(random());
        let response = backend.confirm(&executable, &pending.request, &challenge)?;
        if response.executable != executable {
            return Err("Verified helper was substituted".into());
        }
        let outcome = authenticate(&pending.request, &challenge, &response.bytes)?;
        if outcome != Outcome::Approved {
            return Err("Confirmation cancelled".into());
        }
        if pending.created.elapsed() >= LIFETIME || revalidate()? != pending.plan {
            return Err("Plan changed after confirmation".into());
        }
        Ok(Confirmed {
            exact_receipt: pending.plan.exact_receipt,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedExecutable {
    pub path: String,
    pub signing_requirement: String,
    pub owner: u32,
    pub file_identity: String,
}
pub(crate) struct Response {
    pub executable: VerifiedExecutable,
    pub bytes: Vec<u8>,
}
pub(crate) trait Backend {
    fn verify_expected_identity(&mut self) -> Result<VerifiedExecutable, String>;
    fn confirm(
        &mut self,
        executable: &VerifiedExecutable,
        request: &Request,
        challenge: &[u8; 32],
    ) -> Result<Response, String>;
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
fn authenticate(request: &Request, key: &[u8; 32], bytes: &[u8]) -> Result<Outcome, String> {
    if bytes.len() > 4096 {
        return Err("Oversized confirmation outcome".into());
    }
    let wire: WireOutcome =
        serde_json::from_slice(bytes).map_err(|_| "Malformed confirmation outcome")?;
    let request_digest = digest(&serde_json::to_vec(request).map_err(|_| "Request encoding")?);
    let expected = tag(key, &request.request_id, &request_digest, wire.outcome);
    if wire.request_id != request.request_id
        || wire.request_digest != request_digest
        || wire.authentication.len() != 64
        || !bool::from(expected.as_bytes().ct_eq(wire.authentication.as_bytes()))
    {
        return Err("Unauthenticated confirmation outcome".into());
    }
    Ok(wire.outcome)
}

pub(crate) struct SyntheticBackend;
impl Backend for SyntheticBackend {
    fn verify_expected_identity(&mut self) -> Result<VerifiedExecutable, String> {
        Ok(VerifiedExecutable {
            path: "in-process synthetic helper".into(),
            signing_requirement: "synthetic fixture identity only".into(),
            owner: 0,
            file_identity: "installed-fake-v1".into(),
        })
    }
    fn confirm(
        &mut self,
        executable: &VerifiedExecutable,
        request: &Request,
        challenge: &[u8; 32],
    ) -> Result<Response, String> {
        let request_digest = digest(&serde_json::to_vec(request).map_err(|_| "Request encoding")?);
        let outcome = Outcome::Approved;
        let authentication = tag(challenge, &request.request_id, &request_digest, outcome);
        let bytes = serde_json::to_vec(&WireOutcome {
            request_id: request.request_id.clone(),
            request_digest,
            outcome,
            authentication,
        })
        .map_err(|_| "Outcome encoding")?;
        Ok(Response {
            executable: executable.clone(),
            bytes,
        })
    }
}

/// No path-based spawn is allowed while executable continuity is unproven.
pub(crate) struct MacOsBackend;
impl MacOsBackend {
    pub fn unavailable_reason(&self) -> &'static str {
        NATIVE_GAP
    }
}
impl Backend for MacOsBackend {
    fn verify_expected_identity(&mut self) -> Result<VerifiedExecutable, String> {
        Err(NATIVE_GAP.into())
    }
    fn confirm(
        &mut self,
        _: &VerifiedExecutable,
        _: &Request,
        _: &[u8; 32],
    ) -> Result<Response, String> {
        Err(NATIVE_GAP.into())
    }
}

#[cfg(test)]
mod tests;
