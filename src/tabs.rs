//! Moving tabs between panes and workspaces outside a drag: the
//! `move-surface`, `reorder-surface`, `split-off`, `join-pane` and
//! `move-tab-to-new-workspace` commands, Move Tab to New Workspace, and
//! dropping a tab on a workspace in the sidebar. Dragging within a
//! workspace is `Workspace::move_tab`.

use crate::{
    AbroMux,
    workspace::{Dir, Node, Pane, Surface, TabDrop, Workspace},
};
use anyhow::{Context as _, bail};
use gpui::{Context, EntityId, Window};

/// Where focus was, to put it back after a move that shouldn't take it:
/// the selected workspace, and each workspace's focused tab.
pub(crate) struct FocusMark {
    active: usize,
    tabs: Vec<EntityId>,
}

impl AbroMux {
    pub(crate) fn mark_focus(&self) -> FocusMark {
        FocusMark {
            active: self.workspaces.get(self.active).map_or(0, |w| w.id),
            tabs: self
                .workspaces
                .iter()
                .filter_map(|w| w.focused_surface().map(Surface::entity_id))
                .collect(),
        }
    }

    /// Focus tab `id` where it is now, or with `focus` false put focus back
    /// to `mark`. Either way the keyboard goes to the result.
    pub(crate) fn settle_focus(
        &mut self,
        id: EntityId,
        focus: bool,
        mark: FocusMark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if focus {
            self.focus_surface(id, cx);
            if let Some(wi) = self.workspaces.iter().position(|w| w.contains(id)) {
                self.active = wi;
            }
        } else {
            for t in mark.tabs {
                self.focus_surface(t, cx);
            }
            if let Some(wi) = self.ws_index(mark.active) {
                self.active = wi;
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Move tab `id` into pane `pane` of workspace `dest` (an index) at
    /// `index` (past the end appends). A workspace it leaves empty closes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn move_surface(
        &mut self,
        id: EntityId,
        dest: usize,
        pane: usize,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let src = self.workspace_of(id).context("no such surface")?;
        if !self.workspaces[dest]
            .root
            .panes()
            .iter()
            .any(|p| p.id == pane)
        {
            bail!("pane:{pane} isn't in that workspace");
        }
        let mark = self.mark_focus();
        if src == dest {
            let (from, _, count) = self.workspaces[src].tab_position(id).unwrap();
            // A pane's only tab is already where it would go.
            if !(from == pane && count == 1) {
                let (new_pane, split) = (self.pane_id(), self.id());
                self.workspaces[src].move_tab(id, TabDrop::Tabs { pane, index }, new_pane, split);
            }
        } else {
            let (surface, emptied) = self.workspaces[src].detach_tab(id).unwrap();
            let ws = &mut self.workspaces[dest];
            if ws.insert_tab(pane, index, surface).is_err() {
                unreachable!("pane checked above");
            }
            ws.focused_pane = pane;
            self.carry_status(src, dest, id, cx);
            if emptied {
                self.close_workspace(src, window, cx);
            }
        }
        self.settle_focus(id, focus, mark, window, cx);
        Ok(())
    }

    /// Move tab `id` into a new pane beside its own, on side `dir`.
    pub(crate) fn split_off(
        &mut self,
        id: EntityId,
        dir: Dir,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<usize> {
        let wi = self.workspace_of(id).context("no such surface")?;
        let (pane, _, _) = self.workspaces[wi].tab_position(id).unwrap();
        let to = TabDrop::Split { pane, dir };
        if !self.workspaces[wi].accepts(id, to) {
            bail!("it's the only tab in its pane; there's nothing to split it from");
        }
        let mark = self.mark_focus();
        let (new_pane, split) = (self.pane_id(), self.id());
        self.workspaces[wi].move_tab(id, to, new_pane, split);
        self.settle_focus(id, focus, mark, window, cx);
        Ok(new_pane)
    }

    /// Move tab `id` into a workspace of its own, placed as ⌘N places one.
    /// Returns the new workspace's index.
    pub(crate) fn tab_to_new_workspace(
        &mut self,
        id: EntityId,
        name: Option<String>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<usize> {
        let src = self.workspace_of(id).context("no such surface")?;
        if self.workspaces[src]
            .root
            .panes()
            .iter()
            .map(|p| p.tabs.len())
            .sum::<usize>()
            == 1
        {
            bail!("it's the only tab in its workspace");
        }
        let mark = self.mark_focus();
        let (surface, _) = self.workspaces[src].detach_tab(id).unwrap();
        self.next_ws += 1;
        let ws_id = self.next_ws;
        let pane = Pane {
            id: self.pane_id(),
            tabs: vec![surface],
            active: 0,
        };
        let focused = pane.id;
        let mut ws = Workspace::new(ws_id, name, Node::Leaf(pane), focused);
        let (index, group) = self.new_workspace_slot(cx);
        ws.group = group;
        let index = self.insert_workspace(index, ws);
        if let Some(g) = self.groups.iter_mut().find(|g| Some(g.id) == group) {
            g.collapsed = false;
        }
        // By owner: inserting may have shifted the source workspace along.
        let from = self
            .workspaces
            .iter()
            .position(|w| w.id != ws_id && w.status.iter().any(|s| s.owner == Some(id)));
        if let Some(from) = from {
            self.carry_status(from, index, id, cx);
        }
        self.settle_focus(id, focus, mark, window, cx);
        Ok(self.ws_index(ws_id).unwrap_or(index))
    }

    /// Dropping a tab on a workspace in the sidebar: into that workspace's
    /// focused pane, after its active tab.
    pub(crate) fn drop_tab_on_workspace(
        &mut self,
        id: EntityId,
        ws_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dest) = self.ws_index(ws_id) else {
            return;
        };
        if self.workspaces[dest].contains(id) {
            return;
        }
        let ws = &self.workspaces[dest];
        let pane = ws.focused_pane;
        let index = ws.focused().map_or(0, |p| p.active + 1);
        let _ = self.move_surface(id, dest, pane, index, true, window, cx);
        self.tab_drop = None;
    }

    pub(crate) fn workspace_of(&self, id: EntityId) -> Option<usize> {
        self.workspaces.iter().position(|w| w.contains(id))
    }

    /// The sidebar pills tab `id`'s agent set go with it to workspace `to`.
    fn carry_status(&mut self, from: usize, to: usize, id: EntityId, cx: &mut Context<Self>) {
        let (moved, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.workspaces[from].status)
            .into_iter()
            .partition(|s| s.owner == Some(id));
        self.workspaces[from].status = kept;
        for s in moved {
            self.set_status(to, s, cx);
        }
    }
}
