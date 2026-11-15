//! Window dragging. With a transparent titlebar, AppKit moves the window on a
//! drag anywhere in the top strip, which also catches the top panes' tabs
//! when they're dragged. So the window isn't movable (`is_movable: false`),
//! and only the empty titlebar areas move it, through `drag_or_zoom`. Linux
//! has no titlebar of its own (server-side decorations, which tiling
//! compositors leave off), so the same areas move it there.

use gpui::{MouseDownEvent, Window};

/// Mouse down on an empty titlebar area: a double click zooms (or whatever
/// the system setting says), a drag moves the window. On Linux the
/// compositor moves it (gpui's `start_window_move`) and a double click
/// maximizes; a tiling one like Hyprland ignores both for tiled windows.
pub fn drag_or_zoom(ev: &MouseDownEvent, window: &Window) {
    let mac = abromux_platform::MACOS;
    match (ev.click_count, mac) {
        (2, true) => window.titlebar_double_click(),
        (2, false) => window.zoom_window(),
        (_, true) => abromux_platform::window::begin_drag(window),
        (_, false) => window.start_window_move(),
    }
}
