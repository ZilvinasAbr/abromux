//! The control socket: where it is, and calls over it. The protocol itself
//! is `abromux_core::ipc`.

pub use abromux_core::ipc::{err, ok};
use serde_json::Value;
use std::{os::unix::net::UnixStream, path::PathBuf};

/// `$ABROMUX_SOCKET_PATH`, else `~/Library/Application Support/abromux/abromux.sock`
/// (`abromux-nightly/` for the nightly app).
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("ABROMUX_SOCKET_PATH") {
        return p.into();
    }
    crate::channel::support_dir().join("abromux.sock")
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
    abromux_core::ipc::round_trip(&socket_path(), method, params, None)
}
