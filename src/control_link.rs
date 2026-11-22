//! The app's link to its server's control socket (docs/server-plan.md,
//! phase 2; abromux_server::control).
//!
//! The server owns `abromux.sock`, so the CLI, agent hooks and the phone app
//! reach it whether or not the app is open. While the app runs, it's
//! attached (`{"op":"app"}`): for each control connection the server says
//! `{"open":N}`, the app opens its side (`{"op":"control","conn":N}`) and
//! serves it with the same code that served the socket when the app owned
//! it (`server::serve`), while the server splices the bytes. Connection 0 is
//! the server replaying what it kept while the app was away.
//!
//! The app tells the server about its workspaces (`{"op":"state"}`) as they
//! change, so the server can answer `list-workspaces` and know which
//! workspace an agent's report is about while the app is closed.
//!
//! If the server can't do this (one from before the control socket, or one
//! serving another socket), [`attach`] says so and the app binds the socket
//! itself, as before.

use crate::server::Request;
use abromux_term::hold;
use futures::channel::mpsc;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where state goes to the server: the link's writer while attached.
#[derive(Clone, Default)]
pub struct Link {
    writer: Arc<Mutex<Option<UnixStream>>>,
    /// What was last sent, to send only changes.
    last: Arc<Mutex<Option<Value>>>,
}

impl Link {
    /// Tell the server about the workspaces (`server::describe_ws` with
    /// their surfaces) and the session, if they changed.
    pub fn push_state(&self, workspaces: Value, session: Value) {
        let msg = json!({ "op": "state", "workspaces": workspaces, "session": session });
        let mut last = self.last.lock().unwrap();
        if last.as_ref() == Some(&msg) {
            return;
        }
        let mut w = self.writer.lock().unwrap();
        if let Some(s) = w.as_mut()
            && writeln!(s, "{msg}").is_ok()
        {
            *last = Some(msg);
        }
    }
}

static CURRENT: std::sync::OnceLock<Link> = std::sync::OnceLock::new();

/// The link, once attached.
pub fn current() -> Option<&'static Link> {
    CURRENT.get()
}

/// Attach to the server as the app, serving its control connections on
/// `tx`. None if it can't (the app then binds `control` itself).
pub fn attach(control: &Path, tx: mpsc::UnboundedSender<Request>) -> Option<Link> {
    let (reader, writer) = connect(control)
        .map_err(|e| eprintln!("abromux: control socket: {e}"))
        .ok()?;
    let link = Link::default();
    *link.writer.lock().unwrap() = Some(writer);
    let l = link.clone();
    let control = control.to_path_buf();
    std::thread::spawn(move || run(reader, control, tx, l));
    let _ = CURRENT.set(link.clone());
    Some(link)
}

/// The server's socket, its hello checked, attached as the app.
fn connect(control: &Path) -> Result<(BufReader<UnixStream>, UnixStream), String> {
    let socket = hold::socket_path();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let conn = abromux_server::connect_or_start(&socket, &exe, &hold::server_args())
        .map_err(|e| format!("can't start abromux server: {e}"))?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut w = conn.try_clone().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(conn);
    let mut ask = |req: Value| -> Result<Value, String> {
        writeln!(w, "{req}").map_err(|e| e.to_string())?;
        let mut line = String::new();
        r.read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    };
    let hello = ask(json!({ "op": "hello" }))?;
    if hello["protocol"].as_u64().unwrap_or(1) < abromux_core::wire::CONTROL {
        return Err("the running server is older than the control socket".into());
    }
    if hello["control"].as_str().map(PathBuf::from).as_deref() != Some(control) {
        return Err(format!(
            "the running server serves {} as its control socket, not {}",
            hello["control"],
            control.display()
        ));
    }
    let app = ask(json!({ "op": "app" }))?;
    if app["ok"] != true {
        return Err(app["error"].as_str().unwrap_or("refused").to_string());
    }
    r.get_ref().set_read_timeout(None).ok();
    Ok((r, w))
}

/// Serve the link: open the app's side of each connection the server hands
/// over. When the link breaks (the server restarted), attach again.
fn run(
    mut reader: BufReader<UnixStream>,
    control: PathBuf,
    tx: mpsc::UnboundedSender<Request>,
    link: Link,
) {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(n) if n > 0 => {
                let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if let Some(conn) = msg["open"].as_u64() {
                    let tx = tx.clone();
                    std::thread::spawn(move || open(conn, tx));
                }
                continue;
            }
            _ => {}
        }
        if tx.is_closed() {
            return; // the app is quitting
        }
        // The server went away: come back to the next one.
        *link.writer.lock().unwrap() = None;
        *link.last.lock().unwrap() = None;
        let mut delay = Duration::from_millis(200);
        loop {
            std::thread::sleep(delay);
            if tx.is_closed() {
                return;
            }
            if let Ok((r, w)) = connect(&control) {
                reader = r;
                *link.writer.lock().unwrap() = Some(w);
                break;
            }
            delay = (delay * 2).min(Duration::from_secs(10));
        }
    }
}

/// The app's side of control connection `conn`, served as the socket's own
/// connections were.
fn open(conn: u64, tx: mpsc::UnboundedSender<Request>) {
    let Ok(mut s) = UnixStream::connect(hold::socket_path()) else {
        return;
    };
    if writeln!(s, "{}", json!({ "op": "control", "conn": conn })).is_err() {
        return;
    }
    crate::server::serve(s, tx);
}
