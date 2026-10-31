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
    agent::Agent,
    workspace::{Group, Node, Pane, Surface, Workspace},
};
use gpui::{App, Axis, Bounds, Context, Pixels, Window, point, px, size};
use gpui_component::WindowExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 1;
/// Lines of scrollback kept per terminal.
const SCROLLBACK_LINES: usize = 2000;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Session {
    pub version: u32,
    #[serde(default)]
    pub window: Option<WindowState>,
    pub active: usize,
    #[serde(default = "yes")]
    pub sidebar: bool,
    #[serde(default = "sidebar_width")]
    pub sidebar_width: f32,
    pub workspaces: Vec<WorkspaceState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupState>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GroupState {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default)]
    pub collapsed: bool,
}

fn yes() -> bool {
    true
}

fn sidebar_width() -> f32 {
    crate::SIDEBAR_WIDTH
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorkspaceState {
    #[serde(default)]
    pub name: Option<String>,
    /// Index of the focused pane in tree order.
    #[serde(default)]
    pub focused: usize,
    pub root: NodeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Index into the session's `groups`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeState {
    Pane {
        #[serde(default)]
        active: usize,
        tabs: Vec<TabState>,
    },
    Split {
        vertical: bool,
        ratio: f32,
        children: Box<[NodeState; 2]>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabState {
    Terminal {
        cwd: String,
        #[serde(default)]
        font_size: Option<f32>,
        /// Screen and scrollback as VT text (styles kept).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scrollback: Option<String>,
        /// A coding agent to resume (see agent.rs).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<Agent>,
        /// The tab's name from `rename-tab`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Its terminal in abromux's holder, still running (`crate::hold`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<String>,
        /// Its surface number, which that terminal's shell has in
        /// `ABROMUX_SURFACE_ID`; only kept with `held`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<usize>,
    },
    Browser {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The back/forward list (`BrowserView::history_state`), saved on quit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history: Option<String>,
    },
}

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
    std::env::var("HOME").ok()?;
    Some(crate::channel::support_dir().join("session.json"))
}

pub fn load() -> Option<Session> {
    let path = path()?;
    if !path.exists() {
        return None;
    }
    match read(&path) {
        Ok(s) if !s.workspaces.is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            eprintln!("abromux: ignoring unreadable session: {e}");
            None
        }
    }
}

/// A session file, or why it couldn't be read.
pub fn read(path: &std::path::Path) -> Result<Session, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
/// `nightly`, `nightly-<name>`), or a file.
pub fn source(from: &str) -> PathBuf {
    if let Some(dir) = crate::channel::support_dir_named(from) {
        return dir.join("session.json");
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    from.strip_prefix("~/")
        .map_or_else(|| from.into(), |rest| home.join(rest))
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
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

impl WindowState {
    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }
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
                name: ws.custom_name.clone(),
                auto_name: ws.auto_title.clone(),
                description: ws.description.clone(),
                color: ws.color,
                group: ws
                    .group
                    .and_then(|g| self.groups.iter().position(|x| x.id == g)),
                focused: ws
                    .root
                    .panes()
                    .iter()
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
                })
                .collect(),
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
                        workspace_key(w.custom_name.as_deref(), &node_state(&w.root, false, &self.surface_refs, cx))
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
        }
        let mut ids = vec![];
        for state in states {
            self.next_ws += 1;
            let id = self.next_ws;
            let Some(root) = self.build_node(state.root, id, window, cx) else {
                ids.push(None);
                continue;
            };
            let panes: Vec<usize> = root.panes().iter().map(|p| p.id).collect();
            let focused_pane = panes
                .get(state.focused)
                .or(panes.first())
                .copied()
                .unwrap_or(0);
            let mut ws = Workspace::new(id, state.name, root, focused_pane);
            ws.auto_title = state.auto_name;
            ws.description = state.description;
            ws.color = state.color;
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
        Some(match state {
            NodeState::Pane { active, tabs } => {
                let tabs: Vec<Surface> = tabs
                    .into_iter()
                    .map(|t| self.build_tab(t, ws, window, cx))
                    .collect();
                if tabs.is_empty() {
                    return None;
                }
                Node::Leaf(Pane {
                    id: self.pane_id(),
                    active: active.min(tabs.len() - 1),
                    tabs,
                })
            }
            NodeState::Split {
                vertical,
                ratio,
                children,
            } => {
                let [a, b] = *children;
                // A side that came back empty collapses into the other.
                match (
                    self.build_node(a, ws, window, cx),
                    self.build_node(b, ws, window, cx),
                ) {
                    (Some(a), Some(b)) => Node::Split {
                        id: self.id(),
                        axis: if vertical {
                            Axis::Vertical
                        } else {
                            Axis::Horizontal
                        },
                        ratio: ratio.clamp(0.1, 0.9),
                        children: Box::new([a, b]),
                    },
                    (a, b) => a.or(b)?,
                }
            }
        })
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
                name,
                held,
                surface,
            } => {
                let cwd = std::path::Path::new(&cwd).is_dir().then_some(cwd);
                let attach = surface.zip(held.as_deref());
                let s = self.make_terminal_full(cwd, ws, vec![], None, attach, window, cx);
                if let Surface::Terminal(t) = &s {
                    t.update(cx, |t, cx| {
                        if t.attached {
                            // It ran on: its screen and agent are still there.
                            t.restore(None, font_size, None, cx);
                            t.agent = agent;
                        } else {
                            t.restore(scrollback.as_deref(), font_size, agent, cx)
                        }
                    });
                }
                (s, name)
            }
            // terminal-browser can't take the built-in browser's history.
            TabState::Browser { url, name, .. } if crate::settings::terminal_browser(cx) => {
                (self.make_browser(&url, window, cx), name)
            }
            TabState::Browser { url, name, history } => match history {
                Some(h) => {
                    let s = self.make_browser("", window, cx);
                    if let Surface::Browser(b) = &s {
                        b.update(cx, |b, cx| {
                            if b.restore_history(&h, cx) {
                                b.url = url;
                            } else if !url.is_empty() {
                                b.navigate(&url, cx);
                            }
                        });
                    }
                    (s, name)
                }
                None => (self.make_browser(&url, window, cx), name),
            },
        };
        if name.is_some() {
            s.set_custom_title(name, cx);
        }
        s
    }
}

