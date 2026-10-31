//! Workspaces built from a layout, in cmux's `cmux.json` format: a palette
//! command's `"workspace"` and `abromux new-workspace --layout <json>`.
//!
//! ```json
//! {
//!   "name": "Web",
//!   "cwd": "./app",
//!   "env": { "PORT": "3000" },
//!   "layout": {
//!     "direction": "horizontal",
//!     "split": 0.6,
//!     "children": [
//!       { "pane": { "surfaces": [
//!         { "type": "terminal", "name": "Dev", "command": "npm run dev", "focus": true },
//!         { "type": "terminal", "name": "Tests", "cwd": "./tests" }
//!       ] } },
//!       { "pane": { "surfaces": [{ "type": "browser", "url": "http://localhost:3000" }] } }
//!     ]
//!   }
//! }
//! ```
//!
//! `"horizontal"` puts the two children side by side, `"vertical"` stacks
//! them. A `cwd` of `~/x` is under home, `/x` absolute, and anything else
//! relative to the workspace's own (which is relative to the project, or to
//! the focused terminal's directory).

use crate::{
    AbroMux, browser,
    workspace::{Node, Pane, Surface, Workspace},
};
use gpui::{Axis, Context, Window};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSpec {
    /// The workspace's name; defaults to the command's title.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// One terminal when left out.
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    /// For every shell in the workspace.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Typed into the first terminal before its own command.
    #[serde(default)]
    pub setup: Option<String>,
    /// cmux's workspace colour; abromux has none, so it's ignored.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    Pane { pane: PaneSpec },
    Split(SplitSpec),
}

/// By hand rather than `untagged`, whose error names no field: a node with
/// `"pane"` is a pane, any other a split.
impl<'de> Deserialize<'de> for LayoutNode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(d)?;
        let Some(map) = v.as_object() else {
            return Err(D::Error::custom("a layout node must be an object"));
        };
        if let Some(pane) = map.get("pane") {
            if map.len() > 1 {
                return Err(D::Error::custom("a pane node has only \"pane\""));
            }
            let pane = PaneSpec::deserialize(pane).map_err(D::Error::custom)?;
            return Ok(LayoutNode::Pane { pane });
        }
        if !map.contains_key("direction") {
            return Err(D::Error::custom(
                "a layout node needs \"pane\" or \"direction\" and \"children\"",
            ));
        }
        SplitSpec::deserialize(v)
            .map(LayoutNode::Split)
            .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PaneSpec {
    pub surfaces: Vec<SurfaceSpec>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SplitSpec {
    pub direction: Direction,
    /// The first child's share, 0.1–0.9.
    #[serde(default = "half")]
    pub split: f32,
    pub children: Box<[LayoutNode; 2]>,
}

fn half() -> f32 {
    0.5
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Terminal,
    Browser,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSpec {
    #[serde(default, rename = "type")]
    pub kind: Kind,
    /// The tab's name.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub focus: bool,
    /// Terminals: run when it opens.
    #[serde(default)]
    pub command: Option<String>,
    /// Browsers: the page to load.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl LayoutNode {
    /// Every surface in tree order.
    fn surfaces(&self) -> Vec<&SurfaceSpec> {
        match self {
            LayoutNode::Pane { pane } => pane.surfaces.iter().collect(),
            LayoutNode::Split(s) => s.children.iter().flat_map(|c| c.surfaces()).collect(),
        }
    }

    /// Why it can't be built, if it can't.
    pub fn check(&self) -> Result<(), String> {
        match self {
            LayoutNode::Pane { pane } if pane.surfaces.is_empty() => {
                Err("a pane needs at least one surface".into())
            }
            LayoutNode::Pane { pane } => pane.surfaces.iter().try_for_each(|s| match s.kind {
                Kind::Browser if s.command.is_some() => {
                    Err("a browser surface can't have a \"command\"".into())
                }
                Kind::Terminal if s.url.is_some() => {
                    Err("a terminal surface can't have a \"url\"".into())
                }
                _ => Ok(()),
            }),
            LayoutNode::Split(s) => s.children.iter().try_for_each(LayoutNode::check),
        }
    }
}

impl WorkspaceSpec {
    /// The shell commands it types: `setup`, then each terminal's.
    pub fn commands(&self) -> Vec<String> {
        let surfaces = self
            .layout
            .as_ref()
            .map(LayoutNode::surfaces)
            .unwrap_or_default();
        self.setup
            .iter()
            .chain(surfaces.iter().filter_map(|s| s.command.as_ref()))
            .cloned()
            .collect()
    }

    pub fn check(&self) -> Result<(), String> {
        self.layout.as_ref().map_or(Ok(()), LayoutNode::check)
    }
}

/// `cwd` from a layout, against `base`: `~/…` is under home, `/…` absolute,
/// anything else relative to `base`.
pub fn resolve(cwd: Option<&str>, base: &Path) -> PathBuf {
    let path = match cwd {
        None | Some("" | ".") => base.to_path_buf(),
        Some("~") => home(),
        Some(c) if c.starts_with("~/") => home().join(&c[2..]),
        Some(c) => base.join(c),
    };
    // `app/../tests` as `tests`, as the shell would show it.
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            p => out.push(p),
        }
    }
    out
}

