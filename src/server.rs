//! The control socket: a listener thread hands each request to the main
//! thread, which runs it against the app state (see ipc.rs for the wire
//! format and cli.rs for the commands that drive it).
//!
//! Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or plain 0-based
//! indexes. Without an explicit target a command acts on the caller (the
//! terminal `abromux` runs in, from `ABROMUX_SURFACE_ID`), else on what's focused.

use crate::{
    AbroMux, BrowserView, DEFAULT_URL,
    agent::{Agent, AgentEvent, Kind},
    browser::normalize_url,
    browser_auto, debug, ipc,
    notification::Kind as NoteKind,
    one_line,
    status::{self, RUNNING, SUCCESS, WARNING},
    workspace::{
        Dir, LogLevel, Progress, Status, Surface, Workspace, git_branch, group_span, parse_color,
    },
};
use anyhow::{Context as _, anyhow, bail};
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
    future::{Either, LocalBoxFuture, select},
};
use gpui::{App, AsyncApp, Axis, Context, Entity, EntityId, Keystroke, Timer, Window};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::fd::AsRawFd,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::{Duration, Instant},
};

type Request = (Value, oneshot::Sender<Value>, Hangup);

/// Resolves once the connection a request came on has closed.
pub(crate) type Hangup = futures::future::Shared<oneshot::Receiver<()>>;

/// How long a page gets to answer one script.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
/// `browser wait`'s default timeout, and how often it checks.
const WAIT_TIMEOUT_MS: u64 = 10_000;
const WAIT_POLL: Duration = Duration::from_millis(100);

enum Reply {
    Now(Value),
    /// Answered later (e.g. a webview script result). Other requests are
    /// served meanwhile.
    Later(LocalBoxFuture<'static, anyhow::Result<Value>>),
}

/// `rx`'s value, or an error once `limit` has passed.
fn later(
    rx: oneshot::Receiver<anyhow::Result<Value>>,
    limit: Duration,
    what: &'static str,
) -> Reply {
    Reply::Later(
        async move {
            match select(rx, Timer::after(limit)).await {
                Either::Left((Ok(v), _)) => v,
                Either::Left((Err(_), _)) => Err(anyhow!("{what}: request dropped")),
                Either::Right(_) => Err(anyhow!("{what}: no answer after {}s", limit.as_secs())),
            }
        }
        .boxed_local(),
    )
}

/// Run `js` where automation acts (the frame `browser frame` picked, else
/// the page) and wait up to `limit` for its JSON. Ok(None) if the page
/// didn't run it (still loading, or navigating away); Err if the picked
/// frame is gone or the script threw there.
async fn eval_json(
    b: &Entity<BrowserView>,
    js: &str,
    cx: &AsyncApp,
    limit: Duration,
) -> anyhow::Result<Option<String>> {
    let rx = b
        .read_with(cx, |b, cx| b.eval_target(js, cx))
        .map_err(|_| anyhow!("that browser is gone"))?;
    match select(rx, Timer::after(limit)).await {
        Either::Left((Ok(Ok(out)), _)) => Ok(Some(out)),
        Either::Left((Ok(Err(e)), _)) => Err(anyhow!(e)),
        _ => Ok(None),
    }
}

/// An answer from the native side (cookies), or an error after `limit`.
async fn native(
    rx: oneshot::Receiver<abromux_browser::backend::Answer>,
    limit: Duration,
) -> anyhow::Result<Value> {
    match select(rx, Timer::after(limit)).await {
        Either::Left((Ok(Ok(json)), _)) => Ok(serde_json::from_str(&json)?),
        Either::Left((Ok(Err(e)), _)) => Err(anyhow!(e)),
        Either::Left((Err(_), _)) => Err(anyhow!("WebKit dropped the request")),
        Either::Right(_) => Err(anyhow!("WebKit didn't answer in {}s", limit.as_secs())),
    }
}

/// Run `js` in the page itself (never a picked frame). None if it didn't.
async fn eval_page(b: &Entity<BrowserView>, js: &str, cx: &AsyncApp) -> Option<String> {
    let rx = b.read_with(cx, |b, cx| b.eval(js, cx)).ok()?;
    match select(rx, Timer::after(SCRIPT_TIMEOUT)).await {
        Either::Left((Ok(Ok(out)), _)) => Some(out),
        _ => None,
    }
}

/// The host of `url` (`https://a.b:8080/x` → `a.b`).
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split_once(']')
            .map_or(host, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or(host)
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether a cookie for `domain` goes to `host`.
fn cookie_matches(domain: &str, host: &str) -> bool {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    host == d || host.ends_with(&format!(".{d}"))
}

/// The page's log entries for `browser console|errors|network`, oldest
/// first: `level` (console) and `filter` (text in the entry) narrow them,
/// `limit` keeps the newest.
fn log_entries(list: &std::collections::VecDeque<Value>, p: &Value) -> anyhow::Result<Vec<Value>> {
    let level = str_param(p, "level");
    let filter = str_param(p, "filter").map(str::to_lowercase);
    let mut out: Vec<Value> = list
        .iter()
        .filter(|e| level.is_none_or(|l| e["level"].as_str() == Some(l)))
        .filter(|e| {
            filter.as_deref().is_none_or(|f| {
                ["text", "url", "source", "method"]
                    .iter()
                    .any(|k| e[*k].as_str().is_some_and(|v| v.to_lowercase().contains(f)))
            })
        })
        .cloned()
        .collect();
    if let Some(n) = usize_param(p, "limit")? {
        out.drain(..out.len().saturating_sub(n));
    }
    Ok(out)
}

/// Bind the socket and start serving. Returns the path, or None if another
/// abromux already owns it.
pub fn start(window: &mut Window, cx: &mut Context<AbroMux>) -> Option<PathBuf> {
    let path = ipc::socket_path();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            eprintln!(
                "abromux: another instance is listening on {}; control socket disabled",
                path.display()
            );
            return None;
        }
        let _ = std::fs::remove_file(&path);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("abromux: can't bind {}: {e}", path.display());
            return None;
        }
    };

    let (tx, mut rx) = mpsc::unbounded::<Request>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    });

    cx.spawn_in(window, async move |this, cx| {
        while let Some((req, reply, gone)) = rx.next().await {
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let method = req["method"].as_str().unwrap_or_default().to_string();
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let Ok(out) = this.update_in(cx, |this, window, cx| {
                let out = this.handle(&method, &params, &gone, window, cx);
                this.refresh_unread(cx);
                out
            }) else {
                break;
            };
            match out {
                Ok(Reply::Now(v)) => {
                    let _ = reply.send(ipc::ok(&id, v));
                }
                Ok(Reply::Later(fut)) => cx
                    .spawn(async move |_| {
                        let resp = match fut.await {
                            Ok(v) => ipc::ok(&id, v),
                            Err(e) => error_reply(&id, &e),
                        };
                        let _ = reply.send(resp);
                    })
                    .detach(),
                Err(e) => {
                    let _ = reply.send(error_reply(&id, &e));
                }
            }
        }
    })
    .detach();
    let stale = path.clone();
    cx.on_app_quit(move |_, _| {
        let _ = std::fs::remove_file(&stale);
        async {}
    })
    .detach();
    Some(path)
}

fn serve(stream: UnixStream, tx: mpsc::UnboundedSender<Request>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let peer = stream.as_raw_fd();
    // Dropped when this returns, which is when the connection closes.
    let (_open, closed) = oneshot::channel::<()>();
    let gone = closed.shared();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) if req["method"] == "events.subscribe" => {
                // Listen before asking, so nothing in between is missed.
                let events = crate::events::subscribe();
                let (reply_tx, reply_rx) = oneshot::channel();
                if tx.unbounded_send((req, reply_tx, gone.clone())).is_err() {
                    return;
                }
                let resp = futures::executor::block_on(reply_rx)
                    .unwrap_or_else(|_| ipc::err(&Value::Null, "app closed"));
                if writer.write_all(format!("{resp}\n").as_bytes()).is_err() || resp["ok"] != true {
                    return;
                }
                stream_events(&mut writer, peer, events, &resp["result"]);
                return;
            }
            Ok(req) if req["method"] == "surface.stream" => {
                let (reply_tx, reply_rx) = oneshot::channel();
                if tx.unbounded_send((req, reply_tx, gone.clone())).is_err() {
                    return;
                }
                let resp = futures::executor::block_on(reply_rx)
                    .unwrap_or_else(|_| ipc::err(&Value::Null, "app closed"));
                let tap = resp["result"]["token"]
                    .as_u64()
                    .and_then(crate::stream::take);
                if writer.write_all(format!("{resp}\n").as_bytes()).is_err() {
                    return;
                }
                if let Some(tap) = tap {
                    stream_output(&mut writer, peer, tap);
                }
                return;
            }
            Ok(req) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                // A question waits on a person, as long as it takes: notice
                // the asker going away meanwhile, to take it down.
                let watch = (req["method"] == "ui.prompt").then(|| watch_hangup(peer));
                let req_gone = watch.as_ref().map_or_else(|| gone.clone(), |w| w.1.clone());
                if tx.unbounded_send((req, reply_tx, req_gone)).is_err() {
                    return;
                }
                let resp = futures::executor::block_on(reply_rx)
                    .unwrap_or_else(|_| ipc::err(&Value::Null, "app closed"));
                if let Some((done, _)) = watch {
                    done.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                resp
            }
            Err(e) => ipc::err(&Value::Null, format!("bad request: {e}")),
        };
        let mut out = resp.to_string();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            return;
        }
    }
}

/// A `Hangup` for one request whose answer may take long: a thread watches
/// `peer` until the flag is set, and resolves it if the caller hangs up
/// first (`serve` reads nothing while it waits for an answer, so it wouldn't
/// see that).
fn watch_hangup(
    peer: std::os::fd::RawFd,
) -> (std::sync::Arc<std::sync::atomic::AtomicBool>, Hangup) {
    use std::sync::atomic::Ordering;
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (hup, gone) = oneshot::channel::<()>();
    let stop = done.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let mut fd = libc::pollfd {
                fd: peer,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: polls one descriptor, for at most 200 ms.
            if unsafe { libc::poll(&mut fd, 1, 200) } <= 0 {
                continue;
            }
            let mut b = 0u8;
            // SAFETY: peeks at most one byte into `b`, without blocking.
            let n = unsafe {
                libc::recv(
                    peer,
                    (&raw mut b).cast(),
                    1,
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                )
            };
            if n == 0 || (n < 0 && fd.revents & (libc::POLLHUP | libc::POLLERR) != 0) {
                drop(hup);
                return;
            }
            // More requests queued behind this one: not a hangup.
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    (done, gone.shared())
}

/// Write a terminal's output (stream.rs) to `writer`, one JSON line per
/// batch, until the terminal or the caller is gone:
/// `{"output":"<base64>"}`, `{"resize":[cols,rows]}`, `{"closed":true}`.
/// A tap that ends without `closed` was cut off for falling behind: the
/// caller should start over.
fn stream_output(
    writer: &mut UnixStream,
    peer: std::os::fd::RawFd,
    tap: std::sync::mpsc::Receiver<crate::stream::Chunk>,
) {
    use crate::stream::Chunk;
    use base64::Engine as _;
    use std::sync::mpsc::RecvTimeoutError;
    let b64 = base64::engine::general_purpose::STANDARD;
    loop {
        let first = match tap.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                if hung_up(peer) {
                    return;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = writer.write_all(b"{\"behind\":true}\n");
                return;
            }
        };
        // Everything already queued goes in one line: a flood costs one write.
        let mut out = String::new();
        let mut bytes = Vec::new();
        let mut next = Some(first);
        while let Some(c) = next.take() {
            match c {
                Chunk::Output(b) => bytes.extend_from_slice(&b),
                other => {
                    if !bytes.is_empty() {
                        out += &json!({ "output": b64.encode(&bytes) }).to_string();
                        out.push('\n');
                        bytes.clear();
                    }
                    if let Chunk::Events(batch) = &other {
                        out += "{\"events\":";
                        out += batch;
                        out += "}\n";
                        if out.len() < 1 << 20 {
                            next = tap.try_recv().ok();
                        }
                        continue;
                    }
                    let line = match other {
                        Chunk::Resize(c, r) => json!({ "resize": [c, r] }),
                        _ => json!({ "closed": true }),
                    };
                    out += &line.to_string();
                    out.push('\n');
                    if line["closed"] == true {
                        let _ = writer.write_all(out.as_bytes());
                        return;
                    }
                }
            }
            if bytes.len() < 1 << 20 {
                next = tap.try_recv().ok();
            }
        }
        if !bytes.is_empty() {
            out += &json!({ "output": b64.encode(&bytes) }).to_string();
            out.push('\n');
        }
        if writer.write_all(out.as_bytes()).is_err() {
            return;
        }
    }
}

/// The peer closed its end (a non-blocking peek reads end of file).
fn hung_up(peer: std::os::fd::RawFd) -> bool {
    let mut b = 0u8;
    // SAFETY: peeks at most one byte into `b`, without blocking.
    let n = unsafe {
        libc::recv(
            peer,
            (&raw mut b).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    n == 0
}

/// Write each event that passes `filter` to `writer` until the caller hangs up.
fn stream_events(
    writer: &mut UnixStream,
    peer: std::os::fd::RawFd,
    events: std::sync::mpsc::Receiver<String>,
    filter: &Value,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let filtered = ["events", "workspace", "surface"]
        .iter()
        .any(|k| !filter[*k].is_null() && filter[*k] != json!([]));
    loop {
        match events.recv_timeout(Duration::from_secs(1)) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                if filtered
                    && !serde_json::from_str(&line)
                        .is_ok_and(|ev: Value| crate::events::matches(&ev, filter))
                {
                    continue;
                }
                if writer.write_all(format!("{line}\n").as_bytes()).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Quiet for a while: is anyone still there?
                let mut b = 0u8;
                // SAFETY: peeks at most one byte into `b`, without blocking.
                let n = unsafe {
                    libc::recv(
                        peer,
                        (&raw mut b).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                if n == 0 {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// `--id` as `notification:N` or N.
fn notification_id(p: &Value) -> anyhow::Result<Option<usize>> {
    Ok(match p.get("id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(
            s.trim_start_matches("notification:")
                .parse()
                .with_context(|| format!("bad notification id {s:?}"))?,
        ),
        Some(v) => Some(v.as_u64().context("bad notification id")? as usize),
    })
}

/// A page zoom: `1.25`, or `125%`.
fn parse_zoom(z: &str) -> anyhow::Result<f64> {
    let bad = || {
        anyhow!("bad zoom {z:?}: use in, out, reset, a factor like 1.25 or a percentage like 125%")
    };
    match z.strip_suffix('%') {
        Some(pct) => pct
            .trim()
            .parse::<f64>()
            .map(|p| p / 100.)
            .map_err(|_| bad()),
        None => z.trim().parse().map_err(|_| bad()),
    }
}

fn str_param<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// A count given as a number or (from the CLI) as text.
fn usize_param(p: &Value, key: &str) -> anyhow::Result<Option<usize>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => s
            .parse()
            .map(Some)
            .with_context(|| format!("bad --{key} {s:?}")),
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .with_context(|| format!("--{key} must be a number")),
    }
}

/// Give terminal `t`'s program `text` as if pasted, then press Return
/// (`Terminal::submit`): what an agent's input box takes as one message.
fn submit_to<V: 'static>(t: &Entity<crate::TerminalView>, text: &str, cx: &mut Context<V>) {
    let sent = t.update(cx, |t, cx| {
        cx.notify();
        t.submit(text)
    });
    if sent {
        return;
    }
    // No bracketed paste: Return on its own, a moment after the text, or a
    // program reading it all at once takes it as a new line in what was
    // typed.
    let t = t.downgrade();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        let _ = t.update(cx, |t, cx| {
            t.send(b"\r");
            cx.notify();
        });
    })
    .detach();
}

fn bool_param(p: &Value, key: &str, default: bool) -> bool {
    match p.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.as_str(), "false" | "0" | "no"),
        _ => default,
    }
}

/// `kind:N` → N as a ref, a bare number → an index, anything else → a
/// label (a workspace's name, a tab's name, a group's name).
enum Sel {
    Ref(usize),
    Index(usize),
    Label(String),
}

