//! Phone access: control abromux on this Mac from a phone, over Tailscale
//! (Settings › Phone Access, or `abromux remote enable`). As Rex does with
//! `tailscale serve`, the network and who's asking are Tailscale's job:
//!
//! - The app listens on a loopback port only (`port`, 7298; the nightly
//!   7299). `tailscale serve` puts it on the tailnet at
//!   `https://<this mac>.<tailnet>.ts.net:8443` (the nightly 10000), with
//!   Tailscale's certificate, and adds `Tailscale-User-Login` to every
//!   request: the tailnet user the phone is signed in as. The app sets that
//!   up itself while phone access is on, and takes it down when it's turned
//!   off.
//! - A request is served only when that login is the Mac's own Tailscale
//!   user or in `allow`. Without the header (anything that isn't `tailscale
//!   serve`) nothing is. WebSockets and POSTs must also come from the page's
//!   own origin, so another site open on the phone can't use it.
//!
//! What it serves: the phone app (`web/`, a web app to add to the Home
//! Screen), and `/api/ws`, a WebSocket that relays calls to the control
//! socket, as `abromux` does: `{"id":1,"method":"workspace.tree"}` →
//! `{"id":1,"ok":true,"result":…}`. Subscriptions (`events.subscribe`,
//! `surface.stream`) answer the same way and then carry `{"sub":id,
//! "data":…}` per line until `{"cancel":id}`. Only some methods pass
//! (`allowed`): no browser automation (your logged-in browser), files,
//! quitting or agent reports.
//!
//! While it runs, a watcher follows the app's notifications and sends them
//! to the phones that turned notifications on (push.rs), when no phone has
//! the app open and you're away from the Mac, and keeps the Mac from
//! sleeping while an agent is working (`keep_awake`).
//!
//! The settings are `"remote"` in the config file (`crate::settings`); the
//! push keys and the phones that subscribed are `push.json` in the support
//! folder.

mod http;
mod push;
pub mod tailscale;
mod web;
mod ws;

use crate::ipc;
use crate::settings::RemoteSettings;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub use push::{clear as push_clear, send_all as push_all, subscriptions as push_subscriptions};

/// The `"remote"` settings, with this app's ports filled in.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    /// The loopback port the app listens on.
    pub port: u16,
    /// The tailnet HTTPS port `tailscale serve` uses (443, 8443 or 10000).
    pub https_port: u16,
    /// Tailscale logins let in besides this Mac's own user.
    pub allow: Vec<String>,
    pub all_methods: bool,
    pub keep_awake: bool,
    pub push_when_idle: u64,
    pub push_preview: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self::from(&RemoteSettings::default())
    }
}

impl From<&RemoteSettings> for Config {
    fn from(r: &RemoteSettings) -> Self {
        let nightly = crate::channel::nightly();
        Self {
            enabled: r.enabled,
            port: r.port.unwrap_or(if nightly { 7299 } else { 7298 }),
            https_port: r.https_port.unwrap_or(if nightly { 10000 } else { 8443 }),
            allow: r.allow.clone(),
            all_methods: r.all_methods,
            keep_awake: r.keep_awake,
            push_when_idle: r.push_when_idle,
            push_preview: r.push_preview,
        }
    }
}

impl Config {
    /// The phone app's address, once this Mac's tailnet name is known.
    pub fn url(&self) -> Option<String> {
        let (host, _) = tailnet()?;
        Some(if self.https_port == 443 {
            format!("https://{host}/")
        } else {
            format!("https://{host}:{}/", self.https_port)
        })
    }

    /// The Mac's own Tailscale user, or one of `allow`.
    fn allows(&self, login: &str) -> bool {
        self.allow.iter().any(|a| a.eq_ignore_ascii_case(login))
            || tailnet().is_some_and(|(_, owner)| owner.eq_ignore_ascii_case(login))
    }

