//! Workspace model: each workspace owns a tree of split panes; each pane
//! holds tabs ("surfaces") that are either terminals or browsers.

use crate::{agent::Kind, browser::BrowserView, terminal::TerminalView};
use gpui::{App, Axis, Entity, EntityId, Focusable, Window};

#[derive(Clone)]
pub enum Surface {
    Terminal(Entity<TerminalView>),
    Browser(Entity<BrowserView>),
}

impl Surface {
    pub fn entity_id(&self) -> EntityId {
        match self {
            Surface::Terminal(t) => t.entity_id(),
            Surface::Browser(b) => b.entity_id(),
        }
    }

    /// A terminal running a coding agent shows the agent's logo.
    pub fn icon(&self, cx: &App) -> &'static str {
        match self {
            Surface::Terminal(t) if let Some(app) = &t.read(cx).app => app.icon(),
            Surface::Terminal(t) => match t.read(cx).agent.as_ref() {
                Some(a) if a.alive() => match Kind::parse(&a.kind) {
                    Some(Kind::Claude) => "claude",
                    Some(Kind::Codex) => "codex",
                    Some(_) => "bot",
                    None => "terminal",
                },
                _ => "terminal",
            },
            Surface::Browser(_) => "globe",
        }
    }

    /// Claude Code is working in it; its logo turns.
    pub fn claude_running(&self, cx: &App) -> bool {
        matches!(self, Surface::Terminal(t) if t.read(cx).claude_running())
    }

    pub fn title(&self, cx: &App) -> String {
        if let Some(t) = self.custom_title(cx) {
            return t;
        }
        match self {
            Surface::Terminal(t) => {
                let t = t.read(cx);
                if let Some(app) = &t.app {
                    return app.title();
                }
                if !t.shown_title().is_empty() {
                    t.shown_title().to_string()
                } else {
                    tilde(&t.cwd)
                }
            }
            Surface::Browser(b) => {
                let b = b.read(cx);
                if !b.title.is_empty() {
                    b.title.clone()
                } else if !b.url.is_empty() {
                    b.url.clone()
                } else {
                    "New Tab".into()
                }
            }
        }
    }

    /// The name the tab was given (`rename-tab`), if any.
    pub fn custom_title(&self, cx: &App) -> Option<String> {
        match self {
            Surface::Terminal(t) => t.read(cx).custom_title.clone(),
            Surface::Browser(b) => b.read(cx).custom_title.clone(),
        }
    }

    /// Name the tab, or with None go back to the program's or page's title.
    pub fn set_custom_title(&self, title: Option<String>, cx: &mut App) {
        let title = title
            .map(|t| t.replace(['\n', '\r'], " ").trim().to_string())
            .filter(|t| !t.is_empty());
        match self {
            Surface::Terminal(t) => t.update(cx, |t, cx| {
                t.custom_title = title;
                cx.notify();
            }),
            Surface::Browser(b) => b.update(cx, |b, cx| {
                b.custom_title = title;
                cx.notify();
            }),
        }
    }

    /// A browser tab: the built-in one, or a terminal running `terminal-browser`.
    pub fn is_browser(&self, cx: &App) -> bool {
        match self {
            Surface::Terminal(t) => t
                .read(cx)
                .app
                .as_ref()
                .is_some_and(|a| a.page_url().is_some()),
            Surface::Browser(_) => true,
        }
    }

    pub fn cwd(&self, cx: &App) -> Option<String> {
        match self {
            Surface::Terminal(t) => Some(t.read(cx).cwd.clone()),
            Surface::Browser(_) => None,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Surface::Terminal(t) => window.focus(&t.focus_handle(cx)),
            Surface::Browser(b) => BrowserView::focus_address(b, window, cx),
        }
    }
}

pub struct Pane {
    pub id: usize,
    pub tabs: Vec<Surface>,
    pub active: usize,
}

impl Pane {
    pub fn active_surface(&self) -> Option<&Surface> {
        self.tabs.get(self.active)
    }
}

pub enum Node {
    Leaf(Pane),
    Split {
        id: usize,
        axis: Axis,
        ratio: f32,
        children: Box<[Node; 2]>,
    },
}

