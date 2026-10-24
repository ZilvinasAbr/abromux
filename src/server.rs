//! The control socket: a listener thread hands each request to the main
//! thread, which runs it against the app state (see ipc.rs for the wire
//! format and cli.rs for the commands that drive it).
//!
//! Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or plain 0-based
//! indexes. Without an explicit target a command acts on the caller (the
//! terminal `abromux` runs in, from `ABROMUX_SURFACE_ID`), else on what's focused.

use crate::{
    AbroMux, DEFAULT_URL,
    browser::normalize_url,
    debug, ipc,
    workspace::{Surface, Workspace, git_branch},
};
use anyhow::{Context as _, anyhow, bail};
use futures::{
    StreamExt,
    channel::{mpsc, oneshot},
};
use gpui::{App, Axis, Context, Keystroke, Window};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::Mutex,
};

type Request = (Value, oneshot::Sender<Value>);

enum Reply {
    Now(Value),
    /// Answered later, off the main thread (e.g. a webview script result).
    Later(oneshot::Receiver<anyhow::Result<Value>>),
}

/// Bind the socket and start serving. Returns the path, or None if another
/// abromux already owns it.
pub fn start(window: &mut Window, cx: &mut Context<AbroMux>) -> Option<PathBuf> {
    let path = ipc::socket_path();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            eprintln!("abromux: another instance is listening on {}; control socket disabled", path.display());
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
            let Ok(out) = this.update_in(cx, |this, window, cx| this.handle(&method, &params, window, cx)) else { break };
            let resp = match out {
                Ok(Reply::Now(v)) => ipc::ok(&id, v),
                Ok(Reply::Later(rx)) => match rx.await {
                    Ok(Ok(v)) => ipc::ok(&id, v),
                    Ok(Err(e)) => ipc::err(&id, e),
                    Err(_) => ipc::err(&id, "request dropped"),
                },
                Err(e) => ipc::err(&id, format!("{e:#}")),
            };
            let _ = reply.send(resp);
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
    let Ok(mut writer) = stream.try_clone() else { return };
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
                futures::executor::block_on(reply_rx).unwrap_or_else(|_| ipc::err(&Value::Null, "app closed"))
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
        return Ok(Sel::Ref(n.parse().with_context(|| format!("bad {kind} ref {s:?}"))?));
    }
    s.parse().map(Sel::Index).map_err(|_| anyhow!("expected {kind}:N or an index, got {s:?}"))
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
    fn handle(&mut self, method: &str, p: &Value, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<Reply> {
        let v = match method {
            "system.ping" => json!("pong"),
            "system.identify" => {
                let caller = self.caller(p).map(|(wi, _, s)| json!({ "workspace": self.ws_ref(wi), "surface": self.sref(&s) }));
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

            "workspace.list" => json!((0..self.workspaces.len()).map(|i| self.describe_ws(i, false, cx)).collect::<Vec<_>>()),
            "workspace.current" => self.describe_ws(self.active, false, cx),
            "workspace.create" => {
                let cwd = str_param(p, "cwd").map(String::from);
                let wi = self.create_workspace(cwd, bool_param(p, "focus", true), window, cx);
                if let Some(name) = str_param(p, "name") {
                    self.workspaces[wi].custom_name = Some(name.into());
                }
                if let (Some(cmd), Some(Surface::Terminal(t))) = (str_param(p, "command"), self.workspaces[wi].focused_surface()) {
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
            "workspace.tree" => {
                if bool_param(p, "all", false) {
                    json!((0..self.workspaces.len()).map(|i| self.describe_ws(i, true, cx)).collect::<Vec<_>>())
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
            "pane.create" => {
                let wi = self.workspace_target(p)?;
                let pane = self.workspaces[wi].focused_pane;
                self.split_with(wi, pane, str_param(p, "direction").unwrap_or("right"), p, window, cx)?
            }

            "surface.list" => {
                let (wi, pane) = self.pane_target(p)?;
                let ws = &self.workspaces[wi];
                let pane = ws.root.panes().into_iter().find(|x| x.id == pane).context("no such pane")?;
                json!(pane.tabs.iter().enumerate().map(|(i, s)| self.describe_surface(s, i == pane.active, cx)).collect::<Vec<_>>())
            }
            "surface.split" => {
                let (wi, pane, _) = self.surface_target(p)?;
                self.split_with(wi, pane, str_param(p, "direction").context("missing direction")?, p, window, cx)?
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
                let text = unescape(p.get("text").and_then(Value::as_str).context("missing text")?);
                t.update(cx, |t, cx| {
                    t.send(text.as_bytes());
                    cx.notify();
                });
                json!({ "bytes": text.len() })
            }
            "surface.send_key" => {
                let t = self.terminal_target(p)?;
                let key = str_param(p, "key").context("missing key")?;
                let mut ks = Keystroke::parse(key).map_err(|e| anyhow!("bad key {key:?}: {e:?}"))?;
                if ks.key.chars().count() == 1 && ks.key_char.is_none() {
                    ks.key_char = Some(if ks.modifiers.shift { ks.key.to_uppercase() } else { ks.key.clone() });
                }
                if !t.update(cx, |t, cx| t.send_keystroke(&ks, cx)) {
                    bail!("key {key:?} doesn't map to terminal input");
                }
                json!({ "key": key })
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

            "browser.open" => {
                let (wi, pane, _) = self.surface_target(p)?;
                let mut p = p.clone();
                p["type"] = json!("browser");
                self.split_with(wi, pane, str_param(&p, "direction").unwrap_or("right"), &p, window, cx)?
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
                let script = p.get("script").and_then(Value::as_str).context("missing script")?;
                let (tx, rx) = oneshot::channel();
                let tx = Mutex::new(Some(tx));
                b.read(cx).eval(script, cx, move |out| {
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(Ok(serde_json::from_str(&out).unwrap_or(Value::String(out))));
                    }
                })?;
                return Ok(Reply::Later(rx));
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
            "debug.frame" => return Ok(Reply::Later(debug::after_next_frame(window, cx))),
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
                        if let Err(e) = debug::run_step(&view, step, AbroMux::script_mouse, window, cx) {
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

    // ---- targets --------------------------------------------------------

    fn ws_ref(&self, wi: usize) -> String {
        format!("workspace:{}", self.workspaces[wi].id)
    }

    fn sref(&self, s: &Surface) -> String {
        format!("surface:{}", self.surface_refs.get(&s.entity_id()).copied().unwrap_or(0))
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
        p["caller"]["surface"].as_str().and_then(|s| self.find_surface(s, None).ok())
    }

    /// `--workspace`, else the caller's workspace, else the selected one.
    fn workspace_target(&self, p: &Value) -> anyhow::Result<usize> {
        if let Some(w) = str_param(p, "workspace") {
            return self.find_workspace(w);
        }
        Ok(self.caller(p).map(|(wi, _, _)| wi).unwrap_or(self.active))
    }

    /// A surface ref, or an index into `within`'s surfaces in tree order.
    fn find_surface(&self, s: &str, within: Option<usize>) -> anyhow::Result<(usize, usize, Surface)> {
        let all = |wi: usize| {
            let ws = &self.workspaces[wi];
            ws.root.panes().into_iter().flat_map(|p| p.tabs.iter().map(move |t| (wi, p.id, t.clone()))).collect::<Vec<_>>()
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
        let ws = str_param(p, "workspace").map(|w| self.find_workspace(w)).transpose()?;
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
        let s = w.focused_surface().context("workspace has no surfaces")?.clone();
        Ok((wi, w.focused_pane, s))
    }

    fn terminal_target(&self, p: &Value) -> anyhow::Result<gpui::Entity<crate::TerminalView>> {
        match self.surface_target(p)?.2 {
            Surface::Terminal(t) => Ok(t),
            Surface::Browser(_) => bail!("that surface is a browser, not a terminal"),
        }
    }

    /// `--surface`, else the workspace's focused surface if it's a browser,
    /// else its first browser.
    fn browser_target(&self, p: &Value) -> anyhow::Result<gpui::Entity<crate::BrowserView>> {
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
            .or_else(|| ws.root.panes().iter().flat_map(|p| p.tabs.iter()).find_map(browser))
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
                .find_map(|(wi, w)| w.root.panes().iter().any(|p| p.id == id).then_some((wi, id))),
            Sel::Index(i) => {
                let wi = self.workspace_target(p)?;
                self.workspaces[wi].root.panes().get(i).map(|p| (wi, p.id))
            }
        }
        .with_context(|| format!("no pane {s}"))
    }

    // ---- actions --------------------------------------------------------

    /// A terminal (`--cwd`, `--command`) or browser (`--url`) for workspace `wi`.
    fn make_surface(&mut self, wi: usize, p: &Value, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<Surface> {
        Ok(match str_param(p, "type").unwrap_or("terminal") {
            "terminal" => {
                let cwd = str_param(p, "cwd").map(String::from).or_else(|| self.workspaces[wi].cwd(cx));
                let s = self.make_terminal(cwd, self.workspaces[wi].id, window, cx);
                if let (Some(cmd), Surface::Terminal(t)) = (str_param(p, "command"), &s) {
                    t.update(cx, |t, _| t.send(format!("{cmd}\r").as_bytes()));
                }
                s
            }
            "browser" => {
                let url = str_param(p, "url").map(normalize_url).unwrap_or_else(|| DEFAULT_URL.into());
                self.make_browser(&url, window, cx)
            }
            t => bail!("unknown surface type {t:?} (terminal or browser)"),
        })
    }

    fn split_with(&mut self, wi: usize, pane: usize, dir: &str, p: &Value, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<Value> {
        let (axis, before) = direction(dir)?;
        let s = self.make_surface(wi, p, window, cx)?;
        let surface = self.describe_surface(&s, true, cx);
        let new = self.split_pane(wi, pane, axis, before, s, bool_param(p, "focus", true), window, cx).context("pane not found")?;
        Ok(json!({ "workspace": self.ws_ref(wi), "pane": format!("pane:{new}"), "surface": surface }))
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

    fn describe_ws(&self, wi: usize, panes: bool, cx: &App) -> Value {
        let ws = &self.workspaces[wi];
        let cwd = ws.cwd(cx);
        let mut v = json!({
            "ref": self.ws_ref(wi),
            "index": wi,
            "name": ws.name(cx),
            "selected": wi == self.active,
            "unread": ws.unread,
            "cwd": cwd,
            "branch": cwd.as_deref().and_then(git_branch),
            "pane_count": ws.root.panes().len(),
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
        assert!(matches!(parse_sel("workspace:3", "workspace"), Ok(Sel::Ref(3))));
        assert!(matches!(parse_sel("2", "pane"), Ok(Sel::Index(2))));
        assert!(parse_sel("pane:x", "pane").is_err());
        assert!(parse_sel("surface:1", "pane").is_err());
    }
}
