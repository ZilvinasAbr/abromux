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

use crate::pty::Attach;
use crate::{
    AbroMux,
    agent::Agent,
    workspace::{Node, Pane, Surface, Workspace},
};
use gpui::{App, Axis, Bounds, Context, Pixels, Window, point, px, size};
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
        /// Its terminal in herdr-core, still running (`crate::hc`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hc: Option<String>,
        /// Its terminal in abromux's holder, still running (`crate::hold`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<String>,
        /// Its surface number, which that terminal's shell has in
        /// `ABROMUX_SURFACE_ID`; only kept with `hc` or `held`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<usize>,
    },
    Browser {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
    let text = std::fs::read_to_string(path()?).ok()?;
    match serde_json::from_str::<Session>(&text) {
        Ok(s) if s.version == VERSION && !s.workspaces.is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            eprintln!("abromux: ignoring unreadable session: {e}");
            None
        }
    }
}

/// Write atomically (temp file + rename) so a crash mid-write can't leave a
/// truncated session behind.
pub fn save(session: &Session) {
    let Some(path) = path() else { return };
    let Ok(json) = serde_json::to_string(session) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
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
        // herdr-core's shells keep their surface numbers: leave those free.
        for ws in &session.workspaces {
            self.next_surface = self.next_surface.max(ws.root.max_surface());
        }
        for state in session.workspaces {
            self.next_ws += 1;
            let id = self.next_ws;
            let Some(root) = self.build_node(state.root, id, window, cx) else {
                continue;
            };
            let panes: Vec<usize> = root.panes().iter().map(|p| p.id).collect();
            let focused_pane = panes
                .get(state.focused)
                .or(panes.first())
                .copied()
                .unwrap_or(0);
            self.workspaces
                .push(Workspace::new(id, state.name, root, focused_pane));
        }
        if self.workspaces.is_empty() {
            return false;
        }
        self.sidebar = session.sidebar;
        self.sidebar_width = session
            .sidebar_width
            .clamp(crate::SIDEBAR_MIN, crate::SIDEBAR_MAX);
        self.select_workspace(session.active.min(self.workspaces.len() - 1), window, cx);
        true
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
                hc,
                held,
                surface,
            } => {
                let cwd = std::path::Path::new(&cwd).is_dir().then_some(cwd);
                let attach = hc
                    .map(Attach::Hc)
                    .or(held.map(Attach::Held))
                    .zip(surface);
                let attach = attach.as_ref().map(|(a, n)| (*n, a));
                let s = self.make_terminal_full(cwd, ws, vec![], attach, window, cx);
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
            TabState::Browser { url, name } => (self.make_browser(&url, window, cx), name),
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
            let (hc, held) = match t.attach() {
                Some(Attach::Hc(a)) => (Some(a), None),
                Some(Attach::Held(id)) => (None, Some(id)),
                None => (None, None),
            };
            TabState::Terminal {
                surface: surface.filter(|_| hc.is_some() || held.is_some()),
                hc,
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
                                hc: Some("t3".into()),
                                held: None,
                                surface: Some(7),
                            }],
                        },
                        NodeState::Pane {
                            active: 0,
                            tabs: vec![TabState::Browser {
                                url: "https://example.com".into(),
                                name: None,
                            }],
                        },
                    ]),
                },
            }],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);

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
