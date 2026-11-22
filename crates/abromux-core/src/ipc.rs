//! Control-socket protocol shared by the app and the `abromux` CLI: one JSON
//! object per line over a Unix socket.
//!
//! request:  `{"id":1,"method":"workspace.list","params":{...}}`
//! response: `{"id":1,"ok":true,"result":...}` or `{"id":1,"ok":false,"error":"..."}`
//! An error may have a `code`: `target` when a workspace, pane, surface or
//! group wasn't found or was ambiguous.

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

pub fn ok(id: &Value, result: Value) -> Value {
    json!({ "id": id, "ok": true, "result": result })
}

pub fn err(id: &Value, error: impl std::fmt::Display) -> Value {
    json!({ "id": id, "ok": false, "error": error.to_string() })
}

/// `err` with a `code` saying what kind of failure it was.
pub fn err_code(id: &Value, error: impl std::fmt::Display, code: &str) -> Value {
    json!({ "id": id, "ok": false, "error": error.to_string(), "code": code })
}

/// Why a call failed, for the CLI's exit status.
#[derive(Debug)]
pub enum CallError {
    /// Nothing listens on the socket.
    NotRunning(String),
    /// The app answered with an error, and its `code` if it gave one.
    Refused {
        message: String,
        code: Option<String>,
    },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotRunning(m) | CallError::Refused { message: m, .. } => f.write_str(m),
        }
    }
}

impl std::error::Error for CallError {}

/// One request/response round trip over the socket at `path`, giving up
/// after `timeout` if there is one. The connection stays open until the
/// returned stream is dropped.
pub fn round_trip(
    path: &Path,
    method: &str,
    params: Value,
    timeout: Option<Duration>,
) -> anyhow::Result<(Value, UnixStream)> {
    let stream = UnixStream::connect(path).map_err(|e| {
        CallError::NotRunning(format!("abromux is not running ({}: {e})", path.display()))
    })?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut writer = stream.try_clone()?;
    let mut line = serde_json::to_string(&json!({ "id": 1, "method": method, "params": params }))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    let reply: Value =
        serde_json::from_str(&reply).map_err(|e| anyhow::anyhow!("bad reply from abromux: {e}"))?;
    if reply["ok"].as_bool() == Some(true) {
        Ok((reply["result"].clone(), stream))
    } else if reply["code"] == "app_closed" {
        // The server answered for the app, which isn't open.
        Err(CallError::NotRunning(
            reply["error"]
                .as_str()
                .unwrap_or("abromux is not running")
                .to_string(),
        )
        .into())
    } else {
        Err(CallError::Refused {
            message: reply["error"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string(),
            code: reply["code"].as_str().map(String::from),
        }
        .into())
    }
}
