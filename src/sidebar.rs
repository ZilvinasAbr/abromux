//! The sidebar: a row per workspace under group headers, in the order
//! you've dragged them into, each with its name, description, color, latest
//! notification, status and branch. Right-click a row or header for its
//! menu. What a row shows follows the config file's `sidebar` keys
//! (`settings::SidebarSettings`), cmux's names for the same things.

use crate::{
    AbroMux, Hints, SIDEBAR_WIDTH, TITLEBAR_H, hint_pill, one_line,
    settings::{IndicatorStyle, Placement, Settings},
    shortcut_digit, terminal,
    theme::*,
    titlebar,
    workspace::{self, Group, Workspace, group_span, tilde},
};
use gpui::{prelude::*, *};
use gpui_component::{
    input::{Input, InputEvent, InputState, SelectAll},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
};

/// How far a group's workspaces sit in from the sidebar's edge.
const GROUP_INDENT: f32 = 18.;
/// The most description lines a row shows.
const DESCRIPTION_LINES: usize = 6;

/// A workspace row being dragged.
#[derive(Clone)]
pub struct DraggedWorkspace {
    id: usize,
    name: SharedString,
}

/// A group header being dragged, which moves the whole group.
#[derive(Clone)]
pub struct DraggedGroup {
    id: usize,
    name: SharedString,
}

impl Render for DraggedWorkspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone())
    }
}

impl Render for DraggedGroup {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone())
    }
}

fn drag_chip(name: SharedString) -> Div {
    div()
        .w(px(SIDEBAR_WIDTH - 16.))
        .px(px(10.))
        .py(px(7.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(ACCENT))
        .bg(rgb(PANE_BG))
        .opacity(0.9)
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT))
        .font_family(".AppleSystemUIFont")
        .truncate()
        .child(name)
}

/// `path` as the sidebar shows it: `~/code/api`, or `…/api` with
/// `pathLastSegmentOnly`.
fn short_path(path: &str, last_only: bool) -> String {
    let t = tilde(path);
    if !last_only || t == "~" || t == "/" {
        return t;
    }
    match t.trim_end_matches('/').rsplit_once('/') {
        Some((head, last)) if !head.is_empty() && head != "~" => format!("…/{last}"),
        _ => t,
    }
}

/// The folder a workspace is grouped by: its repository's root, else its
/// directory.
fn group_key(cwd: &str) -> String {
    // Components drop a trailing slash, so `a/` and `a` match.
    let cwd: std::path::PathBuf = std::path::Path::new(cwd).components().collect();
    let cwd = cwd.to_string_lossy().into_owned();
    let mut p = std::path::PathBuf::from(&cwd);
    loop {
        if p.join(".git").exists() {
            return p.to_string_lossy().into_owned();
        }
        if !p.pop() {
            return cwd;
        }
    }
}

fn folder_name(path: &str) -> String {
    if tilde(path) == "~" {
        return "Home".into();
    }
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("/")
        .to_string()
}

impl AbroMux {
    pub(crate) fn ws_index(&self, id: usize) -> Option<usize> {
        self.workspaces.iter().position(|w| w.id == id)
    }

