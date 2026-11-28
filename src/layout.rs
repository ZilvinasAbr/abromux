//! Building workspaces from a layout (`abromux_core::layout`, cmux's
//! `cmux.json` format): a palette command's `"workspace"` and
//! `abromux new-workspace --layout <json>`.

use crate::{
    AbroMux, browser,
    workspace::{Node, Surface, Workspace},
};
pub use abromux_core::layout::*;
use gpui::{Axis, Context, Window};
use std::path::Path;

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
        // `check` keeps panes from being empty; if one were, it would
        // collapse away, and with nothing left the workspace gets a shell.
        let root =
            match self.build_layout(layout, id, &dir, spec, &mut setup, &mut focus, window, cx) {
                Some(root) => root,
                None => {
                    let t = self.make_terminal(
                        id,
                        crate::TerminalOpts::cwd(Some(dir.to_string_lossy().into_owned())),
                        window,
                        cx,
                    );
                    self.leaf_node(vec![t], 0).expect("one tab")
                }
            };
        let panes: Vec<usize> = root.panes().map(|p| p.id).collect();
        let focused = focus.unwrap_or(panes[0]);
        let name = spec.name.clone().or(name);
        let mut ws = Workspace::new(id, name, root, focused);
        let colors = &cx.global::<crate::settings::Settings>().workspace_colors;
        ws.color = spec.color.as_deref().and_then(|c| colors.parse(c));
        ws.description = spec
            .description
            .as_deref()
            .and_then(abromux_core::text::clean_description);
        let (index, group) = self.new_workspace_slot(cx);
        ws.group = group;
        let index = self.insert_workspace(index, ws);
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
    ) -> Option<Node> {
        match node {
            LayoutNode::Pane { pane } => {
                // The first surface asking for focus, in this pane if no
                // earlier pane had one.
                let wants = pane
                    .surfaces
                    .iter()
                    .position(|s| s.focus)
                    .filter(|_| focus.is_none());
                let tabs = pane
                    .surfaces
                    .iter()
                    .map(|s| self.build_surface(s, ws, dir, spec, setup, window, cx))
                    .collect();
                let node = self.leaf_node(tabs, wants.unwrap_or(0))?;
                if wants.is_some()
                    && let Node::Leaf(p) = &node
                {
                    *focus = Some(p.id);
                }
                Some(node)
            }
            LayoutNode::Split(s) => {
                let [a, b] = &*s.children;
                let a = self.build_layout(a, ws, dir, spec, setup, focus, window, cx);
                let b = self.build_layout(b, ws, dir, spec, setup, focus, window, cx);
                let axis = match s.direction {
                    Direction::Horizontal => Axis::Horizontal,
                    Direction::Vertical => Axis::Vertical,
                };
                self.split_node(axis, s.split, a, b)
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
                let surface = self.make_terminal(
                    ws,
                    crate::TerminalOpts {
                        cwd,
                        env,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
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