/// The kinds of refs, so `surface:3` given for a pane is a mistake rather
/// than a label.
const REF_KINDS: &[&str] = &[
    "workspace",
    "pane",
    "surface",
    "group",
    "tab",
    "notification",
];

fn parse_sel(s: &str, kind: &str) -> anyhow::Result<Sel> {
    if let Some(n) = s.strip_prefix(kind).and_then(|r| r.strip_prefix(':')) {
        return Ok(Sel::Ref(
            n.parse().with_context(|| format!("bad {kind} ref {s:?}"))?,
        ));
    }
    if let Ok(i) = s.parse() {
        return Ok(Sel::Index(i));
    }
    if let Some((k, _)) = s.split_once(':')
        && REF_KINDS.contains(&k)
    {
        return Err(
            TargetError(format!("expected {kind}:N, an index or a name, got {s:?}")).into(),
        );
    }
    Ok(Sel::Label(s.to_string()))
}

/// A workspace, pane, surface or group that isn't there, or a name that
/// fits more than one: the CLI exits with status 3 for it.
#[derive(Debug)]
pub(crate) struct TargetError(String);

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TargetError {}

/// The one of `found` (ref, name) that `label` names; an error naming them
/// all when it's more than one.
fn one_labelled<T>(kind: &str, label: &str, found: Vec<(T, String, String)>) -> anyhow::Result<T> {
    match found.len() {
        0 => Err(TargetError(format!("no {kind} named {label:?}")).into()),
        1 => Ok(found.into_iter().next().unwrap().0),
        _ => {
            let list: Vec<String> = found
                .iter()
                .map(|(_, r, name)| format!("  {r}  {name}"))
                .collect();
            Err(TargetError(format!(
                "{kind} name {label:?} is ambiguous; use a ref:\n{}",
                list.join("\n")
            ))
            .into())
        }
    }
}

/// A method the app doesn't have (an older app than the CLI, or a typo).
#[derive(Debug)]
struct MethodNotFound(String);

impl std::fmt::Display for MethodNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown method {:?} (see `abromux rpc list`)", self.0)
    }
}

impl std::error::Error for MethodNotFound {}

/// An error reply for `e`, with a `code` for the kinds that have one:
/// `target` (not found or ambiguous), `method_not_found`.
fn error_reply(id: &Value, e: &anyhow::Error) -> Value {
    if e.chain().any(|c| c.is::<TargetError>()) {
        ipc::err_code(id, format!("{e:#}"), "target")
    } else if e.chain().any(|c| c.is::<MethodNotFound>()) {
        ipc::err_code(id, format!("{e:#}"), "method_not_found")
    } else {
        ipc::err(id, format!("{e:#}"))
    }
}

fn direction(d: &str) -> anyhow::Result<(Axis, bool)> {
    Ok(match d {
        "right" => (Axis::Horizontal, false),
        "left" => (Axis::Horizontal, true),
        "down" => (Axis::Vertical, false),
        "up" => (Axis::Vertical, true),
        _ => bail!("direction must be left, right, up or down"),
    })
}

/// `\n`, `\r`, `\t`, `\e` and `\\` escapes, so `send 'ls\n'` presses enter.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

