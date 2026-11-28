//! Session restore: on quit (and every few seconds, layout only) the app
//! writes its windows, workspaces, split tree, tabs, working directories,
//! browser URLs and terminal scrollback to `session.json`; the next launch
//! rebuilds them. Shells start fresh in the saved cwd; their old output is
//! replayed above the new prompt (best effort, like cmux), and a Claude Code
//! session that was running in one is resumed there.
//!
//! `$ABROMUX_SESSION_PATH` overrides the file
//! (default `~/Library/Application Support/abromux/session.json`, or
//! `abromux-nightly/` for the nightly app).
//! `ABROMUX_NO_RESTORE=1` turns it off, as do `ABROMUX_DEMO`/`ABROMUX_SCRIPT` unless
//! a session path is given, so test runs never touch the real session.

use crate::{
    AbroMux,
    workspace::{Group, Node, Surface, Workspace},
};
pub use abromux_core::session::*;
use gpui::{App, Axis, Bounds, Context, Pixels, Window, point, px, size};
use gpui_component::WindowExt;
use std::path::PathBuf;

/// Lines of scrollback kept per terminal.
const SCROLLBACK_LINES: usize = 2000;

pub fn path() -> Option<PathBuf> {
    if std::env::var("ABROMUX_NO_RESTORE").is_ok() {
        return None;
    }
    if let Ok(p) = std::env::var("ABROMUX_SESSION_PATH") {
        return Some(p.into());
    }
    if std::env::var("ABROMUX_DEMO").is_ok() || std::env::var("ABROMUX_SCRIPT").is_ok() {
        return None;
    }
    crate::paths::home_var()?;
    Some(crate::channel::support_dir().join("session.json"))
}

pub fn load() -> Option<Session> {
    let path = path()?;
    let local = if path.exists() {
        match read(&path) {
            Ok(s) if !s.workspaces.is_empty() => Some(s),
            Ok(_) => None,
            Err(e) => {
                eprintln!("abromux: ignoring unreadable session: {e}");
                None
            }
        }
    } else {
        None
    };
    // The server keeps the session the app last sent it (abromux_server::
    // control): the app starts from that snapshot when it's newer than its
    // own file, or the file is gone (docs/server-plan.md, phase 2).
    let local_at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    match server_session() {
        Some((s, at)) if local.is_none() || at > local_at + 2000 => {
            eprintln!(
                "abromux: starting from the server's session (newer than {})",
                path.display()
            );
            Some(s)
        }
        _ => local,
    }
}

/// The session this machine's server keeps, and when the app sent it: read
/// from its saved state, so it's there before the server is (a reboot).
fn server_session() -> Option<(Session, u64)> {
    let state = abromux_server::control::state_path(&abromux_term::hold::socket_path());
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(state).ok()?).ok()?;
    let at = v["session_at"].as_u64()?;
    let s: Session = serde_json::from_value(v["session"].clone()).ok()?;
    (s.version == VERSION && !s.workspaces.is_empty()).then_some((s, at))
}

/// A session file, or why it couldn't be read. cmux's session files are
/// read too, converted (`crate::cmux`).
pub fn read(path: &std::path::Path) -> Result<Session, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && crate::cmux::is_cmux(&v)
    {
        return crate::cmux::convert(&v).map_err(|e| format!("{}: {e}", path.display()));
    }
    match serde_json::from_str::<Session>(&text) {
        Ok(s) if s.version == VERSION => Ok(s),
        Ok(s) => Err(format!(
            "{}: session version {} isn't {VERSION}",
            path.display(),
            s.version
        )),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Where `restore-session --from` reads: a channel's session (`stable`,
/// `nightly`, `nightly-<name>`), cmux's (`cmux`), or a file.
pub fn source(from: &str) -> PathBuf {
    if let Some(p) = named_source(from) {
        return p;
    }
    crate::paths::expand_tilde(from)
}

/// The session a `--from` name stands for, None for a file path.
pub fn named_source(from: &str) -> Option<PathBuf> {
    if from == "cmux" {
        return Some(crate::cmux::session_path());
    }
    crate::channel::support_dir_named(from).map(|dir| dir.join("session.json"))
}

/// Write atomically (temp file + rename) so a crash mid-write can't leave a
/// truncated session behind.
pub fn save(session: &Session) {
    let Some(path) = path() else { return };
    let _ = write(session, &path);
}

/// Write `session` to `path` atomically.
pub fn write(session: &Session, path: &std::path::Path) -> std::io::Result<()> {
    let json = serde_json::to_string(session)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    // Only this user's: it has what resumes programs (OSC 7577 records)
    // and held shells' tokens.
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(json.as_bytes())?;
    }
    std::fs::rename(&tmp, path)
}