impl Node {
    pub fn panes(&self) -> Vec<&Pane> {
        match self {
            Node::Leaf(p) => vec![p],
            Node::Split { children, .. } => children.iter().flat_map(|c| c.panes()).collect(),
        }
    }

    pub fn pane_mut(&mut self, id: usize) -> Option<&mut Pane> {
        match self {
            Node::Leaf(p) => (p.id == id).then_some(p),
            Node::Split { children, .. } => children.iter_mut().find_map(|c| c.pane_mut(id)),
        }
    }

    pub fn split_mut(&mut self, id: usize) -> Option<&mut f32> {
        match self {
            Node::Leaf(_) => None,
            Node::Split {
                id: sid,
                ratio,
                children,
                ..
            } => {
                if *sid == id {
                    Some(ratio)
                } else {
                    children.iter_mut().find_map(|c| c.split_mut(id))
                }
            }
        }
    }

    /// Replace pane `target` with a split of [target, new] ([new, target]
    /// with `before`, i.e. a left/up split).
    pub fn split(
        &mut self,
        target: usize,
        axis: Axis,
        before: bool,
        new: Pane,
        split_id: usize,
    ) -> Result<(), Pane> {
        match self {
            Node::Leaf(p) if p.id == target => {
                let old = std::mem::replace(self, Node::Leaf(dummy()));
                let children = if before {
                    [Node::Leaf(new), old]
                } else {
                    [old, Node::Leaf(new)]
                };
                *self = Node::Split {
                    id: split_id,
                    axis,
                    ratio: 0.5,
                    children: Box::new(children),
                };
                Ok(())
            }
            Node::Leaf(_) => Err(new),
            Node::Split { children, .. } => {
                let [a, b] = &mut **children;
                match a.split(target, axis, before, new, split_id) {
                    Ok(()) => Ok(()),
                    Err(new) => b.split(target, axis, before, new, split_id),
                }
            }
        }
    }

    /// Remove pane `target`, collapsing its parent split. Returns false if
    /// the target is the root leaf (caller decides what to do).
    pub fn remove(&mut self, target: usize) -> bool {
        let Node::Split { children, .. } = self else {
            return false;
        };
        let hit = children
            .iter()
            .position(|c| matches!(c, Node::Leaf(p) if p.id == target));
        if let Some(i) = hit {
            let [a, b] = *std::mem::replace(
                children,
                Box::new([Node::Leaf(dummy()), Node::Leaf(dummy())]),
            );
            *self = if i == 0 { b } else { a };
            return true;
        }
        children.iter_mut().any(|c| c.remove(target))
    }

    /// Put `new` beside the whole tree on side `dir`: a full-width (or
    /// full-height) split.
    pub fn wrap(&mut self, dir: Dir, new: Pane, split_id: usize) {
        let old = std::mem::replace(self, Node::Leaf(dummy()));
        let before = matches!(dir, Dir::Left | Dir::Up);
        let children = if before {
            [Node::Leaf(new), old]
        } else {
            [old, Node::Leaf(new)]
        };
        *self = Node::Split {
            id: split_id,
            axis: dir.axis(),
            ratio: 0.5,
            children: Box::new(children),
        };
    }

    /// The innermost split along `axis` that holds pane `pane`: the one
    /// whose divider a resize of that pane along `axis` moves.
    pub fn split_along(&self, pane: usize, axis: Axis) -> Option<usize> {
        let Node::Split {
            id,
            axis: a,
            children,
            ..
        } = self
        else {
            return None;
        };
        let child = children
            .iter()
            .find(|c| c.panes().iter().any(|p| p.id == pane))?;
        child
            .split_along(pane, axis)
            .or((*a == axis).then_some(*id))
    }

    /// Each pane's rect `[x0, y0, x1, y1]` inside `r`, from the split ratios.
    fn layout(&self, r: [f32; 4], out: &mut Vec<(usize, [f32; 4])>) {
        match self {
            Node::Leaf(p) => out.push((p.id, r)),
            Node::Split {
                axis,
                ratio,
                children,
                ..
            } => {
                let [x0, y0, x1, y1] = r;
                let (a, b) = if *axis == Axis::Horizontal {
                    let x = x0 + (x1 - x0) * ratio;
                    ([x0, y0, x, y1], [x, y0, x1, y1])
                } else {
                    let y = y0 + (y1 - y0) * ratio;
                    ([x0, y0, x1, y], [x0, y, x1, y1])
                };
                children[0].layout(a, out);
                children[1].layout(b, out);
            }
        }
    }

