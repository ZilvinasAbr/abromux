//! The session file's model: windows, workspaces, the split tree and each
//! tab, as `session.json` keeps them. Reading, writing and rebuilding the
//! app from one are the app's (`src/session.rs`).

use crate::agent::Agent;
use serde::{Deserialize, Serialize};

/// The session file's format version.
pub const VERSION: u32 = 1;

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
    /// The merge requests panel is showing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mr_panel: bool,
    pub workspaces: Vec<WorkspaceState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupState>,
    /// Where the window was with each arrangement of screens it has been
    /// on, latest last, so it goes back when a monitor comes back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placements: Vec<Placement>,
}

/// The window's frame (global screen points, from the primary screen's top
/// left) while the screens were `screens`, a key for their arrangement.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Placement {
    pub screens: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GroupState {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleanup: bool,
}

fn yes() -> bool {
    true
}

/// The sidebar's width until it's dragged.
pub const SIDEBAR_WIDTH: f32 = 232.;

fn sidebar_width() -> f32 {
    SIDEBAR_WIDTH
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
    /// The app's `Workspace::name_surface`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_surface: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Index into the session's `groups`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<usize>,
    /// Its workspace folder (⇧⌘I).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Its tracked links (`crate::links`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<crate::links::Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_links: Vec<String>,
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
        /// A coding agent to resume (see `crate::agent`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<Agent>,
        /// The tab's name from `rename-tab`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Its terminal in abromux's holder, still running (`abromux_term::hold`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<String>,
        /// Its surface number, which that terminal's shell has in
        /// `ABROMUX_SURFACE_ID`; only kept with `held`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<usize>,
        /// The file its editor had open (the app's `editor`), opened again
        /// in the editor set then.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit: Option<String>,
    },
    Browser {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The back/forward list (`BrowserView::history_state`), saved on quit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history: Option<String>,
        /// Page zoom (⌘= / ⌘-), when not actual size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zoom: Option<f64>,
    },
    /// A plugin's tab (the app's src/plugins.rs), with what it saved.
    Plugin {
        plugin: String,
        #[serde(default)]
        state: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl NodeState {
    /// The highest surface number a terminal in it keeps.
    pub fn max_surface(&self) -> usize {
        match self {
            NodeState::Pane { tabs, .. } => tabs
                .iter()
                .filter_map(|t| match t {
                    TabState::Terminal { surface, .. } => *surface,
                    TabState::Browser { .. } | TabState::Plugin { .. } => None,
                })
                .max()
                .unwrap_or(0),
            NodeState::Split { children, .. } => {
                children[0].max_surface().max(children[1].max_surface())
            }
        }
    }
}

/// Forget the terminals' scrollback (the layout-only saves).
pub fn drop_scrollback(n: &mut NodeState) {
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
pub fn workspace_key(name: Option<&str>, root: &NodeState) -> String {
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
                    TabState::Plugin { plugin, state, .. } => format!("p:{plugin}:{state}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_tabs_round_trip() {
        let t = TabState::Plugin {
            plugin: "gitlab".into(),
            state: serde_json::json!({ "host": "http://gl.test" }),
            name: Some("Reviews".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"plugin","plugin":"gitlab","state":{"host":"http://gl.test"},"name":"Reviews"}"#
        );
        let back: TabState = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        // No state saved: the plugin opens on `null`.
        let bare: TabState = serde_json::from_str(r#"{"kind":"plugin","plugin":"x"}"#).unwrap();
        assert!(matches!(bare, TabState::Plugin { state, .. } if state.is_null()));
    }

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
            mr_panel: true,
            workspaces: vec![WorkspaceState {
                name: Some("api".into()),
                auto_name: None,
                name_surface: None,
                description: Some("Ship it\n- build".into()),
                color: Some(0x1565c0),
                group: Some(0),
                home: Some("/tmp/ws".into()),
                links: vec![crate::links::Link {
                    kind: crate::links::Kind::Jira,
                    key: "PAY-1".into(),
                    url: "https://acme.atlassian.net/browse/PAY-1".into(),
                    title: Some("Refunds".into()),
                }],
                hidden_links: vec!["acme/api!3".into()],
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
                                edit: Some("/tmp/main.rs".into()),
                            }],
                        },
                        NodeState::Pane {
                            active: 0,
                            tabs: vec![TabState::Browser {
                                url: "https://example.com".into(),
                                name: None,
                                history: Some("YnBsaXN0".into()),
                                zoom: Some(1.25),
                            }],
                        },
                    ]),
                },
            }],
            groups: vec![GroupState {
                name: "web".into(),
                color: None,
                collapsed: true,
                cleanup: false,
            }],
            placements: vec![Placement {
                screens: "0,0,1512x982".into(),
                x: 10.,
                y: 40.,
                width: 800.,
                height: 600.,
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
                && m.sidebar_width == SIDEBAR_WIDTH
                && m.window.is_none()
                && m.workspaces[0].name.is_none()
        );
    }
}
