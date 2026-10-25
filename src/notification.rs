//! Notifications from terminals (OSC 9/99/777, the bell) and `abromux
//! notify`: the list behind ⌘I, the ring on panes that want attention,
//! the sidebar's latest-notification line, ⌘⇧U, banners and the dock badge.
//!
//! A notification is read once you look at its terminal: the window is
//! active and it's the focused tab of the selected workspace.

use crate::{AbroMux, TITLEBAR_H, banner, theme::*, workspace::Workspace};
use gpui::{prelude::*, *};
use std::time::{Duration, SystemTime};

/// Oldest ones are dropped past this.
const KEEP: usize = 200;

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
        if !bell && !window.is_window_active() {
            banner::post(
                id,
                if title.is_empty() {
                    crate::channel::app_name()
                } else {
                    &title
                },
                &body,
            );
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
        cx.notify();
        Some(id)
    }

    fn surface_title(&self, surface: EntityId, cx: &App) -> Option<String> {
        self.workspaces
            .iter()
            .flat_map(|w| w.root.panes())
            .flat_map(|p| p.tabs.iter())
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
    pub(crate) fn mark_viewed(&mut self, window: &Window) {
        if let Some(id) = self.ws().focused_surface().map(|s| s.entity_id())
            && window.is_window_active()
        {
            for n in self.notifications.iter_mut().filter(|n| n.surface == id) {
                n.read = true;
            }
        }
        let unread = self.notifications.iter().filter(|n| !n.read).count();
        if unread != self.badge {
            self.badge = unread;
            banner::set_badge(unread);
        }
    }

    pub(crate) fn surface_unread(&self, surface: EntityId) -> bool {
        self.notifications
            .iter()
            .any(|n| n.surface == surface && !n.read)
    }

    pub(crate) fn ws_unread(&self, ws: &Workspace) -> bool {
        self.latest_unread(ws).is_some()
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
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else {
            return false;
        };
        self.notifications_open = false;
        self.focus_surface(surface, cx);
        self.select_workspace(wi, window, cx);
        true
    }

    /// ⌘⇧U: the newest unread notification. False if there's none.
    pub(crate) fn jump_to_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self
            .notifications
            .iter()
            .rev()
            .find(|n| !n.read)
            .map(|n| n.id)
        {
            Some(id) => self.open_notification(id, window, cx),
            None => false,
        }
    }

    pub(crate) fn clear_notifications(&mut self, cx: &mut Context<Self>) {
        self.notifications.clear();
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

/// "now", "5m", "3h", "2d".
fn ago(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
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