    /// The pane next to `from` in direction `dir`: the nearest one past that
    /// edge that overlaps it on the other axis, preferring the most overlap.
    pub fn neighbor(&self, from: usize, dir: Dir) -> Option<usize> {
        const EPS: f32 = 1e-4;
        let mut rects = vec![];
        self.layout([0., 0., 1., 1.], &mut rects);
        let &(_, c) = rects.iter().find(|(id, _)| *id == from)?;
        let span = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        let mut hits: Vec<(usize, f32, f32)> = rects
            .iter()
            .filter(|(id, _)| *id != from)
            .map(|&(id, r)| {
                let (gap, overlap) = match dir {
                    Dir::Left => (c[0] - r[2], span(c[1], c[3], r[1], r[3])),
                    Dir::Right => (r[0] - c[2], span(c[1], c[3], r[1], r[3])),
                    Dir::Up => (c[1] - r[3], span(c[0], c[2], r[0], r[2])),
                    Dir::Down => (r[1] - c[3], span(c[0], c[2], r[0], r[2])),
                };
                (id, gap, overlap)
            })
            .filter(|&(_, gap, overlap)| gap > -EPS && overlap > EPS)
            .collect();
        hits.sort_by(|a, b| {
            if (a.1 - b.1).abs() > EPS {
                a.1.total_cmp(&b.1)
            } else {
                b.2.total_cmp(&a.2)
            }
        });
        hits.first().map(|h| h.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Horizontal,
            Dir::Up | Dir::Down => Axis::Vertical,
        }
    }
}

/// Where a dragged tab lands.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TabDrop {
    /// Into pane `pane`'s tabs at `index` (past the end appends).
    Tabs { pane: usize, index: usize },
    /// A new pane split off pane `pane` on side `dir`.
    Split { pane: usize, dir: Dir },
    /// A new pane beside the whole workspace on side `dir`.
    Edge(Dir),
}

fn dummy() -> Pane {
    Pane {
        id: usize::MAX,
        tabs: vec![],
        active: 0,
    }
}

pub struct Workspace {
    pub id: usize,
    pub custom_name: Option<String>,
    /// A name `automation.workspaceAutoNaming` made from its agent's
    /// first prompt; `custom_name` wins over it.
    pub auto_title: Option<String>,
    /// Shown under the name (⌥⌘E, `workspace-action set-description`).
    pub description: Option<String>,
    /// `#rrggbb`, from cmux's palette or any.
    pub color: Option<u32>,
    /// The id of the [`Group`] it's in. A group's workspaces sit together
    /// in the sidebar; [`tidy_groups`] keeps them so.
    pub group: Option<usize>,
    /// Its workspace folder (⇧⌘I, [`crate::folder`]), if it has one.
    pub home: Option<std::path::PathBuf>,
    pub root: Node,
    pub focused_pane: usize,
    /// The pane filling the workspace (⌘⇧↩), hiding the others. Only holds
    /// while that pane keeps focus; see [`zoomed`](Self::zoomed).
    pub zoom: Option<usize>,
    /// Sidebar pills from `set-status` and agent hooks, in the order first set.
    pub status: Vec<Status>,
    pub progress: Option<Progress>,
    /// `abromux log` entries, oldest first; the sidebar shows the newest.
    pub log: Vec<LogEntry>,
    /// The pane focused before this one, for `last-pane`, and the one
    /// `track_focus` last saw.
    prev_pane: Option<usize>,
    seen_pane: usize,
}

pub struct Status {
    pub key: String,
    pub value: String,
    pub icon: Option<String>,
    pub color: Option<u32>,
    /// The terminal it describes (an agent's status), cleared when it closes.
    pub owner: Option<EntityId>,
}

