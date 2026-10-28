//! Real send adapters: SMTP submission, mail-app handoff and the policy-gated
//! wiring that production routes reach them through.
pub(crate) mod handoff;
pub(crate) mod smtp;
mod smtp_secret;
pub(crate) mod wiring;

/// What the owner recorded for a send whose outcome was uncertain, after checking
/// the Sent folder. Recording never sends anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Sent,
    NotSent,
}
