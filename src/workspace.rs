//! Workspace model: each workspace owns a tree of split panes; each pane
//! holds tabs ("surfaces") that are either terminals or browsers.

use crate::{browser::BrowserView, terminal::TerminalView};
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

    pub fn icon(&self) -> &'static str {
        match self {
            Surface::Terminal(_) => "terminal",
            Surface::Browser(_) => "globe",
        }
    }

    pub fn title(&self, cx: &App) -> String {
        match self {
            Surface::Terminal(t) => {
                let t = t.read(cx);
                if !t.title.is_empty() { t.title.clone() } else { tilde(&t.cwd) }
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
    Split { id: usize, axis: Axis, ratio: f32, children: Box<[Node; 2]> },
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
            Node::Split { id: sid, ratio, children, .. } => {
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
    pub fn split(&mut self, target: usize, axis: Axis, before: bool, new: Pane, split_id: usize) -> Result<(), Pane> {
        match self {
            Node::Leaf(p) if p.id == target => {
                let old = std::mem::replace(self, Node::Leaf(dummy()));
                let children = if before { [Node::Leaf(new), old] } else { [old, Node::Leaf(new)] };
                *self = Node::Split { id: split_id, axis, ratio: 0.5, children: Box::new(children) };
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
        let Node::Split { children, .. } = self else { return false };
        let hit = children.iter().position(|c| matches!(c, Node::Leaf(p) if p.id == target));
        if let Some(i) = hit {
            let [a, b] = *std::mem::replace(children, Box::new([Node::Leaf(dummy()), Node::Leaf(dummy())]));
            *self = if i == 0 { b } else { a };
            return true;
        }
        children.iter_mut().any(|c| c.remove(target))
    }

    /// Each pane's rect `[x0, y0, x1, y1]` inside `r`, from the split ratios.
    fn layout(&self, r: [f32; 4], out: &mut Vec<(usize, [f32; 4])>) {
        match self {
            Node::Leaf(p) => out.push((p.id, r)),
            Node::Split { axis, ratio, children, .. } => {
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
            if (a.1 - b.1).abs() > EPS { a.1.total_cmp(&b.1) } else { b.2.total_cmp(&a.2) }
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

fn dummy() -> Pane {
    Pane { id: usize::MAX, tabs: vec![], active: 0 }
}

pub struct Workspace {
    pub id: usize,
    pub custom_name: Option<String>,
    pub root: Node,
    pub focused_pane: usize,
}

impl Workspace {
    pub fn focused(&self) -> Option<&Pane> {
        self.root.panes().into_iter().find(|p| p.id == self.focused_pane)
    }

    pub fn focused_surface(&self) -> Option<&Surface> {
        self.focused().and_then(|p| p.active_surface())
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.root.panes().iter().any(|p| p.tabs.iter().any(|s| s.entity_id() == id))
    }

    /// cwd of the focused terminal, or of any terminal in the workspace.
    pub fn cwd(&self, cx: &App) -> Option<String> {
        self.focused_surface()
            .and_then(|s| s.cwd(cx))
            .or_else(|| self.root.panes().iter().flat_map(|p| p.tabs.iter()).find_map(|s| s.cwd(cx)))
    }

    pub fn name(&self, cx: &App) -> String {
        match &self.custom_name {
            Some(n) => n.clone(),
            None => self.auto_name(cx),
        }
    }

    /// Name when not renamed: the title a program (e.g. an agent) set on the
    /// focused terminal, or any terminal, else the cwd's folder name.
    pub fn auto_name(&self, cx: &App) -> String {
        let title = |s: &Surface| match s {
            Surface::Terminal(t) => Some(t.read(cx).title.trim().to_string()).filter(|t| !t.is_empty()),
            Surface::Browser(_) => None,
        };
        if let Some(t) = self
            .focused_surface()
            .and_then(title)
            .or_else(|| self.root.panes().iter().flat_map(|p| p.tabs.iter()).find_map(title))
        {
            return t;
        }
        self.cwd(cx)
            .map(|c| {
                let t = tilde(&c);
                if t == "~" { "Home".into() } else { c.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/").to_string() }
            })
            .unwrap_or_else(|| format!("Workspace {}", self.id))
    }
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
        Pane { id, tabs: vec![], active: 0 }
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
        assert_eq!(ids(&root), vec![5, 3], "left/up splits put the new pane first");
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

        assert_eq!(root.neighbor(1, Dir::Right), Some(3), "more overlap with the bigger lower half");
        assert_eq!(root.neighbor(1, Dir::Left), None);
        assert_eq!(root.neighbor(2, Dir::Left), Some(1));
        assert_eq!(root.neighbor(2, Dir::Down), Some(3));
        assert_eq!(root.neighbor(3, Dir::Up), Some(2));
        assert_eq!(root.neighbor(4, Dir::Up), Some(2));
        assert_eq!(root.neighbor(4, Dir::Left), Some(3), "nearest wins over farther pane 1");
        assert_eq!(root.neighbor(3, Dir::Left), Some(1));
        assert_eq!(root.neighbor(3, Dir::Down), None);
        assert_eq!(root.neighbor(9, Dir::Up), None);
    }
}
