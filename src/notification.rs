//! Notifications from terminals (OSC 9/99/777, the bell) and `abromux
//! notify`: the list behind ⌘I, the ring on panes that want attention,
//! the sidebar's latest-notification line, ⌘⇧U, banners and the dock badge.
//!
//! A notification is read once you look at its terminal: the window is
//! active and it's the focused tab of the selected workspace.

use crate::{AbroMux, TITLEBAR_H, banner, menubar, notify_hook, theme::*, workspace::Workspace};
use gpui::{prelude::*, *};
use std::time::{Duration, SystemTime};

/// Oldest ones are dropped past this.
const KEEP: usize = 200;
/// Unread notifications listed in the menu-bar extra.
const MENU_RECENT: usize = 10;

pub struct Notification {
    pub id: usize,
    pub surface: EntityId,
    pub title: String,
    pub body: String,
    pub at: SystemTime,
    pub read: bool,
    /// A bell (BEL) rather than a message: no banner, and repeats merge.
    pub bell: bool,
}

impl Notification {
    /// One line for the sidebar and lists: "title: body".
    pub fn text(&self) -> String {
        match (self.title.is_empty(), self.body.is_empty()) {
            (false, false) => format!("{}: {}", self.title, self.body),
            (true, _) => self.body.clone(),
            (_, true) => self.title.clone(),
        }
    }
}

impl AbroMux {
    /// Record a notification from `surface`. Returns its id, or None for a
    /// bell you were looking at.
    pub(crate) fn add_notification(
        &mut self,
        surface: EntityId,
        title: String,
        body: String,
        bell: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let viewing = self.viewing(surface, window);
        if bell && viewing {
            return None;
        }
        if bell
            && let Some(last) = self
                .notifications
                .iter_mut()
                .rev()
                .find(|n| n.surface == surface)
            && last.bell
            && !last.read
        {
            last.at = SystemTime::now();
            return Some(last.id);
        }
        // The same message twice in a row (an agent's hook and its own OSC 9
        // both reporting it) is one notification.
        if !bell
            && !body.is_empty()
            && let Some(last) = self
                .notifications
                .iter_mut()
                .rev()
                .find(|n| n.surface == surface)
            && !last.read
            && last.body == body
            && last.at.elapsed().is_ok_and(|d| d < Duration::from_secs(10))
        {
            last.at = SystemTime::now();
            return Some(last.id);
        }
        let title = if title.is_empty() {
            self.surface_title(surface, cx).unwrap_or_default()
        } else {
            title
        };
        let body = if body.is_empty() && bell {
            "Bell".into()
        } else {
            body
        };
        self.next_notification += 1;
        let id = self.next_notification;
        let hooks: Vec<_> = cx
            .try_global::<crate::settings::Settings>()
            .map(|s| s.notifications.hooks.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.enabled)
            .collect();
        if !bell && !hooks.is_empty() {
            self.run_hooks(id, surface, &title, &body, viewing, hooks, window, cx);
        } else if !bell && !window.is_window_active() {
            banner::post(id, banner_title(&title), &body, true);
        }
        self.notifications.push(Notification {
            id,
            surface,
            title,
            body,
            at: SystemTime::now(),
            read: viewing,
            bell,
        });
        let excess = self.notifications.len().saturating_sub(KEEP);
        self.notifications.drain(..excess);
        self.refresh_unread(cx);
        cx.notify();
        Some(id)
    }