impl NodeState {
    /// The highest surface number a terminal in it keeps.
    fn max_surface(&self) -> usize {
        match self {
            NodeState::Pane { tabs, .. } => tabs
                .iter()
                .filter_map(|t| match t {
                    TabState::Terminal { surface, .. } => *surface,
                    TabState::Browser { .. } => None,
                })
                .max()
                .unwrap_or(0),
            NodeState::Split { children, .. } => {
                children[0].max_surface().max(children[1].max_surface())
            }
        }
    }
}

type Refs = std::collections::HashMap<gpui::EntityId, usize>;

fn drop_scrollback(n: &mut NodeState) {
    match n {
        NodeState::Pane { tabs, .. } => {
            for t in tabs {
                if let TabState::Terminal { scrollback, .. } = t {
                    *scrollback = None;
                }
            }
        }
        NodeState::Split { children, .. } => children.iter_mut().for_each(drop_scrollback),
    }
}

/// A workspace's name and layout, to tell whether a saved one is open
/// already.
fn workspace_key(name: Option<&str>, root: &NodeState) -> String {
    format!("{}\n{}", name.unwrap_or_default(), layout_key(root))
}

/// What a workspace's layout is: its splits and each tab's directory or
/// page.
fn layout_key(n: &NodeState) -> String {
    match n {
        NodeState::Pane { tabs, .. } => {
            let tabs: Vec<String> = tabs
                .iter()
                .map(|t| match t {
                    TabState::Terminal { cwd, .. } => format!("t:{cwd}"),
                    TabState::Browser { url, .. } => format!("b:{url}"),
                })
                .collect();
            format!("[{}]", tabs.join(","))
        }
        NodeState::Split {
            vertical, children, ..
        } => format!(
            "({}{}|{})",
            if *vertical { "v" } else { "h" },
            layout_key(&children[0]),
            layout_key(&children[1])
        ),
    }
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
            if let Some(url) = t.app.as_ref().and_then(|a| a.page_url()) {
                return TabState::Browser {
                    url: url.into(),
                    name: t.custom_title.clone(),
                    history: None,
                };
            }
            let held = t.held_id().map(str::to_string);
            TabState::Terminal {
                surface: surface.filter(|_| held.is_some()),
                held,
                cwd: t.live_cwd(),
                font_size: Some(t.font_size()),
                scrollback: scrollback
                    .then(|| t.scrollback_vt(SCROLLBACK_LINES))
                    .flatten(),
                agent: t.agent.clone().filter(Agent::alive),
                name: t.custom_title.clone(),
            }
        }
        Surface::Browser(b) => TabState::Browser {
            url: b.read(cx).url.clone(),
            name: b.read(cx).custom_title.clone(),
            history: scrollback.then(|| b.read(cx).history_state(cx)).flatten(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let s = Session {
            version: VERSION,
            window: Some(WindowState {
                x: 10.,
                y: 20.,
                width: 800.,
                height: 600.,
            }),
            active: 1,
            sidebar: false,
            sidebar_width: 300.,
            workspaces: vec![WorkspaceState {
                name: Some("api".into()),
                auto_name: None,
                description: Some("Ship it\n- build".into()),
                color: Some(0x1565c0),
                group: Some(0),
                focused: 1,
                root: NodeState::Split {
                    vertical: false,
                    ratio: 0.3,
                    children: Box::new([
                        NodeState::Pane {
                            active: 0,
                            tabs: vec![TabState::Terminal {
                                cwd: "/tmp".into(),
                                font_size: Some(14.),
                                scrollback: Some("\x1b[1mhi\x1b[0m".into()),
                                agent: Some(Agent {
                                    kind: "claude".into(),
                                    session_id: "abc".into(),
                                    cwd: "/tmp".into(),
                                    permission_mode: None,
                                    pid: None,
                                }),
                                name: Some("server".into()),
                                held: Some("4242-3".into()),
                                surface: Some(7),
                            }],
                        },
                        NodeState::Pane {
                            active: 0,
                            tabs: vec![TabState::Browser {
                                url: "https://example.com".into(),
                                name: None,
                                history: Some("YnBsaXN0".into()),
                            }],
                        },
                    ]),
                },
            }],
            groups: vec![GroupState {
                name: "web".into(),
                color: None,
                collapsed: true,
            }],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);

        let mut root = s.workspaces[0].root.clone();
        drop_scrollback(&mut root);
        assert!(json.contains("[1mhi") && !serde_json::to_string(&root).unwrap().contains("[1mhi"));

        let minimal = r#"{"version":1,"active":0,"workspaces":[{"root":{"kind":"pane","tabs":[{"kind":"terminal","cwd":"/"}]}}]}"#;
        let m: Session = serde_json::from_str(minimal).unwrap();
        assert!(
            m.sidebar
                && m.sidebar_width == crate::SIDEBAR_WIDTH
                && m.window.is_none()
                && m.workspaces[0].name.is_none()
        );
    }
}
