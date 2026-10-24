//! Control-socket protocol shared by the app and the `abromux` CLI: one JSON
//! object per line over a Unix socket.
//!
//! request:  `{"id":1,"method":"workspace.list","params":{...}}`
//! response: `{"id":1,"ok":true,"result":...}` or `{"id":1,"ok":false,"error":"..."}`

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

/// `$ABROMUX_SOCKET_PATH`, else `~/Library/Application Support/abromux/abromux.sock`.
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("ABROMUX_SOCKET_PATH") {
        return p.into();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/Application Support/abromux/abromux.sock")
}

pub fn ok(id: &Value, result: Value) -> Value {
    json!({ "id": id, "ok": true, "result": result })
}

pub fn err(id: &Value, error: impl std::fmt::Display) -> Value {
    json!({ "id": id, "ok": false, "error": error.to_string() })
}

/// One request/response round trip.
pub fn call(method: &str, params: Value) -> anyhow::Result<Value> {
    round_trip(method, params, None)
}

/// `call`, giving up after `timeout` (for hooks that mustn't stall their caller).
pub fn call_timeout(method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
    round_trip(method, params, Some(timeout))
}

fn round_trip(method: &str, params: Value, timeout: Option<Duration>) -> anyhow::Result<Value> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).map_err(|e| anyhow::anyhow!("abromux is not running ({}: {e})", path.display()))?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut writer = stream.try_clone()?;
    let mut line = serde_json::to_string(&json!({ "id": 1, "method": method, "params": params }))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply: Value = serde_json::from_str(&reply).map_err(|e| anyhow::anyhow!("bad reply from abromux: {e}"))?;
    if reply["ok"].as_bool() == Some(true) {
        Ok(reply["result"].clone())
    } else {
        anyhow::bail!("{}", reply["error"].as_str().unwrap_or("unknown error"))
    }
}