fn home() -> PathBuf {
    std::env::var("HOME").unwrap_or_else(|_| "/".into()).into()
}

impl AbroMux {
    /// Build `spec` as a new workspace in `base` and select it. Returns its
    /// index.
    pub(crate) fn open_layout(
        &mut self,
        spec: &WorkspaceSpec,
        name: Option<String>,
        base: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let dir = resolve(spec.cwd.as_deref(), base);
        let one = LayoutNode::Pane {
            pane: PaneSpec {
                surfaces: vec![SurfaceSpec::default()],
            },
        };
        let layout = spec.layout.as_ref().unwrap_or(&one);
        self.next_ws += 1;
        let id = self.next_ws;
        let mut focus = None;
        let mut setup = spec.setup.clone();
        let root = self.build_layout(layout, id, &dir, spec, &mut setup, &mut focus, window, cx);
        let panes: Vec<usize> = root.panes().iter().map(|p| p.id).collect();
        let focused = focus.unwrap_or(panes[0]);
        let name = spec.name.clone().or(name);
        self.workspaces
            .push(Workspace::new(id, name, root, focused));
        let index = self.workspaces.len() - 1;
        self.select_workspace(index, window, cx);
        index
    }

    #[allow(clippy::too_many_arguments)]
    fn build_layout(
        &mut self,
        node: &LayoutNode,
        ws: usize,
        dir: &Path,
        spec: &WorkspaceSpec,
        setup: &mut Option<String>,
        focus: &mut Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Node {
        match node {
            LayoutNode::Pane { pane } => {
                let id = self.pane_id();
                let mut active = 0;
                let tabs = pane
                    .surfaces
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        if s.focus && focus.is_none() {
                            *focus = Some(id);
                            active = i;
                        }
                        self.build_surface(s, ws, dir, spec, setup, window, cx)
                    })
                    .collect();
                Node::Leaf(Pane { id, tabs, active })
            }
            LayoutNode::Split(s) => {
                let [a, b] = &*s.children;
                let a = self.build_layout(a, ws, dir, spec, setup, focus, window, cx);
                let b = self.build_layout(b, ws, dir, spec, setup, focus, window, cx);
                Node::Split {
                    id: self.id(),
                    axis: match s.direction {
                        Direction::Horizontal => Axis::Horizontal,
                        Direction::Vertical => Axis::Vertical,
                    },
                    ratio: s.split.clamp(0.1, 0.9),
                    children: Box::new([a, b]),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_surface(
        &mut self,
        s: &SurfaceSpec,
        ws: usize,
        dir: &Path,
        spec: &WorkspaceSpec,
        setup: &mut Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Surface {
        let surface = match s.kind {
            Kind::Browser => {
                let url = browser::normalize_url(s.url.as_deref().unwrap_or("about:blank"));
                self.make_browser(&url, window, cx)
            }
            Kind::Terminal => {
                let cwd = resolve(s.cwd.as_deref(), dir);
                // A missing directory would leave the shell in $HOME silently.
                let cwd = cwd.is_dir().then(|| cwd.to_string_lossy().into_owned());
                let env: Vec<_> = spec
                    .env
                    .iter()
                    .chain(&s.env)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let surface = self.make_terminal_with(cwd, ws, env, window, cx);
                if let Surface::Terminal(t) = &surface {
                    // The shell reads these once it starts.
                    let text: String = setup
                        .take()
                        .into_iter()
                        .chain(s.command.clone())
                        .map(|c| format!("{c}\r"))
                        .collect();
                    if !text.is_empty() {
                        t.update(cx, |t, _| t.send(text.as_bytes()));
                    }
                }
                surface
            }
        };
        if s.name.is_some() {
            surface.set_custom_title(s.name.clone(), cx);
        }
        surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cmux_layouts() {
        let spec: WorkspaceSpec = serde_json::from_str(
            r##"{
              "name": "Web", "cwd": "./app", "color": "#ff0000",
              "layout": {
                "direction": "horizontal", "split": 0.6,
                "children": [
                  { "pane": { "surfaces": [{ "type": "terminal", "command": "npm run dev", "focus": true }] } },
                  { "direction": "vertical", "children": [
                    { "pane": { "surfaces": [{ "type": "browser", "url": "localhost:3000" }] } },
                    { "pane": { "surfaces": [{ "name": "Shell" }] } }
                  ] }
                ]
              }
            }"##,
        )
        .unwrap();
        spec.check().unwrap();
        let layout = spec.layout.unwrap();
        let LayoutNode::Split(s) = &layout else {
            panic!("{layout:?}")
        };
        assert_eq!((s.direction, s.split), (Direction::Horizontal, 0.6));
        let kinds: Vec<_> = layout.surfaces().iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [Kind::Terminal, Kind::Browser, Kind::Terminal]);
    }

    #[test]
    fn rejects_bad_layouts() {
        let parse = |j: &str| serde_json::from_str::<WorkspaceSpec>(j);
        assert!(parse(r#"{ "layout": { "direction": "diagonal", "children": [] } }"#).is_err());
        let empty = parse(r#"{ "layout": { "pane": { "surfaces": [] } } }"#).unwrap();
        assert!(empty.check().unwrap_err().contains("at least one"));
        let mixed = parse(
            r#"{ "layout": { "pane": { "surfaces": [{ "type": "browser", "command": "x" }] } } }"#,
        )
        .unwrap();
        assert!(mixed.check().is_err());
        assert!(parse(r#"{ "layuot": {} }"#).is_err(), "typos are errors");
        let e = parse(r#"{ "layout": { "bogus": 1 } }"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs \"pane\" or \"direction\""), "{e}");
        let e = parse(r#"{ "layout": { "pane": { "surfaces": [{ "kind": "x" }] } } }"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `kind`"), "{e}");
    }

    #[test]
    fn resolves_cwds() {
        let base = Path::new("/p");
        let home = home();
        assert_eq!(resolve(None, base), base);
        assert_eq!(resolve(Some("."), base), base);
        assert_eq!(resolve(Some("./app"), base), Path::new("/p/app"));
        assert_eq!(resolve(Some("app"), base), Path::new("/p/app"));
        assert_eq!(resolve(Some("/x"), base), Path::new("/x"));
        assert_eq!(resolve(Some("./app/../tests"), base), Path::new("/p/tests"));
        assert_eq!(resolve(Some("~/x"), base), home.join("x"));
    }
}
