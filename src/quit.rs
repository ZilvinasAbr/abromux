//! ⌘Q asks before quitting, like cmux: an alert sheet with a "Don't warn
//! again" checkbox. The choice lives in the prefs file.
//! `abromux quit` and the debug `quit` step still quit straight away.

use crate::alert::{self, Confirm};
use gpui::{App, Window};

/// The prefs key of "Don't warn again for Cmd+Q".
pub const SKIP_KEY: &str = "AbroMuxSkipQuitWarning";

/// Quit, after asking unless the user turned the warning off.
pub fn request(window: &mut Window, cx: &mut App) {
    let key = if abromux_platform::MACOS {
        "Cmd+Q".to_string()
    } else {
        crate::keymap::default_label("quit")
    };
    let rx = alert::confirm(
        window,
        cx,
        Confirm {
            message: &format!("Quit {}?", crate::channel::app_name()),
            info: "This will close all windows and workspaces.",
            button: "Quit",
            suppress: &format!("Don't warn again for {key}"),
            skip_key: SKIP_KEY,
        },
    );
    cx.spawn(async move |cx| {
        if rx.await == Ok(true) {
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}
