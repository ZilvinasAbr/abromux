//! Banners and the unread badge. Phase 2 posts freedesktop notifications
//! (Mako on Omarchy) over D-Bus.

use futures::channel::mpsc;

/// The receiver would yield the ids of clicked banners; none yet.
pub fn init() -> mpsc::UnboundedReceiver<usize> {
    mpsc::unbounded().1
}

pub fn post(_id: usize, _title: &str, _body: &str, _sound: bool) {}

pub fn set_badge(_count: usize) {}