    fn proxy_target(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// The settings in the config file (for the CLI; the app has its own copy).
pub fn load() -> Result<Config, String> {
    crate::settings::load().map(|s| Config::from(&s.remote))
}

/// This Mac on the tailnet, `(MagicDNS name, owner's login)`: asked of
/// `tailscale status` until it answers, then kept.
static TAILNET: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn tailnet() -> Option<(String, String)> {
    let mut t = TAILNET.lock().unwrap();
    if t.is_none() {
        *t = tailscale::me().ok().map(|m| (m.host, m.login));
    }
    t.clone()
}

/// Write a file only its owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Have `tailscale serve` put this app on the tailnet, unless it already
/// does. Says why not when the port serves something else, or Tailscale
/// can't (not installed, signed out, HTTPS off for the tailnet…).
pub fn ensure_served(cfg: &Config) -> Result<(), String> {
    let ours = cfg.proxy_target();
    match tailscale::serving(cfg.https_port).map_err(|e| format!("{e:#}"))? {
        Some(to) if to == ours => Ok(()),
        Some(to) => Err(format!(
            "Port {} of this Mac's tailnet address already serves {to}. \
             Pick another HTTPS port (443, 8443 or 10000).",
            cfg.https_port
        )),
        None => tailscale::serve(cfg.https_port, cfg.port).map_err(|e| format!("{e:#}")),
    }
}

/// Stop `tailscale serve` on this app's port, if it's this app it serves.
pub fn unserve(cfg: &Config) -> Result<(), String> {
    match tailscale::serving(cfg.https_port) {
        Ok(Some(to)) if to == cfg.proxy_target() => {
            tailscale::unserve(cfg.https_port).map_err(|e| format!("{e:#}"))
        }
        _ => Ok(()),
    }
}

// ---- in the app ------------------------------------------------------------

/// Bumped by each `start`: threads of an older one wind down.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Phone app connections, and how many have the app on screen.
static CLIENTS: AtomicUsize = AtomicUsize::new(0);
static VISIBLE: AtomicUsize = AtomicUsize::new(0);
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// The settings serving now, once the app has started serving at all.
static APPLIED: Mutex<Option<Config>> = Mutex::new(None);
/// Bumped whenever what `status` says changes, for the Settings page.
static CHANGED: AtomicU64 = AtomicU64::new(0);

struct Running {
    port: u16,
    since: Instant,
    error: Option<String>,
    /// `tailscale serve`: None while it's being checked, then its outcome.
    serve: Option<Result<(), String>>,
}

fn current(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

/// Counts changes to `status`, so a view can redraw when it moves.
pub fn changes() -> u64 {
    CHANGED.load(Ordering::Relaxed)
}

fn changed() {
    CHANGED.fetch_add(1, Ordering::Relaxed);
}

/// The settings changed (Settings, a reload, `abromux remote …`): serve as
/// they now say, if that's different. Before the app has started serving
/// (its control socket isn't up yet) this does nothing.
pub fn apply(r: &RemoteSettings) {
    let cfg = Config::from(r);
    let same = match &*APPLIED.lock().unwrap() {
        None => return,
        Some(old) => *old == cfg,
    };
    if !same {
        start(r);
    }
}

/// Serve phones as the settings say: stop what ran before, and listen
/// again if it's enabled. At launch, and from `apply`.
pub fn start(r: &RemoteSettings) {
    start_with(Config::from(r));
}

/// Set up `tailscale serve` again, as the settings are (Settings' "Try
/// Again" after fixing what was wrong).
pub fn retry() {
    let cfg = APPLIED.lock().unwrap().clone();
    if let Some(cfg) = cfg {
        *TAILNET.lock().unwrap() = None;
        start_with(cfg);
    }
}

fn start_with(cfg: Config) {
    let old = APPLIED.lock().unwrap().replace(cfg.clone());
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    changed();
    // Wake the old listener so it sees it's done.
    if let Some(old) = RUNNING.lock().unwrap().take() {
        let _ = TcpStream::connect(("127.0.0.1", old.port));
    }
    awake::set(false);
    // Turned off, or moved: take the old address off the tailnet.
    if let Some(old) = old.filter(|o| {
        o.enabled && (!cfg.enabled || o.port != cfg.port || o.https_port != cfg.https_port)
    }) {
        std::thread::spawn(move || {
            if let Err(e) = unserve(&old) {
                eprintln!("abromux remote: {e}");
            }
        });
    }
    if !cfg.enabled {
        return;
    }
    let listener = match TcpListener::bind(("127.0.0.1", cfg.port)) {
        Ok(l) => l,
        Err(e) => {
            let error = format!("can't listen on 127.0.0.1:{}: {e}", cfg.port);
            eprintln!("abromux remote: {error}");
            *RUNNING.lock().unwrap() = Some(Running {
                port: cfg.port,
                since: Instant::now(),
                error: Some(error),
                serve: None,
            });
            return;
        }
    };
    *RUNNING.lock().unwrap() = Some(Running {
        port: cfg.port,
        since: Instant::now(),
        error: None,
        serve: None,
    });
    let cfg = Arc::new(cfg);
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if !current(generation) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let cfg = cfg.clone();
                std::thread::spawn(move || serve(stream, &cfg, generation));
            }
        });
    }
    // On the tailnet: this Mac's name and owner, and `tailscale serve`.
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            let result = match tailscale::me() {
                Ok(me) => {
                    *TAILNET.lock().unwrap() = Some((me.host, me.login));
                    ensure_served(&cfg)
                }
                Err(e) => Err(format!("{e:#}")),
            };
            if let Err(e) = &result {
                eprintln!("abromux remote: {e}");
            }
            if current(generation)
                && let Some(r) = RUNNING.lock().unwrap().as_mut()
            {
                r.serve = Some(result);
            }
            changed();
        });
    }
    std::thread::spawn(move || watch(&cfg, generation));
}

