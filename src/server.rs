//! The control socket: a listener thread hands each request to the main
//! thread, which runs it against the app state (see ipc.rs for the wire
//! format and cli.rs for the commands that drive it).
//!
//! Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or plain 0-based
//! indexes. Without an explicit target a command acts on the caller (the
//! terminal `abromux` runs in, from `ABROMUX_SURFACE_ID`), else on what's focused.

use crate::{
    AbroMux, BrowserView, DEFAULT_URL,
    agent::{Agent, Kind},
    browser::normalize_url,
    browser_auto, debug, ipc, one_line,
    status::{self, RUNNING, WARNING},
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
use gpui::{
    App, AppContext as _, AsyncApp, Axis, Context, Entity, EntityId, Keystroke, Timer, Window,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::{Duration, Instant},
};

type Request = (Value, oneshot::Sender<Value>);

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

/// Run `js` in the page and wait up to `limit` for its JSON. None if the
/// page didn't run it (still loading, or navigating away).
async fn eval_json(
    b: &Entity<BrowserView>,
    js: &str,
    cx: &AsyncApp,
    limit: Duration,
) -> Option<String> {
    let rx = b.read_with(cx, |b, cx| b.eval_async(js, cx)).ok()?;
    match select(rx, Timer::after(limit)).await {
        Either::Left((Ok(out), _)) => Some(out),
        _ => None,
    }
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
        while let Some((req, reply)) = rx.next().await {
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let method = req["method"].as_str().unwrap_or_default().to_string();
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let Ok(out) = this.update_in(cx, |this, window, cx| {
                this.handle(&method, &params, window, cx)
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
                            Err(e) => ipc::err(&id, format!("{e:#}")),
                        };
                        let _ = reply.send(resp);
                    })
                    .detach(),
                Err(e) => {
                    let _ = reply.send(ipc::err(&id, format!("{e:#}")));
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
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                if tx.unbounded_send((req, reply_tx)).is_err() {
                    return;
                }
                futures::executor::block_on(reply_rx)
                    .unwrap_or_else(|_| ipc::err(&Value::Null, "app closed"))
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

fn bool_param(p: &Value, key: &str, default: bool) -> bool {
    match p.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.as_str(), "false" | "0" | "no"),
        _ => default,
    }
}

/// `kind:N` → N as a ref, a bare number → an index.
enum Sel {
    Ref(usize),
    Index(usize),
}

fn parse_sel(s: &str, kind: &str) -> anyhow::Result<Sel> {
    if let Some(n) = s.strip_prefix(kind).and_then(|r| r.strip_prefix(':')) {
        return Ok(Sel::Ref(
            n.parse().with_context(|| format!("bad {kind} ref {s:?}"))?,
        ));
    }
    s.parse()
        .map(Sel::Index)
        .map_err(|_| anyhow!("expected {kind}:N or an index, got {s:?}"))
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
    fn handle(
        &mut self,
        method: &str,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Reply> {
        let v = match method {
            "system.ping" => json!("pong"),
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
                    .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default().into());
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
                let cwd = str_param(p, "cwd").map(String::from);
                let group = str_param(p, "group")
                    .map(|g| self.find_group(g))
                    .transpose()?;
                let prev = self.workspaces.get(self.active).map(|w| w.id);
                if let Some((start, _)) = group.and_then(|g| group_span(&self.workspaces, g)) {
                    // With one of its workspaces selected, the new one lands in the group.
                    self.active = start;
                }
                let focus = bool_param(p, "focus", true);
                let wi = self.create_workspace(cwd, focus, window, cx);
                if !focus {
                    self.active = prev.and_then(|id| self.ws_index(id)).unwrap_or(0);
                }
                if let Some(name) = str_param(p, "name") {
                    self.workspaces[wi].custom_name = Some(name.into());
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
            // `abromux folder remove`, which has checked for lost work: if a
            // workspace has that folder, close it, then delete the folder.
            "folder.remove" => {
                let path = std::path::PathBuf::from(str_param(p, "path").context("missing path")?);
                let same = |h: &std::path::Path| {
                    h == path || h.canonicalize().is_ok_and(|h| path.canonicalize().is_ok_and(|p| h == p))
                };
                match self.workspaces.iter().position(|w| w.home.as_deref().is_some_and(same)) {
                    Some(wi) => {
                        let r = self.ws_ref(wi);
                        let id = self.workspaces[wi].id;
                        self.remove_folder_later(id, path, bool_param(p, "force", false), window, cx);
                        json!({ "workspace": r })
                    }
                    None => json!({ "workspace": null }),
                }
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
                let pane = ws
                    .root
                    .panes()
                    .into_iter()
                    .find(|x| x.id == pane)
                    .context("no such pane")?;
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
                let (_, _, s) = self.surface_target(p)?;
                let r = self.sref(&s);
                self.close_surface(s.entity_id(), window, cx);
                json!({ "closed": r })
            }
            "surface.focus" => {
                let (wi, _, s) = self.surface_target(p)?;
                self.focus_surface(s.entity_id(), cx);
                self.select_workspace(wi, window, cx);
                self.describe_surface(&s, true, cx)
            }
            "surface.send_text" => {
                let t = self.terminal_target(p)?;
                let text = unescape(
                    p.get("text")
                        .and_then(Value::as_str)
                        .context("missing text")?,
                );
                t.update(cx, |t, cx| {
                    t.send(text.as_bytes());
                    cx.notify();
                });
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
                        let pane = self.workspaces[dwi]
                            .root
                            .panes()
                            .into_iter()
                            .find(|x| x.id == dpane);
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

            // From `abromux hooks claude-event` (agent.rs).
            "agent.report" => {
                let t = self.terminal_target(p)?;
                let ended = str_param(p, "event") == Some("SessionEnd");
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
                            .await
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
            | "browser.find" => {
                let b = self.browser_target(p)?;
                let js = browser_auto::script(method.trim_start_matches("browser."), p);
                let acx = cx.to_async();
                return Ok(Reply::Later(
                    async move {
                        let out = eval_json(&b, &js, &acx, SCRIPT_TIMEOUT)
                            .await
                            .context("the page didn't run the script (is it still loading?)")?;
                        browser_auto::parse(&out)
                    }
                    .boxed_local(),
                ));
            }
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
                let id = self.add_notification(s.entity_id(), title, body, false, window, cx);
                json!({ "id": id, "surface": self.sref(&s) })
            }
            "notification.list" => {
                let now = std::time::SystemTime::now();
                json!(self
                    .notifications
                    .iter()
                    .rev()
                    .map(|n| {
                        let ws = self.workspaces.iter().position(|w| w.contains(n.surface));
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
                let id = match p.get("id") {
                    Some(Value::String(s)) => s.trim_start_matches("notification:").parse().ok(),
                    Some(v) => v.as_u64().map(|n| n as usize),
                    None => None,
                }
                .context("missing notification id")?;
                if !self.open_notification(id, window, cx) {
                    bail!("no notification {id}");
                }
                json!({ "id": id })
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
                        parse_color(c).with_context(|| format!("bad color {c:?} (use #rrggbb)"))
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
            "window.focus" => {
                cx.activate(true);
                window.activate_window();
                json!({ "window": "window:1" })
            }
            // For scripts/perf.py: frame timing and synthetic input.
            "debug.frame" => {
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
            _ => bail!("unknown method {method:?}"),
        };
        Ok(Reply::Now(v))
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
                    if let Some(out) = eval_json(&b, &script, &acx, left).await
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
        let (label, title) = (kind.label(), kind.title());
        if let Some(prompt) = str_param(p, "prompt") {
            self.auto_name(surface, kind, prompt.to_string(), cx);
        }
        match str_param(p, "event").unwrap_or_default() {
            "UserPromptSubmit" | "PostToolUse" => {
                self.set_agent_status(surface, label, Some(("Running", RUNNING)), cx)
            }
            // Claude's Notification, Codex's PermissionRequest.
            event @ ("Notification" | "PermissionRequest") => {
                let message = str_param(p, "message")
                    .map(String::from)
                    .unwrap_or_else(|| format!("{label} needs your attention"));
                // Claude also says so after a minute idle; Stop already did.
                if event == "Notification"
                    && (str_param(p, "notification_type") == Some("idle_prompt")
                        || message.contains("waiting for your input"))
                {
                    return;
                }
                self.set_agent_status(surface, label, Some(("Needs input", WARNING)), cx);
                self.add_notification(surface, title.into(), message, false, window, cx);
            }
            "Stop" => {
                self.set_agent_status(surface, label, None, cx);
                let body = str_param(p, "summary").unwrap_or("Finished").to_string();
                self.add_notification(surface, title.into(), body, false, window, cx);
            }
            "SessionEnd" => self.set_agent_status(surface, label, None, cx),
            _ => {}
        }
    }

    /// `automation.workspaceAutoNaming`: name the workspace of `surface`
    /// from the first prompt its agent gets, unless it has a name already.
    fn auto_name(
        &mut self,
        surface: gpui::EntityId,
        kind: Kind,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        if !cx
            .global::<crate::settings::Settings>()
            .automation
            .workspace_auto_naming
        {
            return;
        }
        let Some(ws) = self.workspaces.iter().find(|w| w.contains(surface)) else {
            return;
        };
        let id = ws.id;
        if ws.custom_name.is_some() || ws.auto_title.is_some() || self.naming.contains(&id) {
            return;
        }
        self.naming.push(id);
        let task = cx.background_spawn(async move { crate::agent::title_for(kind, &prompt) });
        cx.spawn(async move |this, cx| {
            let title = task.await;
            // `naming` keeps the id either way: one try per workspace, so a
            // CLI that fails doesn't run again on every prompt.
            this.update(cx, |this, cx| {
                if let (Some(t), Some(i)) = (title, this.ws_index(id))
                    && this.workspaces[i].custom_name.is_none()
                {
                    this.workspaces[i].auto_title = Some(one_line(&t));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- targets --------------------------------------------------------

    fn ws_ref(&self, wi: usize) -> String {
        format!("workspace:{}", self.workspaces[wi].id)
    }

    fn sref(&self, s: &Surface) -> String {
        format!(
            "surface:{}",
            self.surface_refs.get(&s.entity_id()).copied().unwrap_or(0)
        )
    }

    fn find_workspace(&self, s: &str) -> anyhow::Result<usize> {
        match parse_sel(s, "workspace")? {
            Sel::Ref(id) => self.workspaces.iter().position(|w| w.id == id),
            Sel::Index(i) => (i < self.workspaces.len()).then_some(i),
        }
        .with_context(|| format!("no workspace {s}"))
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
    fn workspace_target(&self, p: &Value) -> anyhow::Result<usize> {
        if let Some(w) = str_param(p, "workspace") {
            return self.find_workspace(w);
        }
        Ok(self.caller(p).map(|(wi, _, _)| wi).unwrap_or(self.active))
    }

    /// A surface ref, or an index into `within`'s surfaces in tree order.
    fn find_surface(
        &self,
        s: &str,
        within: Option<usize>,
    ) -> anyhow::Result<(usize, usize, Surface)> {
        let all = |wi: usize| {
            let ws = &self.workspaces[wi];
            ws.root
                .panes()
                .into_iter()
                .flat_map(|p| p.tabs.iter().map(move |t| (wi, p.id, t.clone())))
                .collect::<Vec<_>>()
        };
        match parse_sel(s, "surface")? {
            Sel::Ref(n) => (0..self.workspaces.len())
                .flat_map(all)
                .find(|(_, _, t)| self.surface_refs.get(&t.entity_id()) == Some(&n)),
            Sel::Index(i) => all(within.unwrap_or(self.active)).into_iter().nth(i),
        }
        .with_context(|| format!("no surface {s}"))
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
        }
    }

    /// `--surface`, else the workspace's focused surface if it's a browser,
    /// else its first browser.
    fn browser_target(&self, p: &Value) -> anyhow::Result<Entity<BrowserView>> {
        if str_param(p, "surface").is_some() {
            return match self.surface_target(p)?.2 {
                Surface::Browser(b) => Ok(b),
                Surface::Terminal(_) => bail!("that surface is a terminal, not a browser"),
            };
        }
        let ws = &self.workspaces[self.workspace_target(p)?];
        let browser = |s: &Surface| match s {
            Surface::Browser(b) => Some(b.clone()),
            _ => None,
        };
        ws.focused_surface()
            .and_then(browser)
            .or_else(|| {
                ws.root
                    .panes()
                    .iter()
                    .flat_map(|p| p.tabs.iter())
                    .find_map(browser)
            })
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
            Sel::Ref(id) => self.workspaces.iter().enumerate().find_map(|(wi, w)| {
                w.root
                    .panes()
                    .iter()
                    .any(|p| p.id == id)
                    .then_some((wi, id))
            }),
            Sel::Index(i) => {
                let wi = self.workspace_target(p)?;
                self.workspaces[wi].root.panes().get(i).map(|p| (wi, p.id))
            }
        }
        .with_context(|| format!("no pane {s}"))
    }

    /// `--surface`, else `--pane`'s active tab, else the usual target.
    fn source_surface(&self, p: &Value) -> anyhow::Result<(usize, usize, Surface)> {
        if str_param(p, "surface").is_none() && str_param(p, "pane").is_some() {
            let (wi, pane) = self.pane_target(p)?;
            let s = self.workspaces[wi]
                .root
                .panes()
                .into_iter()
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

    /// A terminal (`--cwd`, `--command`) or browser (`--url`) for workspace `wi`.
    fn make_surface(
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
                let s = self.make_terminal(cwd, self.workspaces[wi].id, window, cx);
                if let (Some(cmd), Surface::Terminal(t)) = (str_param(p, "command"), &s) {
                    t.update(cx, |t, _| t.send(format!("{cmd}\r").as_bytes()));
                }
                s
            }
            "browser" => {
                let url = str_param(p, "url")
                    .map(normalize_url)
                    .unwrap_or_else(|| DEFAULT_URL.into());
                if bool_param(p, "builtIn", false) {
                    self.make_builtin_browser(&url, window, cx)
                } else {
                    self.make_browser(&url, window, cx)
                }
            }
            t => bail!("unknown surface type {t:?} (terminal or browser)"),
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
        match s {
            Surface::Terminal(t) => {
                v["type"] = json!("terminal");
                v["cwd"] = json!(t.read(cx).cwd);
            }
            Surface::Browser(b) => {
                v["type"] = json!("browser");
                v["url"] = json!(b.read(cx).url);
            }
        }
        v
    }

    fn panes_json(&self, ws: &Workspace, cx: &App) -> Vec<Value> {
        ws.root
            .panes()
            .iter()
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
        }
        .with_context(|| format!("no group {s}"))
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
                self.naming.retain(|&n| n != id);
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
                for n in self.notifications.iter_mut() {
                    if ws.contains(n.surface) {
                        n.read = true;
                    }
                }
            }
            "pin" | "unpin" | "mark-unread" => bail!("abromux has no {action:?} yet"),
            a => bail!(
                "unknown action {a:?}; expected rename, clear-name, set-description, clear-description, \
                 set-color, clear-color, move-up, move-down, move-top, close-others, close-above, \
                 close-below or mark-read"
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
            "pane_count": ws.root.panes().len(),
            "description": ws.description,
            "color": ws.color.map(|c| format!("#{c:06x}")),
            "group": ws.group.map(|g| format!("group:{g}")),
            "home": ws.home.as_ref().map(|h| h.to_string_lossy()),
            "status": ws.status.iter().map(status::status_json).collect::<Vec<_>>(),
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
