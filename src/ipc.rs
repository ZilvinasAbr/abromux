//! The control socket: where it is, and calls over it. The protocol itself
//! is `abromux_core::ipc`.

use abromux_core::ipc::CallError;
pub use abromux_core::ipc::{err, err_code, ok};
use serde_json::Value;
use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::atomic::{AtomicI32, Ordering},
};

/// The CLI's exit status when a command fails (see `cli::EXIT_STATUS`):
/// set by a call that failed in a way that has its own status.
static FAILED_WITH: AtomicI32 = AtomicI32::new(1);

/// The exit status for the failure so far: 3 for a target that wasn't
/// found or was ambiguous, 4 when abromux isn't running, else 1.
pub fn failure_status() -> i32 {
    FAILED_WITH.load(Ordering::Relaxed)
}

pub fn set_failure_status(status: i32) {
    FAILED_WITH.store(status, Ordering::Relaxed);
}

/// `$ABROMUX_SOCKET_PATH`, else `abromux.sock` in the runtime folder:
/// `~/Library/Application Support/abromux/` on macOS,
/// `$XDG_RUNTIME_DIR/abromux/` on Linux (`abromux-nightly/` for the nightly).
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("ABROMUX_SOCKET_PATH") {
        return p.into();
    }
    crate::channel::runtime_dir().join("abromux.sock")
}

/// One request/response round trip.
pub fn call(method: &str, params: Value) -> anyhow::Result<Value> {
    round_trip(method, params).map(|(v, _)| v)
}

/// `call`, keeping the connection open until the returned stream is
/// dropped: the app can wait for that to know the caller is done (has
/// printed what it got) before doing something that would end it.
pub fn call_held(method: &str, params: Value) -> anyhow::Result<(Value, UnixStream)> {
    round_trip(method, params)
}

fn round_trip(method: &str, params: Value) -> anyhow::Result<(Value, UnixStream)> {
    abromux_core::ipc::round_trip(&socket_path(), method, params, None).inspect_err(|e| {
        match e.downcast_ref::<CallError>() {
            Some(CallError::NotRunning(_)) => set_failure_status(4),
            Some(CallError::Refused { code: Some(c), .. }) if c == "target" => {
                set_failure_status(3)
            }
            _ => {}
        }
    })
}