/// `remote.status`: whether it's listening, and who's connected.
pub fn status() -> Value {
    let cfg = APPLIED.lock().unwrap().clone().unwrap_or_default();
    let running = RUNNING.lock().unwrap();
    let serve = running.as_ref().and_then(|r| r.serve.clone());
    json!({
        "enabled": cfg.enabled,
        "url": cfg.enabled.then(|| cfg.url()).flatten(),
        "owner": TAILNET.lock().unwrap().as_ref().map(|(_, login)| login.clone()),
        "listening": running.as_ref().filter(|r| r.error.is_none()).map(|r| format!("127.0.0.1:{}", r.port)),
        "error": running.as_ref().and_then(|r| r.error.clone()),
        "serve": match &serve {
            None => Value::Null,
            Some(Ok(())) => json!(format!("https :{} → {}", cfg.https_port, cfg.proxy_target())),
            Some(Err(_)) => Value::Null,
        },
        "serve_error": serve.and_then(Result::err),
        "uptime": running.as_ref().map(|r| r.since.elapsed().as_secs()),
        "clients": CLIENTS.load(Ordering::Relaxed),
        "visible": VISIBLE.load(Ordering::Relaxed),
        "allow": cfg.allow,
        "keep_awake": cfg.keep_awake,
        "awake": awake::held(),
        "push_subscriptions": push::subscriptions().len(),
    })
}

/// One HTTP request (or WebSocket) from `tailscale serve`.
fn serve(stream: TcpStream, cfg: &Config, generation: u64) {
    let Ok(req) = http::read(&stream) else { return };
    let _ = route(stream, req, cfg, generation);
}

