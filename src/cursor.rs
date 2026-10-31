//! Leave the cursor over browser panes to WebKit (see `cursor.m`).

unsafe extern "C" {
    fn abromux_cursor_install();
}

/// Stop GPUI from resetting the cursor while the mouse is over a webview.
/// Safe to call more than once.
pub fn install() {
    unsafe { abromux_cursor_install() };
}
