//! What the Chromium engine needs from AppKit (`chromium.m`): NSApp as a
//! CefAppProtocol app, and placing, focusing and removing a page's NSView.

use std::ffi::c_void;

unsafe extern "C" {
    fn abx_chromium_app_install();
    fn abx_chromium_view_place(view: *mut c_void, x: f64, y: f64, w: f64, h: f64, shown: bool);
    fn abx_chromium_view_has_focus(view: *mut c_void) -> bool;
    fn abx_chromium_view_unfocus(view: *mut c_void);
    fn abx_chromium_view_remove(view: *mut c_void);
}

/// Give NSApp the methods Chromium's event handling asks for. Before CEF
/// starts; main thread.
pub fn app_install() {
    unsafe { abx_chromium_app_install() }
}

/// Place `view` at `x, y` (points from its superview's top left), `w × h`,
/// shown or hidden.
pub fn place(view: *mut c_void, x: f64, y: f64, w: f64, h: f64, shown: bool) {
    unsafe { abx_chromium_view_place(view, x, y, w, h, shown) }
}

/// Whether the window's first responder is `view` or inside it.
pub fn has_focus(view: *mut c_void) -> bool {
    unsafe { abx_chromium_view_has_focus(view) }
}

/// Move keyboard focus from `view` back to the window's content.
pub fn unfocus(view: *mut c_void) {
    unsafe { abx_chromium_view_unfocus(view) }
}

/// Take `view` out of its window.
pub fn remove(view: *mut c_void) {
    unsafe { abx_chromium_view_remove(view) }
}
