//! The server's web client (docs/server-plan.md, phase 6): its terminals in
//! a browser, a phone's above all, over the tailnet listener
//! ([`crate::tailnet`], which has already let the caller in).
//!
//! - `GET /`: the page (xterm.js, the same copy the app's phone access
//!   embeds).
//! - `GET /api/terms`: the terminals (`{"op":"terms"}`).
//! - `GET /ws/<id>` (WebSocket): terminal `<id>`, streamed like any other
//!   client's (`attach` with `stream`): the screen so far as VT text (the
//!   server's parsed copy, or what it kept), then its output as binary
//!   messages; the page sends `{"input":".."}` and `{"resize":[cols,rows]}`.
//!
//! A WebSocket must come from this page's own origin, so another site open
//! in the same browser can't reach the terminals.

use crate::tailnet::Who;
use crate::{http, ws};
use abromux_core::wire::{self, Size};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;

const INDEX: &str = include_str!("web/index.html");
const XTERM_JS: &str = include_str!("../../../src/remote/web/vendor/xterm.js");
const XTERM_CSS: &str = include_str!("../../../src/remote/web/vendor/xterm.css");

pub fn serve(conn: TcpStream, socket: &Path, who: &Who) {
    let Ok(req) = http::read(&conn) else { return };
    let path = req.path.clone();
    let _ = match (req.method.as_str(), path.as_str()) {
        ("GET", "/") => http::respond(
            &conn,
            200,
            "text/html; charset=utf-8",
            &[("Cache-Control", "no-store")],
            INDEX.as_bytes(),
        ),
        ("GET", "/xterm.js") => {
            http::respond(&conn, 200, "text/javascript", &[], XTERM_JS.as_bytes())
        }
        ("GET", "/xterm.css") => http::respond(&conn, 200, "text/css", &[], XTERM_CSS.as_bytes()),
        ("GET", "/api/terms") => match ask(socket, &json!({ "op": "terms" })) {
            Ok(v) => http::json(
                &conn,
                200,
                &json!({ "terms": v["terms"], "you": who.login }),
            ),
            Err(e) => http::json(&conn, 502, &json!({ "error": e })),
        },
        ("GET", p) if p.starts_with("/ws/") && req.upgrade() => {
            if !same_origin(&req) {
                return drop(http::refuse(&conn, 403, "not from this page"));
            }
            let id = p.trim_start_matches("/ws/").to_string();
            let Some(key) = req.header("sec-websocket-key").map(String::from) else {
                return drop(http::refuse(&conn, 400, "no key"));
            };
            if ws::handshake(&conn, &key).is_err() {
                return;
            }
            terminal(conn, socket, &id, who);
            Ok(())
        }
        _ => http::refuse(&conn, 404, "nothing here"),
    };
}

/// The page's own origin: `Origin` names the host it was asked of.
fn same_origin(req: &http::Request) -> bool {
    let (Some(origin), Some(host)) = (req.header("origin"), req.header("host")) else {
        return false;
    };
    origin
        .split_once("://")
        .is_some_and(|(_, o)| o.eq_ignore_ascii_case(host))
}

/// One request on the server's socket: its reply.
fn ask(socket: &Path, req: &Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    writeln!(s, "{req}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

/// Terminal `id` over the WebSocket `conn`, until either side ends.
fn terminal(conn: TcpStream, socket: &Path, id: &str, who: &Who) {
    let Ok(writer) = conn.try_clone().map(ws::Writer::new) else {
        return;
    };
    let fail = |why: &str| {
        let _ = writer.text(&json!({ "error": why }).to_string());
        writer.close();
    };
    let Ok(mut server) = UnixStream::connect(socket) else {
        return fail("the server isn't answering");
    };
    let req = json!({
        "op": "attach", "id": id, "stream": true, "vt": true,
        "owner": format!("web:{}", who.login),
    });
    if writeln!(server, "{req}").is_err() {
        return fail("the server isn't answering");
    }
    // The reply line, then the screen so far.
    let mut line = Vec::new();
    let mut b = [0u8];
    while b[0] != b'\n' {
        if server.read(&mut b).unwrap_or(0) == 0 {
            return fail("the server closed the connection");
        }
        line.push(b[0]);
    }
    let reply: Value = serde_json::from_slice(&line).unwrap_or_default();
    if reply["ok"] != true {
        return fail(reply["error"].as_str().unwrap_or("refused"));
    }
    let len = ["screen", "snapshot", "backlog"]
        .iter()
        .map(|k| reply[*k].as_u64().unwrap_or(0) as usize)
        .collect::<Vec<_>>();
    let mut skip_snapshot = vec![0u8; len[1]];
    let mut screen = vec![0u8; len[0]];
    let mut backlog = vec![0u8; len[2]];
    if server.read_exact(&mut screen).is_err()
        || server.read_exact(&mut skip_snapshot).is_err()
        || server.read_exact(&mut backlog).is_err()
    {
        return fail("the server closed the connection");
    }
    screen.extend_from_slice(&backlog);
    if writer.binary(&screen).is_err() {
        return;
    }
    // Its output, to the page.
    let (down, w) = (server.try_clone(), writer.clone());
    let Ok(mut down) = down else { return };
    let reader = std::thread::spawn(move || {
        let mut dec = wire::Decoder::default();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match down.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            dec.push(&buf[..n]);
            while let Ok(Some((kind, data))) = dec.next_frame() {
                match kind {
                    wire::OUTPUT => {
                        if w.binary(&data).is_err() {
                            return;
                        }
                    }
                    wire::EXIT => {
                        let _ = w.text(&json!({ "exit": serde_json::from_slice::<Value>(&data).unwrap_or_default()["code"] }).to_string());
                        w.close();
                        return;
                    }
                    _ => {}
                }
            }
        }
        w.close();
    });
    // The page's typing and size, to the terminal.
    let mut r = ws::Reader::new(conn, writer.clone());
    while let Ok(ws::Message::Text(t)) = r.next() {
        let v: Value = serde_json::from_str(&t).unwrap_or_default();
        let frame = if let Some(input) = v["input"].as_str() {
            wire::frame(wire::INPUT, input.as_bytes())
        } else if let (Some(c), Some(r)) = (v["resize"][0].as_u64(), v["resize"][1].as_u64()) {
            let size = Size {
                cols: c.clamp(2, 1000) as u16,
                rows: r.clamp(2, 1000) as u16,
                width: 0,
                height: 0,
            };
            wire::frame(wire::RESIZE, &size.encode())
        } else {
            continue;
        };
        if server.write_all(&frame).is_err() {
            break;
        }
    }
    let _ = server.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
}
