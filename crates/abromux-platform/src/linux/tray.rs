//! The tray bell: a StatusNotifierItem over D-Bus (ksni), which the Omarchy
//! shell's tray (or Waybar's `tray` module) shows, as do KDE and GNOME with the AppIndicator
//! extension. Its menu is the macOS menu-bar bell's. Off unless
//! `notifications.showInMenuBar` turns it on.

use futures::channel::mpsc;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};
use std::sync::Mutex;

/// A menu item the user chose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    ShowApp,
    ShowNotifications,
    JumpToUnread,
    MarkAllRead,
    ClearAll,
    Settings,
    Quit,
    /// A notification, by id.
    Open(usize),
}

/// One unread notification in the menu.
#[derive(Clone)]
pub struct Entry {
    pub id: usize,
    /// Its menu item's title: one short line.
    pub text: String,
}

static PICKS: Mutex<Option<mpsc::UnboundedSender<Pick>>> = Mutex::new(None);
static BELL: Mutex<Option<Handle<Bell>>> = Mutex::new(None);

/// Start listening; the receiver yields what the user picks.
pub fn init() -> mpsc::UnboundedReceiver<Pick> {
    let (tx, rx) = mpsc::unbounded();
    *PICKS.lock().unwrap() = Some(tx);
    rx
}

/// Show the bell with `unread` and `recent` (newest first), or remove it.
pub fn update(show: bool, unread: usize, recent: &[Entry]) {
    let mut bell = BELL.lock().unwrap();
    if !show {
        if let Some(h) = bell.take() {
            let _ = h.shutdown();
        }
        return;
    }
    let recent = recent.to_vec();
    if let Some(h) = bell.as_ref().filter(|h| !h.is_closed()) {
        h.update(|b| {
            b.unread = unread;
            b.recent = recent;
        });
        return;
    }
    match (Bell { unread, recent }).spawn() {
        Ok(h) => *bell = Some(h),
        // No StatusNotifierWatcher (no tray in the bar): nothing to show.
        Err(e) => eprintln!("abromux: tray: {e}"),
    }
}

fn pick(p: Pick) {
    if p == Pick::ShowApp {
        super::window::raise();
    }
    if let Some(tx) = PICKS.lock().unwrap().as_ref() {
        let _ = tx.unbounded_send(p);
    }
}

struct Bell {
    unread: usize,
    recent: Vec<Entry>,
}

impl ksni::Tray for Bell {
    fn id(&self) -> String {
        crate::channel::app_id()
    }

    fn title(&self) -> String {
        match self.unread {
            0 => crate::channel::app_name().into(),
            n => format!("{} ({n})", crate::channel::app_name()),
        }
    }

    /// Drawn here rather than named: icon themes differ in what they call a
    /// bell, and a missing name shows as a broken image.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 44]
            .into_iter()
            .map(|size| bell_icon(size, self.unread > 0))
            .collect()
    }

    fn status(&self) -> ksni::Status {
        if self.unread > 0 {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title(),
            description: hint(self.unread),
            ..Default::default()
        }
    }

    /// A click on the bell shows the app.
    fn activate(&mut self, _x: i32, _y: i32) {
        pick(Pick::ShowApp);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let app = crate::channel::app_name();
        let item = |label: String, p: Option<Pick>| -> MenuItem<Self> {
            StandardItem {
                label,
                enabled: p.is_some(),
                activate: Box::new(move |_: &mut Self| {
                    if let Some(p) = p {
                        pick(p);
                    }
                }),
                ..Default::default()
            }
            .into()
        };
        let unread = (self.unread > 0).then_some(());
        let mut items = vec![item(hint(self.unread), None)];
        items.extend(
            self.recent
                .iter()
                .map(|e| item(e.text.clone(), Some(Pick::Open(e.id)))),
        );
        items.extend([
            MenuItem::Separator,
            item(format!("Show {app}"), Some(Pick::ShowApp)),
            item("Show Notifications".into(), Some(Pick::ShowNotifications)),
            item(
                "Jump to Latest Unread".into(),
                unread.map(|()| Pick::JumpToUnread),
            ),
            item("Mark All Read".into(), unread.map(|()| Pick::MarkAllRead)),
            item("Clear All".into(), Some(Pick::ClearAll)),
            MenuItem::Separator,
            item("Settings…".into(), Some(Pick::Settings)),
            item(format!("Quit {app}"), Some(Pick::Quit)),
        ]);
        items
    }
}

fn hint(unread: usize) -> String {
    match unread {
        0 => "No unread notifications".into(),
        1 => "1 unread notification".into(),
        n => format!("{n} unread notifications"),
    }
}

/// A bell, light grey for dark bars, with a red dot for unread: `size`
/// pixels square, ARGB32 (big endian, as StatusNotifierItem wants).
fn bell_icon(size: i32, dot: bool) -> ksni::Icon {
    let k = size as f32 / 22.;
    let inside = |x: f32, y: f32| {
        let (x, y) = (x / k, y / k);
        let cx = 11.;
        let dx = (x - cx).abs();
        let dome = (x - cx).powi(2) + (y - 7.).powi(2) <= 4.2f32.powi(2) && y <= 7.;
        let body = (7. ..=15.5).contains(&y) && dx <= 4.2 + (y - 7.) * 0.28;
        let rim = (15.5..=17.2).contains(&y) && dx <= 8.;
        let handle = (1.8..=3.).contains(&y) && dx <= 1.;
        let clapper = (x - cx).powi(2) + (y - 18.8).powi(2) <= 1.9f32.powi(2);
        dome || body || rim || handle || clapper
    };
    let red =
        |x: f32, y: f32| dot && (x / k - 17.).powi(2) + (y / k - 5.).powi(2) <= 3.6f32.powi(2);
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            // 4 samples per pixel, for soft edges.
            let (mut bell, mut dotted) = (0u32, 0u32);
            for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (x, y) = (px as f32 + ox, py as f32 + oy);
                if red(x, y) {
                    dotted += 1;
                } else if inside(x, y) {
                    bell += 1;
                }
            }
            let (a, r, g, b) = if dotted > 0 {
                ((dotted * 255 / 4) as u8, 0xe5, 0x53, 0x3d)
            } else {
                ((bell * 255 / 4) as u8, 0xe8, 0xe8, 0xe8)
            };
            data.extend([a, r, g, b]);
        }
    }
    ksni::Icon {
        width: size,
        height: size,
        data,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bell_icons_are_argb_squares() {
        for (size, dot) in [(22, false), (44, true)] {
            let icon = super::bell_icon(size, dot);
            assert_eq!(icon.data.len(), (size * size * 4) as usize);
            // Some pixels drawn, some left clear.
            assert!(icon.data.chunks(4).any(|p| p[0] == 255));
            assert!(icon.data.chunks(4).any(|p| p[0] == 0));
        }
    }
}