impl AbroMux {
    /// One request; `gone` resolves when its caller has hung up.
    fn handle(
        &mut self,
        method: &str,
        p: &Value,
        gone: &Hangup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Reply> {
        let named = self.resolve_names(p, cx)?;
        let p = named.as_ref().unwrap_or(p);
        let v = match method {
            "system.ping" => json!("pong"),
            "remote.status" => crate::remote::status(),
            "remote.reload" => {
                let s = crate::settings::load().map_err(anyhow::Error::msg)?;
                crate::settings_window::apply(s, cx);
                crate::remote::status()
            }
            "system.methods" => crate::api::methods(p)?,
            "system.identify" => {
                let caller = self.caller(p).map(
                    |(wi, _, s)| json!({ "workspace": self.ws_ref(wi), "surface": self.sref(&s) }),
                );
                let ws = self.ws();
                json!({
                    "socket": self.socket.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    "caller": caller,
                    "focused": {
                        "workspace": self.ws_ref(self.active),
                        "pane": format!("pane:{}", ws.focused_pane),
                        "surface": ws.focused_surface().map(|s| self.sref(s)),
                    },
                })
            }

            "action.list" => {
                let init = crate::lua::init_path();
                let actions = crate::lua::collect_cached(&init).map_err(anyhow::Error::msg)?;
                json!(
                    actions
                        .iter()
                        .map(|a| json!({ "name": a.name, "title": a.title,
                                         "description": a.description, "palette": a.palette }))
                        .collect::<Vec<_>>()
                )
            }
            "action.run" => return self.run_action(p),

            "workspace.list" => json!(
                (0..self.workspaces.len())
                    .map(|i| self.describe_ws(i, false, cx))
                    .collect::<Vec<_>>()
            ),
            "workspace.current" => self.describe_ws(self.active, false, cx),
            "workspace.create" if p.get("layout").is_some_and(|l| !l.is_null()) => {
                // The CLI passes the JSON as text.
                let layout = match &p["layout"] {
                    Value::String(s) => serde_json::from_str(s),
                    v => serde_json::from_value(v.clone()),
                }
                .context("bad --layout")?;
                let spec = crate::layout::WorkspaceSpec {
                    layout: Some(layout),
                    ..Default::default()
                };
                spec.check().map_err(anyhow::Error::msg)?;
                let base = str_param(p, "cwd")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(crate::paths::home);
                let prev = self.workspaces.get(self.active).map(|w| w.id);
                let name = str_param(p, "name").map(String::from);
                let wi = self.open_layout(&spec, name, &base, window, cx);
                self.workspaces[wi].description =
                    str_param(p, "description").and_then(crate::sidebar::clean_description);
                let new_id = self.workspaces[wi].id;
                if let Some(g) = str_param(p, "group") {
                    let g = self.find_group(g)?;
                    self.set_group(wi, Some(g), cx);
                }
                // By id: the new one may have gone in above it.
                if !bool_param(p, "focus", true)
                    && let Some(i) = prev.and_then(|id| self.ws_index(id))
                {
                    self.select_workspace(i, window, cx);
                }
                let wi = self.ws_index(new_id).unwrap_or(wi);
                self.describe_ws(wi, false, cx)
            }
            "workspace.create" => {
                let folder = str_param(p, "folder").map(std::path::PathBuf::from);
                if let Some(f) = &folder {
                    anyhow::ensure!(f.is_absolute(), "folder must be absolute: {}", f.display());
                    anyhow::ensure!(f.is_dir(), "no such folder: {}", f.display());
                }
                let env: std::collections::BTreeMap<String, String> = match p.get("env") {
                    None | Some(Value::Null) => Default::default(),
                    Some(Value::Object(m)) => m
                        .iter()
                        .map(|(k, v)| match v {
                            Value::String(s) => Ok((k.clone(), s.clone())),
                            _ => Err(anyhow::anyhow!("env {k}: expected a string")),
                        })
                        .collect::<anyhow::Result<_>>()?,
                    Some(_) => bail!("env: expected an object of strings"),
                };
                let cwd = str_param(p, "cwd")
                    .map(String::from)
                    .or_else(|| folder.as_ref().map(|f| f.to_string_lossy().into_owned()));
                let group = str_param(p, "group")
                    .map(|g| self.find_group(g))
                    .transpose()?;
                let prev = self.workspaces.get(self.active).map(|w| w.id);
                if let Some((start, _)) = group.and_then(|g| group_span(&self.workspaces, g)) {
                    // With one of its workspaces selected, the new one lands in the group.
                    self.active = start;
                }
                let focus = bool_param(p, "focus", true);
                // Its first terminal gets the folder and env too.
                self.building_home = folder.clone();
                self.building_env = env.clone();
                let wi = self.create_workspace(cwd, focus, window, cx);
                self.building_home = None;
                self.building_env.clear();
                if !focus {
                    self.active = prev.and_then(|id| self.ws_index(id)).unwrap_or(0);
                }
                let ws = &mut self.workspaces[wi];
                ws.home = folder;
                ws.env = env;
                if let Some(name) = str_param(p, "name") {
                    // A stand-in name (a made-up one) is the agents' to replace
                    // (`naming`), until you name it yourself.
                    if bool_param(p, "provisional", false) {
                        ws.auto_title = Some(name.into());
                    } else {
                        ws.custom_name = Some(name.into());
                    }
                }
                self.workspaces[wi].description =
                    str_param(p, "description").and_then(crate::sidebar::clean_description);
                if let (Some(cmd), Some(Surface::Terminal(t))) = (
                    str_param(p, "command"),
                    self.workspaces[wi].focused_surface(),
                ) {
                    t.update(cx, |t, _| t.send(format!("{cmd}\r").as_bytes()));
                }
                self.describe_ws(wi, false, cx)
            }
            "workspace.select" => {
                let wi = self.workspace_arg(p)?;
                self.select_workspace(wi, window, cx);
                self.describe_ws(wi, false, cx)
            }
            "workspace.close" => {
                let wi = self.workspace_arg(p)?;
                let r = self.ws_ref(wi);
                self.close_workspace(wi, window, cx);
                json!({ "closed": r })
            }
            "workspace.rename" => {
                let wi = self.workspace_target(p)?;
                let title = str_param(p, "title").context("missing title")?;
                self.workspaces[wi].custom_name = Some(title.into());
                cx.notify();
                self.describe_ws(wi, false, cx)
            }
            "workspace.action" => return self.workspace_action(p, window, cx),
            "workspace.reorder" => {
                let wi = self.workspace_target(p)?;
                let n = self.workspaces.len();
                let to = if let Some(i) = p.get("index") {
                    let i = match i {
                        Value::String(s) => s.parse().context("bad --index")?,
                        v => v.as_u64().context("bad --index")? as usize,
                    };
                    i.min(n - 1)
                } else if let Some(b) = str_param(p, "before") {
                    let t = self.find_workspace(b)?;
                    if t > wi { t - 1 } else { t }
                } else if let Some(a) = str_param(p, "after") {
                    let t = self.find_workspace(a)?;
                    if t >= wi { t } else { t + 1 }
                } else {
                    bail!("reorder-workspace needs --index, --before or --after");
                };
                let id = self.workspaces[wi].id;
                let group = self.workspaces[wi].group;
                let dry = bool_param(p, "dry_run", false);
                let order = self.reorder(dry, cx, |ws, groups| {
                    crate::workspace::move_workspace(ws, groups, wi, to, group);
                });
                let at = order.iter().position(|&w| w == id).unwrap_or(to);
                json!({ "workspace": format!("workspace:{id}"), "index": at })
            }
            "workspace.reorder_all" => {
                let order = str_param(p, "order").context("missing --order")?;
                let mut ids: Vec<usize> = vec![];
                for s in order.split(',') {
                    let id = self.workspaces[self.find_workspace(s.trim())?].id;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                // The ones not named keep their order, after the named ones.
                for w in &self.workspaces {
                    if !ids.contains(&w.id) {
                        ids.push(w.id);
                    }
                }
                let dry = bool_param(p, "dry_run", false);
                let order = self.reorder(dry, cx, |ws, _| {
                    ws.sort_by_key(|w| ids.iter().position(|&id| id == w.id));
                });
                json!(
                    order
                        .iter()
                        .map(|id| format!("workspace:{id}"))
                        .collect::<Vec<_>>()
                )
            }
            "group.list" => json!(
                self.groups
                    .iter()
                    .map(|g| self.describe_group(g.id))
                    .collect::<Vec<_>>()
            ),
            "group.create" => {
                let wi = self.workspace_target(p)?;
                let g = self.new_group(wi, cx).context("no workspace")?;
                if let Some(name) = str_param(p, "name") {
                    self.groups.iter_mut().find(|x| x.id == g).unwrap().name = one_line(name);
                }
                self.describe_group(g)
            }
            "group.add" => {
                let wi = self.workspace_target(p)?;
                let g = self.find_group(str_param(p, "group").context("missing --group")?)?;
                self.set_group(wi, Some(g), cx);
                self.describe_group(g)
            }
            "group.remove" => {
                let wi = self.workspace_target(p)?;
                self.set_group(wi, None, cx);
                json!(null)
            }
            "group.by_folder" => json!({ "created": self.group_by_folder(cx) }),
            "group.rename" | "group.color" | "group.collapse" | "group.expand"
            | "group.ungroup" => {
                let g = match str_param(p, "group") {
                    Some(g) => self.find_group(g)?,
                    None => self.workspaces[self.workspace_target(p)?]
                        .group
                        .context("the workspace isn't in a group; pass --group")?,
                };
                let colors = cx
                    .global::<crate::settings::Settings>()
                    .workspace_colors
                    .clone();
                let group = self.groups.iter_mut().find(|x| x.id == g).unwrap();
                match method {
                    "group.rename" => {
                        group.name = one_line(str_param(p, "title").context("missing title")?)
                    }
                    "group.color" => {
                        group.color = match str_param(p, "color") {
                            None | Some("none" | "clear") => None,
                            Some(c) => Some(
                                colors
                                    .parse(c)
                                    .with_context(|| format!("unknown color {c:?}"))?,
                            ),
                        }
                    }
                    "group.collapse" => group.collapsed = true,
                    "group.expand" => group.collapsed = false,
                    _ => {
                        self.ungroup(g, cx);
                        return Ok(Reply::Now(json!(null)));
                    }
                }
                cx.notify();
                self.describe_group(g)
            }
            "session.restore" => {
                let session = match str_param(p, "from") {
                    Some(from) => Some(
                        crate::session::read(&crate::session::source(from))
                            .map_err(anyhow::Error::msg)?,
                    ),
                    None if self.launch_session.is_none() => {
                        bail!(
                            "this launch restored no session; pass --from <stable|nightly|cmux|file>"
                        )
                    }
                    None => None,
                };
                json!({ "added": self.reopen_session(session, window, cx) })
            }
            "session.export" => {
                let path = std::path::PathBuf::from(str_param(p, "path").context("missing path")?);
                if path.exists() && !bool_param(p, "force", false) {
                    bail!("{} exists; pass --force to replace it", path.display());
                }
                crate::session::write(&self.snapshot(true, cx), &path)
                    .with_context(|| format!("writing {}", path.display()))?;
                json!({ "path": path })
            }
            "workspace.tree" => {
                if bool_param(p, "all", false) {
                    json!(
                        (0..self.workspaces.len())
                            .map(|i| self.describe_ws(i, true, cx))
                            .collect::<Vec<_>>()
                    )
                } else {
                    json!([self.describe_ws(self.workspace_target(p)?, true, cx)])
                }
            }

            "pane.list" => {
                let wi = self.workspace_target(p)?;
                json!(self.panes_json(&self.workspaces[wi], cx))
            }
            "pane.focus" => {
                let (wi, pane) = self.pane_target(p)?;
                self.workspaces[wi].focused_pane = pane;
                if wi == self.active {
                    self.focus_active(window, cx);
                }
                cx.notify();
                json!({ "pane": format!("pane:{pane}") })
            }
            "pane.last" => {
                let wi = self.workspace_target(p)?;
                let pane = self.workspaces[wi]
                    .last_pane()
                    .context("no previously focused pane")?;
                self.workspaces[wi].focused_pane = pane;
                if wi == self.active {
                    self.focus_active(window, cx);
                }
                cx.notify();
                json!({ "pane": format!("pane:{pane}") })
            }
            "pane.swap" => {
                let (wi, pane) = self.pane_target(p)?;
                let dir = match str_param(p, "direction").unwrap_or("right") {
                    "left" => Dir::Left,
                    "right" => Dir::Right,
                    "up" => Dir::Up,
                    "down" => Dir::Down,
                    _ => bail!("direction must be left, right, up or down"),
                };
                if !self.workspaces[wi].swap_pane(pane, dir) {
                    bail!("pane:{pane} has no pane to swap with in that direction");
                }
                if wi == self.active {
                    self.focus_active(window, cx);
                }
                cx.notify();
                let to = self.workspaces[wi].root.neighbor(pane, dir).unwrap_or(pane);
                json!({ "pane": format!("pane:{to}") })
            }
            "pane.resize" => {
                let (wi, pane) = self.pane_target(p)?;
                let dir = match str_param(p, "direction").unwrap_or("right") {
                    "left" => Dir::Left,
                    "right" => Dir::Right,
                    "up" => Dir::Up,
                    "down" => Dir::Down,
                    _ => bail!("direction must be left, right, up or down"),
                };
                let amount = match p.get("amount") {
                    Some(Value::String(a)) => {
                        a.parse().with_context(|| format!("bad amount {a:?}"))?
                    }
                    Some(v) => v.as_f64().context("amount must be a number")? as f32,
                    None => cx.global::<crate::settings::Settings>().panes.resize_step,
                };
                if !self.resize_pane(wi, pane, dir, amount, cx) {
                    bail!("pane:{pane} has no split to resize in that direction");
                }
                json!({ "pane": format!("pane:{pane}") })
            }
            "pane.create" => {
                let wi = self.workspace_target(p)?;
                let pane = self.workspaces[wi].focused_pane;
                self.split_with(
                    wi,
                    pane,
                    str_param(p, "direction").unwrap_or("right"),
                    p,
                    window,
                    cx,
                )?
            }

            "surface.list" => {
                let (wi, pane) = self.pane_target(p)?;
                let ws = &self.workspaces[wi];
                let pane = ws.panes().find(|x| x.id == pane).context("no such pane")?;
                json!(
                    pane.tabs
                        .iter()
                        .enumerate()
                        .map(|(i, s)| self.describe_surface(s, i == pane.active, cx))
                        .collect::<Vec<_>>()
                )
            }
            "surface.split" => {
                let (wi, pane, _) = self.surface_target(p)?;
                self.split_with(
                    wi,
                    pane,
                    str_param(p, "direction").context("missing direction")?,
                    p,
                    window,
                    cx,
                )?
            }
            "surface.create" => {
                let (wi, pane) = self.pane_target(p)?;
                let s = self.make_surface(wi, p, window, cx)?;
                let out = self.describe_surface(&s, true, cx);
                self.add_tab_to(wi, pane, s, bool_param(p, "focus", true), window, cx);
                out
            }
            "surface.close" => {
                let (wi, _, s) = self.surface_target(p)?;
                let r = self.sref(&s);
                let id = s.entity_id();
                let ws = &self.workspaces[wi];
                // A pinned tab is kept, out of the layout, unless --unpin.
                if ws.is_pinned(id) && !bool_param(p, "unpin", false) {
                    if ws.contains(id) {
                        self.hide_tab(id, window, cx);
                    }
                    json!({ "hidden": r })
                } else {
                    self.unpin_tab(id, window, cx);
                    self.close_surface(id, window, cx);
                    json!({ "closed": r })
                }
            }
            "surface.focus" => {
                let (_, _, s) = self.surface_target(p)?;
                self.reveal_tab(s.entity_id(), window, cx);
                self.describe_surface(&s, true, cx)
            }
            "tab.pin" => {
                let (_, _, s) = self.surface_target(p)?;
                let pinned = self.pin_tab(s.entity_id(), cx);
                json!({ "surface": self.sref(&s), "pinned": pinned })
            }
            "tab.unpin" => {
                let (_, _, s) = self.surface_target(p)?;
                let id = s.entity_id();
                let r = self.sref(&s);
                let closed = self.workspaces.iter().any(|w| w.hidden_index(id).is_some());
                let unpinned = self.unpin_tab(id, window, cx);
                json!({ "surface": r, "unpinned": unpinned, "closed": unpinned && closed })
            }
            "tab.list" => {
                let wi = self.workspace_target(p)?;
                let ws = &self.workspaces[wi];
                let focused = ws.focused_surface().map(|s| s.entity_id());
                let list: Vec<Value> = ws
                    .pinned_surfaces()
                    .into_iter()
                    .map(|(s, hidden)| {
                        let mut v = self.describe_surface(&s, Some(s.entity_id()) == focused, cx);
                        v["pinned"] = json!(true);
                        v["hidden"] = json!(hidden);
                        v
                    })
                    .collect();
                json!(list)
            }
            "surface.stream" => {
                // The connection thread (`serve`) takes the tap by its token
                // and writes what comes on it (stream.rs).
                let (_, _, s) = self.surface_target(p)?;
                let r = self.sref(&s);
                if let Surface::Browser(b) = &s {
                    let (events, id) = b.update(cx, |b, _| (b.chat_mirror(), b.stream_id()));
                    let events = events.context("that browser tab isn't a Claude tab")?;
                    json!({
                        "chat": true,
                        "events": events,
                        "token": crate::stream::open(id),
                        "surface": r,
                    })
                } else if bool_param(p, "claude", false) {
                    // The terminal's Claude Code session as a chat, read
                    // from its transcript; typing still goes to the terminal.
                    let t = self.terminal_target(p)?;
                    let config = crate::settings::claude_config_dir(cx);
                    let t = t.read(cx);
                    let shell = t.shell_pid();
                    let found = shell
                        .and_then(|pid| crate::claude_chat::running_under(pid, config.as_deref()));
                    let hooked = t
                        .agent
                        .as_ref()
                        .filter(|a| a.kind == Kind::Claude.as_str() && a.alive());
                    let mode = hooked.and_then(|a| a.permission_mode.clone());
                    let follow = match (found, hooked) {
                        (Some(r), _) => crate::claude_chat::Follow {
                            shell,
                            config,
                            session: r.session,
                            cwd: r.cwd,
                            mode,
                            busy: r.busy,
                            watched: true,
                        },
                        (None, Some(a)) => crate::claude_chat::Follow {
                            shell,
                            config,
                            session: a.session_id.clone(),
                            cwd: std::path::PathBuf::from(if a.cwd.is_empty() {
                                &t.cwd
                            } else {
                                &a.cwd
                            }),
                            mode,
                            busy: t.claude_running(),
                            watched: false,
                        },
                        (None, None) => bail!("no Claude Code session in that terminal"),
                    };
                    let key = crate::stream::private_key();
                    let token = crate::stream::open(key);
                    crate::claude_chat::follow(key, follow);
                    json!({
                        "chat": true,
                        "terminal": true,
                        "events": [],
                        "token": token,
                        "surface": r,
                    })
                } else {
                    let t = self.terminal_target(p)?;
                    let (mut start, id) = t.update(cx, |t, _| (t.stream_start(), t.stream_id()));
                    start["token"] = json!(crate::stream::open(id));
                    start["surface"] = json!(r);
                    start
                }
            }
            "surface.chat" => {
                // What a Claude tab's page would post, from a copy of it on
                // a phone (`surface.stream` feeds that copy).
                let b = self.browser_target(p)?;
                let msg = p
                    .get("message")
                    .filter(|m| m.is_object())
                    .context("missing message")?;
                if msg["chat"] == "ready" {
                    bail!("start with surface.stream");
                }
                let body = msg.to_string();
                if !b.update(cx, |b, cx| b.chat_message(&body, cx)) {
                    bail!("that browser tab isn't a Claude tab");
                }
                json!({ "ok": true })
            }
            "surface.send_text" => {
                let t = self.terminal_target(p)?;
                let text = p
                    .get("text")
                    .and_then(Value::as_str)
                    .context("missing text")?;
                // `raw`: exactly these characters (a phone's typing), no
                // backslash escapes.
                let text = if bool_param(p, "raw", false) {
                    text.to_string()
                } else {
                    unescape(text)
                };
                if bool_param(p, "submit", false) {
                    submit_to(&t, &text, cx);
                } else {
                    t.update(cx, |t, cx| {
                        t.send(text.as_bytes());
                        cx.notify();
                    });
                }
                json!({ "bytes": text.len() })
            }
            "surface.send_key" => {
                let t = self.terminal_target(p)?;
                let key = str_param(p, "key").context("missing key")?;
                let mut ks =
                    Keystroke::parse(key).map_err(|e| anyhow!("bad key {key:?}: {e:?}"))?;
                if ks.key.chars().count() == 1 && ks.key_char.is_none() {
                    ks.key_char = Some(if ks.modifiers.shift {
                        ks.key.to_uppercase()
                    } else {
                        ks.key.clone()
                    });
                }
                if !t.update(cx, |t, cx| t.send_keystroke(&ks, cx)) {
                    bail!("key {key:?} doesn't map to terminal input");
                }
                json!({ "key": key })
            }
            "surface.rename" => {
                let (_, _, s) = self.surface_target(p)?;
                // An empty title goes back to the program's or page's own.
                let title = str_param(p, "title").map(String::from);
                s.set_custom_title(title, cx);
                cx.notify();
                self.describe_surface(&s, true, cx)
            }
            "surface.respawn" => {
                let t = self.terminal_target(p)?;
                t.update(cx, |t, cx| {
                    t.respawn(cx);
                    if let Some(cmd) = str_param(p, "command") {
                        t.send(format!("{cmd}\r").as_bytes());
                    }
                });
                self.describe_surface(&Surface::Terminal(t), true, cx)
            }
            "surface.move" | "surface.reorder" => {
                // `--workspace` is where it goes, not where to find it.
                let mut src_p = p.clone();
                src_p.as_object_mut().map(|o| o.remove("workspace"));
                let (swi, spane, s) = self.surface_target(&src_p)?;
                let id = s.entity_id();
                let anchor = match (str_param(p, "before"), str_param(p, "after")) {
                    (Some(_), Some(_)) => bail!("pass --before or --after, not both"),
                    (Some(b), None) => Some((self.find_surface(b, Some(swi))?, false)),
                    (None, Some(a)) => Some((self.find_surface(a, Some(swi))?, true)),
                    (None, None) => None,
                };
                let index = usize_param(p, "index")?;
                let (dwi, dpane) = if method == "surface.reorder" {
                    if anchor.is_none() && index.is_none() {
                        bail!("reorder-surface needs --index, --before or --after");
                    }
                    if let Some(((awi, apane, _), _)) = &anchor
                        && (*awi, *apane) != (swi, spane)
                    {
                        bail!("--before/--after must name a tab in the same pane");
                    }
                    (swi, spane)
                } else if str_param(p, "pane").is_some() {
                    self.pane_target(p)?
                } else if let Some(((awi, apane, _), _)) = &anchor {
                    (*awi, *apane)
                } else if str_param(p, "workspace").is_some() {
                    let wi = self.workspace_target(p)?;
                    (wi, self.workspaces[wi].focused_pane)
                } else {
                    bail!("move-surface needs --pane, --workspace, --before or --after");
                };
                let at = match (&anchor, index) {
                    (Some(((_, _, a), after)), _) => {
                        let pane = self.workspaces[dwi].panes().find(|x| x.id == dpane);
                        let i = pane
                            .and_then(|x| {
                                x.tabs.iter().position(|t| t.entity_id() == a.entity_id())
                            })
                            .context("--before/--after tab isn't in the target pane")?;
                        if a.entity_id() == id {
                            bail!("--before/--after names the tab being moved");
                        }
                        i + usize::from(*after)
                    }
                    (None, Some(i)) => {
                        // An index into the pane as it will be: past the tab's
                        // own slot when it moves later within its pane.
                        match self.workspaces[dwi].tab_position(id) {
                            Some((from, idx, _)) if from == dpane && i > idx => i + 1,
                            _ => i,
                        }
                    }
                    (None, None) => usize::MAX,
                };
                self.move_surface(id, dwi, dpane, at, bool_param(p, "focus", true), window, cx)?;
                self.describe_moved(id, cx)?
            }
            "surface.split_off" => {
                let (_, _, s) = self.surface_target(p)?;
                let dir = match str_param(p, "direction").context("missing direction")? {
                    "left" => Dir::Left,
                    "right" => Dir::Right,
                    "up" => Dir::Up,
                    "down" => Dir::Down,
                    _ => bail!("direction must be left, right, up or down"),
                };
                let id = s.entity_id();
                self.split_off(id, dir, bool_param(p, "focus", false), window, cx)?;
                self.describe_moved(id, cx)?
            }
            "surface.move_to_new_workspace" => {
                let (_, _, s) = self.source_surface(p)?;
                let id = s.entity_id();
                let name = str_param(p, "title").map(one_line);
                self.tab_to_new_workspace(id, name, bool_param(p, "focus", false), window, cx)?;
                self.describe_moved(id, cx)?
            }
            "pane.join" => {
                let target =
                    str_param(p, "target_pane").context("join-pane needs --target-pane")?;
                let mut tp = p.clone();
                tp["pane"] = json!(target);
                let (dwi, dpane) = self.pane_target(&tp)?;
                let (_, spane, s) = self.source_surface(p)?;
                if spane == dpane {
                    bail!("it's already in pane:{dpane}");
                }
                let id = s.entity_id();
                self.move_surface(
                    id,
                    dwi,
                    dpane,
                    usize::MAX,
                    bool_param(p, "focus", false),
                    window,
                    cx,
                )?;
                self.describe_moved(id, cx)?
            }
            "pane.zoom" => {
                let (wi, pane) = self.pane_target(p)?;
                let ws = &mut self.workspaces[wi];
                let on = match str_param(p, "state").unwrap_or("toggle") {
                    "on" => true,
                    "off" => false,
                    "toggle" => ws.zoomed() != Some(pane),
                    s => bail!("expected on, off or toggle, got {s:?}"),
                };
                if on && !matches!(ws.root, crate::workspace::Node::Split { .. }) {
                    bail!("the workspace has one pane; there's nothing to zoom");
                }
                ws.focused_pane = pane;
                ws.zoom = on.then_some(pane);
                if wi == self.active {
                    self.focus_active(window, cx);
                }
                cx.notify();
                json!({ "pane": format!("pane:{pane}"), "zoomed": on })
            }
            "surface.flash" => {
                let (_, pane, _) = self.surface_target(p)?;
                self.flash_pane(pane, cx);
                json!({ "pane": format!("pane:{pane}") })
            }
            "surface.clear_history" => {
                let t = self.terminal_target(p)?;
                t.update(cx, |t, cx| t.clear_scrollback(cx));
                json!(null)
            }
            "surface.read_text" => {
                let t = self.terminal_target(p)?;
                let lines = p.get("lines").and_then(Value::as_u64).map(|n| n as usize);
                let scrollback = bool_param(p, "scrollback", false);
                json!(t.update(cx, |t, _| t.read_text(scrollback, lines)))
            }

            // Until the command running in a terminal finishes (or, with
            // `close`, the terminal closes): its exit status.
            "surface.wait" => return self.wait_surface(p, cx),

            // The connection becomes a stream of events (`serve` does the
            // streaming); this resolves its filters.
            "ui.prompt" => return self.ui_prompt(p, gone, window, cx),
            "events.subscribe" => {
                let workspace = str_param(p, "workspace")
                    .map(|w| self.find_workspace(w).map(|wi| self.ws_ref(wi)))
                    .transpose()?;
                let surface = str_param(p, "surface")
                    .map(|s| self.find_surface(s, None).map(|(_, _, s)| self.sref(&s)))
                    .transpose()?;
                let names: Vec<String> = match p.get("events") {
                    Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).collect(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                    _ => vec![],
                };
                for n in &names {
                    if !crate::events::NAMES
                        .iter()
                        .any(|(e, _)| crate::events::name_matches(e, n))
                    {
                        bail!(
                            "no event {n:?}; there are {}",
                            crate::events::NAMES
                                .iter()
                                .map(|(e, _)| *e)
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
                json!({ "workspace": workspace, "surface": surface, "events": names })
            }

            // A plugin tab's own command (crates/abromux-plugin: Tab::command).
            "surface.plugin" => {
                let (_, _, s) = self.surface_target(p)?;
                let Surface::Plugin(t) = s else {
                    bail!("that surface isn't a plugin tab")
                };
                let name = str_param(p, "command").context("missing command")?;
                let args = p.get("args").cloned().unwrap_or(json!({}));
                let out = t.command(name, &args, window, cx)?;
                cx.notify();
                out
            }
            "surface.read_selection" => {
                let (_, _, s) = self.surface_target(p)?;
                match s {
                    Surface::Terminal(t) => json!(t.read(cx).selection_text().unwrap_or_default()),
                    Surface::Plugin(_) => bail!("that surface has no text selection"),
                    Surface::Browser(b) => {
                        let acx = cx.to_async();
                        return Ok(Reply::Later(
                            async move {
                                let out = eval_json(
                                    &b,
                                    "String(window.getSelection() || '')",
                                    &acx,
                                    SCRIPT_TIMEOUT,
                                )
                                .await?
                                .context("the page didn't answer (is it still loading?)")?;
                                Ok(match serde_json::from_str(&out) {
                                    Ok(v @ Value::String(_)) => v,
                                    _ => Value::String(out),
                                })
                            }
                            .boxed_local(),
                        ));
                    }
                }
            }
            "surface.action" => return self.tab_action(p, window, cx),

            // From `abromux hooks claude-event` (agent.rs).
            "agent.report" => {
                let t = self.terminal_target(p)?;
                let ended = AgentEvent::of_report(p) == Some(AgentEvent::Ended);
                let reported = Agent::from_report(p);
                let has_mode = p.get("permission_mode").is_some_and(|m| !m.is_null());
                t.update(cx, |t, _| match (ended, reported) {
                    // A nested `claude -p` ending mustn't drop the outer
                    // session; the outer one's next Stop reasserts it anyway.
                    (true, Some(a)) => {
                        if t.agent
                            .as_ref()
                            .is_some_and(|cur| cur.session_id == a.session_id)
                        {
                            t.agent = None;
                        }
                    }
                    (false, Some(mut a)) => {
                        if let Some(cur) = &t.agent
                            && cur.session_id == a.session_id
                        {
                            // `--resume` needs the directory the session
                            // started in; Claude's cwd can wander since.
                            a.cwd = cur.cwd.clone();
                            // SessionStart doesn't carry the permission mode.
                            if !has_mode {
                                a.permission_mode = cur.permission_mode.clone();
                            }
                        }
                        t.agent = Some(a);
                    }
                    (_, None) => {}
                });
                // The tab's icon follows the agent.
                cx.notify();
                self.agent_event(t.entity_id(), p, window, cx);
                json!(null)
            }

            "browser.open" => {
                let (wi, pane, _) = self.surface_target(p)?;
                let mut p = p.clone();
                p["type"] = json!("browser");
                // The automation commands that follow only drive the built-in
                // browser, whatever `browser.engine` says.
                p["builtIn"] = json!(true);
                self.split_with(
                    wi,
                    pane,
                    str_param(&p, "direction").unwrap_or("right"),
                    &p,
                    window,
                    cx,
                )?
            }
            "file.edit" => {
                let path = PathBuf::from(str_param(p, "path").context("missing path")?);
                anyhow::ensure!(
                    path.is_absolute(),
                    "the path must be absolute: {}",
                    path.display()
                );
                let (wi, pane, _) = self.surface_target(p)?;
                let focus = bool_param(p, "focus", true);
                let (s, new) = self
                    .edit_file(wi, pane, &path, focus, window, cx)
                    .map_err(anyhow::Error::msg)?;
                json!({
                    "workspace": self.ws_ref(wi),
                    "pane": format!("pane:{new}"),
                    "surface": self.describe_surface(&s, true, cx),
                })
            }
            "browser.navigate" => {
                let b = self.browser_target(p)?;
                let url = normalize_url(str_param(p, "url").context("missing url")?);
                b.update(cx, |b, cx| b.navigate(&url, cx));
                json!({ "url": url })
            }
            "browser.back" | "browser.forward" | "browser.reload" => {
                let b = self.browser_target(p)?;
                b.update(cx, |b, cx| match method {
                    "browser.back" => b.script("history.back()", cx),
                    "browser.forward" => b.script("history.forward()", cx),
                    _ => b.reload(cx),
                });
                json!(null)
            }
            "browser.zoom" => {
                let b = self.browser_target(p)?;
                let now = b.read(cx).zoom();
                let to = match p.get("zoom") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(z)) => Some(match z.as_str() {
                        "in" => crate::browser::zoom_step(now, true),
                        "out" => crate::browser::zoom_step(now, false),
                        "reset" | "actual" => 1.0,
                        z => parse_zoom(z)?,
                    }),
                    Some(v) => Some(v.as_f64().context("zoom must be a number")?),
                };
                if let Some(z) = to {
                    if !(z.is_finite() && z > 0.) {
                        bail!("zoom must be greater than 0");
                    }
                    b.update(cx, |b, cx| b.set_zoom(z, cx));
                }
                json!({ "zoom": b.read(cx).zoom() })
            }
            "browser.url" => {
                let b = self.browser_target(p)?;
                let b = b.read(cx);
                json!({ "url": b.url, "title": b.title })
            }
            "browser.eval" => {
                let b = self.browser_target(p)?;
                let script = p
                    .get("script")
                    .and_then(Value::as_str)
                    .context("missing script")?
                    .to_string();
                let acx = cx.to_async();
                return Ok(Reply::Later(
                    async move {
                        let out = eval_json(&b, &script, &acx, SCRIPT_TIMEOUT)
                            .await?
                            .context("the page didn't run the script (is it still loading?)")?;
                        Ok(serde_json::from_str(&out).unwrap_or(Value::String(out)))
                    }
                    .boxed_local(),
                ));
            }
            "browser.snapshot"
            | "browser.click"
            | "browser.dblclick"
            | "browser.hover"
            | "browser.focus"
            | "browser.check"
            | "browser.scroll_into_view"
            | "browser.fill"
            | "browser.type"
            | "browser.press"
            | "browser.select"
            | "browser.scroll"
            | "browser.get"
            | "browser.is"
            | "browser.find"
            | "browser.keydown"
            | "browser.keyup"
            | "browser.storage" => {
                let b = self.browser_target(p)?;
                let js = browser_auto::script(method.trim_start_matches("browser."), p);
                let acx = cx.to_async();
                return Ok(Reply::Later(
                    async move {
                        let out = eval_json(&b, &js, &acx, SCRIPT_TIMEOUT)
                            .await?
                            .context("the page didn't run the script (is it still loading?)")?;
                        browser_auto::parse(&out)
                    }
                    .boxed_local(),
                ));
            }
            "browser.console" | "browser.errors" | "browser.network" => {
                let b = self.browser_target(p)?;
                let clear = bool_param(p, "clear", false);
                return Ok(Reply::Now(b.update(cx, |b, _| {
                    let list = match method {
                        "browser.console" => &mut b.log.console,
                        "browser.errors" => &mut b.log.errors,
                        _ => &mut b.log.network,
                    };
                    let out = log_entries(list, p)?;
                    if clear {
                        list.clear();
                    }
                    anyhow::Ok(json!(out))
                })?));
            }
            "browser.frame" => return self.browser_frame(p, cx),
            "browser.cookies" => return self.browser_cookies(p, cx),
            "browser.state" => return self.browser_state(p, cx),
            "browser.tab" => self.browser_tab(p, window, cx)?,
            "browser.downloads" => {
                let d = crate::downloads::entity(cx);
                match str_param(p, "op").unwrap_or("list") {
                    "list" => json!(
                        d.read(cx)
                            .list
                            .iter()
                            .map(|d| d.to_json())
                            .collect::<Vec<_>>()
                    ),
                    "clear" => json!({ "cleared": d.update(cx, |d, cx| d.clear(cx)) }),
                    "cancel" => {
                        let id =
                            usize_param(p, "id")?.context("downloads cancel needs an id")? as u64;
                        if !d.update(cx, |d, cx| d.cancel(id, cx)) {
                            bail!("download {id} isn't running");
                        }
                        json!({ "canceled": id })
                    }
                    op => bail!("unknown downloads operation {op:?} (list, clear or cancel)"),
                }
            }
            "browser.wait_download" => return self.wait_download(p, cx),
            "browser.wait" => return self.browser_wait(p, cx),
            "browser.screenshot" => {
                let b = self.browser_target(p)?;
                let path = match str_param(p, "path") {
                    Some(path) => PathBuf::from(path),
                    None => {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        std::env::temp_dir().join(format!("abromux-screenshot-{ms}.png"))
                    }
                };
                return Ok(later(
                    b.read(cx).screenshot(path, cx),
                    SCRIPT_TIMEOUT,
                    "screenshot",
                ));
            }

            "notification.create" => {
                let (_, _, s) = self.surface_target(p)?;
                let mut title = str_param(p, "title").unwrap_or_default().to_string();
                if let Some(sub) = str_param(p, "subtitle") {
                    title = if title.is_empty() {
                        sub.into()
                    } else {
                        format!("{title} — {sub}")
                    };
                }
                let body = str_param(p, "body").unwrap_or_default().to_string();
                if title.is_empty() && body.is_empty() {
                    bail!("notify needs --title or --body");
                }
                let id = self.add_notification(
                    s.entity_id(),
                    title,
                    body,
                    NoteKind::Message,
                    window,
                    cx,
                );
                json!({ "id": id, "surface": self.sref(&s) })
            }
            "notification.list" => {
                let now = std::time::SystemTime::now();
                json!(self
                    .notifications
                    .iter()
                    .rev()
                    .map(|n| {
                        let ws = self.workspaces.iter().position(|w| w.owns(n.surface));
                        json!({
                            "id": n.id,
                            "title": n.title,
                            "body": n.body,
                            "read": n.read,
                            "surface": format!("surface:{}", self.surface_refs.get(&n.surface).copied().unwrap_or(0)),
                            "workspace": ws.map(|wi| self.ws_ref(wi)),
                            "age_seconds": now.duration_since(n.at).unwrap_or_default().as_secs(),
                        })
                    })
                    .collect::<Vec<_>>())
            }
            "notification.open" => {
                let id = notification_id(p)?.context("missing notification id")?;
                if !self.open_notification(id, window, cx) {
                    bail!("no notification {id}");
                }
                json!({ "id": id })
            }
            "notification.dismiss" => {
                let all_read = bool_param(p, "all_read", false);
                match (notification_id(p)?, all_read) {
                    (Some(id), false) => {
                        if !self.dismiss_notification(id, cx) {
                            bail!("no notification {id}");
                        }
                        json!({ "dismissed": 1 })
                    }
                    (None, true) => json!({ "dismissed": self.dismiss_read(cx) }),
                    _ => bail!("dismiss-notification needs exactly one of --id or --all-read"),
                }
            }
            "notification.mark_read" => {
                let all = bool_param(p, "all", false);
                let ws = str_param(p, "workspace").is_some();
                let marked = match (notification_id(p)?, ws, all) {
                    (Some(id), false, false) => {
                        let n = self
                            .notifications
                            .iter_mut()
                            .find(|n| n.id == id)
                            .with_context(|| format!("no notification {id}"))?;
                        n.read = true;
                        1
                    }
                    (None, true, false) => {
                        let wi = self.workspace_target(p)?;
                        let surfaces: Vec<EntityId> = if str_param(p, "surface").is_some() {
                            vec![self.surface_target(p)?.2.entity_id()]
                        } else {
                            self.workspaces[wi]
                                .panes()
                                .flat_map(|p| p.tabs.iter().map(|t| t.entity_id()))
                                .collect()
                        };
                        let n = self
                            .notifications
                            .iter()
                            .filter(|n| !n.read && surfaces.contains(&n.surface))
                            .count();
                        for s in surfaces {
                            self.mark_surface_read(s);
                        }
                        n
                    }
                    (None, false, true) => {
                        let n = self.notifications.iter().filter(|n| !n.read).count();
                        for n in self.notifications.iter_mut() {
                            n.read = true;
                        }
                        self.manual_unread.clear();
                        n
                    }
                    _ => bail!(
                        "mark-notification-read needs exactly one of --id, --workspace or --all"
                    ),
                };
                cx.notify();
                json!({ "marked": marked })
            }
            "notification.jump_unread" => json!({ "jumped": self.jump_to_unread(window, cx) }),
            "notification.clear" => {
                self.clear_notifications(cx);
                json!(null)
            }

            "status.set" => {
                let wi = self.workspace_target(p)?;
                let key = str_param(p, "key").context("missing <key>")?.to_string();
                let value = str_param(p, "value")
                    .context("missing <value>")?
                    .to_string();
                let color = str_param(p, "color")
                    .map(|c| {
                        parse_color(c)
                            .with_context(|| format!("bad color {c:?} (use #rgb or #rrggbb)"))
                    })
                    .transpose()?;
                let icon = str_param(p, "icon").map(String::from);
                self.set_status(
                    wi,
                    Status {
                        key,
                        value,
                        icon,
                        color,
                        owner: None,
                        program: false,
                    },
                    cx,
                );
                json!({ "workspace": self.ws_ref(wi) })
            }
            "status.clear" => {
                let wi = self.workspace_target(p)?;
                let key = str_param(p, "key").context("missing <key>")?;
                json!({ "cleared": self.clear_status(wi, key, cx) })
            }
            "status.list" => {
                let wi = self.workspace_target(p)?;
                json!(
                    self.workspaces[wi]
                        .status
                        .iter()
                        .map(status::status_json)
                        .collect::<Vec<_>>()
                )
            }
            "progress.set" => {
                let wi = self.workspace_target(p)?;
                let value = match p.get("value") {
                    Some(Value::String(v)) => v
                        .parse::<f32>()
                        .with_context(|| format!("bad progress {v:?}"))?,
                    Some(v) => v.as_f64().context("progress must be a number")? as f32,
                    None => bail!("missing <value> (0.0 to 1.0)"),
                };
                if !(0. ..=1.).contains(&value) {
                    bail!("progress must be between 0.0 and 1.0");
                }
                let label = str_param(p, "label").map(String::from);
                self.set_progress(wi, Some(Progress { value, label }), cx);
                json!({ "workspace": self.ws_ref(wi) })
            }
            "progress.clear" => {
                let wi = self.workspace_target(p)?;
                self.set_progress(wi, None, cx);
                json!(null)
            }
            "log.add" => {
                let wi = self.workspace_target(p)?;
                let level = str_param(p, "level").unwrap_or("info");
                let level = LogLevel::parse(level).with_context(|| {
                    format!("bad level {level:?} (info, progress, success, warning or error)")
                })?;
                let message = str_param(p, "message")
                    .context("missing <message>")?
                    .to_string();
                self.add_log(
                    wi,
                    level,
                    str_param(p, "source").map(String::from),
                    message,
                    cx,
                );
                json!({ "workspace": self.ws_ref(wi) })
            }
            "log.list" => {
                let wi = self.workspace_target(p)?;
                let limit = match p.get("limit") {
                    Some(Value::String(n)) => {
                        n.parse().with_context(|| format!("bad limit {n:?}"))?
                    }
                    Some(n) => n.as_u64().context("limit must be a number")? as usize,
                    None => usize::MAX,
                };
                let log = &self.workspaces[wi].log;
                let now = std::time::SystemTime::now();
                json!(
                    log[log.len().saturating_sub(limit)..]
                        .iter()
                        .map(|l| status::log_json(l, now))
                        .collect::<Vec<_>>()
                )
            }
            "log.clear" => {
                let wi = self.workspace_target(p)?;
                self.workspaces[wi].log.clear();
                cx.notify();
                json!(null)
            }

            "app.reload_config" => {
                self.reload_settings(window, cx)
                    .map_err(anyhow::Error::msg)?;
                json!({ "path": crate::settings::path() })
            }
            "app.sidebar" => {
                self.sidebar = match str_param(p, "state").unwrap_or("toggle") {
                    "show" => true,
                    "hide" => false,
                    _ => !self.sidebar,
                };
                cx.notify();
                json!({ "visible": self.sidebar })
            }
            "mr.list" => {
                let st = crate::gitlab::store(cx).read(cx);
                let ws_id = self.workspaces[self.active].id;
                let here: Vec<_> = st.pinned.get(&ws_id).cloned().unwrap_or_default();
                let group = str_param(p, "group");
                let list: Vec<Value> = st
                    .mrs
                    .values()
                    .filter_map(|m| {
                        let mut groups = vec![];
                        if here.contains(&m.key) {
                            groups.push("workspace");
                        }
                        if m.roles.any() {
                            groups.push("mine");
                        }
                        if m.inspected_at.is_some() {
                            groups.push("recent");
                        }
                        if st.open.contains(&m.key) {
                            groups.push("open");
                        }
                        if let Some(g) = group
                            && !groups.contains(&g)
                        {
                            return None;
                        }
                        let l = m.light.as_ref();
                        Some(json!({
                            "ref": m.key.reference(),
                            "title": m.title(),
                            "url": l.map(|l| l.web_url.clone()),
                            "state": l.map(|l| l.state.clone()),
                            "draft": l.map(|l| l.draft),
                            "author": l.map(|l| l.author.clone()),
                            "source_branch": l.map(|l| l.source_branch.clone()),
                            "target_branch": l.map(|l| l.target_branch.clone()),
                            "pipeline": l.and_then(|l| l.pipeline.clone()),
                            "merge_status": l.and_then(|l| l.merge_status.clone()),
                            "updated_at": l.map(|l| l.updated_at.clone()),
                            "fetched_age_seconds": (m.fetched_at > 0)
                                .then(|| crate::gitlab::now_secs().saturating_sub(m.fetched_at)),
                            "review_cached": m.review.as_ref().map(|r| json!({
                                "bytes": r.bytes, "current": !m.review_stale(),
                            })),
                            "loading": st.refreshing(&m.key),
                            "groups": groups,
                        }))
                    })
                    .collect();
                json!(list)
            }
            "mr.open" => {
                let r = str_param(p, "ref").context("missing ref")?;
                let key = self.open_mr_ref(r, window, cx)?;
                let surface = self.ws().focused_surface().map(|s| self.sref(s));
                json!({ "ref": key.reference(), "surface": surface })
            }
            "mr.refresh" => {
                let store = crate::gitlab::store(cx);
                match str_param(p, "ref") {
                    Some(r) => {
                        let key = crate::gitlab::model::MrKey::parse(r)
                            .context("not a merge request: give its URL or group/project!iid")?;
                        store.update(cx, |s, cx| {
                            s.refetch(&key);
                            cx.notify();
                        });
                    }
                    None => store.update(cx, |s, cx| s.refresh_all(cx)),
                }
                json!(null)
            }
            "link.list" => {
                let wi = self.workspace_target(p)?;
                let list: Vec<Value> = self
                    .items(wi, cx)
                    .into_iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let tab = it.tab.as_ref().map(|(s, _)| self.sref(s));
                        let title = it
                            .link
                            .as_ref()
                            .and_then(|l| l.title.clone())
                            .or_else(|| it.tab.as_ref().map(|(s, _)| s.title(cx)));
                        json!({
                            "index": i + 1,
                            "kind": it.link.as_ref().map_or("tab", |l| l.kind.name()),
                            "key": it.link.as_ref().map(|l| l.key.clone()),
                            "url": it.link.as_ref().map(|l| l.url.clone()),
                            "title": title,
                            "pinned": it.tab.is_some(),
                            "hidden": it.tab.as_ref().is_some_and(|(_, h)| *h),
                            "tab": tab,
                        })
                    })
                    .collect();
                json!(list)
            }
            "link.add" => {
                let wi = self.workspace_target(p)?;
                let r = str_param(p, "ref").context("missing <url|PAY-123|group/project!iid>")?;
                let link = self.parse_link(r, cx).map_err(anyhow::Error::msg)?;
                let had = self.link_tab(wi, &link.key, cx);
                let was_pinned = had.is_some_and(|id| self.workspaces[wi].is_pinned(id));
                let id = self.show_link(wi, &link, false, true, window, cx);
                let tab = id
                    .and_then(|id| self.surface_refs.get(&id))
                    .map(|n| format!("surface:{n}"));
                json!({
                    "key": link.key, "kind": link.kind.name(), "url": link.url,
                    "added": !was_pinned, "tab": tab,
                })
            }
            "link.remove" => {
                let wi = self.workspace_target(p)?;
                let r = str_param(p, "ref").context("missing <key|url|index>")?;
                let it = self.find_item(wi, r, cx)?;
                let key = it.link.as_ref().map(|l| l.key.clone());
                let tab = it.tab.as_ref().map(|(s, _)| self.sref(s));
                match (&it.tab, &it.link) {
                    (Some((s, _)), _) => {
                        self.unpin_tab(s.entity_id(), window, cx);
                    }
                    (None, Some(l)) => self.dismiss_link(wi, &l.key, cx),
                    (None, None) => {}
                }
                json!({ "key": key, "tab": tab })
            }
            "link.open" => {
                let wi = self.workspace_target(p)?;
                let focus = bool_param(p, "focus", true);
                let ids = if bool_param(p, "all", false) {
                    self.show_all_links(wi, window, cx)
                } else {
                    let r = str_param(p, "ref").context("missing <key|url|index>, or --all")?;
                    match self.find_item(wi, r, cx) {
                        Ok(it) => self.open_item(wi, &it, focus, window, cx),
                        Err(e) if r.parse::<usize>().is_ok() => return Err(e),
                        // Not listed yet: open it, pinned.
                        Err(_) => {
                            let l = self.parse_link(r, cx).map_err(anyhow::Error::msg)?;
                            self.show_link(wi, &l, focus, false, window, cx)
                        }
                    }
                    .into_iter()
                    .collect()
                };
                let refs: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| self.surface_refs.get(id))
                    .map(|n| json!(format!("surface:{n}")))
                    .collect();
                json!({ "surfaces": refs })
            }
            "mr.panel" => {
                self.mr_panel = match str_param(p, "state").unwrap_or("toggle") {
                    "show" => true,
                    "hide" => false,
                    _ => !self.mr_panel,
                };
                cx.notify();
                json!({ "visible": self.mr_panel })
            }
            "window.focus" => {
                cx.activate(true);
                window.activate_window();
                // Wayland ignores an app activating itself; ask the compositor.
                abromux_platform::window::raise();
                json!({ "window": "window:1" })
            }
            // For scripts/perf.py: frame timing and synthetic input.
            "debug.frame" => {
                // `refresh`: draw the next frame even if nothing changed.
                if bool_param(p, "refresh", false) {
                    window.refresh();
                }
                return Ok(later(
                    debug::after_next_frame(window, cx),
                    Duration::from_secs(10),
                    "debug.frame",
                ));
            }
            "debug.frames" => {
                if bool_param(p, "record", true) {
                    debug::FrameLog::start(&self.frames, window);
                    json!(null)
                } else {
                    debug::FrameLog::stop(&self.frames)
                }
            }
            "debug.input" => {
                let steps = str_param(p, "steps").context("missing steps")?.to_string();
                let view = cx.entity();
                // Deferred: keystrokes dispatch actions to this view, which
                // is borrowed until the handler returns.
                window.defer(cx, move |window, cx| {
                    for step in steps.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                        if let Err(e) =
                            debug::run_step(&view, step, AbroMux::script_mouse, window, cx)
                        {
                            eprintln!("abromux: debug.input: {e:#}");
                        }
                    }
                });
                json!(null)
            }
            "app.quit" => {
                cx.defer(|cx| cx.quit());
                json!(null)
            }
            _ => return Err(MethodNotFound(method.to_string()).into()),
        };
        Ok(Reply::Now(v))
    }

    /// `action.run`: an init.lua action, run as the palette runs one
    /// (`abromux do <init.lua> --action`, in the background), but answered
    /// with what it returned, or why it failed. Not on any tab: there's no
    /// caller, so it acts on what's selected.
    fn run_action(&self, p: &Value) -> anyhow::Result<Reply> {
        let name = str_param(p, "action").context("missing action")?;
        let init = crate::lua::init_path();
        let actions = crate::lua::collect_cached(&init).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            actions.iter().any(|a| a.name == name),
            "init.lua has no action {name:?}"
        );
        let args = match p.get("args") {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => bail!("args: expected an object"),
        };
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.arg("do")
            .arg(&init)
            .args(["--action", name, "--args"])
            .arg(Value::Object(args).to_string())
            .env(
                "ABROMUX_ACTION_ORIGIN",
                str_param(p, "origin").unwrap_or("api"),
            )
            .env_remove("ABROMUX_SURFACE_ID")
            .env_remove("ABROMUX_WORKSPACE_ID")
            .current_dir(crate::paths::home())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(sock) = &self.socket {
            cmd.env("ABROMUX_SOCKET_PATH", sock);
        }
        let child = cmd
            .spawn()
            .with_context(|| format!("{name}: couldn't start it"))?;
        let (tx, rx) = oneshot::channel();
        let name = name.to_string();
        // Off the main thread: the action calls back into this socket.
        std::thread::spawn(move || {
            let r = match child.wait_with_output() {
                Err(e) => Err(anyhow!("{name}: {e}")),
                Ok(out) if !out.status.success() => {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let err = err.trim().trim_start_matches("abromux: ");
                    Err(anyhow!(
                        "{}",
                        if err.is_empty() {
                            format!("{name} failed")
                        } else {
                            err.to_string()
                        }
                    ))
                }
                Ok(out) => {
                    // What it returned is the last line `abromux do` printed.
                    let text = String::from_utf8_lossy(&out.stdout);
                    let last = text.trim().lines().last().unwrap_or_default();
                    Ok(serde_json::from_str(last).unwrap_or_else(|_| {
                        if last.is_empty() {
                            Value::Null
                        } else {
                            json!(text.trim())
                        }
                    }))
                }
            };
            let _ = tx.send(r);
        });
        Ok(later(rx, Duration::from_secs(120), "action.run"))
    }

    /// `browser wait`: poll the page until the condition holds.
    fn browser_wait(&self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let b = self.browser_target(p)?;
        let (text, url, load, js) = (
            str_param(p, "text"),
            str_param(p, "url"),
            str_param(p, "load"),
            str_param(p, "js"),
        );
        let script = match (str_param(p, "selector"), js) {
            (Some(_), _) => browser_auto::script("wait_for", p),
            (None, Some(js)) => browser_auto::condition(js),
            (None, None) if text.is_some() || url.is_some() || load.is_some() => {
                browser_auto::page_condition(text, url, load)?
            }
            _ => bail!("wait for what? give a selector or ref, --text, --url, --load or --js"),
        };
        let ms = match p.get("timeout") {
            Some(Value::String(t)) => t
                .parse()
                .with_context(|| format!("bad timeout {t:?} (milliseconds)"))?,
            Some(t) => t.as_u64().context("timeout must be milliseconds")?,
            None => WAIT_TIMEOUT_MS,
        };
        let acx = cx.to_async();
        Ok(Reply::Later(
            async move {
                let start = Instant::now();
                let deadline = start + Duration::from_millis(ms);
                loop {
                    let left = deadline
                        .saturating_duration_since(Instant::now())
                        .max(WAIT_POLL);
                    // A page mid-navigation doesn't answer; try again.
                    if let Some(out) = eval_json(&b, &script, &acx, left).await?
                        && serde_json::from_str::<String>(&out).is_ok()
                        && browser_auto::parse(&out)? == json!(true)
                    {
                        return Ok(json!({ "waited_ms": start.elapsed().as_millis() as u64 }));
                    }
                    if Instant::now() >= deadline {
                        bail!("timed out after {ms} ms");
                    }
                    Timer::after(WAIT_POLL).await;
                }
            }
            .boxed_local(),
        ))
    }

    /// `browser frame <iframe>`: point automation commands at that frame
    /// (cross-origin ones too); `main` goes back to the page, and no
    /// argument says which is picked.
    fn browser_frame(&self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let b = self.browser_target(p)?;
        match str_param(p, "selector") {
            None => {
                let f = b.read(cx).frame.clone();
                return Ok(Reply::Now(json!({ "frame": f.map(|(_, url)| url) })));
            }
            Some("main" | "top" | "page") => {
                b.update(cx, |b, cx| b.set_frame(None, cx));
                return Ok(Reply::Now(json!({ "frame": null })));
            }
            Some(_) => {}
        }
        let nonce = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let answer = b.update(cx, |b, _| b.ask_frame(nonce.clone()));
        let mut opts = p.clone();
        opts["nonce"] = json!(nonce);
        let js = browser_auto::script("frame_ask", &opts);
        let mut acx = cx.to_async();
        Ok(Reply::Later(
            async move {
                let out = eval_json(&b, &js, &acx, SCRIPT_TIMEOUT)
                    .await?
                    .context("the page didn't run the script (is it still loading?)")?;
                let info = browser_auto::parse(&out)?;
                let token = match select(answer, Timer::after(Duration::from_secs(5))).await {
                    Either::Left((Ok(t), _)) => t,
                    _ => bail!(
                        "the frame didn't answer (still loading, or sandboxed without scripts?)"
                    ),
                };
                let url = info["src"].as_str().unwrap_or("").to_string();
                let ok = b.update(&mut acx, |b, cx| {
                    b.set_frame(Some((token, url.clone())), cx)
                })?;
                if !ok {
                    bail!("the frame went away before it could be picked");
                }
                Ok(json!({ "frame": url, "name": info["name"] }))
            }
            .boxed_local(),
        ))
    }

    /// `browser cookies [get|set|clear]`, through WebKit's cookie store, so
    /// HttpOnly cookies count too. Without `--all`, only the page's.
    fn browser_cookies(&self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let b = self.browser_target(p)?;
        let web = b.read(cx).page();
        let page_url = str_param(p, "url")
            .map(normalize_url)
            .unwrap_or_else(|| b.read(cx).url.clone());
        let host = host_of(&page_url);
        let all = bool_param(p, "all", false);
        let name = str_param(p, "name").map(String::from);
        let wanted = move |c: &Value| {
            let domain = c["domain"].as_str().unwrap_or("");
            (all || host.as_deref().is_some_and(|h| cookie_matches(domain, h)))
                && name
                    .as_deref()
                    .is_none_or(|n| c["name"].as_str() == Some(n))
        };
        match str_param(p, "op").unwrap_or("get") {
            "get" => Ok(Reply::Later(
                async move {
                    let list = native(web.cookies(), SCRIPT_TIMEOUT).await?;
                    let list: Vec<Value> = list.as_array().into_iter().flatten().filter(|c| wanted(c)).cloned().collect();
                    Ok(json!(list))
                }
                .boxed_local(),
            )),
            "set" => {
                let cname = str_param(p, "name").context("cookies set needs a name")?;
                let domain = match str_param(p, "domain") {
                    Some(d) => d.to_string(),
                    None => host_of(&page_url).context("no page to set the cookie for; give --domain or --url")?,
                };
                let expires = match p.get("expires") {
                    None | Some(Value::Null) => -1.,
                    Some(Value::String(e)) => e.parse::<f64>().with_context(|| format!("bad --expires {e:?} (seconds since 1970)"))?,
                    Some(v) => v.as_f64().context("--expires must be seconds since 1970")?,
                };
                let cookie = json!([{
                    "name": cname,
                    "value": str_param(p, "value").unwrap_or(""),
                    "domain": domain,
                    "path": str_param(p, "path").unwrap_or("/"),
                    "expires": expires,
                    "secure": bool_param(p, "secure", false),
                    "httpOnly": bool_param(p, "http_only", false),
                    "sameSite": str_param(p, "same_site").unwrap_or("Lax"),
                }]);
                let rx = web.set_cookies(&cookie.to_string());
                Ok(Reply::Later(
                    async move {
                        native(rx, SCRIPT_TIMEOUT).await?;
                        Ok(cookie[0].clone())
                    }
                    .boxed_local(),
                ))
            }
            "clear" | "delete" => Ok(Reply::Later(
                async move {
                    let list = native(web.cookies(), SCRIPT_TIMEOUT).await?;
                    let gone: Vec<Value> = list
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|c| wanted(c))
                        .map(|c| json!({ "name": c["name"], "domain": c["domain"], "path": c["path"] }))
                        .collect();
                    let n = native(web.delete_cookies(&json!(gone).to_string()), SCRIPT_TIMEOUT).await?;
                    Ok(json!({ "deleted": n }))
                }
                .boxed_local(),
            )),
            op => bail!("unknown cookies operation {op:?} (get, set or clear)"),
        }
    }

    /// `browser state save|load <file>`: cookies and the page origin's
    /// localStorage, in Playwright's storageState format.
    fn browser_state(&self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let b = self.browser_target(p)?;
        let web = b.read(cx).page();
        let path = PathBuf::from(str_param(p, "path").context("state needs a file")?);
        let op = str_param(p, "op").unwrap_or("save").to_string();
        let acx = cx.to_async();
        let origin_js = "JSON.stringify({ origin: location.origin, localStorage: Object.keys(localStorage).map(name => ({ name, value: localStorage.getItem(name) })) })";
        Ok(Reply::Later(
            async move {
                let page = eval_page(&b, origin_js, &acx).await;
                let page: Option<Value> = page
                    .and_then(|s| serde_json::from_str::<String>(&s).ok())
                    .and_then(|s| serde_json::from_str(&s).ok());
                match op.as_str() {
                    "save" => {
                        let cookies = native(web.cookies(), SCRIPT_TIMEOUT).await?;
                        let origins: Vec<Value> = page
                            .into_iter()
                            .filter(|o| o["origin"].as_str().is_some_and(|o| o != "null"))
                            .collect();
                        let state = json!({ "cookies": cookies, "origins": origins });
                        std::fs::write(&path, serde_json::to_string_pretty(&state)?)
                            .with_context(|| format!("can't write {}", path.display()))?;
                        Ok(json!({
                            "path": path,
                            "cookies": cookies.as_array().map_or(0, Vec::len),
                            "origins": origins.len(),
                        }))
                    }
                    "load" => {
                        let text = std::fs::read_to_string(&path)
                            .with_context(|| format!("can't read {}", path.display()))?;
                        let state: Value = serde_json::from_str(&text)
                            .with_context(|| format!("{} isn't a state file", path.display()))?;
                        let cookies = state["cookies"].as_array().cloned().unwrap_or_default();
                        let set = if cookies.is_empty() {
                            json!(0)
                        } else {
                            native(web.set_cookies(&json!(cookies).to_string()), SCRIPT_TIMEOUT).await?
                        };
                        let here = page.as_ref().and_then(|p| p["origin"].as_str()).unwrap_or("").to_string();
                        let (mut applied, mut skipped) = (vec![], vec![]);
                        for o in state["origins"].as_array().into_iter().flatten() {
                            let origin = o["origin"].as_str().unwrap_or("").to_string();
                            if origin != here || here.is_empty() {
                                skipped.push(origin);
                                continue;
                            }
                            let items = o["localStorage"].clone();
                            let js = format!(
                                "(() => {{ for (const i of {}) localStorage.setItem(i.name, i.value); return 'ok'; }})()",
                                items
                            );
                            if eval_page(&b, &js, &acx).await.is_none() {
                                bail!("the page didn't take its localStorage (is it still loading?)");
                            }
                            applied.push(origin);
                        }
                        Ok(json!({ "cookies": set, "origins": applied, "skipped": skipped }))
                    }
                    op => bail!("unknown state operation {op:?} (save or load)"),
                }
            }
            .boxed_local(),
        ))
    }

    /// The workspace and pane holding surface `id`.
    fn pane_of(&self, id: EntityId) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(wi, ws)| {
            ws.panes()
                .find(|p| p.tabs.iter().any(|t| t.entity_id() == id))
                .map(|p| (wi, p.id))
        })
    }

    /// `browser tab list|new|switch|close`: the browser tabs of the pane the
    /// browser is in.
    fn browser_tab(
        &mut self,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        let current = self.browser_target(p).ok();
        let (wi, pane_id) = match &current {
            Some(b) => self
                .pane_of(b.entity_id())
                .context("that browser isn't in a pane")?,
            None => self.pane_target(p)?,
        };
        let tabs: Vec<(usize, Surface, bool)> = {
            let ws = &self.workspaces[wi];
            let pane = ws
                .panes()
                .find(|q| q.id == pane_id)
                .context("no such pane")?;
            pane.tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| matches!(t, Surface::Browser(_)))
                .map(|(i, t)| (i, t.clone(), i == pane.active))
                .collect()
        };
        let pick = |t: &str| -> anyhow::Result<Surface> {
            if let Ok(n) = t.parse::<usize>() {
                return tabs
                    .get(n)
                    .map(|(_, s, _)| s.clone())
                    .with_context(|| format!("no browser tab {n}: the pane has {}", tabs.len()));
            }
            let (_, _, s) = self.find_surface(t, Some(wi))?;
            tabs.iter()
                .find(|(_, x, _)| x.entity_id() == s.entity_id())
                .map(|(_, s, _)| s.clone())
                .with_context(|| format!("{t} isn't a browser tab in this pane"))
        };
        match str_param(p, "op").unwrap_or("list") {
            "list" => Ok(json!(
                tabs.iter()
                    .enumerate()
                    .map(|(n, (_, s, active))| {
                        let mut v = self.describe_surface(s, *active, cx);
                        v["index"] = json!(n);
                        v
                    })
                    .collect::<Vec<_>>()
            )),
            "new" => {
                let mut opts = json!({ "type": "browser", "builtIn": true });
                if let Some(url) = str_param(p, "url") {
                    opts["url"] = json!(url);
                }
                let s = self.make_surface(wi, &opts, window, cx)?;
                let out = self.describe_surface(&s, true, cx);
                self.add_tab_to(wi, pane_id, s, bool_param(p, "focus", true), window, cx);
                Ok(out)
            }
            "switch" => {
                let s =
                    pick(str_param(p, "target").context("tab switch needs an index or surface")?)?;
                self.focus_surface(s.entity_id(), cx);
                self.select_workspace(wi, window, cx);
                Ok(self.describe_surface(&s, true, cx))
            }
            "close" => {
                let s = match str_param(p, "target") {
                    Some(t) => pick(t)?,
                    None => current
                        .map(Surface::Browser)
                        .context("no browser tab to close")?,
                };
                let r = self.sref(&s);
                self.close_surface(s.entity_id(), window, cx);
                Ok(json!({ "closed": r }))
            }
            op => bail!("unknown tab operation {op:?} (list, new, switch or close)"),
        }
    }

    /// `browser wait-download`: wait for a download to finish (one running
    /// now, else the next to start, or `--id`) and say where it went;
    /// `--path` moves it there.
    fn wait_download(&self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let d = crate::downloads::entity(cx);
        let ms = match p.get("timeout") {
            Some(Value::String(t)) => t
                .parse()
                .with_context(|| format!("bad timeout {t:?} (milliseconds)"))?,
            Some(t) => t.as_u64().context("timeout must be milliseconds")?,
            None => 30_000,
        };
        let since = d.read(cx).last_id();
        let want = usize_param(p, "id")?.map(|n| n as u64);
        let running = d
            .read(cx)
            .list
            .iter()
            .rev()
            .find(|x| x.state == crate::downloads::State::Running)
            .map(|x| x.id);
        let target = want.or(running);
        let dest = str_param(p, "path").map(PathBuf::from);
        let mut acx = cx.to_async();
        Ok(Reply::Later(
            async move {
                let deadline = Instant::now() + Duration::from_millis(ms);
                loop {
                    let found = d.read_with(&acx, |d, _| {
                        let pick = match target {
                            Some(id) => d.get(id),
                            None => d.list.iter().find(|x| x.id > since),
                        };
                        pick.cloned()
                    })?;
                    if let Some(x) = found {
                        match x.state {
                            crate::downloads::State::Running => {}
                            crate::downloads::State::Done => {
                                let mut out = x.to_json();
                                if let Some(dest) = &dest {
                                    let to = if dest.is_dir() {
                                        dest.join(x.name())
                                    } else {
                                        dest.clone()
                                    };
                                    if std::fs::rename(&x.path, &to).is_err() {
                                        std::fs::copy(&x.path, &to).with_context(|| {
                                            format!("can't move it to {}", to.display())
                                        })?;
                                        let _ = std::fs::remove_file(&x.path);
                                    }
                                    out["path"] = json!(to);
                                    d.update(&mut acx, |d, cx| d.moved(x.id, to.clone(), cx))?;
                                }
                                return Ok(out);
                            }
                            s => bail!("download {} {}", x.id, s.name()),
                        }
                    } else if let Some(want) = want {
                        bail!("no download {want}");
                    }
                    if Instant::now() >= deadline {
                        bail!("timed out after {ms} ms");
                    }
                    Timer::after(WAIT_POLL).await;
                }
            }
            .boxed_local(),
        ))
    }

    /// `surface.wait`: until the command in a terminal finishes (from the
    /// shell integration's marks; without them, until it closes), or with
    /// `close` until it closes. `after` waits for a command after that many
    /// had finished (`new-split --command … --wait`), not one already done.
    /// `ui.prompt`: ask in the palette and answer once the person does,
    /// cancels, the time runs out or the asker hangs up.
    fn ui_prompt(
        &mut self,
        p: &Value,
        gone: &Hangup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Reply> {
        let pick = match str_param(p, "kind") {
            Some("input") => false,
            Some("pick") => true,
            _ => bail!("ui.prompt: kind must be input or pick"),
        };
        let title = str_param(p, "title")
            .context("ui.prompt: title is required")?
            .to_string();
        let mut items: Vec<crate::palette::PromptItem> = vec![];
        for it in p["items"].as_array().into_iter().flatten() {
            let text = |v: &Value| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            };
            let item = match it {
                Value::Object(o) => {
                    let id = o
                        .get("id")
                        .and_then(text)
                        .context("ui.prompt: each item needs an id")?;
                    let title = o.get("title").and_then(text).unwrap_or_else(|| id.clone());
                    let sub = o.get("subtitle").and_then(text);
                    (id, title, sub, o.get("detail").and_then(text))
                }
                v => {
                    let id =
                        text(v).context("ui.prompt: an item is a string or a table with an id")?;
                    (id.clone(), id, None, None)
                }
            };
            if items.iter().any(|(id, ..)| *id == item.0) {
                bail!("ui.prompt: two items have the id {:?}", item.0);
            }
            items.push(item);
        }
        if items.len() > 2000 {
            bail!("ui.prompt: at most 2000 items");
        }
        if self
            .palette
            .as_ref()
            .is_some_and(|q| q.mode == crate::palette::Mode::Prompt)
        {
            return Ok(Reply::Now(json!({ "reason": "busy" })));
        }
        let timeout = match p.get("timeout") {
            None | Some(Value::Null) => 300.,
            Some(v) => v.as_f64().context("ui.prompt: bad timeout")?,
        }
        .clamp(0., 3600.);
        let (tx, rx) = oneshot::channel::<Value>();
        let prompt = crate::palette::Prompt {
            id: 0,
            title,
            pick,
            allow_other: bool_param(p, "allow_other", false),
            confirm: str_param(p, "confirm").map(String::from),
            answer: Some(tx),
        };
        let id = self.open_prompt(
            prompt,
            items,
            str_param(p, "placeholder").map(String::from),
            str_param(p, "value").map(String::from),
            window,
            cx,
        );
        window.activate_window();
        let this = cx.weak_entity();
        let handle = window.window_handle();
        let mut acx = cx.to_async();
        let gone = gone.clone();
        Ok(Reply::Later(
            async move {
                let limit = Timer::after(Duration::from_secs_f64(timeout));
                let (reason, close) = match select(rx, select(limit, gone)).await {
                    Either::Left((Ok(v), _)) => return Ok(v),
                    Either::Left((Err(_), _)) => ("cancelled", false),
                    Either::Right((Either::Left(_), _)) => ("timeout", true),
                    Either::Right((Either::Right(_), _)) => ("cancelled", true),
                };
                if close {
                    let _ = gpui::AppContext::update_window(&mut acx, handle, |_, window, cx| {
                        this.update(cx, |this, cx| this.close_prompt(id, window, cx))
                    });
                }
                Ok(json!({ "reason": reason }))
            }
            .boxed_local(),
        ))
    }

    fn wait_surface(&mut self, p: &Value, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let (_, _, s) = self.surface_target(p)?;
        let close = bool_param(p, "close", false);
        let term = match &s {
            Surface::Terminal(t) => Some(t.clone()),
            _ if close => None,
            _ => bail!("only a terminal runs commands to wait for (use --close for any tab)"),
        };
        let n = self.surface_refs.get(&s.entity_id()).copied().unwrap_or(0);
        let after = p.get("after").and_then(|v| match v {
            Value::String(s) => s.parse().ok(),
            v => v.as_u64(),
        });
        let timeout = match p.get("timeout") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) => Some(t.parse::<f64>().context("bad --timeout")?),
            Some(v) => Some(v.as_f64().context("bad --timeout")?),
        };
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t.max(0.)));
        let this = cx.weak_entity();
        let acx = cx.to_async();
        Ok(Reply::Later(
            async move {
                let surface = format!("surface:{n}");
                loop {
                    let alive = this
                        .read_with(&acx, |this, _| this.surface_refs.values().any(|&r| r == n))
                        .unwrap_or(false);
                    if !alive {
                        // Exited by itself: its status, None if a signal ended
                        // it. Not there: the tab was closed.
                        let exit = this
                            .read_with(&acx, |this, _| this.exits.get(&n).copied())
                            .ok()
                            .flatten();
                        return Ok(json!({ "surface": surface, "closed": true,
                                          "exit_code": exit.flatten(),
                                          "signaled": exit == Some(None) }));
                    }
                    if !close
                        && let Some(c) = term
                            .as_ref()
                            .and_then(|t| t.read_with(&acx, |t, _| t.commands).ok())
                        && c.marked
                    {
                        let done = match after {
                            Some(a) => c.finished > a,
                            None => !c.running,
                        };
                        if done {
                            return Ok(json!({ "surface": surface, "closed": false,
                                              "exit_code": c.last_exit }));
                        }
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        bail!("timed out waiting for {surface}");
                    }
                    Timer::after(WAIT_POLL).await;
                }
            }
            .boxed_local(),
        ))
    }

    /// Status and notifications from a coding agent's hooks (agent.rs).
    fn agent_event(
        &mut self,
        surface: gpui::EntityId,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = str_param(p, "kind").and_then(Kind::parse) else {
            return;
        };
        self.claim_naming(surface, cx);
        if let Some(prompt) = str_param(p, "prompt") {
            self.auto_name(surface, kind, prompt.to_string(), cx);
        }
        let pending = self.agent_pending.entry(surface).or_default();
        pending.note(p);
        let mode = cx
            .try_global::<crate::settings::Settings>()
            .map(|s| s.notifications.agent_turn_complete)
            .unwrap_or_default();
        let stop = pending.on_stop(mode);
        let Some(event) = AgentEvent::of_report(p) else {
            return;
        };
        let (label, title) = (kind.label(), kind.title());
        let state = match &event {
            AgentEvent::Working => "working",
            AgentEvent::NeedsInput(_) => "needs_input",
            AgentEvent::Stopped(_) => "stopped",
            AgentEvent::Ended => "ended",
        };
        self.emit_for(
            "agent",
            surface,
            json!({ "agent": kind.as_str(), "state": state }),
        );
        match event {
            AgentEvent::Working => {
                self.set_agent_status(surface, label, Some(("Running", RUNNING)), cx)
            }
            AgentEvent::NeedsInput(message) => {
                let message = message.unwrap_or_else(|| format!("{label} needs your attention"));
                self.set_agent_status(surface, label, Some(("Needs input", WARNING)), cx);
                self.add_notification(
                    surface,
                    title.into(),
                    message,
                    NoteKind::NeedsInput,
                    window,
                    cx,
                );
            }
            // A turn that left work running which will wake the agent again
            // (a background shell, a monitor, a scheduled wakeup) isn't the
            // end: it stays "Running", and by default says nothing.
            AgentEvent::Stopped(summary) => {
                let state = if stop.done {
                    ("Done", SUCCESS)
                } else {
                    ("Running", RUNNING)
                };
                self.set_agent_status(surface, label, Some(state), cx);
                if stop.notify {
                    let body = summary.unwrap_or_else(|| "Finished".into());
                    self.add_notification(surface, title.into(), body, NoteKind::Done, window, cx);
                }
            }
            AgentEvent::Ended => self.set_agent_status(surface, label, None, cx),
        }
    }

    /// `tab-action`: what the tab bar's right-click menu does, on one tab.
    fn tab_action(
        &mut self,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Reply> {
        let action = str_param(p, "action").context("missing <action>")?;
        let (wi, pane, s) = self.surface_target(p)?;
        let id = s.entity_id();
        let tabs: Vec<EntityId> = self.workspaces[wi]
            .panes()
            .find(|x| x.id == pane)
            .map(|x| x.tabs.iter().map(|t| t.entity_id()).collect())
            .unwrap_or_default();
        let at = tabs.iter().position(|&t| t == id).unwrap_or(0);
        // cmux's defaults: new tabs take focus, a moved one doesn't.
        let focus = bool_param(p, "focus", action != "move-to-new-workspace");
        let v = match action {
            "rename" => {
                let title = str_param(p, "title").context("rename needs a title")?;
                s.set_custom_title(Some(one_line(title)), cx);
                self.describe_moved(id, cx)?
            }
            "clear-name" => {
                s.set_custom_title(None, cx);
                self.describe_moved(id, cx)?
            }
            "close-left" | "close-right" | "close-others" => {
                let close: Vec<EntityId> = match action {
                    "close-left" => tabs[..at].to_vec(),
                    "close-right" => tabs[at + 1..].to_vec(),
                    _ => tabs.iter().copied().filter(|&t| t != id).collect(),
                };
                for t in &close {
                    self.close_surface(*t, window, cx);
                }
                let mut v = self.describe_moved(id, cx)?;
                v["closed"] = json!(close.len());
                v
            }
            "new-terminal-right" | "new-browser-right" | "duplicate" => {
                let mut q = json!({});
                if action == "new-browser-right" {
                    q["type"] = json!("browser");
                    if let Some(url) = str_param(p, "url") {
                        q["url"] = json!(url);
                    }
                } else if action == "duplicate" {
                    match &s {
                        Surface::Browser(b) => {
                            q["type"] = json!("browser");
                            q["url"] = json!(b.read(cx).url);
                        }
                        Surface::Terminal(_) => {
                            if let Some(cwd) = s.cwd(cx) {
                                q["cwd"] = json!(cwd);
                            }
                        }
                        Surface::Plugin(t) => {
                            q = t.save(cx);
                            q["type"] = json!(t.plugin());
                        }
                    }
                }
                let new = self.make_surface(wi, &q, window, cx)?;
                let nid = new.entity_id();
                let ws = &mut self.workspaces[wi];
                let was = ws
                    .pane_mut(pane)
                    .and_then(|x| x.active_surface().map(|s| s.entity_id()));
                if ws.insert_tab(pane, at + 1, new).is_err() {
                    bail!("pane:{pane} is gone");
                }
                if focus {
                    self.focus_surface(nid, cx);
                    self.select_workspace(wi, window, cx);
                } else if let Some(x) = ws.pane_mut(pane)
                    && let Some(i) = x.tabs.iter().position(|t| Some(t.entity_id()) == was)
                {
                    // insert_tab made the new tab active; keep the old one.
                    x.active = i;
                }
                self.describe_moved(nid, cx)?
            }
            "move-to-new-workspace" => {
                let name = str_param(p, "title").map(one_line);
                self.tab_to_new_workspace(id, name, focus, window, cx)?;
                self.describe_moved(id, cx)?
            }
            "reload" => {
                match &s {
                    Surface::Browser(b) => b.read(cx).reload(cx),
                    Surface::Terminal(_) => {
                        bail!("reload is for browser tabs (respawn-pane restarts a terminal)")
                    }
                    Surface::Plugin(_) => bail!("reload is for browser tabs"),
                }
                self.describe_moved(id, cx)?
            }
            "mark-read" => {
                self.mark_surface_read(id);
                self.describe_moved(id, cx)?
            }
            "mark-unread" => {
                self.mark_surface_unread(id);
                self.describe_moved(id, cx)?
            }
            "pin" | "unpin" => bail!("abromux has no pinned tabs yet"),
            a => bail!(
                "unknown action {a:?}; expected rename, clear-name, close-left, close-right, \
                 close-others, new-terminal-right, new-browser-right, duplicate, \
                 move-to-new-workspace, reload, mark-read or mark-unread"
            ),
        };
        cx.notify();
        Ok(Reply::Now(v))
    }

    // ---- targets --------------------------------------------------------

    pub(crate) fn ws_ref(&self, wi: usize) -> String {
        format!("workspace:{}", self.workspaces[wi].id)
    }

    pub(crate) fn sref(&self, s: &Surface) -> String {
        format!(
            "surface:{}",
            self.surface_refs.get(&s.entity_id()).copied().unwrap_or(0)
        )
    }

    fn find_workspace(&self, s: &str) -> anyhow::Result<usize> {
        self.find_workspace_in(s, None)
    }

    /// A workspace by ref, index or name (case aside; its custom name
    /// before the one it shows).
    fn find_workspace_in(&self, s: &str, cx: Option<&App>) -> anyhow::Result<usize> {
        let found = match parse_sel(s, "workspace")? {
            Sel::Ref(id) => self.workspaces.iter().position(|w| w.id == id),
            Sel::Index(i) => (i < self.workspaces.len()).then_some(i),
            Sel::Label(l) => {
                let named = |name: &dyn Fn(&Workspace) -> Option<String>| {
                    self.workspaces
                        .iter()
                        .enumerate()
                        .filter_map(|(i, w)| {
                            let n = name(w)?;
                            n.eq_ignore_ascii_case(&l).then(|| (i, self.ws_ref(i), n))
                        })
                        .collect::<Vec<_>>()
                };
                let mut found = named(&|w| w.custom_name.clone());
                if found.is_empty()
                    && let Some(cx) = cx
                {
                    found = named(&|w| Some(w.name(cx)));
                }
                return one_labelled("workspace", &l, found);
            }
        };
        found.ok_or_else(|| TargetError(format!("no workspace {s}")).into())
    }

    /// The `--workspace` argument, required.
    fn workspace_arg(&self, p: &Value) -> anyhow::Result<usize> {
        self.find_workspace(str_param(p, "workspace").context("missing --workspace")?)
    }

    /// The terminal `abromux` was run from, if it still exists.
    fn caller(&self, p: &Value) -> Option<(usize, usize, Surface)> {
        p["caller"]["surface"]
            .as_str()
            .and_then(|s| self.find_surface(s, None).ok())
    }

    /// `--workspace`, else the caller's workspace, else the selected one.
    /// One of workspace `wi`'s sidebar items by its number in the list
    /// (from 1), a pinned tab's ref, a link's key, or any way of writing it
    /// (`link add` takes).
    fn find_item(&self, wi: usize, r: &str, cx: &App) -> anyhow::Result<crate::links::Item> {
        let items = self.items(wi, cx);
        if let Ok(n) = r.parse::<usize>() {
            return n
                .checked_sub(1)
                .and_then(|i| items.get(i).cloned())
                .with_context(|| format!("no link {n}: the workspace has {}", items.len()));
        }
        if r.starts_with("surface:") {
            return items
                .into_iter()
                .find(|it| it.tab.as_ref().is_some_and(|(s, _)| self.sref(s) == r))
                .with_context(|| format!("{r} isn't pinned here"));
        }
        let key = self
            .parse_link(r, cx)
            .map(|l| l.key)
            .unwrap_or_else(|_| r.to_string());
        items
            .into_iter()
            .find(|it| it.link.as_ref().is_some_and(|l| l.key == key))
            .with_context(|| format!("{r} isn't one of this workspace's links"))
    }

    fn workspace_target(&self, p: &Value) -> anyhow::Result<usize> {
        if let Some(w) = str_param(p, "workspace") {
            return self.find_workspace(w);
        }
        Ok(self.caller(p).map(|(wi, _, _)| wi).unwrap_or(self.active))
    }

    /// A tab by its own name (`rename-tab`, `--label`), case aside: in
    /// workspace `within` when given, else anywhere.
    fn surface_named(&self, l: &str, within: Option<usize>, cx: &App) -> anyhow::Result<String> {
        let found = within
            .map_or_else(|| (0..self.workspaces.len()).collect(), |w| vec![w])
            .into_iter()
            .flat_map(|wi| {
                self.workspaces[wi]
                    .all_surfaces()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .filter_map(|t| {
                let name = t.custom_title(cx)?;
                name.eq_ignore_ascii_case(l).then(|| {
                    let r = self.sref(&t);
                    (r.clone(), r, name)
                })
            })
            .collect();
        one_labelled("surface", l, found)
    }

    /// Workspace, surface, tab and group names in `p` turned into refs, so
    /// the request can find them the usual way. None if it names nothing.
    fn resolve_names(&self, p: &Value, cx: &App) -> anyhow::Result<Option<Value>> {
        let is_name = |k: &str, kind: &str| {
            str_param(p, k).is_some_and(|s| matches!(parse_sel(s, kind), Ok(Sel::Label(_))))
        };
        let keys = [
            ("workspace", "workspace"),
            ("surface", "surface"),
            ("tab", "surface"),
            ("group", "group"),
        ];
        if !keys.iter().any(|(k, kind)| is_name(k, kind)) {
            return Ok(None);
        }
        let mut q = p.clone();
        let mut within = None;
        if is_name("workspace", "workspace") {
            let wi = self.find_workspace_in(str_param(p, "workspace").unwrap(), Some(cx))?;
            q["workspace"] = json!(self.ws_ref(wi));
            within = Some(wi);
        } else if let Some(w) = str_param(p, "workspace") {
            within = self.find_workspace(w).ok();
        }
        for k in ["surface", "tab"] {
            if is_name(k, "surface") {
                q[k] = json!(self.surface_named(str_param(p, k).unwrap(), within, cx)?);
            }
        }
        if is_name("group", "group") {
            q["group"] = json!(format!(
                "group:{}",
                self.find_group(str_param(p, "group").unwrap())?
            ));
        }
        Ok(Some(q))
    }

    /// A surface ref, or an index into `within`'s surfaces in tree order.
    fn find_surface(
        &self,
        s: &str,
        within: Option<usize>,
    ) -> anyhow::Result<(usize, usize, Surface)> {
        let all = |wi: usize| {
            let ws = &self.workspaces[wi];
            ws.panes()
                .flat_map(|p| p.tabs.iter().map(move |t| (wi, p.id, t.clone())))
                .collect::<Vec<_>>()
        };
        // A hidden pinned tab, by its ref: with the pane it was in.
        let hidden = |wi: usize| {
            self.workspaces[wi]
                .hidden
                .iter()
                .map(move |h| (wi, h.pane, h.surface.clone()))
                .collect::<Vec<_>>()
        };
        match parse_sel(s, "surface")? {
            Sel::Ref(n) => (0..self.workspaces.len())
                .flat_map(|wi| all(wi).into_iter().chain(hidden(wi)))
                .find(|(_, _, t)| self.surface_refs.get(&t.entity_id()) == Some(&n)),
            Sel::Index(i) => all(within.unwrap_or(self.active)).into_iter().nth(i),
            // Names were turned into refs before the request ran
            // (`resolve_names`); one left here names nothing.
            Sel::Label(l) => {
                return Err(TargetError(format!("no surface named {l:?}")).into());
            }
        }
        .ok_or_else(|| TargetError(format!("no surface {s}")).into())
    }

    /// `--surface`, else the focused surface of `--workspace`, else the
    /// caller, else the focused surface of the selected workspace.
    fn surface_target(&self, p: &Value) -> anyhow::Result<(usize, usize, Surface)> {
        let ws = str_param(p, "workspace")
            .map(|w| self.find_workspace(w))
            .transpose()?;
        if let Some(s) = str_param(p, "surface") {
            return self.find_surface(s, ws);
        }
        if ws.is_none()
            && let Some(c) = self.caller(p)
        {
            return Ok(c);
        }
        let wi = ws.unwrap_or(self.active);
        let w = &self.workspaces[wi];
        let s = w
            .focused_surface()
            .context("workspace has no surfaces")?
            .clone();
        Ok((wi, w.focused_pane, s))
    }

    fn terminal_target(&self, p: &Value) -> anyhow::Result<Entity<crate::TerminalView>> {
        match self.surface_target(p)?.2 {
            Surface::Terminal(t) => Ok(t),
            Surface::Browser(_) => bail!("that surface is a browser, not a terminal"),
            Surface::Plugin(t) => bail!("that surface is a {} tab, not a terminal", t.plugin()),
        }
    }

    /// `--surface`, else the workspace's focused surface if it's a browser,
    /// else its first browser.
    fn browser_target(&self, p: &Value) -> anyhow::Result<Entity<BrowserView>> {
        if str_param(p, "surface").is_some() {
            return match self.surface_target(p)?.2 {
                Surface::Browser(b) => Ok(b),
                Surface::Terminal(_) => bail!("that surface is a terminal, not a browser"),
                Surface::Plugin(t) => bail!("that surface is a {} tab, not a browser", t.plugin()),
            };
        }
        let ws = &self.workspaces[self.workspace_target(p)?];
        let browser = |s: &Surface| match s {
            Surface::Browser(b) => Some(b.clone()),
            _ => None,
        };
        ws.focused_surface()
            .and_then(browser)
            .or_else(|| ws.surfaces().find_map(browser))
            .context("no browser surface in this workspace (open one with `abromux browser open`)")
    }

    /// `--pane` (ref or index within the workspace), else the pane holding
    /// the target surface.
    fn pane_target(&self, p: &Value) -> anyhow::Result<(usize, usize)> {
        let Some(s) = str_param(p, "pane") else {
            let (wi, pane, _) = self.surface_target(p)?;
            return Ok((wi, pane));
        };
        match parse_sel(s, "pane")? {
            Sel::Ref(id) => self
                .workspaces
                .iter()
                .enumerate()
                .find_map(|(wi, w)| w.panes().any(|p| p.id == id).then_some((wi, id))),
            Sel::Index(i) => {
                let wi = self.workspace_target(p)?;
                self.workspaces[wi].panes().nth(i).map(|p| (wi, p.id))
            }
            Sel::Label(_) => {
                return Err(TargetError(format!(
                    "panes have no names: give pane:N or an index, not {s:?}"
                ))
                .into());
            }
        }
        .ok_or_else(|| TargetError(format!("no pane {s}")).into())
    }

    /// `--surface`, else `--pane`'s active tab, else the usual target.
    fn source_surface(&self, p: &Value) -> anyhow::Result<(usize, usize, Surface)> {
        if str_param(p, "surface").is_none() && str_param(p, "pane").is_some() {
            let (wi, pane) = self.pane_target(p)?;
            let s = self.workspaces[wi]
                .panes()
                .find(|x| x.id == pane)
                .and_then(|x| x.active_surface())
                .cloned()
                .context("that pane has no tabs")?;
            return Ok((wi, pane, s));
        }
        self.surface_target(p)
    }

    /// Where tab `id` is after a move: its ref, workspace, pane and index.
    fn describe_moved(&self, id: EntityId, cx: &App) -> anyhow::Result<Value> {
        let wi = self.workspace_of(id).context("the tab is gone")?;
        let ws = &self.workspaces[wi];
        let (pane, index, _) = ws.tab_position(id).context("the tab is gone")?;
        let s = self.find_tab(id).context("the tab is gone")?;
        let mut v = self.describe_surface(
            &s,
            ws.focused_surface().is_some_and(|f| f.entity_id() == id),
            cx,
        );
        v["workspace"] = json!(self.ws_ref(wi));
        v["pane"] = json!(format!("pane:{pane}"));
        v["index"] = json!(index);
        Ok(v)
    }

    // ---- actions --------------------------------------------------------

    /// A plugin's [`abromux_plugin::Run`]: typed into terminal `send_to`,
    /// or a new terminal running `command` beside tab `beside`: a tab in
    /// the pane to the right of (else any pane besides) its own, else a
    /// split to its right. Focus stays where it is.
    pub(crate) fn run_beside(
        &mut self,
        run: abromux_plugin::Run,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if let Some(to) = &run.send_to {
            let t = self.terminal_target(&json!({ "surface": to }))?;
            submit_to(&t, &run.text.clone().unwrap_or_default(), cx);
            return Ok(());
        }
        let (wi, pane) = match run.beside.and_then(|id| Some((self.workspace_of(id)?, id))) {
            Some((wi, id)) => (
                wi,
                self.workspaces[wi]
                    .tab_position(id)
                    .context("the tab is gone")?
                    .0,
            ),
            None => (self.active, self.workspaces[self.active].focused_pane),
        };
        let p = json!({ "type": "terminal", "cwd": run.cwd, "command": run.command });
        let s = self.make_surface(wi, &p, window, cx)?;
        if run.title.is_some() {
            s.set_custom_title(run.title.clone(), cx);
        }
        let new_id = s.entity_id();
        let panes: Vec<usize> = self.workspaces[wi].panes().map(|p| p.id).collect();
        let at = panes.iter().position(|p| *p == pane).unwrap_or(0);
        let other = panes
            .get(at + 1)
            .or(at.checked_sub(1).and_then(|i| panes.get(i)))
            .copied();
        match other {
            Some(o) => {
                self.add_tab_to(wi, o, s, false, window, cx);
                // Showing in its pane, but the focus stays on the plugin's tab.
                if let Some(p) = self.workspaces[wi].pane_mut(o)
                    && let Some(i) = p.tabs.iter().position(|t| t.entity_id() == new_id)
                {
                    p.active = i;
                }
            }
            None => {
                self.split_pane(
                    wi,
                    pane,
                    gpui::Axis::Horizontal,
                    false,
                    s,
                    false,
                    window,
                    cx,
                )
                .context("pane not found")?;
            }
        }
        cx.notify();
        Ok(())
    }

    /// A terminal (`--cwd`, `--command`) or browser (`--url`) for workspace `wi`.
    /// A new tab as `p` says (`type`, `url`, `command`, …), named `label`
    /// if given, so scripts can target it by that name.
    fn make_surface(
        &mut self,
        wi: usize,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Surface> {
        let s = self.make_surface_of_type(wi, p, window, cx)?;
        if let Some(label) = str_param(p, "label") {
            s.set_custom_title(Some(one_line(label)), cx);
        }
        Ok(s)
    }

    fn make_surface_of_type(
        &mut self,
        wi: usize,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Surface> {
        Ok(match str_param(p, "type").unwrap_or("terminal") {
            "terminal" => {
                let cwd = str_param(p, "cwd")
                    .map(String::from)
                    .or_else(|| self.workspaces[wi].cwd(cx));
                let s = self.make_terminal(
                    self.workspaces[wi].id,
                    crate::TerminalOpts::cwd(cwd),
                    window,
                    cx,
                );
                if let (Some(cmd), Surface::Terminal(t)) = (str_param(p, "command"), &s) {
                    t.update(cx, |t, _| t.send(format!("{cmd}\r").as_bytes()));
                }
                s
            }
            "browser" => {
                let url = str_param(p, "url")
                    .map(normalize_url)
                    .unwrap_or_else(|| DEFAULT_URL.into());
                let claimed = crate::plugins::claiming(&url, cx);
                if bool_param(p, "builtIn", false) {
                    self.make_builtin_browser(&url, window, cx)
                } else if let Some(Ok(s)) =
                    claimed.map(|pl| self.make_plugin(pl.id, &json!({ "url": url }), window, cx))
                {
                    // A link a plugin shows itself (a GitLab merge request)
                    // opens as its tab, as from a terminal or the built-in
                    // browser: terminal-browser's new-tab links come here.
                    s
                } else if self.open_in_system_browser(&url, window, cx) {
                    anyhow::bail!(
                        "browser.engine is \"system\": {url} opened in the default browser, \
                         and no tab was made"
                    )
                } else {
                    self.make_browser(&url, window, cx)
                }
            }
            "markdown" => {
                let path =
                    str_param(p, "path").context("missing path (the Markdown file to show)")?;
                let path = std::path::Path::new(path);
                anyhow::ensure!(
                    path.is_absolute(),
                    "the path must be absolute: {}",
                    path.display()
                );
                anyhow::ensure!(path.exists(), "no such file: {}", path.display());
                let url = crate::markdown::view_url_for(path);
                if self.open_in_system_browser(&url, window, cx) {
                    anyhow::bail!(
                        "browser.engine is \"system\": {} opened in the default browser, \
                         and no tab was made",
                        path.display()
                    )
                }
                self.make_browser(&url, window, cx)
            }
            "claude" => {
                let cwd = str_param(p, "cwd")
                    .map(String::from)
                    .or_else(|| self.workspaces[wi].cwd(cx))
                    .map(PathBuf::from)
                    .unwrap_or_else(abromux_core::paths::home);
                anyhow::ensure!(
                    cwd.is_absolute(),
                    "the cwd must be absolute: {}",
                    cwd.display()
                );
                use crate::claude_chat::{self as chat, Start};
                let mut start = Start {
                    session: str_param(p, "session")
                        .or(str_param(p, "resume"))
                        .map(String::from),
                    ..Default::default()
                };
                if let Some(m) = str_param(p, "model") {
                    anyhow::ensure!(chat::is_model_name(m), "not a model name: {m:?}");
                    start.model = Some(m.into());
                }
                if let Some(e) = str_param(p, "effort") {
                    let e = chat::effort_name(e).with_context(|| {
                        format!("unknown effort {e:?} (low, medium, high, xhigh, max)")
                    })?;
                    start.effort = Some(e.into());
                }
                if let Some(m) = str_param(p, "mode") {
                    let m = chat::mode_name(m).with_context(|| {
                        format!(
                            "unknown mode {m:?} (ask, accept-edits, plan, auto, dont-ask, bypass)"
                        )
                    })?;
                    start.mode = Some(m.into());
                }
                if start.session.is_none() && bool_param(p, "continue", false) {
                    let config = crate::settings::claude_config_dir(cx);
                    start.session = chat::Chat::latest_session(&cwd, config.as_deref());
                    anyhow::ensure!(
                        start.session.is_some(),
                        "no earlier conversation in {}",
                        cwd.display()
                    );
                }
                let s = self.make_browser(&chat::url_for(&cwd, &start), window, cx);
                if let (Some(text), Surface::Browser(b)) = (str_param(p, "prompt"), &s)
                    && !text.trim().is_empty()
                {
                    b.update(cx, |b, cx| b.chat_send(text, cx));
                }
                s
            }
            t if crate::plugins::find(t).is_some() => {
                // GitLab turned off: its tab still shows local diffs.
                let local = str_param(p, "host").is_some_and(abromux_gitlab::local::is_local);
                anyhow::ensure!(
                    crate::plugins::enabled(t, cx) || local,
                    "GitLab is turned off (\"gitlab\": {{ \"enabled\": false }} in the config file)"
                );
                self.make_plugin(t, p, window, cx)?
            }
            t => {
                let mut kinds = vec!["terminal", "browser", "markdown", "claude"];
                kinds.extend(crate::plugins::all().iter().map(|p| p.id));
                bail!("unknown surface type {t:?} ({})", kinds.join(", "))
            }
        })
    }

    fn split_with(
        &mut self,
        wi: usize,
        pane: usize,
        dir: &str,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        let (axis, before) = direction(dir)?;
        let s = self.make_surface(wi, p, window, cx)?;
        let surface = self.describe_surface(&s, true, cx);
        let new = self
            .split_pane(
                wi,
                pane,
                axis,
                before,
                s,
                bool_param(p, "focus", true),
                window,
                cx,
            )
            .context("pane not found")?;
        Ok(
            json!({ "workspace": self.ws_ref(wi), "pane": format!("pane:{new}"), "surface": surface }),
        )
    }

    // ---- descriptions ---------------------------------------------------

    fn describe_surface(&self, s: &Surface, active: bool, cx: &App) -> Value {
        let mut v = json!({ "ref": self.sref(s), "title": s.title(cx), "active": active });
        if let Some(label) = s.custom_title(cx) {
            v["label"] = json!(label);
        }
        match s {
            Surface::Terminal(t) => {
                let t = t.read(cx);
                v["type"] = json!("terminal");
                v["cwd"] = json!(t.cwd);
                if t.commands.marked {
                    v["command"] = json!({
                        "running": t.commands.running,
                        "finished": t.commands.finished,
                        "last_exit_code": t.commands.last_exit,
                    });
                }
                // Claude Code runs here (its hooks say, or its process is
                // among the shell's): a phone may show it as a chat
                // (`surface.stream` with `claude`).
                if t.has_claude()
                    || t.shell_pid().is_some_and(|pid| {
                        let config = crate::settings::claude_config_dir(cx);
                        crate::claude_chat::running_under(pid, config.as_deref()).is_some()
                    })
                {
                    v["claude_session"] = json!(true);
                }
                let status = crate::events::program_status_json(t.program.shown().as_ref());
                if !status.is_null() {
                    v["program_status"] = status;
                }
            }
            Surface::Browser(b) => {
                let b = b.read(cx);
                v["type"] = json!("browser");
                v["url"] = json!(b.url);
                if let Some(chat) = &b.chat {
                    v["claude"] = json!({ "state": chat.state_name(), "cwd": chat.cwd });
                }
            }
            Surface::Plugin(t) => {
                v["type"] = json!(t.plugin());
                v["state"] = t.state(cx);
            }
        }
        v
    }

    fn panes_json(&self, ws: &Workspace, cx: &App) -> Vec<Value> {
        ws.panes()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "ref": format!("pane:{}", p.id),
                    "index": i,
                    "focused": p.id == ws.focused_pane,
                    "surfaces": p.tabs.iter().enumerate().map(|(t, s)| self.describe_surface(s, t == p.active, cx)).collect::<Vec<_>>(),
                })
            })
            .collect()
    }

    fn find_group(&self, s: &str) -> anyhow::Result<usize> {
        match parse_sel(s, "group")? {
            Sel::Ref(id) => self.groups.iter().any(|g| g.id == id).then_some(id),
            Sel::Index(i) => self.groups.get(i).map(|g| g.id),
            Sel::Label(l) => {
                let found = self
                    .groups
                    .iter()
                    .filter(|g| g.name.eq_ignore_ascii_case(&l))
                    .map(|g| (g.id, format!("group:{}", g.id), g.name.clone()))
                    .collect();
                return one_labelled("group", &l, found);
            }
        }
        .ok_or_else(|| TargetError(format!("no group {s}")).into())
    }

    fn describe_group(&self, id: usize) -> Value {
        let Some(g) = self.groups.iter().find(|g| g.id == id) else {
            return json!(null);
        };
        json!({
            "ref": format!("group:{id}"),
            "name": g.name,
            "color": g.color.map(|c| format!("#{c:06x}")),
            "collapsed": g.collapsed,
            "workspaces": self.workspaces.iter().filter(|w| w.group == Some(id)).map(|w| format!("workspace:{}", w.id)).collect::<Vec<_>>(),
        })
    }

    /// `workspace-action`: cmux's workspace context-menu actions.
    fn workspace_action(
        &mut self,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Reply> {
        let wi = self.workspace_target(p)?;
        let id = self.workspaces[wi].id;
        let action = str_param(p, "action").context("missing --action")?;
        let ws = &mut self.workspaces[wi];
        match action {
            "rename" => {
                ws.custom_name = Some(one_line(
                    str_param(p, "title").context("rename needs --title")?,
                ))
            }
            // Back to following the terminal's title; auto-naming may name it again.
            "clear-name" => {
                ws.custom_name = None;
                ws.auto_title = None;
                ws.name_surface = None;
                self.naming.remove(&id);
            }
            "set-description" => {
                ws.description = crate::sidebar::clean_description(
                    str_param(p, "description").context("set-description needs --description")?,
                )
            }
            "clear-description" => ws.description = None,
            "set-color" => {
                let c = str_param(p, "color").context("set-color needs --color")?;
                let colors = &cx.global::<crate::settings::Settings>().workspace_colors;
                ws.color = Some(colors.parse(c).with_context(|| {
                    let names: Vec<&str> = colors.colors.iter().map(|(n, _)| n.as_str()).collect();
                    format!(
                        "unknown color {c:?}; use #rrggbb or one of {}",
                        names.join(", ")
                    )
                })?);
            }
            "clear-color" => ws.color = None,
            "move-up" => self.nudge_workspace(wi, false, cx),
            "move-down" => self.nudge_workspace(wi, true, cx),
            "move-top" => self.workspace_to_top(wi, cx),
            "move-bottom" => self.workspace_to_bottom(wi, cx),
            "move-to-cleanup" => {
                self.move_to_cleanup(wi, cx);
            }
            "close-others" => self.close_others(wi, window, cx),
            "close-above" | "close-below" => {
                let ids: Vec<usize> = match action {
                    "close-above" => self.workspaces[..wi].iter().map(|w| w.id).collect(),
                    _ => self.workspaces[wi + 1..].iter().map(|w| w.id).collect(),
                };
                for id in ids {
                    if let Some(i) = self.ws_index(id) {
                        self.close_workspace(i, window, cx);
                    }
                }
            }
            "mark-read" => {
                let ws = &self.workspaces[wi];
                let ids: Vec<EntityId> = ws
                    .panes()
                    .flat_map(|p| p.tabs.iter().map(|t| t.entity_id()))
                    .collect();
                for id in ids {
                    self.mark_surface_read(id);
                }
            }
            "mark-unread" => {
                let id = self.workspaces[wi]
                    .focused_surface()
                    .map(|s| s.entity_id())
                    .context("workspace has no tabs")?;
                self.mark_surface_unread(id);
            }
            "pin" | "unpin" => bail!("abromux has no {action:?} yet"),
            a => bail!(
                "unknown action {a:?}; expected rename, clear-name, set-description, clear-description, \
                 set-color, clear-color, move-up, move-down, move-top, move-bottom, move-to-cleanup, close-others, \
                 close-above, close-below, mark-read or mark-unread"
            ),
        }
        cx.notify();
        let v = match self.ws_index(id) {
            Some(i) => self.describe_ws(i, false, cx),
            None => json!(null),
        };
        Ok(Reply::Now(v))
    }

    fn describe_ws(&self, wi: usize, panes: bool, cx: &App) -> Value {
        let ws = &self.workspaces[wi];
        let cwd = ws.cwd(cx);
        let mut v = json!({
            "ref": self.ws_ref(wi),
            "index": wi,
            "name": ws.name(cx),
            "selected": wi == self.active,
            "unread": self.ws_unread(ws),
            "cwd": cwd,
            "branch": cwd.as_deref().and_then(git_branch),
            "pane_count": ws.panes().count(),
            "description": ws.description,
            "color": ws.color.map(|c| format!("#{c:06x}")),
            "group": ws.group.map(|g| format!("group:{g}")),
            "home": ws.home.as_ref().map(|h| h.to_string_lossy()),
            "status": ws.status.iter().map(|s| {
                let mut v = status::status_json(s);
                // An agent's pill: the tab it's about.
                if let Some(n) = s.owner.and_then(|o| self.surface_refs.get(&o)) {
                    v["surface"] = json!(format!("surface:{n}"));
                }
                v
            }).collect::<Vec<_>>(),
            "progress": ws.progress.as_ref().map(|p| json!({ "value": (p.value as f64 * 1000.).round() / 1000., "label": p.label })),
        });
        if panes {
            v["panes"] = json!(self.panes_json(ws, cx));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangup_waits_for_the_caller_to_close() {
        let (app_end, caller) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::unbounded();
        let served = std::thread::spawn(move || serve(app_end, tx));
        let mut w = caller.try_clone().unwrap();
        w.write_all(b"{\"id\":1,\"method\":\"system.ping\"}\n")
            .unwrap();
        let (_, reply, gone) = futures::executor::block_on(rx.next()).unwrap();
        reply.send(ipc::ok(&json!(1), json!("pong"))).unwrap();
        let mut line = String::new();
        BufReader::new(&caller).read_line(&mut line).unwrap();
        assert!(line.contains("pong"));
        // Answered, but the caller is still connected.
        assert!(gone.peek().is_none());
        drop(w);
        drop(caller);
        served.join().unwrap();
        assert!(
            futures::executor::block_on(gone).is_err(),
            "resolves (sender dropped)"
        );
    }

    #[test]
    fn zoom_and_notification_ids() {
        assert_eq!(parse_zoom("1.25").unwrap(), 1.25);
        assert_eq!(parse_zoom("150%").unwrap(), 1.5);
        assert!(parse_zoom("big").is_err());
        assert_eq!(
            notification_id(&json!({ "id": "notification:4" })).unwrap(),
            Some(4)
        );
        assert_eq!(notification_id(&json!({ "id": 7 })).unwrap(), Some(7));
        assert_eq!(notification_id(&json!({})).unwrap(), None);
        assert!(notification_id(&json!({ "id": "x" })).is_err());
    }

    #[test]
    fn unescape_maps_newline_to_enter() {
        assert_eq!(unescape(r"ls -la\n"), "ls -la\r");
        assert_eq!(unescape(r"a\tb\\c\e[1m\q"), "a\tb\\c\x1b[1m\\q");
    }

    #[test]
    fn selectors() {
        assert!(matches!(
            parse_sel("workspace:3", "workspace"),
            Ok(Sel::Ref(3))
        ));
        assert!(matches!(parse_sel("2", "pane"), Ok(Sel::Index(2))));
        assert!(parse_sel("pane:x", "pane").is_err());
        assert!(parse_sel("surface:1", "pane").is_err());
    }
}
