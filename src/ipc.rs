//! The control socket: where it is, and calls over it. The protocol itself
//! is `abromux_core::ipc`.

use abromux_core::ipc::CallError;
pub use abromux_core::ipc::{err, err_code, ok};
use serde_json::Value;
use std::{
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
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
///
/// A `$ABROMUX_SOCKET_PATH` that is another channel's own socket (a shell in
/// the release app starting the nightly, or `abromux-nightly` run there) is
/// not this app's, so it's ignored. A path set by hand (perf.py, a test
/// instance) still counts.
pub fn socket_path() -> PathBuf {
    let own = crate::channel::runtime_dir().join("abromux.sock");
    match std::env::var_os("ABROMUX_SOCKET_PATH") {
        Some(p) if !foreign(Path::new(&p), &own) => p.into(),
        _ => own,
    }
}

/// `path` is the default socket of another channel: `abromux.sock` in a
/// sibling of this channel's runtime folder named `abromux…`.
fn foreign(path: &Path, own: &Path) -> bool {
    let (Some(dir), Some(own_dir)) = (path.parent(), own.parent()) else {
        return false;
    };
    path.file_name() == own.file_name()
        && dir != own_dir
        && dir.parent() == own_dir.parent()
        && dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("abromux"))
}

/// The app starting from a shell of another channel's app: drop what that
/// shell had from it (its socket, surface, workspace and CLI), so this
/// app's terminals and calls don't reach the other one.
pub fn forget_foreign_env() {
    let own = crate::channel::runtime_dir().join("abromux.sock");
    if std::env::var_os("ABROMUX_SOCKET_PATH").is_some_and(|p| foreign(Path::new(&p), &own)) {
        for k in [
            "ABROMUX_SOCKET_PATH",
            "ABROMUX_SURFACE_ID",
            "ABROMUX_WORKSPACE_ID",
            "ABROMUX_WORKSPACE_DIR",
            "ABROMUX_BIN",
        ] {
            // SAFETY: called first thing in main, before any thread starts.
            unsafe { std::env::remove_var(k) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::foreign;
    use std::path::Path;

    #[test]
    fn other_channels_socket() {
        let own = Path::new("/S/abromux-nightly-zoom/abromux.sock");
        assert!(foreign(Path::new("/S/abromux/abromux.sock"), own));
        assert!(foreign(Path::new("/S/abromux-nightly/abromux.sock"), own));
        assert!(!foreign(own, own));
        assert!(!foreign(Path::new("/tmp/x/abromux.sock"), own));
        assert!(!foreign(Path::new("/S/other/abromux.sock"), own));
        assert!(!foreign(Path::new("/S/abromux/other.sock"), own));
    }
}

/// One request/response round trip.
pub fn call(method: &str, params: Value) -> anyhow::Result<Value> {
    round_trip(method, params).map(|(v, _)| v)
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