/// Where a saved window goes.
pub fn window_bounds(w: &WindowState) -> Bounds<Pixels> {
    Bounds::new(point(px(w.x), px(w.y)), size(px(w.width), px(w.height)))
}

impl AbroMux {
    /// The current state; `scrollback` adds every terminal's output, which
    /// is too costly for the periodic save.
    pub(crate) fn snapshot(&self, scrollback: bool, cx: &App) -> Session {
        let window = self.window_bounds.map(|b| WindowState {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        });
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| WorkspaceState {
                id: Some(ws.id),
                name: ws.custom_name.clone(),
                auto_name: ws.auto_title.clone(),
                name_surface: ws.name_surface,
                description: ws.description.clone(),
                color: ws.color,
                home: ws.home.as_ref().map(|h| h.to_string_lossy().into_owned()),
                env: ws.env.clone(),
                host: ws.host.clone(),
                forwards: self
                    .forwards
                    .get(&ws.id)
                    .into_iter()
                    .flatten()
                    .map(|f| abromux_core::session::ForwardState {
                        host: f.host.clone(),
                        port: f.port,
                        local: f.local,
                    })
                    .collect(),
                links: ws.links.clone(),
                hidden_links: ws.hidden_links.clone(),
                pinned: pin_states(ws, scrollback, &self.surface_refs, cx),
                group: ws
                    .group
                    .and_then(|g| self.groups.iter().position(|x| x.id == g)),
                focused: ws
                    .panes()
                    .position(|p| p.id == ws.focused_pane)
                    .unwrap_or(0),
                root: node_state(&ws.root, scrollback, &self.surface_refs, cx),
            })
            .collect();
        Session {
            version: VERSION,
            window,
            active: self.active,
            sidebar: self.sidebar,
            sidebar_width: self.sidebar_width,
            workspaces,
            groups: self
                .groups
                .iter()
                .map(|g| GroupState {
                    name: g.name.clone(),
                    color: g.color,
                    collapsed: g.collapsed,
                    cleanup: g.cleanup,
                })
                .collect(),
            placements: self.placements.list(),
        }
    }

    /// Rebuild the saved workspaces. Returns false (and builds nothing) if
    /// the session holds none.
    pub(crate) fn restore(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let first = self.workspaces.len();
        // Kept for ⌘⇧O without the terminals' output, which is the bulk
        // of it.
        let mut kept = session.clone();
        for w in &mut kept.workspaces {
            drop_scrollback(&mut w.root);
        }
        let group_ids = self.new_groups(&session.groups, |_| None);
        let ids = self.add_workspaces(session.workspaces, &group_ids, window, cx);
        let added = ids.iter().flatten().count();
        if added == 0 {
            return false;
        }
        self.launch_session = Some((kept, ids));
        self.sidebar = session.sidebar;
        self.sidebar_width = session
            .sidebar_width
            .clamp(crate::SIDEBAR_MIN, crate::SIDEBAR_MAX);
        self.select_workspace(first + session.active.min(added - 1), window, cx);
        true
    }

    /// ⌘⇧O and `restore-session`: add back the workspaces of the session
    /// this launch restored that have been closed since, or, given
    /// `session` (`--from`), those of it that aren't open as they were.
    /// Their groups are rejoined if still there. Returns how many it added.
    pub(crate) fn reopen_session(
        &mut self,
        session: Option<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let from_launch = session.is_none();
        let (saved, launch_ids) = match session {
            Some(s) => (s, None),
            None => match self.launch_session.clone() {
                Some((s, ids)) => (s, Some(ids)),
                None => {
                    window.push_notification("There's no previous session to reopen.", cx);
                    return 0;
                }
            },
        };
        let is_open =
            |this: &Self, id: Option<usize>| id.is_some_and(|id| this.ws_index(id).is_some());
        let wanted: Vec<usize> = match &launch_ids {
            // The launch's own workspaces are known by id, wherever their
            // shells have gone since.
            Some(ids) => (0..saved.workspaces.len())
                .filter(|&i| !is_open(self, ids.get(i).copied().flatten()))
                .collect(),
            // Another session's: by name and layout. Each open workspace
            // accounts for one saved one at most.
            None => {
                let mut open: Vec<String> = self
                    .workspaces
                    .iter()
                    .map(|w| {
                        workspace_key(
                            w.custom_name.as_deref(),
                            &node_state(&w.root, false, &self.surface_refs, cx),
                        )
                    })
                    .collect();
                (0..saved.workspaces.len())
                    .filter(|&i| {
                        let w = &saved.workspaces[i];
                        match open
                            .iter()
                            .position(|k| *k == workspace_key(w.name.as_deref(), &w.root))
                        {
                            Some(k) => {
                                open.swap_remove(k);
                                false
                            }
                            None => true,
                        }
                    })
                    .collect()
            }
        };
        // A saved group still on screen is joined again: the group of an
        // open workspace restored from it, or else one of the same name.
        let reuse: Vec<Option<usize>> = (0..saved.groups.len())
            .map(|g| {
                let from_member = launch_ids.as_ref().and_then(|ids| {
                    saved
                        .workspaces
                        .iter()
                        .zip(ids)
                        .filter(|(w, _)| w.group == Some(g))
                        .find_map(|(_, id)| id.and_then(|id| self.ws_index(id)))
                        .and_then(|i| self.workspaces[i].group)
                });
                from_member.or_else(|| {
                    self.groups
                        .iter()
                        .find(|x| x.name == saved.groups[g].name)
                        .map(|x| x.id)
                })
            })
            .collect();
        let group_ids = self.new_groups(&saved.groups, |g| reuse[g]);
        let states: Vec<WorkspaceState> = wanted
            .iter()
            .map(|&i| saved.workspaces[i].clone())
            .collect();
        let first = self.workspaces.len();
        let ids = self.add_workspaces(states, &group_ids, window, cx);
        if from_launch && let Some((_, launch)) = &mut self.launch_session {
            for (&i, id) in wanted.iter().zip(&ids) {
                launch[i] = *id;
            }
        }
        let added = ids.iter().flatten().count();
        let note = match added {
            0 => "Every workspace of that session is already open.".to_string(),
            1 => "Reopened 1 workspace.".to_string(),
            n => format!("Reopened {n} workspaces."),
        };
        window.push_notification(note, cx);
        if let Some(i) = ids.iter().flatten().find_map(|&id| self.ws_index(id)) {
            self.select_workspace(i, window, cx);
        } else if added > 0 {
            self.select_workspace(first.min(self.workspaces.len() - 1), window, cx);
        }
        cx.notify();
        added
    }

    /// File › Import cmux Session: add cmux's workspaces (see `crate::cmux`)
    /// that aren't open here already.
    pub(crate) fn import_cmux(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match read(&crate::cmux::session_path()) {
            Ok(session) => {
                self.reopen_session(Some(session), window, cx);
            }
            Err(e) => window.push_notification(format!("Couldn't import cmux's session: {e}"), cx),
        }
    }

    /// An id for each saved group: `reuse`'s, or a new group.
    fn new_groups(
        &mut self,
        groups: &[GroupState],
        reuse: impl Fn(usize) -> Option<usize>,
    ) -> Vec<usize> {
        groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                reuse(i).unwrap_or_else(|| {
                    self.next_group += 1;
                    self.groups.push(Group {
                        id: self.next_group,
                        name: g.name.clone(),
                        color: g.color,
                        collapsed: g.collapsed,
                        cleanup: g.cleanup,
                    });
                    self.next_group
                })
            })
            .collect()
    }

    /// Build the saved workspaces after the open ones, each saved group in
    /// the group `group_ids` gives it. Returns the id each one was built as,
    /// None for one that came back empty.
    pub(crate) fn add_workspaces(
        &mut self,
        states: Vec<WorkspaceState>,
        group_ids: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Option<usize>> {
        // Held shells keep their surface numbers: leave those free.
        for ws in &states {
            self.next_surface = self.next_surface.max(ws.root.max_surface());
            for p in &ws.pinned {
                self.next_surface = self.next_surface.max(p.max_surface());
            }
        }
        // Each comes back as the workspace it was, unless that number is
        // taken (a workspace open now, or another session's added to this one).
        let mut taken: std::collections::HashSet<usize> =
            self.workspaces.iter().map(|w| w.id).collect();
        let keep: Vec<Option<usize>> = states
            .iter()
            .map(|s| s.id.filter(|id| *id > 0 && taken.insert(*id)))
            .collect();
        self.next_ws = self
            .next_ws
            .max(keep.iter().flatten().copied().max().unwrap_or(0));
        let mut ids = vec![];
        for (state, keep) in states.into_iter().zip(keep) {
            let id = keep.unwrap_or_else(|| {
                self.next_ws += 1;
                self.next_ws
            });
            // Its new shells get the folder's env before it's in the list.
            let home = state.home.map(std::path::PathBuf::from);
            self.building_home = home.clone();
            self.building_env = state.env.clone();
            self.building_host = state.host.clone();
            let root = self.build_node(state.root, id, window, cx);
            self.building_home = None;
            self.building_env.clear();
            self.building_host = None;
            let Some(root) = root else {
                ids.push(None);
                continue;
            };
            let panes: Vec<usize> = root.panes().map(|p| p.id).collect();
            let focused_pane = panes
                .get(state.focused)
                .or(panes.first())
                .copied()
                .unwrap_or(0);
            let mut ws = Workspace::new(id, state.name, root, focused_pane);
            ws.auto_title = state.auto_name;
            ws.name_surface = state.name_surface;
            ws.description = state.description;
            ws.color = state.color;
            ws.home = home;
            ws.env = state.env;
            ws.host = state.host;
            // Its forwarded ports, at the same port here if it's free.
            for f in state.forwards {
                let started = crate::forward::Forward::start(&f.host, f.port, Some(f.local))
                    .or_else(|_| crate::forward::Forward::start(&f.host, f.port, None));
                if let Ok(fw) = started {
                    self.forwards.entry(id).or_default().push(fw);
                }
            }
            ws.links = state.links;
            ws.hidden_links = state.hidden_links;
            for pin in state.pinned {
                if let Some((pane, tab)) = pin.at {
                    let id = ws
                        .panes()
                        .nth(pane)
                        .and_then(|p| p.tabs.get(tab))
                        .map(|s| s.entity_id());
                    if let Some(id) = id {
                        ws.pin(id);
                        if let Some(l) = pin.link {
                            ws.pinned_for.insert(id, l);
                        }
                    }
                } else if let Some(tab) = pin.hidden {
                    let surface = self.build_tab(tab, id, window, cx);
                    if let Surface::Browser(b) = &surface {
                        b.update(cx, |b, cx| b.set_shown(false, cx));
                    }
                    let pane = pin
                        .pane
                        .and_then(|i| ws.panes().nth(i))
                        .map_or(ws.focused_pane, |p| p.id);
                    ws.pin(surface.entity_id());
                    if let Some(l) = pin.link {
                        ws.pinned_for.insert(surface.entity_id(), l);
                    }
                    ws.hidden.push(crate::workspace::Hidden {
                        surface,
                        pane,
                        index: pin.index,
                    });
                }
            }
            ws.group = state.group.and_then(|g| group_ids.get(g).copied());
            self.workspaces.push(ws);
            ids.push(Some(id));
        }
        crate::workspace::tidy_groups(&mut self.workspaces, &mut self.groups);
        ids
    }

    fn build_node(
        &mut self,
        state: NodeState,
        ws: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Node> {
        match state {
            NodeState::Pane { active, tabs } => {
                let tabs: Vec<Surface> = tabs
                    .into_iter()
                    .map(|t| self.build_tab(t, ws, window, cx))
                    .collect();
                self.leaf_node(tabs, active)
            }
            NodeState::Split {
                vertical,
                ratio,
                children,
            } => {
                let [a, b] = *children;
                let a = self.build_node(a, ws, window, cx);
                let b = self.build_node(b, ws, window, cx);
                let axis = if vertical {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                };
                self.split_node(axis, ratio, a, b)
            }
        }
    }

    fn build_tab(
        &mut self,
        state: TabState,
        ws: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Surface {
        let (s, name) = match state {
            TabState::Terminal {
                cwd,
                font_size,
                scrollback,
                agent,
                resume,
                name,
                held,
                host,
                surface,
                edit,
            } => {
                // A remote terminal's folder is on its host: the server
                // checks it there.
                let cwd = (host.is_some() || std::path::Path::new(&cwd).is_dir()).then_some(cwd);
                // An editor whose file is still there; else a shell, as the
                // editor exiting would have left nothing to come back to.
                let editor = edit
                    .map(std::path::PathBuf::from)
                    .filter(|f| f.is_file())
                    .and_then(|f| self.make_editor(ws, &f, cwd.clone(), window, cx).ok());
                if let Some(s) = editor {
                    if let (Some(size), Surface::Terminal(t)) = (font_size, &s) {
                        t.update(cx, |t, cx| t.restore(None, Some(size), None, None, cx));
                    }
                    if name.is_some() {
                        s.set_custom_title(name, cx);
                    }
                    return s;
                }
                let attach = surface.zip(held);
                // What a new shell runs: the command a program here reported
                // (OSC 7577), else the agent's. One program comes back in
                // the shell's foreground, so the nearest the shell wins: the
                // agent when the program ran under it, else the program (the
                // agent ran under it, or was a stopped job beside it).
                let agent_restart = agent.as_ref().and_then(|a| a.restart());
                let program = resume
                    .as_ref()
                    .and_then(|r| r.best())
                    .filter(|r| !(r.under_agent && agent_restart.is_some()))
                    .map(abromux_core::program_resume::Restart::from);
                let restart = program.clone().or(agent_restart);
                let opts = crate::TerminalOpts {
                    cwd,
                    attach,
                    host,
                    resume: restart.clone(),
                    ..Default::default()
                };
                let s = self.make_terminal(ws, opts, window, cx);
                if let Surface::Terminal(t) = &s {
                    t.update(cx, |t, cx| {
                        if t.attached {
                            // It ran on: its screen, agent and records are
                            // still there.
                            t.restore(None, font_size, None, None, cx);
                            t.agent = agent;
                            if let Some(r) = resume {
                                t.load_resume(r, cx);
                            }
                        } else {
                            let agent = agent.filter(|_| program.is_none());
                            t.restore(
                                scrollback.as_deref(),
                                font_size,
                                agent,
                                restart.as_ref(),
                                cx,
                            )
                        }
                    });
                }
                (s, name)
            }
            // terminal-browser can't take the built-in browser's history.
            TabState::Browser { url, name, .. }
                if crate::settings::terminal_browser(cx)
                    && !crate::markdown::is_markdown_url(&url)
                    && !crate::claude_chat::is_chat_url(&url) =>
            {
                (self.make_browser(&url, window, cx), name)
            }
            TabState::Browser {
                url,
                name,
                history,
                zoom,
            } => {
                let s = match history {
                    Some(h) => {
                        // A Markdown tab is one from the start (its scheme is
                        // set up then); others start blank under their history.
                        let first = if crate::markdown::is_markdown_url(&url) {
                            url.as_str()
                        } else {
                            ""
                        };
                        let s = self.make_browser(first, window, cx);
                        if let Surface::Browser(b) = &s {
                            b.update(cx, |b, cx| {
                                if b.restore_history(&h, cx) {
                                    b.url = url;
                                } else if !url.is_empty() {
                                    b.navigate(&url, cx);
                                }
                            });
                        }
                        s
                    }
                    None => self.make_browser(&url, window, cx),
                };
                if let (Some(zoom), Surface::Browser(b)) = (zoom, &s) {
                    b.update(cx, |b, cx| b.set_zoom(zoom, cx));
                }
                (s, name)
            }
            TabState::Plugin {
                plugin,
                state,
                name,
            } => match self.make_plugin(&plugin, &state, window, cx) {
                Ok(s) => (s, name),
                // Not built in any more, or it can't open what it saved: keep
                // the pane, with a terminal that says why.
                Err(e) => {
                    eprintln!("restoring a {plugin} tab: {e:#}");
                    let s = self.make_terminal(ws, crate::TerminalOpts::default(), window, cx);
                    (s, Some(format!("{plugin} (not restored)")))
                }
            },
        };
        if name.is_some() {
            s.set_custom_title(name, cx);
        }
        s
    }
}

type Refs = std::collections::HashMap<gpui::EntityId, usize>;

/// Workspace `ws`'s pinned tabs: where each is in the layout, or, for a
/// hidden one, the tab itself.
fn pin_states(
    ws: &crate::workspace::Workspace,
    scrollback: bool,
    refs: &Refs,
    cx: &App,
) -> Vec<abromux_core::session::PinState> {
    use abromux_core::session::PinState;
    let panes: Vec<&crate::workspace::Pane> = ws.panes().collect();
    ws.pinned
        .iter()
        .filter_map(|&id| {
            if let Some(h) = ws.hidden.iter().find(|h| h.surface.entity_id() == id) {
                return Some(PinState {
                    at: None,
                    hidden: Some(tab_state(&h.surface, scrollback, refs, cx)),
                    pane: panes.iter().position(|p| p.id == h.pane),
                    index: h.index.min(u32::MAX as usize),
                    link: ws.pinned_for.get(&id).cloned(),
                });
            }
            panes.iter().enumerate().find_map(|(pi, p)| {
                let ti = p.tabs.iter().position(|s| s.entity_id() == id)?;
                Some(PinState {
                    at: Some((pi, ti)),
                    hidden: None,
                    pane: None,
                    index: 0,
                    link: ws.pinned_for.get(&id).cloned(),
                })
            })
        })
        .collect()
}

fn node_state(node: &Node, scrollback: bool, refs: &Refs, cx: &App) -> NodeState {
    match node {
        Node::Leaf(p) => NodeState::Pane {
            active: p.active,
            tabs: p
                .tabs
                .iter()
                .map(|s| tab_state(s, scrollback, refs, cx))
                .collect(),
        },
        Node::Split {
            axis,
            ratio,
            children,
            ..
        } => NodeState::Split {
            vertical: *axis == Axis::Vertical,
            ratio: *ratio,
            children: Box::new([
                node_state(&children[0], scrollback, refs, cx),
                node_state(&children[1], scrollback, refs, cx),
            ]),
        },
    }
}

fn tab_state(s: &Surface, scrollback: bool, refs: &Refs, cx: &App) -> TabState {
    match s {
        Surface::Terminal(t) => {
            let surface = refs.get(&t.entity_id()).copied();
            let t = t.read(cx);
            // A tab whose app shows a page (terminal-browser) comes back as a
            // browser tab on that page, in whichever engine is set then.
            if let Some(url) = t
                .app
                .as_ref()
                .filter(|a| a.saves_as_page())
                .and_then(|a| a.page_url())
            {
                return TabState::Browser {
                    url: url.into(),
                    name: t.custom_title.clone(),
                    history: None,
                    zoom: None,
                };
            }
            let held = t.held_id();
            let edit = t
                .app
                .as_ref()
                .and_then(|a| a.edits())
                .map(|f| f.to_string_lossy().into_owned());
            TabState::Terminal {
                surface: surface.filter(|_| held.is_some()),
                held,
                host: t.host.clone(),
                cwd: t.live_cwd(),
                font_size: Some(t.font_size()),
                // An editor draws its screen again; its old one would only
                // show under it.
                scrollback: (scrollback && edit.is_none())
                    .then(|| t.scrollback_vt(SCROLLBACK_LINES))
                    .flatten(),
                edit,
                agent: t.agent.clone().filter(crate::agent::Agent::alive),
                resume: t.saved_resume(),
                name: t.custom_title.clone(),
            }
        }
        Surface::Browser(b) => TabState::Browser {
            url: b.read(cx).url.clone(),
            name: b.read(cx).custom_title.clone(),
            history: scrollback.then(|| b.read(cx).history_state(cx)).flatten(),
            zoom: Some(b.read(cx).zoom()).filter(|&z| z != 1.0),
        },
        Surface::Plugin(p) => TabState::Plugin {
            plugin: p.plugin().into(),
            state: p.save(cx),
            name: p.custom_title(),
        },
    }
}