pub struct Progress {
    /// 0.0–1.0.
    pub value: f32,
    pub label: Option<String>,
}

pub struct LogEntry {
    pub level: LogLevel,
    pub source: Option<String>,
    pub message: String,
    pub at: std::time::SystemTime,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LogLevel {
    Info,
    Progress,
    Success,
    Warning,
    Error,
}

impl LogLevel {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "info" => Self::Info,
            "progress" => Self::Progress,
            "success" => Self::Success,
            "warning" | "warn" => Self::Warning,
            "error" => Self::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Progress => "progress",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// `#rgb` or `#rrggbb` (the `#` is optional).
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok(),
        3 => u32::from_str_radix(&h.chars().flat_map(|c| [c, c]).collect::<String>(), 16).ok(),
        _ => None,
    }
}

impl Workspace {
    pub fn new(id: usize, custom_name: Option<String>, root: Node, focused_pane: usize) -> Self {
        Self {
            id,
            custom_name,
            auto_title: None,
            description: None,
            color: None,
            group: None,
            home: None,
            root,
            focused_pane,
            zoom: None,
            status: vec![],
            progress: None,
            log: vec![],
            prev_pane: None,
            seen_pane: focused_pane,
        }
    }

    /// Notice a change of `focused_pane` since the last call. Focus moves in
    /// many places; this runs on every render, which follows each of them.
    pub fn track_focus(&mut self) {
        if self.seen_pane != self.focused_pane {
            self.prev_pane = Some(std::mem::replace(&mut self.seen_pane, self.focused_pane));
        }
    }

    /// The previously focused pane, if it's still open (tmux's `last-pane`).
    pub fn last_pane(&mut self) -> Option<usize> {
        self.track_focus();
        let prev = self.prev_pane?;
        self.root
            .panes()
            .iter()
            .any(|p| p.id == prev)
            .then_some(prev)
    }

    /// Swap pane `id`'s tabs with those of the pane next to it in `dir`, as
    /// tmux's `swap-pane`. The swapped pane keeps focus, so focus moves to
    /// where it went. False if there's no pane that way.
    pub fn swap_pane(&mut self, id: usize, dir: Dir) -> bool {
        let Some(other) = self.root.neighbor(id, dir) else {
            return false;
        };
        let take = |root: &mut Node, id| {
            let p = root.pane_mut(id).expect("pane exists");
            (std::mem::take(&mut p.tabs), p.active)
        };
        let (a, b) = (take(&mut self.root, id), take(&mut self.root, other));
        for (target, (tabs, active)) in [(id, b), (other, a)] {
            let p = self.root.pane_mut(target).expect("pane exists");
            (p.tabs, p.active) = (tabs, active);
        }
        if self.focused_pane == id {
            self.focused_pane = other;
        }
        self.zoom = None;
        true
    }

    pub fn focused(&self) -> Option<&Pane> {
        self.root
            .panes()
            .into_iter()
            .find(|p| p.id == self.focused_pane)
    }

    /// The zoomed pane, if zoom still applies: it is the focused pane and
    /// there is more than one. Clears a zoom that no longer does, so focusing
    /// another pane (or splitting, which focuses the new one) ends it.
    pub fn zoomed(&mut self) -> Option<usize> {
        let multi = matches!(self.root, Node::Split { .. });
        if self.zoom != Some(self.focused_pane) || !multi {
            self.zoom = None;
        }
        self.zoom
    }

    /// Zoom the focused pane, or unzoom. False when there's nothing to zoom.
    pub fn toggle_zoom(&mut self) -> bool {
        if self.zoomed().is_some() {
            self.zoom = None;
        } else if matches!(self.root, Node::Split { .. }) {
            self.zoom = Some(self.focused_pane);
        } else {
            return false;
        }
        true
    }

    /// Whether pane `id` is on screen: every pane is, unless another is zoomed.
    pub fn shows(&self, id: usize) -> bool {
        self.zoom.is_none_or(|z| z == id)
    }

    pub fn focused_surface(&self) -> Option<&Surface> {
        self.focused().and_then(|p| p.active_surface())
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.root
            .panes()
            .iter()
            .any(|p| p.tabs.iter().any(|s| s.entity_id() == id))
    }

