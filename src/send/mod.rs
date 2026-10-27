//! Real send adapters: SMTP submission and mail-app handoff.
//! Only tests reach this until production wiring passes secret-flow review and
//! owner approval.
#![cfg_attr(not(test), allow(dead_code))]
pub(crate) mod handoff;
pub(crate) mod smtp;
mod smtp_secret;
