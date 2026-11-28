//! Light or dark, applied everywhere it shows: the UI's palette
//! (`crate::theme`) and the terminals' colours, which the Ghostty config
//! picks per appearance (`theme = light:A,dark:B`, `crate::config`).
//! Neither of those knows about the other; this keeps them in step.

use crate::theme::{Appearance, apply_appearance, resolve, set_dark};
use gpui::{App, Window};

/// Make the app's setting macOS's appearance for the app, and follow it.
pub fn apply(a: Appearance, cx: &mut App) {
    apply_appearance(a);
    sync(resolve(cx.window_appearance()), cx);
}

/// Follow a window's appearance (it changed, or the setting did): the
/// palette, the terminals' colors (a Ghostty `theme = light:A,dark:B` has
/// one for each), and every window redrawn.
pub fn sync(dark: bool, cx: &mut App) {
    if !set_dark(dark, cx) {
        return;
    }
    cx.set_global(crate::config::Config::load());
    cx.refresh_windows();
}

/// Keep the palette following `window`'s appearance as macOS's changes.
pub fn follow(window: &Window) {
    window
        .observe_window_appearance(|window, cx| sync(resolve(window.appearance()), cx))
        .detach();
}
