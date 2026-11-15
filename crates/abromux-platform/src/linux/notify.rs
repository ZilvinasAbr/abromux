//! Banners through the desktop's notification server
//! (`org.freedesktop.Notifications` over D-Bus: Mako on Omarchy, dunst,
//! GNOME Shell). Each has a `default` action, which servers run when the
//! banner is clicked, so a click reports its id like a macOS banner click,
//! and brings the window forward ([`raise`]).

use futures::channel::mpsc;
use std::{process::Command, sync::Mutex};

static CLICKS: Mutex<Option<mpsc::UnboundedSender<usize>>> = Mutex::new(None);

/// Start listening; the receiver yields the id passed to `post` for each
/// banner the user clicks.
pub fn init() -> mpsc::UnboundedReceiver<usize> {
    let (tx, rx) = mpsc::unbounded();
    *CLICKS.lock().unwrap() = Some(tx);
    rx
}

/// Show a banner; clicking it reports `id` through `init`'s receiver. The
/// D-Bus call and the wait for a click run on a thread of their own.
pub fn post(id: usize, title: &str, body: &str, sound: bool) {
    let mut n = notify_rust::Notification::new();
    n.appname(crate::channel::app_name())
        .summary(title)
        .body(body)
        .icon(&crate::channel::app_id())
        .action("default", "Show");
    if sound {
        n.sound_name("message-new-instant");
    }
    std::thread::spawn(move || {
        let handle = match n.show() {
            Ok(h) => h,
            Err(e) => return eprintln!("abromux: notification: {e}"),
        };
        handle.wait_for_action(|action| {
            if action == "default"
                && let Some(tx) = CLICKS.lock().unwrap().as_ref()
            {
                raise();
                let _ = tx.unbounded_send(id);
            }
        });
    });
}

/// Bring the app's window forward, as macOS does on a banner click. Wayland
/// lets an app do that itself only with an activation token, which the
/// notification server has but notify-rust doesn't pass on (and gpui can't
/// take), so ask the compositor: Hyprland's and sway's IPC, by `app_id`.
/// Elsewhere the click still shows the terminal, behind other windows.
fn raise() {
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

/// The unread count on the app's launcher icon: not yet. The Unity launcher
/// API would show it in docks that read it (Dash to Dock, Plasma); Omarchy
/// has none.
pub fn set_badge(_count: usize) {}
