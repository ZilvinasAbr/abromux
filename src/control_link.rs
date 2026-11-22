//! The app's links to servers' control sockets (docs/server-plan.md, phases
//! 2 and 3; abromux_server::control).
//!
//! A server owns `abromux.sock`, so the CLI, agent hooks and the phone app
//! reach it whether or not the app is open. While the app runs, it's
//! attached (`{"op":"app"}`): for each control connection the server says
//! `{"open":N}`, the app opens its side (`{"op":"control","conn":N}`) and
//! serves it with the same code that served the socket when the app owned
//! it (`server::serve`), while the server splices the bytes. Connection 0 is
//! the server replaying what it kept while the app was away.
//!
//! The app links to its own server (local), and to the server of every
//! host its workspaces and tabs are on (remote, over ssh): an agent hook on
//! a VPS calls that machine's control socket, and its report comes here, or
//! waits there while the app is closed.
//!
//! The app tells each server about its workspaces there (`{"op":"state"}`)
//! as they change, so it can answer `list-workspaces` and know which
//! workspace an agent's report is about while the app is closed, and so
//! the workspaces can be opened again from it (`get-state`).
//!
//! If the local server can't do this (one from before the control socket,
//! or one serving another socket), [`attach_local`] says so and the app
//! binds the socket itself, as before.

use crate::server::Request;
use abromux_term::hold;
use futures::channel::mpsc;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Which server a link is to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Target {
    /// This app's own server, serving this control socket.
    Local(PathBuf),
    /// The server at this host address (`ssh://..`).
    Remote(String),
}

/// One link: where state goes to its server, and whether it's up.
#[derive(Clone, Default)]
pub struct Link {
    writer: Arc<Mutex<Option<UnixStream>>>,
    /// What was last sent, to send only changes.
    last: Arc<Mutex<Option<Value>>>,
    up: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// The app's replica of this server's model (abromux_model): changed
    /// only by what the server sends (its model when the app attaches, and
    /// the stamped events it applies), on the app's main thread, together
    /// with the views (`model.*` in server.rs). The server is the source.
    model: Arc<Mutex<abromux_model::Model>>,
    /// The last changes proposed and not yet seen applied.
    proposed: Arc<Mutex<Vec<abromux_model::Event>>>,
    resync: Arc<AtomicBool>,
}

impl Link {
    /// Tell the server about its workspaces (`server::control_state`) and
    /// the session (those workspaces of it), if they changed.
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

    /// Propose what the app changed to the server: the events from the
    /// server's model as the app has it to `new`, the app's own state. The
    /// server orders and applies them, and sends them back stamped
    /// (`applied`), which is when the app's replica takes them.
    pub fn push_model(&self, new: abromux_model::Model) {
        if self.resync.swap(false, Ordering::Relaxed) {
            self.send(&json!({ "op": "sync" }));
            return;
        }
        let events = abromux_model::diff(&self.model.lock().unwrap(), &new);
        let mut proposed = self.proposed.lock().unwrap();
        if events.is_empty() || *proposed == events {
            return;
        }
        if self.send(&json!({ "op": "change", "events": events })) {
            *proposed = events;
        }
    }

    fn send(&self, msg: &Value) -> bool {
        self.writer
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|s| writeln!(s, "{msg}").is_ok())
    }

    /// A model method the server had the app carry out is done: its result,
    /// and the change it made to the model (`new` is the app's model now),
    /// for the server to commit before the caller hears back.
    pub fn done(&self, token: &Value, result: Result<Value, String>, new: &abromux_model::Model) {
        let events = abromux_model::diff(&self.model.lock().unwrap(), new);
        let msg = match result {
            Ok(v) => {
                json!({ "op": "done", "token": token, "ok": true, "result": v, "events": events })
            }
            Err(e) => {
                json!({ "op": "done", "token": token, "ok": false, "error": e, "events": events })
            }
        };
        if self.send(&msg) && !events.is_empty() {
            *self.proposed.lock().unwrap() = events;
        }
    }

    /// The server's model (when the app attaches, or asked): the replica
    /// starts over from it.
    pub fn set_replica(&self, model: abromux_model::Model) {
        *self.model.lock().unwrap() = model;
        self.proposed.lock().unwrap().clear();
    }

    /// Events the server applied: the replica follows; on a gap it asks for
    /// the model.
    pub fn apply_replica(&self, events: &[abromux_model::Stamped]) {
        let mut m = self.model.lock().unwrap();
        for e in events {
            if m.apply(e).is_err() {
                self.resync.store(true, Ordering::Relaxed);
                return;
            }
        }
        self.proposed.lock().unwrap().clear();
    }

    pub fn is_up(&self) -> bool {
        self.up.load(Ordering::Relaxed)
    }

    /// Tell the server what the app parses of the terminals it holds
    /// (titles, folders), if it changed: it lists them for other apps.
    pub fn push_terms(&self, terms: Value) {
        let msg = json!({ "op": "terms", "terms": terms });
        static LAST: Mutex<Option<Value>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap();
        if last.as_ref() == Some(&msg) {
            return;
        }
        if let Some(s) = self.writer.lock().unwrap().as_mut()
            && writeln!(s, "{msg}").is_ok()
        {
            *last = Some(msg);
        }
    }
}

