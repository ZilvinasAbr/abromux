//! The tray bell. Phase 2 makes it a StatusNotifierItem (Waybar's tray).

use futures::channel::mpsc;

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
pub struct Entry {
    pub id: usize,
    /// Its menu item's title: one short line.
    pub text: String,
}

/// The receiver would yield what the user picks; nothing yet.
pub fn init() -> mpsc::UnboundedReceiver<Pick> {
    mpsc::unbounded().1
}

pub fn update(_show: bool, _unread: usize, _recent: &[Entry]) {}
