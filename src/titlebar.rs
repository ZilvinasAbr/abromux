//! Window dragging. With a transparent titlebar, AppKit moves the window on a
//! drag anywhere in the top strip, which also catches the top panes' tabs
//! when they're dragged. So the window isn't movable (`is_movable: false`),
//! and only the empty titlebar areas move it, through `drag_or_zoom`.

use gpui::{MouseDownEvent, Window};

/// Mouse down on an empty titlebar area: a double click zooms (or whatever
/// the system setting says), a drag moves the window.
pub fn drag_or_zoom(ev: &MouseDownEvent, window: &Window) {
    if ev.click_count == 2 {
        window.titlebar_double_click();
        return;
    }
    abromux_platform::window::begin_drag(window);
}