/// Where control requests go to be answered (`server::serve_requests`).
static TX: OnceLock<mpsc::UnboundedSender<Request>> = OnceLock::new();
static LOCAL: OnceLock<Link> = OnceLock::new();
static REMOTES: Mutex<Option<HashMap<String, Link>>> = Mutex::new(None);

/// Where requests from servers' control connections go. Once, at launch.
pub fn init(tx: mpsc::UnboundedSender<Request>) {
    let _ = TX.set(tx);
}

/// The link to this app's own server, once attached.
pub fn current() -> Option<&'static Link> {
    LOCAL.get()
}

/// Attach to this app's server as the app. False if it can't: the app then
/// binds `control` itself.
pub fn attach_local(control: &Path) -> bool {
    let target = Target::Local(control.to_path_buf());
    match connect(&target) {
        Ok((reader, writer)) => {
            let link = Link::default();
            *link.writer.lock().unwrap() = Some(writer);
            link.up.store(true, Ordering::Relaxed);
            link.resync.store(true, Ordering::Relaxed);
            let l = link.clone();
            std::thread::spawn(move || run(reader, target, l));
            let _ = LOCAL.set(link);
            true
        }
        Err(e) => {
            eprintln!("abromux: control socket: {e}");
            false
        }
    }
}

/// Keep a link to the server of each of `hosts` (the hosts the app's
/// workspaces and tabs are on), and drop the others.
pub fn sync_remotes(hosts: &[String]) {
    let mut g = REMOTES.lock().unwrap();
    let map = g.get_or_insert_default();
    map.retain(|h, l| {
        let keep = hosts.contains(h);
        if !keep {
            l.stop.store(true, Ordering::Relaxed);
            if let Some(w) = l.writer.lock().unwrap().take() {
                let _ = w.shutdown(std::net::Shutdown::Both);
            }
        }
        keep
    });
    for h in hosts {
        if map.contains_key(h) {
            continue;
        }
        let link = Link::default();
        map.insert(h.clone(), link.clone());
        let target = Target::Remote(h.clone());
        std::thread::spawn(move || {
            // Connect off the main thread; ssh can take a while.
            let mut reader = None;
            let mut delay = Duration::from_millis(500);
            while reader.is_none() && !link.stop.load(Ordering::Relaxed) {
                match connect(&target) {
                    Ok((r, w)) => {
                        *link.writer.lock().unwrap() = Some(w);
                        link.up.store(true, Ordering::Relaxed);
                        link.resync.store(true, Ordering::Relaxed);
                        reader = Some(r);
                    }
                    Err(_) => {
                        std::thread::sleep(delay);
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                }
            }
            if let Some(r) = reader {
                run(r, target, link);
            }
        });
    }
}

/// A link by name: `local`, or a host.
pub fn by_name(name: &str) -> Option<Link> {
    if name == "local" {
        return current().cloned();
    }
    remote(name)
}

/// The hosts the app keeps links to.
pub fn remote_hosts() -> Vec<String> {
    REMOTES
        .lock()
        .unwrap()
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// The link to the server at `host`, if the app keeps one.
pub fn remote(host: &str) -> Option<Link> {
    REMOTES.lock().unwrap().as_ref()?.get(host).cloned()
}

/// Whether the server at `host` is reachable now: None when the app has
/// no link to it (yet).
pub fn host_up(host: &str) -> Option<bool> {
    remote(host).map(|l| l.is_up())
}

/// A fresh connection to `target`'s server socket.
fn dial(target: &Target, start: bool) -> Result<(UnixStream, Option<Value>), String> {
    match target {
        Target::Local(_) => {
            let socket = hold::socket_path();
            let s = if start {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                abromux_server::connect_or_start(&socket, &exe, &hold::server_args())
            } else {
                UnixStream::connect(&socket)
            };
            s.map(|s| (s, None))
                .map_err(|e| format!("can't reach abromux server: {e}"))
        }
        Target::Remote(host) => {
            abromux_term::remote::open_stream(host, None).map(|(s, hello)| (s, Some(hello)))
        }
    }
}

/// The server's socket, its hello checked, attached as the app.
fn connect(target: &Target) -> Result<(BufReader<UnixStream>, UnixStream), String> {
    let (conn, hello) = dial(target, true)?;
    conn.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut w = conn.try_clone().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(conn);
    let mut ask = |req: Value| -> Result<Value, String> {
        writeln!(w, "{req}").map_err(|e| e.to_string())?;
        let mut line = String::new();
        r.read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| format!("{e}: {line:?}"))
    };
    let hello = match hello {
        Some(h) => h,
        None => ask(json!({ "op": "hello" }))?,
    };
    if hello["protocol"].as_u64().unwrap_or(1) < abromux_core::wire::CONTROL {
        return Err("the server is older than the control socket; update it".into());
    }
    match target {
        Target::Local(control)
            if hello["control"].as_str().map(PathBuf::from).as_deref() != Some(control) =>
        {
            return Err(format!(
                "the running server serves {} as its control socket, not {}",
                hello["control"],
                control.display()
            ));
        }
        Target::Remote(_) if hello["control"].is_null() => {
            return Err("the server there has no control socket".into());
        }
        _ => {}
    }
    let app = ask(json!({ "op": "app", "owner": abromux_term::remote::owner() }))?;
    if app["ok"] != true {
        return Err(app["error"].as_str().unwrap_or("refused").to_string());
    }
    // On another machine, a dev server starting is worth forwarding: have
    // the server say when one does.
    if let Target::Remote(_) = target {
        writeln!(w, "{}", json!({ "op": "watch-ports" })).map_err(|e| e.to_string())?;
    }
    r.get_ref().set_read_timeout(None).ok();
    Ok((r, w))
}

/// Serve the link: open the app's side of each connection the server hands
/// over. When the link breaks (the server restarted, the network), attach
/// again.
fn run(mut reader: BufReader<UnixStream>, target: Target, link: Link) {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(n) if n > 0 => {
                let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if let Some(conn) = msg["open"].as_u64() {
                    let target = target.clone();
                    std::thread::spawn(move || open(&target, conn));
                }
                // Another app wants a terminal this one holds: stream it;
                // and once it's gone again, hold it again.
                let ask = |method: &str, id: &str| {
                    if let Some(tx) = TX.get() {
                        use futures::FutureExt as _;
                        let (reply, _) = futures::channel::oneshot::channel();
                        let (_open, gone) = futures::channel::oneshot::channel::<()>();
                        let req = json!({ "id": 0, "method": method, "params": { "held": id } });
                        let _ = tx.unbounded_send((req, reply, gone.shared()));
                    }
                };
                // The server's model, its changes, and the ones it made
                // itself for the app to mirror: on the main thread, where
                // the views and the replica change together.
                let name = match &target {
                    Target::Local(_) => "local".to_string(),
                    Target::Remote(h) => h.clone(),
                };
                let inject = |method: &str, params: Value| {
                    if let Some(tx) = TX.get() {
                        use futures::FutureExt as _;
                        let (reply, _) = futures::channel::oneshot::channel();
                        let (_open, gone) = futures::channel::oneshot::channel::<()>();
                        let req = json!({ "id": 0, "method": method, "params": params });
                        let _ = tx.unbounded_send((req, reply, gone.shared()));
                    }
                };
                if !msg["model"].is_null() {
                    inject(
                        "model.snapshot",
                        json!({ "link": name, "model": msg["model"] }),
                    );
                }
                if !msg["mirror"].is_null() {
                    inject(
                        "model.mirror",
                        json!({ "link": name, "call": msg["mirror"], "events": msg["applied"] }),
                    );
                } else if msg["applied"].is_array() {
                    inject(
                        "model.applied",
                        json!({ "link": name, "events": msg["applied"] }),
                    );
                }
                if msg["resync"] == true {
                    link.resync.store(true, Ordering::Relaxed);
                }
                // A model method the server has the app carry out.
                if !msg["exec"].is_null() {
                    inject("model.exec", json!({ "link": name, "exec": msg["exec"] }));
                }
                if msg["ports_new"].is_array() {
                    inject(
                        "forward.offer",
                        json!({ "link": name, "ports": msg["ports_new"] }),
                    );
                }
                if let Some(id) = msg["release"].as_str() {
                    ask("surface.restream", id);
                }
                if let Some(id) = msg["reclaim"].as_str() {
                    ask("surface.reclaim", id);
                }
                continue;
            }
            _ => {}
        }
        link.up.store(false, Ordering::Relaxed);
        *link.writer.lock().unwrap() = None;
        *link.last.lock().unwrap() = None;
        let mut delay = Duration::from_millis(200);
        loop {
            if link.stop.load(Ordering::Relaxed) || TX.get().is_none_or(|t| t.is_closed()) {
                return;
            }
            std::thread::sleep(delay);
            if let Ok((r, w)) = connect(&target) {
                reader = r;
                *link.writer.lock().unwrap() = Some(w);
                link.up.store(true, Ordering::Relaxed);
                link.resync.store(true, Ordering::Relaxed);
                break;
            }
            delay = (delay * 2).min(Duration::from_secs(15));
        }
    }
}

/// The app's side of control connection `conn`, served as the socket's own
/// connections were.
fn open(target: &Target, conn: u64) {
    let Some(tx) = TX.get().cloned() else { return };
    let first = json!({ "op": "control", "conn": conn });
    let s = match target {
        Target::Local(_) => UnixStream::connect(hold::socket_path())
            .ok()
            .and_then(|mut s| {
                writeln!(s, "{first}").ok()?;
                Some(s)
            }),
        Target::Remote(host) => abromux_term::remote::open_stream(host, Some(&first))
            .ok()
            .map(|(s, _)| s),
    };
    if let Some(s) = s {
        crate::server::serve(s, tx);
    }
}
