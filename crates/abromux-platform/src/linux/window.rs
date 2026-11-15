//! Window calls. Dragging needs gpui's own `start_window_move` on Wayland
//! (Phase 2); the debug snapshots' synthetic input is macOS-only.

use raw_window_handle::HasWindowHandle;
use std::process::Command;

pub type Error = &'static str;

const UNSUPPORTED: Error = "not supported on Linux";

pub fn begin_drag(_window: &impl HasWindowHandle) {}

pub fn number(_window: &impl HasWindowHandle) -> Result<isize, Error> {
    Err(UNSUPPORTED)
}

/// A synthetic mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Down,
    Up,
    Move,
    Drag,
}

pub fn post_mouse(
    _window: &impl HasWindowHandle,
    _mouse: Mouse,
    _x: f64,
    _y: f64,
    _right: bool,
    _clicks: isize,
) -> Result<(), Error> {
    Err(UNSUPPORTED)
}

pub fn post_scroll(
    _window: &impl HasWindowHandle,
    _x: f64,
    _y: f64,
    _dy: f64,
) -> Result<(), Error> {
    Err(UNSUPPORTED)
}

/// Bring the app's window forward, as macOS does on a banner or menu-bar
/// click. Wayland
/// lets an app do that itself only with an activation token, which the
/// notification server has but notify-rust doesn't pass on (and gpui can't
/// take), so ask the compositor: Hyprland's and sway's IPC, by `app_id`.
/// Elsewhere the click still shows the terminal, behind other windows.
pub(crate) fn raise() {
    let pattern = format!("^{}$", crate::channel::app_id().replace('.', "\\."));
    let run = |cmd: &mut Command| {
        if let Err(e) = cmd.output() {
            eprintln!("abromux: raising the window: {e}");
        }
    };
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        run(Command::new("hyprctl").args(["dispatch", "focuswindow", &format!("class:{pattern}")]));
    } else if std::env::var_os("SWAYSOCK").is_some() {
        run(Command::new("swaymsg").arg(format!("[app_id=\"{pattern}\"] focus")));
    }
}