    /// Pane, index and pane tab count of tab `id`.
    fn find_tab(&self, id: EntityId) -> Option<(usize, usize, usize)> {
        self.root.panes().iter().find_map(|p| {
            p.tabs
                .iter()
                .position(|s| s.entity_id() == id)
                .map(|i| (p.id, i, p.tabs.len()))
        })
    }

    /// Whether moving tab `id` to `to` would change anything.
    pub fn accepts(&self, id: EntityId, to: TabDrop) -> bool {
        let Some((from, _, count)) = self.find_tab(id) else {
            return false;
        };
        let alone = count == 1;
        let has = |pane| self.root.panes().iter().any(|p| p.id == pane);
        match to {
            // A pane's only tab can't be split off it or reordered within it.
            TabDrop::Tabs { pane, .. } | TabDrop::Split { pane, .. } => {
                has(pane) && !(pane == from && alone)
            }
            // Nor can a one-pane workspace's only tab move to an edge.
            TabDrop::Edge(_) => !(alone && matches!(self.root, Node::Leaf(_))),
        }
    }

    /// Move tab `id` to `to`, closing the pane it leaves if that empties it.
    /// A new pane, if one is made, gets id `new_pane` under split `split_id`.
    /// Focuses the tab where it lands; false (and no change) unless
    /// [`accepts`](Self::accepts).
    pub fn move_tab(
        &mut self,
        id: EntityId,
        to: TabDrop,
        new_pane: usize,
        split_id: usize,
    ) -> bool {
        if !self.accepts(id, to) {
            return false;
        }
        let (from, idx, count) = self.find_tab(id).unwrap();
        let alone = count == 1;

        let src = self.root.pane_mut(from).unwrap();
        let surface = src.tabs.remove(idx);
        if src.active > idx || src.active >= src.tabs.len() {
            src.active = src.active.saturating_sub(1);
        }
        let dest = match to {
            TabDrop::Tabs { pane, index } => {
                let p = self.root.pane_mut(pane).unwrap();
                // The tab's own removal shifts later slots in its pane.
                let at = if pane == from && index > idx {
                    index - 1
                } else {
                    index
                }
                .min(p.tabs.len());
                p.tabs.insert(at, surface);
                p.active = at;
                pane
            }
            TabDrop::Split { pane, dir } => {
                let new = Pane {
                    id: new_pane,
                    tabs: vec![surface],
                    active: 0,
                };
                let before = matches!(dir, Dir::Left | Dir::Up);
                assert!(
                    self.root
                        .split(pane, dir.axis(), before, new, split_id)
                        .is_ok(),
                    "pane checked above"
                );
                new_pane
            }
            TabDrop::Edge(dir) => {
                self.root.wrap(
                    dir,
                    Pane {
                        id: new_pane,
                        tabs: vec![surface],
                        active: 0,
                    },
                    split_id,
                );
                new_pane
            }
        };
        if alone {
            // Not the root leaf: the drop always leaves another pane.
            self.root.remove(from);
        }
        self.focused_pane = dest;
        true
    }

    /// cwd of the focused terminal, or of any terminal in the workspace.
    pub fn cwd(&self, cx: &App) -> Option<String> {
        self.focused_surface().and_then(|s| s.cwd(cx)).or_else(|| {
            self.root
                .panes()
                .iter()
                .flat_map(|p| p.tabs.iter())
                .find_map(|s| s.cwd(cx))
        })
    }

    /// Claude Code is working in one of its terminals.
    pub fn claude_running(&self, cx: &App) -> bool {
        self.root
            .panes()
            .iter()
            .flat_map(|p| p.tabs.iter())
            .any(|s| s.claude_running(cx))
    }

    pub fn name(&self, cx: &App) -> String {
        match self.custom_name.as_ref().or(self.auto_title.as_ref()) {
            Some(n) => n.clone(),
            None => self.auto_name(cx),
        }
    }

