//! ⌘Q asks before quitting, like cmux: a native alert sheet with a
//! "Don't warn again" checkbox. The choice lives in NSUserDefaults.
//! `abromux quit` and the debug `quit` step still quit straight away.

use crate::alert::{self, Confirm};
use gpui::{App, Window};

/// Quit, after asking unless the user turned the warning off.
pub fn request(window: &Window, cx: &mut App) {
    let rx = alert::confirm(
        window,
        Confirm {
            message: &format!("Quit {}?", crate::channel::app_name()),
            info: "This will close all windows and workspaces.",
            button: "Quit",
            suppress: "Don't warn again for Cmd+Q",
            skip_key: "AbroMuxSkipQuitWarning",
        },
    );
    cx.spawn(async move |cx| {
        if rx.await == Ok(true) {
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}