    /// Pass notification `id` through `notifications.hooks` off the main
    /// thread, then apply what they decided (`notify_hook`). It's recorded
    /// meanwhile; its banner waits for them.
    #[allow(clippy::too_many_arguments)]
    fn run_hooks(
        &self,
        id: usize,
        surface: EntityId,
        title: &str,
        body: &str,
        viewing: bool,
        hooks: Vec<crate::settings::NotificationHook>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let ws = self.workspaces.iter().find(|w| w.contains(surface));
        let input = notify_hook::Input {
            id,
            workspace: ws.map(|w| format!("workspace:{}", w.id)),
            surface: self.surface_refs.get(&surface).map(|r| format!("surface:{r}")),
            title: title.into(),
            body: body.into(),
            cwd: ws.and_then(|w| w.cwd(cx)),
            app_focused: window.is_window_active(),
            focused_panel: viewing,
            socket: self.socket.as_ref().map(|p| p.to_string_lossy().into_owned()),
        };
        let task = cx.background_spawn(async move { notify_hook::run(&hooks, &input) });
        cx.spawn_in(window, async move |this, cx| {
            let out = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.apply_hooks(id, out, window, cx)
            });
        })
        .detach();
    }

    fn apply_hooks(
        &mut self,
        id: usize,
        out: notify_hook::Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(e) = out.error {
            let note = gpui_component::notification::Notification::warning(e)
                .title("Notification hook failed");
            gpui_component::WindowExt::push_notification(window, note, cx);
        }
        let Some(i) = self.notifications.iter().position(|n| n.id == id) else {
            return;
        };
        cx.notify();
        if !out.effects.record {
            self.notifications.remove(i);
            self.refresh_unread(cx);
            return;
        }
        let n = &mut self.notifications[i];
        n.title = out.title;
        n.body = out.body;
        if !out.effects.mark_unread {
            n.read = true;
        }
        if out.effects.desktop && !window.is_window_active() {
            banner::post(id, banner_title(&n.title), &n.body, out.effects.sound);
        }
        self.refresh_unread(cx);
    }

    fn surface_title(&self, surface: EntityId, cx: &App) -> Option<String> {
        self.workspaces
            .iter()
            .flat_map(|w| w.surfaces())
            .find(|s| s.entity_id() == surface)
            .map(|s| s.title(cx))
    }

    /// Whether you're looking at `surface` right now.
    fn viewing(&self, surface: EntityId, window: &Window) -> bool {
        window.is_window_active()
            && self
                .ws()
                .focused_surface()
                .is_some_and(|s| s.entity_id() == surface)
    }

    /// Mark what you're looking at as read and keep the dock badge current.
    /// Runs on every render, which follows focus and window changes.
    ///
    /// Only a change of what you look at counts, so ⌥⌘U can mark the
    /// terminal in front of you unread and it stays so until you look away
    /// and back.
    pub(crate) fn mark_viewed(&mut self, window: &Window, cx: &App) {
        let viewing = self
            .ws()
            .focused_surface()
            .map(|s| s.entity_id())
            .filter(|_| window.is_window_active());
        if viewing != self.last_viewed {
            self.last_viewed = viewing;
            if let Some(id) = viewing {
                self.mark_surface_read(id);
            }
        }
        self.refresh_unread(cx);
    }

    /// Bring the dock badge and the menu-bar extra up to date. Runs on
    /// render and after every socket request and hook, so they follow
    /// along while the window isn't drawing (hidden, screen locked).
    pub(crate) fn refresh_unread(&mut self, cx: &App) {
        let unread = self.notifications.iter().filter(|n| !n.read).count();
        if unread != self.badge {
            self.badge = unread;
            banner::set_badge(unread);
        }
        self.update_menubar(unread, cx);
    }

    /// Rebuild the menu-bar extra if what it shows changed.
    fn update_menubar(&mut self, unread: usize, cx: &App) {
        let show = cx
            .try_global::<crate::settings::Settings>()
            .is_none_or(|s| s.notifications.show_in_menu_bar);
        let recent: Vec<menubar::Entry> = self
            .notifications
            .iter()
            .rev()
            .filter(|n| !n.read)
            .take(MENU_RECENT)
            .map(|n| menubar::Entry { id: n.id, text: n.text() })
            .collect();
        let mut key = format!("{show} {unread}");
        for e in &recent {
            key += &format!("\n{} {}", e.id, e.text);
        }
        if self.menubar.as_ref() != Some(&key) {
            self.menubar = Some(key);
            menubar::update(show, unread, &recent);
        }
    }

    /// Something chosen in the menu-bar extra.
    pub(crate) fn menubar_pick(
        &mut self,
        pick: menubar::Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use menubar::Pick;
        if !matches!(pick, Pick::MarkAllRead | Pick::ClearAll | Pick::Quit) {
            cx.activate(true);
            window.activate_window();
        }
        match pick {
            Pick::ShowApp => {}
            Pick::ShowNotifications => {
                self.notifications_open = true;
                cx.notify();
            }
            Pick::JumpToUnread => {
                self.jump_to_unread(window, cx);
            }
            Pick::MarkAllRead => {
                for n in self.notifications.iter_mut() {
                    n.read = true;
                }
                self.manual_unread.clear();
                cx.notify();
            }
            Pick::ClearAll => self.clear_notifications(cx),
            Pick::Settings => window.dispatch_action(Box::new(crate::OpenSettings), cx),
            Pick::Quit => window.dispatch_action(Box::new(crate::Quit), cx),
            Pick::Open(id) => {
                self.open_notification(id, window, cx);
            }
        }
        self.refresh_unread(cx);
    }

    pub(crate) fn surface_unread(&self, surface: EntityId) -> bool {
        self.manual_unread.contains(&surface)
            || self
                .notifications
                .iter()
                .any(|n| n.surface == surface && !n.read)
    }

    pub(crate) fn ws_unread(&self, ws: &Workspace) -> bool {
        self.latest_unread(ws).is_some() || self.manual_unread.iter().any(|&s| ws.contains(s))
    }

    /// Mark everything from `surface` read, a manual mark included.
    pub(crate) fn mark_surface_read(&mut self, surface: EntityId) {
        self.manual_unread.remove(&surface);
        for n in self.notifications.iter_mut().filter(|n| n.surface == surface) {
            n.read = true;
        }
    }

    /// Mark `surface` unread: its newest notification, or the surface itself
    /// if it has none.
    pub(crate) fn mark_surface_unread(&mut self, surface: EntityId) {
        match self.notifications.iter_mut().rev().find(|n| n.surface == surface) {
            Some(n) => n.read = false,
            None => {
                self.manual_unread.insert(surface);
            }
        }
    }

    /// ⌥⌘U: the focused terminal unread if it's read, read if it's unread.
    pub(crate) fn toggle_unread(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.ws().focused_surface().map(|s| s.entity_id()) else {
            return false;
        };
        if self.surface_unread(id) {
            self.mark_surface_read(id);
        } else {
            self.mark_surface_unread(id);
        }
        cx.notify();
        true
    }

    /// ⌃⌘U: put the focused terminal's newest notification at the back of
    /// the unread queue (unread, but oldest), then jump to the newest unread
    /// elsewhere. Works through a pile of them one at a time, coming back to
    /// this one last. False if there was nowhere else to go.
    pub(crate) fn mark_oldest_and_jump_next(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.ws().focused_surface().map(|s| s.entity_id()) else {
            return false;
        };
        match self.notifications.iter().rposition(|n| n.surface == id) {
            Some(i) => {
                let mut n = self.notifications.remove(i);
                n.read = false;
                self.notifications.insert(0, n);
            }
            None => {
                self.manual_unread.insert(id);
            }
        }
        cx.notify();
        match self.next_unread(Some(id)) {
            Some(Unread::Notification(n)) => self.open_notification(n, window, cx),
            Some(Unread::Surface(s)) => self.open_surface(s, window, cx),
            None => false,
        }
    }

    /// The newest unread notification not from `skip`, else a surface
    /// marked unread by hand.
    fn next_unread(&self, skip: Option<EntityId>) -> Option<Unread> {
        self.notifications
            .iter()
            .rev()
            .find(|n| !n.read && Some(n.surface) != skip)
            .map(|n| Unread::Notification(n.id))
            .or_else(|| {
                self.manual_unread
                    .iter()
                    .find(|&&s| Some(s) != skip && self.workspaces.iter().any(|w| w.contains(s)))
                    .map(|&s| Unread::Surface(s))
            })
    }

    /// Select `surface`'s workspace and tab, and mark it read.
    fn open_surface(&mut self, surface: EntityId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else {
            return false;
        };
        self.mark_surface_read(surface);
        self.notifications_open = false;
        self.focus_surface(surface, cx);
        self.select_workspace(wi, window, cx);
        true
    }

    /// Remove notification `id`. False if there's none.
    pub(crate) fn dismiss_notification(&mut self, id: usize, cx: &mut Context<Self>) -> bool {
        let before = self.notifications.len();
        self.notifications.retain(|n| n.id != id);
        cx.notify();
        self.notifications.len() < before
    }

    /// Remove every read notification; how many went.
    pub(crate) fn dismiss_read(&mut self, cx: &mut Context<Self>) -> usize {
        let before = self.notifications.len();
        self.notifications.retain(|n| !n.read);
        cx.notify();
        before - self.notifications.len()
    }

    pub(crate) fn latest_unread(&self, ws: &Workspace) -> Option<&Notification> {
        self.notifications
            .iter()
            .rev()
            .find(|n| !n.read && ws.contains(n.surface))
    }

    /// A closed surface takes its notifications with it.
    pub(crate) fn forget_notifications(&mut self, surface: EntityId) {
        self.notifications.retain(|n| n.surface != surface);
        self.manual_unread.remove(&surface);
    }

    /// Select the notification's terminal and mark it read.
    pub(crate) fn open_notification(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(n) = self.notifications.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        n.read = true;
        let surface = n.surface;
        self.manual_unread.remove(&surface);
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else {
            return false;
        };
        self.notifications_open = false;
        self.focus_surface(surface, cx);
        self.select_workspace(wi, window, cx);
        true
    }

    /// ⌘⇧U: the newest unread notification. False if there's none.
    /// A terminal marked unread by hand counts when no notification is.
    pub(crate) fn jump_to_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.next_unread(None) {
            Some(Unread::Notification(id)) => self.open_notification(id, window, cx),
            Some(Unread::Surface(s)) => self.open_surface(s, window, cx),
            None => false,
        }
    }

    pub(crate) fn clear_notifications(&mut self, cx: &mut Context<Self>) {
        self.notifications.clear();
        self.manual_unread.clear();
        cx.notify();
    }

    /// The ⌘I list, newest first, under the titlebar.
    pub(crate) fn render_notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = SystemTime::now();
        let rows: Vec<AnyElement> = self
            .notifications
            .iter()
            .rev()
            .map(|n| {
                let id = n.id;
                let place = self
                    .workspaces
                    .iter()
                    .find(|w| w.contains(n.surface))
                    .map(|w| w.name(cx))
                    .unwrap_or_default();
                let age = ago(now.duration_since(n.at).unwrap_or_default());
                div()
                    .id(("notification", id))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .gap(px(8.))
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .hover(|s| s.bg(rgba(HOVER)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_notification(id, window, cx);
                    }))
                    .child(
                        div()
                            .mt(px(5.))
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .when(!n.read, |d| d.bg(rgb(ACCENT_BRIGHT))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(!n.title.is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(n.title.clone()),
                                )
                            })
                            .when(!n.body.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(TEXT))
                                        .line_clamp(3)
                                        .child(n.body.clone()),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(MUTED))
                                    .child(format!("{place} · {age}")),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .absolute()
            .inset_0()
            // Clicking outside closes it.
            .child(
                div()
                    .id("notifications-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.notifications_open = false;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("notifications")
                    .absolute()
                    .top(px(TITLEBAR_H))
                    .left(px(8.))
                    .w(px(360.))
                    .max_h(px(520.))
                    .flex()
                    .flex_col()
                    .rounded(px(10.))
                    .bg(rgb(TABBAR_BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .shadow_lg()
                    .overflow_hidden()
                    .occlude()
                    .child(
                        div()
                            .flex_none()
                            .h(px(36.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Notifications"),
                            )
                            .when(!empty, |d| {
                                d.child(
                                    div()
                                        .id("notifications-clear")
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded(px(4.))
                                        .text_size(px(11.5))
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgba(HOVER)).text_color(rgb(TEXT)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.clear_notifications(cx)
                                        }))
                                        .child("Clear all"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("notifications-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .p(px(16.))
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child("No notifications"),
                                )
                            })
                            .children(rows),
                    ),
            )
            .into_any_element()
    }
}

enum Unread {
    Notification(usize),
    Surface(EntityId),
}

fn banner_title(title: &str) -> &str {
    if title.is_empty() {
        crate::channel::app_name()
    } else {
        title
    }
}

/// "now", "5m", "3h", "2d".
fn ago(d: Duration) -> String {
    abromux_gitlab::time::ago_short(d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ages() {
        use std::time::Duration;
        let ago = |s| super::ago(Duration::from_secs(s));
        assert_eq!(
            [ago(5), ago(120), ago(7200), ago(200000)],
            ["now", "2m", "2h", "2d"]
        );
    }
}