    /// Name when not renamed: the title a program (e.g. an agent) set on the
    /// focused terminal, or any terminal, else the cwd's folder name.
    pub fn auto_name(&self, cx: &App) -> String {
        let title = |s: &Surface| match s {
            Surface::Terminal(t) => {
                Some(t.read(cx).shown_title().trim().to_string()).filter(|t| !t.is_empty())
            }
            Surface::Browser(_) => None,
        };
        if let Some(t) = self.focused_surface().and_then(title).or_else(|| {
            self.root
                .panes()
                .iter()
                .flat_map(|p| p.tabs.iter())
                .find_map(title)
        }) {
            return t;
        }
        self.cwd(cx)
            .map(|c| {
                let t = tilde(&c);
                if t == "~" {
                    "Home".into()
                } else {
                    c.rsplit('/')
                        .find(|s| !s.is_empty())
                        .unwrap_or("/")
                        .to_string()
                }
            })
            .unwrap_or_else(|| format!("Workspace {}", self.id))
    }
}

/// Workspaces in a sidebar section: a header named `name` above the
/// workspaces whose `group` is `id`, which can be folded away.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: usize,
    pub name: String,
    pub color: Option<u32>,
    pub collapsed: bool,
}

/// Put each group's workspaces right after its first one, keeping every
/// other order as it was, and drop groups nobody is in.
pub fn tidy_groups(ws: &mut Vec<Workspace>, groups: &mut Vec<Group>) {
    for w in ws.iter_mut() {
        if w.group.is_some_and(|g| !groups.iter().any(|x| x.id == g)) {
            w.group = None;
        }
    }
    let mut out: Vec<Workspace> = Vec::with_capacity(ws.len());
    let mut rest: Vec<Option<Workspace>> = ws.drain(..).map(Some).collect();
    for i in 0..rest.len() {
        let Some(w) = rest[i].take() else { continue };
        let g = w.group;
        out.push(w);
        if let Some(g) = g {
            for later in rest[i + 1..].iter_mut() {
                if later.as_ref().is_some_and(|l| l.group == Some(g)) {
                    out.push(later.take().unwrap());
                }
            }
        }
    }
    *ws = out;
    groups.retain(|g| ws.iter().any(|w| w.group == Some(g.id)));
}

/// The indexes `[start, end)` of the workspaces in group `group`
/// (contiguous after [`tidy_groups`]).
pub fn group_span(ws: &[Workspace], group: usize) -> Option<(usize, usize)> {
    let start = ws.iter().position(|w| w.group == Some(group))?;
    let len = ws[start..]
        .iter()
        .take_while(|w| w.group == Some(group))
        .count();
    Some((start, start + len))
}

/// Move workspace `from` so it sits at `to` (an index into the list
/// without it), in group `group`. Returns its new index.
pub fn move_workspace(
    ws: &mut Vec<Workspace>,
    groups: &mut Vec<Group>,
    from: usize,
    to: usize,
    group: Option<usize>,
) -> usize {
    let mut w = ws.remove(from);
    w.group = group;
    let id = w.id;
    ws.insert(to.min(ws.len()), w);
    tidy_groups(ws, groups);
    ws.iter().position(|w| w.id == id).expect("still there")
}

pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// Current git branch for a directory (walks up to the repo root).
pub fn git_branch(dir: &str) -> Option<String> {
    let mut p = std::path::PathBuf::from(dir);
    loop {
        let dot = p.join(".git");
        if dot.is_dir() {
            return read_head(&dot.join("HEAD"));
        }
        if dot.is_file() {
            let s = std::fs::read_to_string(&dot).ok()?;
            let gitdir = s.strip_prefix("gitdir:")?.trim();
            return read_head(&p.join(gitdir).join("HEAD"));
        }
        if !p.pop() {
            return None;
        }
    }
}

