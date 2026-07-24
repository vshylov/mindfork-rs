//! Running the application as a single instance.
//! See spec §1.4 (single instance) and plan M0.
//!
//! The lock is OS-level: a named mutex on Windows, an abstract unix socket on
//! Linux (a detail of the `single-instance` crate). The unprefixed name lands
//! in the current login session's namespace — enough to keep one user from
//! launching the app twice; the binary's path doesn't affect the lock (two
//! copies in different directories still conflict by name).

use single_instance::SingleInstance;
use thiserror::Error;

/// Unique lock identifier.
const INSTANCE_ID: &str = "mindfork-rs-single-instance";

/// Holder of the single-instance lock.
///
/// Must live for the process's whole run: on its `drop` the lock is released
/// and a new instance can be launched.
pub struct InstanceGuard {
    _inner: SingleInstance,
}

/// Error acquiring the single-instance lock.
#[derive(Debug, Error)]
pub enum InstanceError {
    /// The application is already running as another instance — not a
    /// failure, but a launch refusal.
    #[error("application is already running")]
    AlreadyRunning,
    /// Failed to initialize the lock (a system error from the crate).
    #[error("failed to initialize the single-instance lock: {0}")]
    Init(String),
}

/// Attempts to acquire the single-instance lock.
///
/// `Err(InstanceError::AlreadyRunning)` — the application is already running
/// (the caller should show a message and exit); `Err(InstanceError::Init)` —
/// a genuine initialization failure.
pub fn acquire() -> Result<InstanceGuard, InstanceError> {
    acquire_named(INSTANCE_ID)
}

/// Implementation of `acquire` with an explicit name — for tests (so as not
/// to conflict with the real lock of a running app instance on the same
/// machine).
fn acquire_named(name: &str) -> Result<InstanceGuard, InstanceError> {
    let inner = SingleInstance::new(name).map_err(|e| InstanceError::Init(e.to_string()))?;
    if !inner.is_single() {
        return Err(InstanceError::AlreadyRunning);
    }
    Ok(InstanceGuard { _inner: inner })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_reports_already_running() {
        // A per-test unique name — doesn't touch the app's real lock.
        let name = "mindfork-rs-test-second-acquire-reports-already-running";

        let first = acquire_named(name).expect("first acquire should succeed");
        match acquire_named(name) {
            Err(InstanceError::AlreadyRunning) => {}
            Err(other) => panic!("expected AlreadyRunning, got: {other:?}"),
            Ok(_) => panic!("second acquire should not succeed while the first is alive"),
        }

        // After the first lock is released, acquiring again is possible.
        drop(first);
        let _again = acquire_named(name).expect("after drop, acquiring again is possible");
    }
}
