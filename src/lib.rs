pub mod acquisition;
pub mod backlog;
pub mod census;
pub(crate) mod confirmation;
pub mod engine;
#[cfg(test)]
mod fake_tls;
mod file_lock;
pub mod invoice;
pub mod ledger;
pub(crate) mod native;
pub(crate) mod original;
#[cfg(target_os = "macos")]
pub mod owner;
#[cfg(unix)]
mod private_fs;

/// Sets and verifies a zero core-file limit. Call before anything can hold a secret.
pub fn disable_core_dumps() -> bool {
    native::disable_core_dumps()
}
pub mod policy;
pub mod rules;
pub(crate) mod send;
pub mod sender_data;
pub mod server;
pub mod unsubscribe;
pub mod winddown;
