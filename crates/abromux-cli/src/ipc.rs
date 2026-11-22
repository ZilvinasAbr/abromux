//! Calls over the control socket, as the CLI makes them: the socket is the
//! [`Host`](crate::Host)'s, and a failure sets the CLI's exit status.

use abromux_core::ipc::CallError;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

/// The CLI's exit status when a command fails (see `EXIT_STATUS`): set by a
/// call that failed in a way that has its own status.
static FAILED_WITH: AtomicI32 = AtomicI32::new(1);

/// The exit status for the failure so far: 3 for a target that wasn't
/// found or was ambiguous, 4 when abromux isn't running, else 1.
pub fn failure_status() -> i32 {
    FAILED_WITH.load(Ordering::Relaxed)
}

pub fn set_failure_status(status: i32) {
    FAILED_WITH.store(status, Ordering::Relaxed);
}

/// The control socket: `$ABROMUX_SOCKET_PATH` (set by `--socket` and
/// `--server` too) as the host reads it.
pub fn socket_path() -> PathBuf {
    crate::host().socket_path()
}

/// One request/response round trip.
pub fn call(method: &str, params: Value) -> anyhow::Result<Value> {
    abromux_core::ipc::round_trip(&socket_path(), method, params, None)
        .map(|(v, _)| v)
        .inspect_err(|e| match e.downcast_ref::<CallError>() {
            Some(CallError::NotRunning(_)) => set_failure_status(4),
            Some(CallError::Refused { code: Some(c), .. }) if c == "target" => {
                set_failure_status(3)
            }
            _ => {}
        })
}
