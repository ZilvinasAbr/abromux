//! Pinned tabs: the tabs a workspace keeps. ⌘P finds them and `abromux
//! link` lists them. ⌘W on a pinned tab hides it rather than closing it: it
//! leaves the layout and keeps running (a terminal's program, a page), and
//! showing it again (⌘P, ⌥⌘1…9 or opening its link again) puts it back
//! where it was. ⇧⌘W closes and unpins.
//!
//! Tabs showing a Jira ticket or a GitLab merge request are pinned on their
//! own (`links::sync_links`); unpinning one stops that for its key.

use crate::{AbroMux, TerminalOpts, alert, workspace::Hidden, workspace::Surface};
use gpui::{prelude::*, *};

impl AbroMux {
    /// The workspace holding tab `id`, in the layout or hidden.
    pub(crate) fn pin_workspace(&self, id: EntityId) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|w| w.contains(id) || w.hidden_index(id).is_some())
    }

    /// The key of the link tab `id` of workspace `wi` shows, if any.
    fn tab_link_key(&self, wi: usize, id: EntityId, cx: &App) -> Option<String> {
        let s = self.workspaces[wi]
            .all_surfaces()
            .find(|s| s.entity_id() == id)?;
        self.tab_link(wi, s, cx).map(|l| l.key)
    }

    /// Pin tab `id`. False if it already was, or there's no such tab.
    pub(crate) fn pin_tab(&mut self, id: EntityId, cx: &mut Context<Self>) -> bool {
        let Some(wi) = self.pin_workspace(id) else {
            return false;
        };
        let key = self.tab_link_key(wi, id, cx);
        let ws = &mut self.workspaces[wi];
        let pinned = ws.pin(id);
        // Pinned by hand: a key unpinned before may pin itself again.
        if let Some(key) = key {
            ws.hidden_links.retain(|k| *k != key);
        }
        cx.notify();
        pinned
    }

    /// Unpin tab `id`. One that's hidden closes, as nothing would show it
    /// again. A tab showing a ticket or merge request isn't pinned again on
    /// its own. False if it wasn't pinned.
    pub(crate) fn unpin_tab(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(wi) = self.pin_workspace(id) else {
            return false;
        };
        let key = self.tab_link_key(wi, id, cx);
        let ws = &mut self.workspaces[wi];
        if !ws.is_pinned(id) {
            return false;
        }
        ws.pinned.retain(|p| *p != id);
        ws.pinned_for.remove(&id);
        if let Some(key) = key {
            ws.links.retain(|l| l.key != key);
            if !ws.hidden_links.contains(&key) {
                ws.hidden_links.push(key);
            }
        }
        if ws.hidden_index(id).is_some() {
            self.close_surface(id, window, cx);
        }
        cx.notify();
        true
    }

    /// Take pinned tab `id` out of the layout, still running. A workspace's
    /// only tab leaves a fresh terminal in its place, so the workspace stays.
    pub(crate) fn hide_tab(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(id)) else {
            return;
        };
        let ws = &self.workspaces[wi];
        let Some((pane, index, _)) = ws.tab_position(id) else {
            return;
        };
        if ws.panes().map(|p| p.tabs.len()).sum::<usize>() == 1 {
            let cwd = ws.cwd(cx);
            let term = self.make_terminal(ws.id, TerminalOpts::cwd(cwd), window, cx);
            self.add_tab_to(wi, pane, term, true, window, cx);
        }
        let ws = &mut self.workspaces[wi];
        let Some((surface, _)) = ws.detach_tab(id) else {
            return;
        };
        if let Surface::Browser(b) = &surface {
            b.update(cx, |b, cx| b.set_shown(false, cx));
        }
        ws.hidden.push(Hidden {
            surface,
            pane,
            index,
        });
        if wi == self.active {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Put hidden tab `id` back in the layout: in the pane it left, else
    /// the workspace's link pane, else the focused one. With `focus`, it's
    /// selected and focused (and its workspace with it).
    pub(crate) fn show_hidden(
        &mut self,
        id: EntityId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(wi) = self
            .workspaces
            .iter()
            .position(|w| w.hidden_index(id).is_some())
        else {
            return false;
        };
        let ws = &mut self.workspaces[wi];
        let h = ws.hidden.remove(ws.hidden_index(id).unwrap());
        let pane = [Some(h.pane), ws.link_pane, Some(ws.focused_pane)]
            .into_iter()
            .flatten()
            .find(|p| ws.pane(*p).is_some())
            .unwrap_or_else(|| ws.panes().next().map_or(0, |p| p.id));
        let Some(p) = ws.pane_mut(pane) else {
            ws.hidden.push(h);
            return false;
        };
        let was = p.active;
        let at = h.index.min(p.tabs.len());
        p.tabs.insert(at, h.surface);
        p.active = if focus {
            at
        } else if was >= at && p.tabs.len() > 1 {
            was + 1
        } else {
            was
        };
        if focus {
            ws.focused_pane = pane;
            if wi != self.active {
                self.select_workspace(wi, window, cx);
            } else {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
        true
    }

    /// Bring tab `id` forward, hidden or not, in whichever workspace.
    pub(crate) fn reveal_tab(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_hidden(id, true, window, cx) {
            return;
        }
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(id)) else {
            return;
        };
        self.focus_surface(id, cx);
        if wi != self.active {
            self.select_workspace(wi, window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }

    /// ⌥⌘T: pin the focused tab; again, unpin it.
    pub(crate) fn toggle_pin_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.ws().focused_surface().map(|s| s.entity_id()) else {
            return;
        };
        if self.ws().is_pinned(id) {
            self.unpin_tab(id, window, cx);
        } else {
            self.pin_tab(id, cx);
        }
    }

    /// Whether closing tab `s` for good would end something running in it:
    /// a coding agent, or a command (with the shell's marks).
    pub(crate) fn tab_is_busy(s: &Surface, cx: &App) -> bool {
        match s {
            Surface::Terminal(t) => {
                let t = t.read(cx);
                t.has_claude() || (t.commands.marked && t.commands.running)
            }
            _ => false,
        }
    }

    /// ⇧⌘W: close tab `id` and unpin it, asking first if something runs in
    /// it.
    pub(crate) fn request_close_and_unpin(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wi) = self.pin_workspace(id) else {
            return;
        };
        let Some(s) = self.workspaces[wi]
            .all_surfaces()
            .find(|s| s.entity_id() == id)
            .cloned()
        else {
            return;
        };
        if !Self::tab_is_busy(&s, cx) {
            return self.close_and_unpin(id, window, cx);
        }
        let rx = alert::confirm(
            window,
            cx,
            alert::Confirm {
                message: "Close tab?",
                info: "Something is still running in this tab. Closing it will end it.",
                button: "Close",
                suppress: "Don't ask again",
                skip_key: crate::SKIP_CLOSE_CLAUDE_KEY,
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            if rx.await == Ok(true) {
                let _ = this.update_in(cx, |this, window, cx| this.close_and_unpin(id, window, cx));
            }
        })
        .detach();
    }

    fn close_and_unpin(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        // Unpinning a hidden tab closes it; one in the layout closes here.
        self.unpin_tab(id, window, cx);
        if self.workspaces.iter().any(|w| w.contains(id)) {
            self.close_surface(id, window, cx);
        }
    }
}