fn route(
    stream: TcpStream,
    req: http::Request,
    cfg: &Config,
    generation: u64,
) -> std::io::Result<()> {
    let Some(login) = req.header("tailscale-user-login").map(str::to_string) else {
        return http::refuse(
            &stream,
            403,
            &format!(
                "Open {} on a device signed in to your tailnet.",
                cfg.url()
                    .unwrap_or_else(|| "this Mac's Tailscale address".into())
            ),
        );
    };
    if !cfg.allows(&login) {
        return http::refuse(
            &stream,
            403,
            &format!(
                "{login} isn't allowed. On the Mac: Settings › Phone Access, or abromux remote allow {login}"
            ),
        );
    }
    let writes = req.method != "GET" || req.upgrade();
    if writes && !same_origin(&req) {
        return http::refuse(
            &stream,
            403,
            "Requests must come from the abromux page itself.",
        );
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/ws") if req.upgrade() => {
            let Some(key) = req.header("sec-websocket-key").map(str::to_string) else {
                return http::refuse(&stream, 400, "No WebSocket key.");
            };
            ws::handshake(&stream, &key)?;
            session(stream, cfg, generation);
            Ok(())
        }
        ("GET", "/api/info") => http::json(
            &stream,
            200,
            &json!({
                "app": crate::channel::app_name(),
                "version": env!("ABROMUX_VERSION"),
                "host": tailnet().map(|(host, _)| host),
                "login": login,
                "push_key": push::public_key().ok(),
            }),
        ),
        ("POST", "/api/push/subscribe") => {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            match push::subscribe(&v) {
                Ok(()) => http::json(&stream, 200, &json!({ "ok": true })),
                Err(e) => http::json(
                    &stream,
                    400,
                    &json!({ "ok": false, "error": e.to_string() }),
                ),
            }
        }
        ("POST", "/api/push/unsubscribe") => {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let removed = push::unsubscribe(v["endpoint"].as_str().unwrap_or_default());
            http::json(&stream, 200, &json!({ "ok": removed.is_ok() }))
        }
        ("POST", "/api/push/test") => {
            let sent = push::send_all(&json!({
                "title": crate::channel::app_name(),
                "body": "Notifications from this Mac work.",
            }));
            http::json(&stream, 200, &json!({ "ok": sent > 0, "sent": sent }))
        }
        ("GET", path) => match web::file(path) {
            Some((ty, body)) => http::respond(
                &stream,
                200,
                ty,
                &[
                    ("Cache-Control", "no-cache"),
                    ("Content-Security-Policy", web::CSP),
                ],
                body,
            ),
            None => http::refuse(&stream, 404, "Not found."),
        },
        _ => http::refuse(&stream, 405, "Not allowed."),
    }
}

/// The request's `Origin` is this page: `https://` and the host it was
/// asked for (or this Mac's tailnet name).
fn same_origin(req: &http::Request) -> bool {
    let Some(origin) = req.header("origin") else {
        return false;
    };
    let Some(host) = origin.strip_prefix("https://") else {
        return false;
    };
    let bare = |h: &str| h.split(':').next().unwrap_or(h).to_ascii_lowercase();
    let asked = [req.header("x-forwarded-host"), req.header("host")];
    asked
        .into_iter()
        .flatten()
        .any(|h| h.eq_ignore_ascii_case(host))
        || tailnet().is_some_and(|(ours, _)| bare(host) == ours.to_ascii_lowercase())
}

/// Whether a phone may call `method`.
pub fn allowed(method: &str, all: bool) -> bool {
    if method.starts_with("remote.") || method == "agent.report" {
        return false;
    }
    if all {
        return true;
    }
    const PREFIXES: &[&str] = &["workspace.", "pane.", "surface.", "tab.", "notification."];
    const METHODS: &[&str] = &[
        "system.ping",
        "system.identify",
        "system.methods",
        "events.subscribe",
        "group.list",
        "status.list",
        "link.list",
        "log.list",
        // init.lua's actions (New Workspace Folder…): a phone can already
        // type into terminals, which is as much as an action can do.
        "action.list",
        "action.run",
    ];
    PREFIXES.iter().any(|p| method.starts_with(p)) || METHODS.contains(&method)
}

