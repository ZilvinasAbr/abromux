//! The primary selection: the text last selected, which a middle click
//! pastes. X11 and Wayland have it; macOS doesn't, so there it holds
//! nothing.

use gpui::App;

/// Make `text` the primary selection.
pub fn write(cx: &App, text: String) {
    #[cfg(target_os = "linux")]
    cx.write_to_primary(gpui::ClipboardItem::new_string(text));
    #[cfg(not(target_os = "linux"))]
    let _ = (cx, text);
}

/// The primary selection's text, if there is one.
pub fn read(cx: &App) -> Option<String> {
    #[cfg(target_os = "linux")]
    return cx.read_from_primary().and_then(|item| item.text());
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        None
    }
}