fn read_head(path: &std::path::Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim();
    Some(match s.strip_prefix("ref: refs/heads/") {
        Some(b) => b.to_string(),
        None => s.chars().take(7).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: usize) -> Pane {
        Pane {
            id,
            tabs: vec![],
            active: 0,
        }
    }
    fn ids(n: &Node) -> Vec<usize> {
        n.panes().iter().map(|p| p.id).collect()
    }

    #[test]
    fn split_and_remove_collapse_the_tree() {
        let mut root = Node::Leaf(pane(1));
        assert!(root.split(1, Axis::Horizontal, false, pane(2), 100).is_ok());
        assert!(root.split(1, Axis::Vertical, false, pane(3), 101).is_ok());
        assert_eq!(ids(&root), vec![1, 3, 2]);
        assert!(root.split(9, Axis::Vertical, false, pane(4), 102).is_err());

        *root.split_mut(100).unwrap() = 0.3;
        assert!(root.remove(1));
        assert_eq!(ids(&root), vec![3, 2]);
        assert_eq!(root.split_mut(100).copied(), Some(0.3));
        assert!(root.remove(2));
        assert!(matches!(root, Node::Leaf(Pane { id: 3, .. })));
        assert!(!root.remove(3), "root leaf is never removed");

        assert!(root.split(3, Axis::Horizontal, true, pane(5), 103).is_ok());
        assert_eq!(
            ids(&root),
            vec![5, 3],
            "left/up splits put the new pane first"
        );
    }

    #[test]
    fn wrap_splits_the_whole_tree() {
        let mut root = Node::Leaf(pane(1));
        assert!(root.split(1, Axis::Horizontal, false, pane(2), 100).is_ok());
        root.wrap(Dir::Down, pane(3), 101);
        assert_eq!(ids(&root), vec![1, 2, 3]);
        // ┌───┬───┐
        // │ 1 │ 2 │
        // ├───┴───┤
        // │   3   │
        // └───────┘
        assert_eq!(root.neighbor(1, Dir::Down), Some(3));
        assert_eq!(root.neighbor(2, Dir::Down), Some(3));
        assert_eq!(root.split_along(3, Axis::Vertical), Some(101));

        root.wrap(Dir::Left, pane(4), 102);
        assert_eq!(
            ids(&root),
            vec![4, 1, 2, 3],
            "left/up wraps put the new pane first"
        );
        assert_eq!(root.neighbor(3, Dir::Left), Some(4));
    }

    #[test]
    fn swap_and_last_pane() {
        let mut ws = Workspace::new(0, None, Node::Leaf(pane(1)), 1);
        assert!(
            ws.root
                .split(1, Axis::Horizontal, false, pane(2), 100)
                .is_ok()
        );
        // `active` stands in for the tabs, which need a window to make.
        ws.root.pane_mut(1).unwrap().active = 7;
        assert!(!ws.swap_pane(1, Dir::Left), "nothing left of pane 1");
        assert!(ws.swap_pane(1, Dir::Right));
        assert_eq!(ws.root.pane_mut(2).unwrap().active, 7);
        assert_eq!(ws.root.pane_mut(1).unwrap().active, 0);
        assert_eq!(ws.focused_pane, 2, "focus follows the swapped pane");

        assert_eq!(ws.last_pane(), Some(1));
        ws.focused_pane = 1;
        assert_eq!(ws.last_pane(), Some(2));
        assert!(ws.root.remove(2));
        assert_eq!(ws.last_pane(), None, "a closed pane isn't last");
    }

    #[test]
    fn zoom_follows_focus() {
        let mut ws = Workspace::new(0, None, Node::Leaf(pane(1)), 1);
        assert!(!ws.toggle_zoom(), "a lone pane has nothing to zoom");
        assert!(
            ws.root
                .split(1, Axis::Horizontal, false, pane(2), 100)
                .is_ok()
        );
        assert!(ws.toggle_zoom());
        assert_eq!(ws.zoomed(), Some(1));
        assert!(ws.shows(1) && !ws.shows(2));
        assert!(ws.toggle_zoom());
        assert_eq!(ws.zoomed(), None);
        assert!(ws.shows(2));

        ws.toggle_zoom();
        ws.focused_pane = 2;
        assert_eq!(ws.zoomed(), None, "focusing another pane unzooms");
        ws.focused_pane = 1;
        assert_eq!(ws.zoomed(), None, "and coming back doesn't rezoom");

        ws.toggle_zoom();
        ws.root.remove(2);
        assert_eq!(ws.zoomed(), None, "the last pane left isn't zoomed");
    }

    fn wss(groups: &[Option<usize>]) -> Vec<Workspace> {
        groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut w = Workspace::new(i, None, Node::Leaf(pane(i)), i);
                w.group = *g;
                w
            })
            .collect()
    }
    fn order(ws: &[Workspace]) -> Vec<(usize, Option<usize>)> {
        ws.iter().map(|w| (w.id, w.group)).collect()
    }
    fn group(id: usize) -> Group {
        Group {
            id,
            name: format!("g{id}"),
            color: None,
            collapsed: false,
        }
    }

    #[test]
    fn groups_stay_together() {
        let mut groups = vec![group(7), group(8)];
        let mut ws = wss(&[Some(7), None, Some(7), Some(9)]);
        tidy_groups(&mut ws, &mut groups);
        assert_eq!(
            order(&ws),
            [(0, Some(7)), (2, Some(7)), (1, None), (3, None)],
            "members follow the first; an unknown group is dropped"
        );
        assert_eq!(groups, [group(7)], "and so is one nobody's in");
        assert_eq!(group_span(&ws, 7), Some((0, 2)));
        assert_eq!(group_span(&ws, 8), None);
    }

    #[test]
    fn moving_in_and_out_of_groups() {
        let mut groups = vec![group(7)];
        let mut ws = wss(&[None, Some(7), Some(7), None]);
        // Into the group, at its end.
        assert_eq!(move_workspace(&mut ws, &mut groups, 3, 3, Some(7)), 3);
        assert_eq!(
            order(&ws),
            [(0, None), (1, Some(7)), (2, Some(7)), (3, Some(7))]
        );
        // Out of it, above it.
        assert_eq!(move_workspace(&mut ws, &mut groups, 2, 0, None), 0);
        assert_eq!(
            order(&ws),
            [(2, None), (0, None), (1, Some(7)), (3, Some(7))]
        );
        // A grouped one dropped among ungrouped ones is pulled back to its group.
        assert_eq!(move_workspace(&mut ws, &mut groups, 3, 0, Some(7)), 0);
        assert_eq!(
            order(&ws),
            [(3, Some(7)), (1, Some(7)), (2, None), (0, None)]
        );
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff9500"), Some(0xff9500));
        assert_eq!(parse_color("0a0"), Some(0x00aa00));
        assert_eq!(parse_color("#ff95"), None);
        assert_eq!(parse_color("orange"), None);
    }

    #[test]
    fn neighbor_follows_the_layout() {
        // ┌───┬───┐
        // │   │ 2 │
        // │ 1 ├─┬─┤
        // │   │3│4│
        // └───┴─┴─┘
        let mut root = Node::Leaf(pane(1));
        assert!(root.split(1, Axis::Horizontal, false, pane(2), 100).is_ok());
        assert!(root.split(2, Axis::Vertical, false, pane(3), 101).is_ok());
        assert!(root.split(3, Axis::Horizontal, false, pane(4), 102).is_ok());
        *root.split_mut(101).unwrap() = 0.3;

        assert_eq!(
            root.neighbor(1, Dir::Right),
            Some(3),
            "more overlap with the bigger lower half"
        );
        assert_eq!(root.neighbor(1, Dir::Left), None);
        assert_eq!(root.neighbor(2, Dir::Left), Some(1));
        assert_eq!(root.neighbor(2, Dir::Down), Some(3));
        assert_eq!(root.neighbor(3, Dir::Up), Some(2));
        assert_eq!(root.neighbor(4, Dir::Up), Some(2));
        assert_eq!(
            root.neighbor(4, Dir::Left),
            Some(3),
            "nearest wins over farther pane 1"
        );
        assert_eq!(root.neighbor(3, Dir::Left), Some(1));
        assert_eq!(root.neighbor(3, Dir::Down), None);
        assert_eq!(root.neighbor(9, Dir::Up), None);

        assert_eq!(
            root.split_along(4, Axis::Horizontal),
            Some(102),
            "innermost split wins"
        );
        assert_eq!(root.split_along(2, Axis::Horizontal), Some(100));
        assert_eq!(root.split_along(3, Axis::Vertical), Some(101));
        assert_eq!(root.split_along(1, Axis::Vertical), None);
        assert_eq!(root.split_along(9, Axis::Horizontal), None);
    }
}