/// A WebSocket from the phone app, until it closes or `start` runs again.
fn session(stream: TcpStream, cfg: &Config, generation: u64) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let writer = ws::Writer::new(write_half);
    let mut reader = ws::Reader::new(BufReader::new(stream), writer.clone());
    CLIENTS.fetch_add(1, Ordering::Relaxed);
    changed();
    let visible = Arc::new(AtomicBool::new(false));
    let subs: Arc<Mutex<HashMap<u64, UnixStream>>> = Arc::default();
    let done = Arc::new(AtomicBool::new(false));
    {
        // Pings keep proxies and the phone's network from timing it out,
        // and notice a phone that went away without closing.
        let (writer, done) = (writer.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(20));
                if !current(generation) || writer.ping().is_err() {
                    writer.close();
                    return;
                }
            }
        });
    }
    while let Ok(ws::Message::Text(text)) = reader.next() {
        if !current(generation) {
            break;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(id) = msg["cancel"].as_u64() {
            if let Some(s) = subs.lock().unwrap().remove(&id) {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            continue;
        }
        if let Some(v) = msg["visible"].as_bool() {
            if visible.swap(v, Ordering::Relaxed) != v {
                if v {
                    VISIBLE.fetch_add(1, Ordering::Relaxed);
                } else {
                    VISIBLE.fetch_sub(1, Ordering::Relaxed);
                }
            }
            continue;
        }
        let Some(method) = msg["method"].as_str() else {
            continue;
        };
        let id = msg["id"].clone();
        if !allowed(method, cfg.all_methods) {
            let _ = writer.text(
                &json!({ "id": id, "ok": false,
                         "error": format!("{method} isn't allowed from a phone") })
                .to_string(),
            );
            continue;
        }
        let req = json!({
            "id": 1,
            "method": method,
            "params": msg.get("params").cloned().unwrap_or_else(|| json!({})),
        });
        let (writer, subs) = (writer.clone(), subs.clone());
        let sub = matches!(method, "events.subscribe" | "surface.stream");
        std::thread::spawn(move || relay(req, id, sub, &writer, &subs));
    }
    done.store(true, Ordering::Relaxed);
    if visible.load(Ordering::Relaxed) {
        VISIBLE.fetch_sub(1, Ordering::Relaxed);
    }
    CLIENTS.fetch_sub(1, Ordering::Relaxed);
    changed();
    for (_, s) in subs.lock().unwrap().drain() {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
    writer.close();
}

/// One call to the control socket for the phone: its answer under the
/// phone's `id`, and for a subscription every line after it.
fn relay(
    req: Value,
    id: Value,
    sub: bool,
    writer: &ws::Writer,
    subs: &Mutex<HashMap<u64, UnixStream>>,
) {
    let fail = |e: String| {
        let _ = writer.text(&json!({ "id": id, "ok": false, "error": e }).to_string());
    };
    let mut conn = match UnixStream::connect(ipc::socket_path()) {
        Ok(c) => c,
        Err(e) => return fail(format!("abromux isn't answering: {e}")),
    };
    if writeln!(conn, "{req}").is_err() {
        return fail("abromux closed the connection".into());
    }
    let sub_id = id.as_u64().filter(|_| sub);
    if let (Some(n), Ok(c)) = (sub_id, conn.try_clone()) {
        subs.lock().unwrap().insert(n, c);
    }
    let mut lines = BufReader::new(conn).lines();
    let Some(Ok(first)) = lines.next() else {
        return fail("abromux closed the connection".into());
    };
    let mut answer: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
    answer["id"] = id.clone();
    if writer.text(&answer.to_string()).is_err() {
        return;
    }
    let Some(n) = sub_id.filter(|_| answer["ok"] == true) else {
        return;
    };
    for line in lines {
        let Ok(line) = line else { break };
        if line.is_empty() {
            continue;
        }
        let data: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if writer
            .text(&json!({ "sub": n, "data": data }).to_string())
            .is_err()
        {
            break;
        }
    }
    if subs.lock().unwrap().remove(&n).is_some() {
        let _ = writer.text(&json!({ "sub": n, "ended": true }).to_string());
    }
}

/// While serving: notifications to phones, and the Mac kept awake while
/// agents work.
fn watch(cfg: &Config, generation: u64) {
    let mut last_check = Instant::now() - Duration::from_secs(3600);
    while current(generation) {
        let Ok(mut conn) = UnixStream::connect(ipc::socket_path()) else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        let req = json!({ "id": 1, "method": "events.subscribe",
                          "params": { "events": ["notification", "agent"] } });
        if writeln!(conn, "{req}").is_err() {
            continue;
        }
        let _ = conn.set_read_timeout(Some(Duration::from_secs(15)));
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        loop {
            if !current(generation) {
                awake::set(false);
                return;
            }
            if cfg.keep_awake && last_check.elapsed() >= Duration::from_secs(30) {
                last_check = Instant::now();
                awake::set(agent_working());
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => break,
            }
            let Ok(ev) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match ev["event"].as_str() {
                Some("agent") if cfg.keep_awake => {
                    if ev["state"] == "working" {
                        awake::set(true);
                    } else {
                        // Others may still be working.
                        last_check = Instant::now() - Duration::from_secs(3600);
                    }
                }
                Some("notification") => notify(cfg, &ev),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    awake::set(false);
}

/// Some agent is working: a workspace shows a "Running" agent pill.
fn agent_working() -> bool {
    ipc::call("workspace.list", json!({}))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .flat_map(|ws| ws["status"].as_array().cloned().unwrap_or_default())
        .any(|s| s["value"] == "Running")
}

/// A notification in the app, to the phones, if nobody's looking at either.
fn notify(cfg: &Config, ev: &Value) {
    if VISIBLE.load(Ordering::Relaxed) > 0 {
        return;
    }
    if cfg.push_when_idle > 0 && idle_secs().is_some_and(|idle| idle < cfg.push_when_idle) {
        return;
    }
    let workspace = ev["workspace"].as_str().unwrap_or_default();
    let name = ipc::call("workspace.list", json!({}))
        .ok()
        .and_then(|v| {
            v.as_array()?
                .iter()
                .find(|w| w["ref"] == workspace)
                .and_then(|w| w["name"].as_str().map(String::from))
        })
        .unwrap_or_default();
    let title = ev["title"].as_str().unwrap_or("abromux");
    let body = ev["body"].as_str().unwrap_or_default();
    let body: String = if cfg.push_preview {
        body.chars().take(180).collect()
    } else {
        String::new()
    };
    push::send_all(&json!({
        "title": if name.is_empty() { title.to_string() } else { format!("{title} · {name}") },
        "body": body,
        "surface": ev["surface"],
        "workspace": ev["workspace"],
        "tag": ev["surface"].as_str().unwrap_or("abromux"),
    }));
}

/// Seconds since the last keyboard or mouse use on this Mac.
#[cfg(target_os = "macos")]
fn idle_secs() -> Option<u64> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-c", "IOHIDSystem", "-d", "4", "-r", "-k", "HIDIdleTime"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
    let ns: u64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(ns / 1_000_000_000)
}

#[cfg(not(target_os = "macos"))]
fn idle_secs() -> Option<u64> {
    None
}

/// Keeping the Mac from idle sleep: `caffeinate -i` for as long as it's
/// held, tied to the app (`-w`), so it can't outlive it.
mod awake {
    use std::{process::Child, sync::Mutex};

    static HELD: Mutex<Option<Child>> = Mutex::new(None);

    pub fn held() -> bool {
        HELD.lock().unwrap().is_some()
    }

    pub fn set(on: bool) {
        let mut held = HELD.lock().unwrap();
        let alive = held
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        match (on, held.as_mut()) {
            (true, Some(_)) if alive => {}
            (true, _) => *held = spawn(),
            (false, Some(c)) => {
                let _ = c.kill();
                let _ = c.wait();
                *held = None;
            }
            (false, None) => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn spawn() -> Option<Child> {
        std::process::Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .spawn()
            .ok()
    }

    #[cfg(not(target_os = "macos"))]
    fn spawn() -> Option<Child> {
        None
    }
}

// ---- the CLI ---------------------------------------------------------------

/// `abromux remote …`.
pub fn cli(
    pos: &[String],
    opts: &serde_json::Map<String, Value>,
    as_json: bool,
) -> Result<(), String> {
    let sub = pos.first().map(String::as_str).unwrap_or("status");
    let arg = pos.get(1).map(String::as_str);
    let port = |name: &str| -> Result<Option<u16>, String> {
        match opts.get(name) {
            Some(Value::String(p)) => p
                .parse()
                .map(Some)
                .map_err(|_| format!("--{name} needs a port number")),
            _ => Ok(None),
        }
    };
    // A change to the config file, then the running app told to apply it.
    let edit = |key: &[&str], value: Option<Value>| -> Result<Config, String> {
        let s = crate::settings::edit(key, value)?;
        let _ = ipc::call("remote.reload", json!({}));
        Ok(Config::from(&s.remote))
    };
    let mut cfg = load()?;
    match sub {
        "status" => {
            let app = ipc::call("remote.status", json!({})).ok();
            let serving = tailscale::serving(cfg.https_port);
            if as_json {
                let v = json!({
                    "config": cfg,
                    "url": cfg.url(),
                    "app": app,
                    "serve": serving.as_ref().ok().cloned().flatten(),
                    "serve_error": serving.as_ref().err().map(|e| e.to_string()),
                });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
                return Ok(());
            }
            if !cfg.enabled {
                println!(
                    "Phone access is off. Turn it on in Settings › Phone Access, or with `abromux remote enable`."
                );
                return Ok(());
            }
            println!("Phone access: on");
            match cfg.url() {
                Some(url) => println!("  URL:     {url}"),
                None => println!("  URL:     unknown (is Tailscale running?)"),
            }
            let owner = tailnet().map(|(_, login)| login);
            println!(
                "  allowed: {}",
                owner
                    .iter()
                    .chain(&cfg.allow)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            match &serving {
                Ok(Some(to)) if *to == cfg.proxy_target() => {
                    println!("  tailscale serve: :{} → {to}", cfg.https_port)
                }
                Ok(Some(to)) => println!(
                    "  tailscale serve: :{} goes to {to}, not abromux",
                    cfg.https_port
                ),
                Ok(None) => println!(
                    "  tailscale serve: not serving :{} (the app sets it up when it starts)",
                    cfg.https_port
                ),
                Err(e) => println!("  tailscale serve: {e}"),
            }
            match app {
                Some(a) if a["error"].is_string() => {
                    println!("  app:     {}", a["error"].as_str().unwrap())
                }
                Some(a) => println!(
                    "  app:     listening on {}, {} phone(s) connected{}",
                    a["listening"].as_str().unwrap_or("?"),
                    a["clients"],
                    if a["awake"] == true {
                        ", keeping the Mac awake (an agent is working)"
                    } else {
                        ""
                    }
                ),
                None => println!("  app:     {} isn't running", crate::channel::app_name()),
            }
            println!("  notifications: {} phone(s)", push::subscriptions().len());
            Ok(())
        }
        "enable" | "on" => {
            tailscale::me().map_err(|e| format!("{e:#}"))?;
            if let Some(p) = port("port")? {
                cfg = edit(&["remote", "port"], Some(json!(p)))?;
            }
            if let Some(p) = port("https-port")? {
                if ![443, 8443, 10000].contains(&p) {
                    return Err("--https-port: tailscale serve takes 443, 8443 or 10000".into());
                }
                cfg = edit(&["remote", "httpsPort"], Some(json!(p)))?;
            }
            ensure_served(&cfg)?;
            cfg = edit(&["remote", "enabled"], Some(json!(true)))?;
            pair(&cfg);
            Ok(())
        }
        "disable" | "off" => {
            unserve(&cfg)?;
            edit(&["remote", "enabled"], None)?;
            println!("Phone access is off.");
            Ok(())
        }
        "pair" | "qr" | "url" => {
            if !cfg.enabled {
                return Err("phone access is off; turn it on with `abromux remote enable`".into());
            }
            pair(&cfg);
            Ok(())
        }
        "allow" | "deny" => {
            let login = arg.ok_or_else(|| format!("remote {sub}: missing <login>"))?;
            let mut allow = cfg.allow.clone();
            allow.retain(|a| !a.eq_ignore_ascii_case(login));
            if sub == "allow" {
                allow.push(login.to_string());
            }
            cfg = edit(
                &["remote", "allow"],
                (!allow.is_empty()).then(|| json!(allow)),
            )?;
            let owner = tailnet().map(|(_, login)| login);
            println!(
                "Allowed: {}",
                owner
                    .iter()
                    .chain(&cfg.allow)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(())
        }
        "push" => match arg.unwrap_or("list") {
            "test" => {
                let sent = push::send_all(&json!({
                    "title": crate::channel::app_name(),
                    "body": "Notifications from this Mac work.",
                }));
                println!("Sent to {sent} phone(s).");
                Ok(())
            }
            "clear" => {
                let n = push::clear().map_err(|e| format!("{e:#}"))?;
                println!("Forgot {n} phone(s).");
                Ok(())
            }
            "list" => {
                for s in push::subscriptions() {
                    println!(
                        "{}",
                        s.endpoint.split('/').take(3).collect::<Vec<_>>().join("/")
                    );
                }
                Ok(())
            }
            s => Err(format!("remote push: unknown {s:?} (list, test, clear)")),
        },
        s => Err(format!(
            "remote: unknown subcommand {s:?} (status, enable, disable, pair, allow, deny, push)"
        )),
    }
}

/// The URL, as text and as a QR code to scan with the phone's camera.
fn pair(cfg: &Config) {
    let Some(url) = cfg.url() else {
        println!(
            "Phone access is on, but this Mac's tailnet name is unknown: is Tailscale running?"
        );
        return;
    };
    if let Ok(code) = qrcode::QrCode::new(url.as_bytes()) {
        let art = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build();
        println!("{art}");
    }
    println!("Phone access is on: {url}");
    println!();
    let who: Vec<String> = tailnet()
        .map(|(_, login)| login)
        .into_iter()
        .chain(cfg.allow.iter().cloned())
        .collect();
    println!(
        "On the phone (signed in to Tailscale as {}):",
        who.join(" or ")
    );
    println!("  1. Scan the code, or open the URL in Safari.");
    println!("  2. Share → Add to Home Screen, then open abromux from there.");
    println!("  3. Open Notifications in the app and tap Turn On Alerts.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(headers: &[(&str, &str)]) -> http::Request {
        http::Request {
            method: "GET".into(),
            path: "/api/ws".into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: vec![],
        }
    }

    #[test]
    fn phones_get_terminals_not_the_browser() {
        for m in [
            "workspace.tree",
            "surface.send_text",
            "surface.stream",
            "events.subscribe",
            "notification.list",
            "action.list",
            "action.run",
        ] {
            assert!(allowed(m, false), "{m}");
        }
        for m in [
            "browser.eval",
            "browser.cookies",
            "file.edit",
            "app.quit",
            "agent.report",
            "remote.reload",
            "debug.input",
        ] {
            assert!(!allowed(m, false), "{m}");
        }
        assert!(allowed("browser.eval", true));
        assert!(!allowed("remote.reload", true));
    }

    #[test]
    fn origin_must_be_the_page() {
        *TAILNET.lock().unwrap() = Some(("mac.tail1.ts.net".into(), "me@example.com".into()));
        let host = ("host", "mac.tail1.ts.net:8443");
        assert!(same_origin(&req(&[
            host,
            ("origin", "https://mac.tail1.ts.net:8443")
        ])));
        assert!(same_origin(&req(&[
            ("host", "127.0.0.1:7298"),
            ("origin", "https://mac.tail1.ts.net:8443")
        ])));
        assert!(!same_origin(&req(&[
            host,
            ("origin", "https://evil.example")
        ])));
        assert!(!same_origin(&req(&[
            host,
            ("origin", "http://mac.tail1.ts.net:8443")
        ])));
        assert!(!same_origin(&req(&[host])));
    }

    #[test]
    fn the_owner_and_the_allowed_get_in() {
        *TAILNET.lock().unwrap() = Some(("mac.tail1.ts.net".into(), "me@example.com".into()));
        let cfg = Config {
            allow: vec!["Friend@Example.com".into()],
            ..Config::default()
        };
        assert!(cfg.allows("me@example.com"));
        assert!(cfg.allows("friend@example.com"));
        assert!(!cfg.allows("someone@example.com"));
    }

    #[test]
    fn ports_default_per_app() {
        let cfg = Config::default();
        assert!(!cfg.enabled);
        assert!([7298, 7299].contains(&cfg.port));
        let set = Config::from(&RemoteSettings {
            port: Some(9000),
            https_port: Some(443),
            ..RemoteSettings::default()
        });
        assert_eq!((set.port, set.https_port), (9000, 443));
    }
}
