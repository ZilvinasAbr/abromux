#![allow(unexpected_cfgs)]
//! Window dragging. With a transparent titlebar, AppKit moves the window on a
//! drag anywhere in the top strip, which also catches the top panes' tabs
//! when they're dragged. So the window isn't movable (`is_movable: false`),
//! and only the empty titlebar areas move it, through `drag_or_zoom`.

use gpui::{MouseDownEvent, Window};
use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn ns_view(window: &Window) -> Option<*mut Object> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window).ok()?.as_raw() else {
        return None;
    };
    Some(h.ns_view.as_ptr() as *mut Object)
}

/// Mouse down on an empty titlebar area: a double click zooms (or whatever
/// the system setting says), a drag moves the window.
pub fn drag_or_zoom(ev: &MouseDownEvent, window: &Window) {
    if ev.click_count == 2 {
        window.titlebar_double_click();
        return;
    }
    let Some(view) = ns_view(window) else { return };
    unsafe {
        let win: *mut Object = msg_send![view, window];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let event: *mut Object = msg_send![app, currentEvent];
        if !win.is_null() && !event.is_null() {
            let _: () = msg_send![win, performWindowDragWithEvent: event];
        }
    }
}
