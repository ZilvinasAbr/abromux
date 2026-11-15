//! Banners through the desktop's notification server
//! (`org.freedesktop.Notifications` over D-Bus: Mako on Omarchy, dunst,
//! GNOME Shell). Each has a `default` action, which servers run when the
//! banner is clicked, so a click reports its id like a macOS banner click,
//! and brings the window forward (`window::raise`).

use futures::channel::mpsc;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

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
                super::window::raise();
                let _ = tx.unbounded_send(id);
            }
        });
    });
}

/// The unread count on the app's launcher icon, none for 0, through the
/// Unity launcher API: docks that read it (Dash to Dock, Plasma's task
/// manager) show it on the icon of `<app_id>.desktop`. Omarchy has none, so
/// it's best effort and errors are dropped.
pub fn set_badge(count: usize) {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    let Some(bus) = BUS
        .get_or_init(|| zbus::blocking::Connection::session().ok())
        .as_ref()
    else {
        return;
    };
    let uri = format!("application://{}.desktop", crate::channel::app_id());
    let props: HashMap<&str, zbus::zvariant::Value> = HashMap::from([
        ("count", zbus::zvariant::Value::from(count as i64)),
        ("count-visible", zbus::zvariant::Value::from(count > 0)),
    ]);
    let _ = bus.emit_signal(
        None::<()>,
        "/dev/abromux/launcher",
        "com.canonical.Unity.LauncherEntry",
        "Update",
        &(uri, props),
    );
}