    /// Run `f` on the workspaces and groups, keeping the same workspace
    /// selected however the order changes.
    pub(crate) fn rearrange<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Vec<Workspace>, &mut Vec<Group>) -> R,
    ) -> R {
        let active = self.workspaces.get(self.active).map(|w| w.id);
        let r = f(&mut self.workspaces, &mut self.groups);
        workspace::tidy_groups(&mut self.workspaces, &mut self.groups);
        if let Some(i) = active.and_then(|id| self.ws_index(id)) {
            self.active = i;
        }
        cx.notify();
        r
    }

    /// Apply `f` to the order, as [`rearrange`](Self::rearrange) does, and
    /// return the workspace ids in their new order. With `dry`, the order
    /// is put back afterwards, so this only tells what `f` would do.
    pub(crate) fn reorder(
        &mut self,
        dry: bool,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Vec<Workspace>, &mut Vec<Group>),
    ) -> Vec<usize> {
        let before: Vec<(usize, Option<usize>)> =
            self.workspaces.iter().map(|w| (w.id, w.group)).collect();
        let groups = self.groups.clone();
        self.rearrange(cx, f);
        let after = self.workspaces.iter().map(|w| w.id).collect();
        if dry {
            self.rearrange(cx, |ws, gs| {
                ws.sort_by_key(|w| before.iter().position(|(id, _)| *id == w.id));
                for (w, (_, g)) in ws.iter_mut().zip(&before) {
                    w.group = *g;
                }
                *gs = groups;
            });
        }
        after
    }

    /// Where a new workspace goes and the group it joins, given the
    /// selected one and the `newWorkspacePlacement` settings.
    pub(crate) fn new_workspace_slot(&self, cx: &App) -> (usize, Option<usize>) {
        let s = cx.global::<Settings>();
        let n = self.workspaces.len();
        let Some(cur) = self.workspaces.get(self.active) else {
            return (n, None);
        };
        if let Some(g) = cur.group
            && let Some((start, end)) = group_span(&self.workspaces, g)
        {
            let at = match s.workspace_groups.new_workspace_placement {
                Placement::Top => start + 1,
                Placement::AfterCurrent => self.active + 1,
                Placement::End => end,
            };
            return (at, Some(g));
        }
        let at = match s.app.new_workspace_placement {
            Placement::Top => 0,
            Placement::AfterCurrent => self.active + 1,
            Placement::End => n,
        };
        (at, None)
    }

    /// Move workspace `from` to just before or `after` workspace `onto`,
    /// into `onto`'s group, except that just above a group's first
    /// workspace is outside the group.
    pub(crate) fn drop_workspace(
        &mut self,
        from: usize,
        onto: usize,
        after: bool,
        cx: &mut Context<Self>,
    ) {
        let n = self.workspaces.len();
        if from >= n || onto >= n {
            return;
        }
        let mut group = self.workspaces[onto].group;
        let first_of_group = onto == 0 || self.workspaces[onto - 1].group != group;
        if !after && first_of_group {
            group = None;
        }
        // The slot in the list without `from`.
        let mut to = onto + after as usize;
        if from < to {
            to -= 1;
        }
        if to == from && group == self.workspaces[from].group {
            return;
        }
        self.rearrange(cx, |ws, groups| {
            workspace::move_workspace(ws, groups, from, to, group)
        });
    }

    /// Put workspace `from` first in group `group` (dropped on its header).
    pub(crate) fn drop_on_group(&mut self, from: usize, group: usize, cx: &mut Context<Self>) {
        let Some((start, _)) = group_span(&self.workspaces, group) else {
            return;
        };
        let to = if from < start { start - 1 } else { start };
        self.rearrange(cx, |ws, groups| {
            workspace::move_workspace(ws, groups, from, to, Some(group))
        });
    }

    /// Move group `group` as a block to where workspace `onto` (or the group
    /// it's in) is.
    pub(crate) fn drop_group(&mut self, group: usize, onto: usize, cx: &mut Context<Self>) {
        let Some((start, end)) = group_span(&self.workspaces, group) else {
            return;
        };
        if (start..end).contains(&onto) {
            return;
        }
        // Land beside the target's whole group, never inside it.
        let (t0, t1) = match self.workspaces[onto].group {
            Some(g) => group_span(&self.workspaces, g).unwrap_or((onto, onto + 1)),
            None => (onto, onto + 1),
        };
        self.rearrange(cx, |ws, _| {
            let block: Vec<Workspace> = ws.drain(start..end).collect();
            let at = if onto > start { t1 - block.len() } else { t0 };
            ws.splice(at..at, block);
        });
    }

    /// Move a workspace one place up or down. Inside a group it stays in the
    /// group until it reaches the edge, then steps out; outside one it hops
    /// over a whole group.
    pub(crate) fn nudge_workspace(&mut self, index: usize, down: bool, cx: &mut Context<Self>) {
        let n = self.workspaces.len();
        if index >= n {
            return;
        }
        let group = self.workspaces[index].group;
        let next = if down {
            index + 1
        } else {
            index.wrapping_sub(1)
        };
        let neighbor = self.workspaces.get(next).map(|w| w.group);
        let (to, group) = match (group, neighbor) {
            (None, None) => return,
            // At the end of the list: a grouped one just leaves its group.
            (Some(_), None) => (index, None),
            (Some(g), Some(ng)) if ng == Some(g) => (next, group),
            (Some(_), Some(_)) => (index, None),
            (None, Some(Some(g))) => {
                let (start, end) = group_span(&self.workspaces, g).expect("in it");
                if down { (end - 1, None) } else { (start, None) }
            }
            (None, Some(None)) => (next, None),
        };
        self.rearrange(cx, |ws, groups| {
            workspace::move_workspace(ws, groups, index, to, group)
        });
    }

    pub(crate) fn workspace_to_top(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(w) = self.workspaces.get(index) else {
            return;
        };
        let group = w.group;
        let to = group
            .and_then(|g| group_span(&self.workspaces, g))
            .map_or(0, |(start, _)| start);
        self.rearrange(cx, |ws, groups| {
            workspace::move_workspace(ws, groups, index, to, group)
        });
    }

    /// A new group holding workspace `index`, named after its folder.
    pub(crate) fn new_group(&mut self, index: usize, cx: &mut Context<Self>) -> Option<usize> {
        let w = self.workspaces.get(index)?;
        let name = w
            .cwd(cx)
            .map_or_else(|| w.name(cx), |c| folder_name(&group_key(&c)));
        self.next_group += 1;
        let id = self.next_group;
        self.groups.push(Group {
            id,
            name,
            color: None,
            collapsed: false,
        });
        self.workspaces[index].group = Some(id);
        self.rearrange(cx, |_, _| ());
        Some(id)
    }

    /// Put workspace `index` last in group `group`, or out of any with None.
    pub(crate) fn set_group(&mut self, index: usize, group: Option<usize>, cx: &mut Context<Self>) {
        if index >= self.workspaces.len() || self.workspaces[index].group == group {
            return;
        }
        let to = match group.and_then(|g| group_span(&self.workspaces, g)) {
            Some((_, end)) if index < end => end - 1,
            Some((_, end)) => end,
            // Leaving a group: just past its end.
            None => match self.workspaces[index].group {
                Some(g) => group_span(&self.workspaces, g).map_or(index, |(_, e)| e - 1),
                None => index,
            },
        };
        self.rearrange(cx, |ws, groups| {
            workspace::move_workspace(ws, groups, index, to, group)
        });
    }

    pub(crate) fn ungroup(&mut self, group: usize, cx: &mut Context<Self>) {
        self.rearrange(cx, |ws, _| {
            for w in ws.iter_mut().filter(|w| w.group == Some(group)) {
                w.group = None;
            }
        });
    }

    /// Group the workspaces that aren't in one by repository (or folder),
    /// where two or more share it. Returns how many groups it made.
    pub(crate) fn group_by_folder(&mut self, cx: &mut Context<Self>) -> usize {
        let keys: Vec<Option<String>> = self
            .workspaces
            .iter()
            .map(|w| {
                w.group
                    .is_none()
                    .then(|| w.cwd(cx).map(|c| group_key(&c)))
                    .flatten()
            })
            .collect();
        let mut made = 0;
        let mut seen: Vec<&String> = vec![];
        for k in keys.iter().flatten() {
            if seen.contains(&k) || keys.iter().filter(|o| o.as_ref() == Some(k)).count() < 2 {
                continue;
            }
            seen.push(k);
            self.next_group += 1;
            let id = self.next_group;
            self.groups.push(Group {
                id,
                name: folder_name(k),
                color: None,
                collapsed: false,
            });
            for (w, wk) in self.workspaces.iter_mut().zip(&keys) {
                if wk.as_ref() == Some(k) {
                    w.group = Some(id);
                }
            }
            made += 1;
        }
        self.rearrange(cx, |_, _| ());
        made
    }

    pub(crate) fn toggle_group(&mut self, group: usize, cx: &mut Context<Self>) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group) {
            g.collapsed = !g.collapsed;
            cx.notify();
        }
    }

    /// A new workspace in group `group`, in its first workspace's folder.
    fn new_workspace_in_group(
        &mut self,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((start, _)) = group_span(&self.workspaces, group) else {
            return;
        };
        let cwd = self.workspaces[start].cwd(cx);
        // Selecting one of its workspaces makes the new one land in the group.
        self.active = start;
        self.new_workspace(cwd, window, cx);
    }

    /// Edit workspace `index`'s description under its name (⌥⌘E). Return
    /// saves, ⇧↩ (which the field doesn't bind) starts a new line, an empty one
    /// clears it.
    pub(crate) fn start_describe(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(index) else {
            return;
        };
        let (id, text) = (ws.id, ws.description.clone().unwrap_or_default());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(1, DESCRIPTION_LINES)
                .placeholder("Description")
                .default_value(text)
        });
        cx.subscribe_in(
            &input,
            window,
            move |this, input, ev: &InputEvent, window, cx| match ev {
                // Return reaches the field's own binding first, which has
                // already put a newline at the cursor: take it out and save.
                InputEvent::PressEnter { secondary: false } => {
                    let (text, at) = {
                        let i = input.read(cx);
                        (i.value().to_string(), i.cursor())
                    };
                    let text = match at.checked_sub(1) {
                        Some(nl) if text.get(nl..at) == Some("\n") => {
                            format!("{}{}", &text[..nl], &text[at..])
                        }
                        _ => text,
                    };
                    this.finish_describe(Some(text), window, cx);
                }
                InputEvent::Blur => {
                    this.describing = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.on_next_frame(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        self.describing = Some((id, input));
        cx.notify();
    }

    /// Close the description editor, saving `text` if given.
    fn finish_describe(
        &mut self,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, _)) = self.describing.take() else {
            return;
        };
        if let (Some(text), Some(i)) = (text, self.ws_index(id)) {
            self.workspaces[i].description = clean_description(&text);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Rename group `group` in its header.
    fn start_group_rename(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.groups.iter().find(|g| g.id == group) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(g.name.clone()));
        cx.subscribe_in(
            &input,
            window,
            move |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let name = one_line(&input.read(cx).value());
                    if let Some(g) = this.groups.iter_mut().find(|g| g.id == group)
                        && !name.is_empty()
                    {
                        g.name = name;
                    }
                    this.renaming_group = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.renaming_group = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.on_next_frame(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        self.renaming_group = Some((group, input));
        cx.notify();
    }

    // ---- menus ----------------------------------------------------------

    fn color_menu(
        menu: PopupMenu,
        current: Option<u32>,
        set: impl Fn(Option<u32>, &mut Window, &mut App) + Clone + 'static,
        cx: &App,
    ) -> PopupMenu {
        let colors = cx.global::<Settings>().workspace_colors.colors.clone();
        let mut menu = menu;
        for (name, c) in colors {
            let set = set.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().size(px(10.)).rounded_full().bg(rgb(c)))
                        .child(name.clone())
                })
                .checked(current == Some(c))
                .on_click(move |_, window, cx| set(Some(c), window, cx)),
            );
        }
        menu.separator().item(
            PopupMenuItem::new("No Color")
                .disabled(current.is_none())
                .on_click(move |_, window, cx| set(None, window, cx)),
        )
    }

    fn workspace_menu(
        this: WeakEntity<Self>,
        id: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(app) = this.upgrade() else {
            return menu;
        };
        let app = app.read(cx);
        let Some(index) = app.ws_index(id) else {
            return menu;
        };
        let ws = &app.workspaces[index];
        let (color, group, described) = (ws.color, ws.group, ws.description.is_some());
        let n = app.workspaces.len();
        let groups: Vec<(usize, String)> =
            app.groups.iter().map(|g| (g.id, g.name.clone())).collect();
        // Every action looks the workspace up again by id: the list may
        // have changed since the menu opened.
        let act = |f: fn(&mut AbroMux, usize, &mut Window, &mut Context<AbroMux>)| {
            let this = this.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    if let Some(i) = this.ws_index(id) {
                        f(this, i, window, cx);
                    }
                })
                .ok();
            }
        };
        let set_color = {
            let this = this.clone();
            move |c: Option<u32>, _: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    if let Some(i) = this.ws_index(id) {
                        this.workspaces[i].color = c;
                        cx.notify();
                    }
                })
                .ok();
            }
        };
        let groups_menu = {
            let this = this.clone();
            move |mut menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let join = |g: Option<usize>| {
                    let this = this.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| {
                            if let Some(i) = this.ws_index(id) {
                                this.set_group(i, g, cx);
                            }
                        })
                        .ok();
                    }
                };
                let make = {
                    let this = this.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| {
                            if let Some(i) = this.ws_index(id) {
                                this.new_group(i, cx);
                            }
                        })
                        .ok();
                    }
                };
                menu = menu.item(PopupMenuItem::new("New Group").on_click(make));
                if !groups.is_empty() {
                    menu = menu.separator();
                }
                for (gid, name) in &groups {
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(group == Some(*gid))
                            .on_click(join(Some(*gid))),
                    );
                }
                if group.is_some() {
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Remove from Group").on_click(join(None)));
                }
                menu
            }
        };
        menu.item(
            PopupMenuItem::new("Rename…").on_click(act(|t, i, w, cx| t.start_rename(i, w, cx))),
        )
        .item(
            PopupMenuItem::new(if described {
                "Edit Description…"
            } else {
                "Add Description…"
            })
            .on_click(act(|t, i, w, cx| t.start_describe(i, w, cx))),
        )
        .when(described, |m| {
            m.item(
                PopupMenuItem::new("Clear Description").on_click(act(|t, i, _, cx| {
                    t.workspaces[i].description = None;
                    cx.notify();
                })),
            )
        })
        .submenu("Color", window, cx, move |m, _, cx| {
            Self::color_menu(m, color, set_color.clone(), cx)
        })
        .submenu("Group", window, cx, groups_menu)
        .separator()
        .item(
            PopupMenuItem::new("Move Up")
                .disabled(index == 0)
                .on_click(act(|t, i, _, cx| t.nudge_workspace(i, false, cx))),
        )
        .item(
            PopupMenuItem::new("Move Down")
                .disabled(index + 1 == n)
                .on_click(act(|t, i, _, cx| t.nudge_workspace(i, true, cx))),
        )
        .item(
            PopupMenuItem::new("Move to Top")
                .disabled(index == 0)
                .on_click(act(|t, i, _, cx| t.workspace_to_top(i, cx))),
        )
        .separator()
        .item(
            PopupMenuItem::new("Close Workspace")
                .on_click(act(|t, i, w, cx| t.close_workspace(i, w, cx))),
        )
        .item(
            PopupMenuItem::new("Close Other Workspaces")
                .disabled(n == 1)
                .on_click(act(|t, i, w, cx| t.close_others(i, w, cx))),
        )
    }

    fn group_menu(
        this: WeakEntity<Self>,
        gid: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(app) = this.upgrade() else {
            return menu;
        };
        let Some(g) = app.read(cx).groups.iter().find(|g| g.id == gid) else {
            return menu;
        };
        let (color, collapsed) = (g.color, g.collapsed);
        let act = |f: fn(&mut AbroMux, usize, &mut Window, &mut Context<AbroMux>)| {
            let this = this.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| f(this, gid, window, cx)).ok();
            }
        };
        let set_color = {
            let this = this.clone();
            move |c: Option<u32>, _: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    if let Some(g) = this.groups.iter_mut().find(|g| g.id == gid) {
                        g.color = c;
                        cx.notify();
                    }
                })
                .ok();
            }
        };
        menu.item(
            PopupMenuItem::new("New Workspace in Group")
                .on_click(act(|t, g, w, cx| t.new_workspace_in_group(g, w, cx))),
        )
        .item(
            PopupMenuItem::new("Rename Group…")
                .on_click(act(|t, g, w, cx| t.start_group_rename(g, w, cx))),
        )
        .submenu("Color", window, cx, move |m, _, cx| {
            Self::color_menu(m, color, set_color.clone(), cx)
        })
        .item(
            PopupMenuItem::new(if collapsed { "Expand" } else { "Collapse" })
                .on_click(act(|t, g, _, cx| t.toggle_group(g, cx))),
        )
        .separator()
        .item(PopupMenuItem::new("Ungroup").on_click(act(|t, g, _, cx| t.ungroup(g, cx))))
        .item(
            PopupMenuItem::new("Close Workspaces in Group")
                .on_click(act(|t, g, w, cx| t.close_group(g, w, cx))),
        )
    }

    /// Close every workspace but `index`.
    pub(crate) fn close_others(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(keep) = self.workspaces.get(index).map(|w| w.id) else {
            return;
        };
        let ids: Vec<usize> = self
            .workspaces
            .iter()
            .map(|w| w.id)
            .filter(|&id| id != keep)
            .collect();
        for id in ids {
            if let Some(i) = self.ws_index(id) {
                self.close_workspace(i, window, cx);
            }
        }
    }

    fn close_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<usize> = self
            .workspaces
            .iter()
            .filter(|w| w.group == Some(group))
            .map(|w| w.id)
            .collect();
        for id in ids {
            if let Some(i) = self.ws_index(id) {
                self.close_workspace(i, window, cx);
            }
        }
    }

    // ---- rendering ------------------------------------------------------

    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = vec![];
        let mut prev_group = None;
        for (i, ws) in self.workspaces.iter().enumerate() {
            if ws.group != prev_group
                && let Some(g) = ws
                    .group
                    .and_then(|g| self.groups.iter().find(|x| x.id == g))
            {
                rows.push(self.render_group_header(g, cx));
            }
            prev_group = ws.group;
            let folded = ws
                .group
                .and_then(|g| self.groups.iter().find(|x| x.id == g))
                .is_some_and(|g| g.collapsed);
            // A folded group still shows its selected workspace.
            if !folded || i == self.active {
                rows.push(self.render_workspace_row(i, ws, cx));
            }
        }

        let s = cx.global::<Settings>();
        let tint = s.sidebar_appearance.tint_color.map(|c| Rgba {
            a: c.a * s.sidebar_appearance.tint_opacity,
            ..c
        });
        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .relative()
            .bg(rgba(SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(BORDER))
            .when_some(tint, |d, t| d.child(div().absolute().inset_0().bg(t)))
            .child(
                // Titlebar row: traffic lights live on the left.
                div()
                    .h(px(TITLEBAR_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .pr(px(10.))
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        titlebar::drag_or_zoom(ev, window)
                    })
                    .children(self.titlebar_buttons(cx)),
            )
            .child(
                div()
                    .id("workspaces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pb_2()
                    .children(rows),
            )
            .child(
                // Drag the right edge to resize; double-click resets.
                div()
                    .id("sidebar-edge")
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            if ev.click_count >= 2 {
                                this.sidebar_width = SIDEBAR_WIDTH;
                                cx.notify();
                            } else {
                                this.resizing_sidebar = true;
                            }
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_group_header(&self, g: &Group, cx: &mut Context<Self>) -> AnyElement {
        let gid = g.id;
        let drop_above = self.ws_drop.is_some_and(|(t, after)| {
            !after
                && cx.has_active_drag()
                && self
                    .workspaces
                    .iter()
                    .find(|w| w.group == Some(gid))
                    .map(|w| w.id)
                    == Some(t)
        });
        let z = cx.global::<Settings>().sidebar.font_size / 12.5;
        let count = self
            .workspaces
            .iter()
            .filter(|w| w.group == Some(gid))
            .count();
        let unread = self
            .workspaces
            .iter()
            .filter(|w| w.group == Some(gid))
            .any(|w| self.ws_unread(w));
        let group: SharedString = format!("group-{gid}").into();
        let this = cx.entity().downgrade();
        let renaming = self
            .renaming_group
            .as_ref()
            .filter(|(id, _)| *id == gid)
            .map(|(_, input)| input.clone());
        div()
            .id(("group", gid))
            .group(group.clone())
            .relative()
            .mx(px(8.))
            .mt(px(6.))
            // Dropping above the group's first workspace lands outside the
            // group, so the line shows above its header.
            .when(drop_above, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .right(px(4.))
                        .top(px(-4.))
                        .h(px(2.))
                        .rounded_full()
                        .bg(rgb(ACCENT_BRIGHT)),
                )
            })
            .px(px(6.))
            .py(px(4.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .gap(px(5.))
            .cursor_pointer()
            .text_size(px(11. * z))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(g.color.map_or(rgb(MUTED), rgb))
            .hover(|s| s.bg(rgba(HOVER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    if ev.click_count >= 2 {
                        this.start_group_rename(gid, window, cx);
                    } else {
                        this.toggle_group(gid, cx);
                    }
                }),
            )
            .on_drag(
                DraggedGroup {
                    id: gid,
                    name: g.name.clone().into(),
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .drag_over::<DraggedWorkspace>(|d, _, _, _| d.bg(rgba(DROP_TINT)))
            .on_drop(cx.listener(move |this, d: &DraggedWorkspace, _, cx| {
                if let Some(from) = this.ws_index(d.id) {
                    this.drop_on_group(from, gid, cx);
                }
            }))
            .drag_over::<DraggedGroup>(|d, _, _, _| d.bg(rgba(DROP_TINT)))
            .on_drop(cx.listener(move |this, d: &DraggedGroup, _, cx| {
                if let Some((start, _)) = group_span(&this.workspaces, gid) {
                    this.drop_group(d.id, start, cx);
                }
            }))
            .child(
                icon(if g.collapsed {
                    "chevron-right"
                } else {
                    "chevron-down"
                })
                .size(px(10. * z))
                .text_color(rgb(MUTED)),
            )
            .child(match renaming {
                Some(input) => div()
                    .flex_1()
                    .min_w_0()
                    .h(px(18.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .bg(rgba(0x0000004d))
                    .border_1()
                    .border_color(rgba(0xffffff59))
                    .text_color(rgb(TEXT))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            this.renaming_group = None;
                            this.focus_active(window, cx);
                            cx.notify();
                        }
                    }))
                    .child(
                        Input::new(&input)
                            .appearance(false)
                            .text_size(px(11. * z))
                            .h_full()
                            .px_0()
                            .py_0(),
                    )
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(g.name.to_uppercase())
                    .into_any_element(),
            })
            .when(g.collapsed && unread, |d| {
                d.child(
                    div()
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(rgb(ACCENT_BRIGHT)),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(MUTED))
                    .font_weight(FontWeight::NORMAL)
                    .child(count.to_string()),
            )
            .child(
                div()
                    .id(("group-new", gid))
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .flex_none()
                    .rounded(px(4.))
                    .p(px(2.))
                    .hover(|s| s.bg(rgba(0xffffff22)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.new_workspace_in_group(gid, window, cx)
                    }))
                    .child(icon("plus").size(px(11. * z)).text_color(rgb(MUTED))),
            )
            .context_menu(move |menu, window, cx| {
                Self::group_menu(this.clone(), gid, menu, window, cx)
            })
            .with_own_id(("group-menu", gid))
    }

    fn render_workspace_row(&self, i: usize, ws: &Workspace, cx: &mut Context<Self>) -> AnyElement {
        let settings = cx.global::<Settings>();
        let sb = settings.sidebar.clone();
        let colors = &settings.workspace_colors;
        let z = sb.font_size / 12.5;
        let details = !sb.hide_all_details;
        let active = i == self.active;
        let name = ws.name(cx);
        let subtitle = ws
            .focused_surface()
            .map(|s| s.title(cx))
            .unwrap_or_default();
        let cwd = ws.cwd(cx);
        let branch = cwd.as_deref().and_then(|c| self.branch(c));
        let path = cwd
            .as_deref()
            .map(|c| short_path(c, sb.path_last_segment_only));
        let meta: Vec<(bool, String)> = match (&branch, &path) {
            (Some(b), Some(p)) if sb.stack_branch_directory => {
                vec![(true, b.clone()), (false, p.clone())]
            }
            (Some(b), Some(p)) => vec![(true, format!("{b} • {p}"))],
            (Some(b), None) => vec![(true, b.clone())],
            (None, Some(p)) => vec![(false, p.clone())],
            (None, None) => vec![],
        };

        // The selected row's fill: the config's, else accent. With
        // `solidFill` a colored workspace is its own color, faded when not
        // selected.
        let selection = sb
            .selection_color
            .or(colors.selection_color)
            .unwrap_or(rgb(ACCENT));
        let solid = colors.indicator_style == IndicatorStyle::SolidFill;
        let bg = match (ws.color, solid, active) {
            (Some(c), true, true) => Some(rgb(c)),
            (Some(c), true, false) => Some(Rgba {
                a: colors.unselected_fill_opacity,
                ..rgb(c)
            }),
            (_, _, true) => Some(selection),
            _ => None,
        };
        let filled = bg.is_some();
        let fg = if filled { rgb(0xffffff) } else { rgb(TEXT) };
        let sub = if filled { rgba(0xffffffc0) } else { rgb(MUTED) };
        let rail = ws.color.filter(|_| !solid);

        let renaming = self
            .renaming
            .as_ref()
            .filter(|(id, _)| *id == ws.id)
            .map(|(_, input)| input.clone());
        let describing = self
            .describing
            .as_ref()
            .filter(|(id, _)| *id == ws.id)
            .map(|(_, input)| input.clone());
        let is_renaming = renaming.is_some();
        let group: SharedString = format!("ws-{}", ws.id).into();
        let hint =
            shortcut_digit(i, self.workspaces.len()).filter(|_| self.hints == Some(Hints::Command));
        let unread = self.latest_unread(ws).map(|n| n.text());
        let latest = unread
            .clone()
            .filter(|_| details && sb.show_notification_message);
        let status = details
            .then(|| self.render_ws_status(ws, filled, &sb))
            .flatten();
        let description: Vec<String> = ws
            .description
            .as_deref()
            .filter(|_| details && sb.show_workspace_description && describing.is_none())
            .map(|d| {
                d.lines()
                    .take(DESCRIPTION_LINES)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let id = ws.id;
        let this = cx.entity().downgrade();
        // Above a group's first workspace, the header draws the line.
        let first_of_group =
            ws.group.is_some() && (i == 0 || self.workspaces[i - 1].group != ws.group);
        let drop_line = self
            .ws_drop
            .filter(|(t, after)| *t == id && cx.has_active_drag() && (*after || !first_of_group))
            .map(|(_, after)| after);

        div()
            .id(("ws", ws.id))
            .group(group.clone())
            .relative()
            .mx(px(8.))
            // A group's workspaces sit in from its header.
            .when(ws.group.is_some(), |s| s.ml(px(GROUP_INDENT)))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(7.))
            .cursor_pointer()
            .when_some(bg, |s, bg| s.bg(bg))
            .when(!filled, |s| s.hover(|s| s.bg(rgba(HOVER))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    let Some(i) = this.ws_index(id) else { return };
                    if ev.click_count >= 2 {
                        this.start_rename(i, window, cx);
                    } else {
                        this.select_workspace(i, window, cx);
                    }
                }),
            )
            .on_drag(
                DraggedWorkspace {
                    id,
                    name: name.clone().into(),
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            // Where it would land shows as a line between rows: above this
            // one over its top half, below it over the bottom half.
            .on_drag_move(
                cx.listener(move |this, ev: &DragMoveEvent<DraggedWorkspace>, _, cx| {
                    if !ev.bounds.contains(&ev.event.position) {
                        return;
                    }
                    let at = Some((id, ev.event.position.y > ev.bounds.center().y));
                    if this.ws_drop != at {
                        this.ws_drop = at;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |this, d: &DraggedWorkspace, _, cx| {
                let after = this
                    .ws_drop
                    .take()
                    .is_some_and(|(t, after)| t == id && after);
                if let (Some(from), Some(onto)) = (this.ws_index(d.id), this.ws_index(id)) {
                    this.drop_workspace(from, onto, after, cx);
                }
            }))
            .when_some(drop_line, |s, after| {
                s.child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .right(px(4.))
                        .h(px(2.))
                        .rounded_full()
                        .bg(rgb(ACCENT_BRIGHT))
                        .map(|d| {
                            if after {
                                d.bottom(px(-2.))
                            } else {
                                d.top(px(-2.))
                            }
                        }),
                )
            })
            .drag_over::<DraggedGroup>(|d, _, _, _| d.bg(rgba(DROP_TINT)))
            .on_drop(cx.listener(move |this, d: &DraggedGroup, _, cx| {
                if let Some(onto) = this.ws_index(id) {
                    this.drop_group(d.id, onto, cx);
                }
            }))
            .when_some(rail, |s, c| {
                s.child(
                    div()
                        .absolute()
                        .left(px(2.))
                        .top(px(6.))
                        .bottom(px(6.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(rgb(c)),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(unread.is_some(), |s| {
                        s.child(
                            div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .bg(rgb(ACCENT_BRIGHT)),
                        )
                    })
                    .child(match renaming {
                        // Edit in place: same font and line as the title, in a
                        // dark inset so it reads as a field on the accent row.
                        Some(input) => div()
                            .flex_1()
                            .min_w_0()
                            .h(px(20. * z))
                            .my(px(-2.))
                            .mx(px(-5.))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .bg(rgba(0x0000004d))
                            .border_1()
                            .border_color(rgba(0xffffff59))
                            .text_color(rgb(TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                                if ev.keystroke.key == "escape" {
                                    this.renaming = None;
                                    this.focus_active(window, cx);
                                    cx.notify();
                                }
                            }))
                            .child(
                                Input::new(&input)
                                    .appearance(false)
                                    .text_size(px(12.5 * z))
                                    .h_full()
                                    .px_0()
                                    .py_0(),
                            )
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .map(|d| {
                                if sb.wrap_workspace_titles {
                                    d
                                } else {
                                    d.truncate()
                                }
                            })
                            .text_size(px(12.5 * z))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(name.clone())
                            .into_any_element(),
                    })
                    .when(!is_renaming, |s| {
                        s.child(
                            div()
                                .id(("ws-close", ws.id))
                                .invisible()
                                .group_hover(group.clone(), |s| s.visible())
                                .flex_none()
                                .rounded(px(4.))
                                .p(px(2.))
                                .hover(|s| s.bg(rgba(0xffffff22)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(i) = this.ws_index(id) {
                                        this.close_workspace(i, window, cx)
                                    }
                                }))
                                .child(icon("x").size(px(11. * z)).text_color(sub)),
                        )
                    }),
            )
            .children(describing.map(|input| {
                div()
                    .mt(px(3.))
                    .mx(px(-5.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .bg(rgba(0x0000004d))
                    .border_1()
                    .border_color(rgba(0xffffff59))
                    .text_color(rgb(TEXT))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        let k = &ev.keystroke;
                        if k.key == "escape" {
                            this.finish_describe(None, window, cx);
                            cx.stop_propagation();
                        } else if k.key == "enter"
                            && let Some((_, input)) = &this.describing
                        {
                            input.update(cx, |i, cx| i.insert("\n", window, cx));
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Input::new(&input)
                            .appearance(false)
                            .text_size(px(11. * z))
                            .px_0()
                            .py_0(),
                    )
            }))
            .when(!description.is_empty(), |s| {
                s.child(
                    div()
                        .mt(px(2.))
                        .flex()
                        .flex_col()
                        .text_size(px(11. * z))
                        .text_color(match sb.workspace_description_color {
                            Some(c) if !filled => c,
                            _ => sub,
                        })
                        .children(
                            description
                                .into_iter()
                                .map(|l| div().min_w_0().line_clamp(2).child(l)),
                        ),
                )
            })
            .when(
                details
                    && !subtitle.is_empty()
                    && subtitle != name
                    && Some(&subtitle) != cwd.as_ref().map(|c| tilde(c)).as_ref(),
                |s| {
                    s.child(
                        div()
                            .mt(px(2.))
                            .text_size(px(11. * z))
                            .text_color(sub)
                            .line_clamp(2)
                            .child(subtitle),
                    )
                },
            )
            .when_some(latest, |s, text| {
                let color = if filled {
                    rgb(0xffffff)
                } else {
                    rgb(ACCENT_BRIGHT)
                };
                s.child(
                    div()
                        .mt(px(2.))
                        .flex()
                        .gap(px(4.))
                        .text_size(px(11. * z))
                        .text_color(color)
                        .child(icon("bell").mt(px(1.)).size(px(10. * z)).text_color(color))
                        .child(div().flex_1().min_w_0().line_clamp(2).child(text)),
                )
            })
            .children(status)
            .when_some(hint, |s, d| {
                s.child(
                    hint_pill(format!("⌘{d}"))
                        .absolute()
                        .top(px(6.))
                        .right(px(8.)),
                )
            })
            .when(details && sb.show_branch_directory, |s| {
                s.children(meta.into_iter().map(|(is_branch, text)| {
                    div()
                        .mt(px(2.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(10.5 * z))
                        .text_color(sub)
                        .when(is_branch, |s| {
                            s.child(icon("git-branch").size(px(10. * z)).text_color(sub))
                        })
                        .child(
                            div()
                                .truncate()
                                .font_family(terminal::FONT_FAMILY)
                                .child(text),
                        )
                }))
            })
            .context_menu(move |menu, window, cx| {
                Self::workspace_menu(this.clone(), id, menu, window, cx)
            })
            .with_own_id(("ws-menu", id))
    }
}

/// gpui-component names every context menu "context-menu", so sibling rows
/// would share one menu's state; a parent with its own id keeps them apart.
trait OwnId: IntoElement {
    fn with_own_id(self, id: impl Into<ElementId>) -> AnyElement {
        div().id(id).child(self).into_any_element()
    }
}

impl<E: IntoElement> OwnId for E {}

/// A description as typed: trailing blank lines and spaces dropped, empty
/// for none.
pub fn clean_description(s: &str) -> Option<String> {
    let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would shadow the built-in.
    use super::{clean_description, short_path};

    #[test]
    fn paths() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(short_path(&format!("{home}/code/api"), false), "~/code/api");
        assert_eq!(short_path(&format!("{home}/code/api"), true), "…/api");
        assert_eq!(short_path(&format!("{home}/code"), true), "~/code");
        assert_eq!(short_path(&home, true), "~");
        assert_eq!(short_path("/usr/local/bin", true), "…/bin");
        assert_eq!(short_path("/tmp", true), "/tmp");
    }

    #[test]
    fn descriptions() {
        assert_eq!(clean_description("  \n"), None);
        assert_eq!(
            clean_description("Ship it  \n- build\n\n").as_deref(),
            Some("Ship it\n- build")
        );
    }
}
