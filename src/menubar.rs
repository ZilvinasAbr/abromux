#![allow(unexpected_cfgs)]
//! The menu-bar extra, as in cmux: a bell in the macOS menu bar with the
//! unread count, and a menu of the newest unread notifications plus Show
//! Notifications, Jump to Latest Unread, Mark All Read and Clear All.
//! `notifications.showInMenuBar: false` in config.json hides it.
//!
//! AppKit calls back on the main thread; picks go through a channel to the
//! app (notification.rs), like banner clicks.

use crate::banner::nsstring;
use futures::channel::mpsc;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Object, Sel},
    sel, sel_impl,
};
use std::{cell::Cell, sync::Mutex};

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

/// Menu item tags; a notification's is `NOTIFICATION + id`.
const FIXED: [(isize, Pick); 7] = [
    (1, Pick::ShowApp),
    (2, Pick::ShowNotifications),
    (3, Pick::JumpToUnread),
    (4, Pick::MarkAllRead),
    (5, Pick::ClearAll),
    (6, Pick::Settings),
    (7, Pick::Quit),
];
const NOTIFICATION: isize = 1000;

fn pick_of(tag: isize) -> Option<Pick> {
    if tag >= NOTIFICATION {
        return Some(Pick::Open((tag - NOTIFICATION) as usize));
    }
    FIXED.iter().find(|(t, _)| *t == tag).map(|(_, p)| *p)
}

fn tag_of(pick: Pick) -> isize {
    match pick {
        Pick::Open(id) => NOTIFICATION + id as isize,
        p => FIXED
            .iter()
            .find(|(_, x)| *x == p)
            .map(|(t, _)| *t)
            .unwrap_or(0),
    }
}

static PICKS: Mutex<Option<mpsc::UnboundedSender<Pick>>> = Mutex::new(None);

thread_local! {
    /// The NSStatusItem while shown, and the object its items call.
    static ITEM: Cell<*mut Object> = const { Cell::new(std::ptr::null_mut()) };
    static TARGET: Cell<*mut Object> = const { Cell::new(std::ptr::null_mut()) };
}

/// One unread notification in the menu.
pub struct Entry {
    pub id: usize,
    pub text: String,
}

/// Start listening; the receiver yields what the user picks.
pub fn init() -> mpsc::UnboundedReceiver<Pick> {
    let (tx, rx) = mpsc::unbounded();
    *PICKS.lock().unwrap() = Some(tx);
    rx
}

extern "C" fn picked(_: &Object, _: Sel, item: *mut Object) {
    let tag: isize = unsafe { msg_send![item, tag] };
    if let (Some(p), Some(tx)) = (pick_of(tag), PICKS.lock().unwrap().as_ref()) {
        let _ = tx.unbounded_send(p);
    }
}

fn target() -> *mut Object {
    TARGET.with(|t| {
        if t.get().is_null() {
            let mut decl = ClassDecl::new("AbroMuxMenuBarTarget", class!(NSObject))
                .expect("menu bar target class");
            unsafe {
                decl.add_method(
                    sel!(pick:),
                    picked as extern "C" fn(&Object, Sel, *mut Object),
                );
            }
            // Lives for the app: menu items hold their target weakly.
            t.set(unsafe { msg_send![decl.register(), new] });
        }
        t.get()
    })
}

/// Show the bell with `unread` and `recent` (newest first), or remove it.
pub fn update(show: bool, unread: usize, recent: &[Entry]) {
    let item = ITEM.with(Cell::get);
    unsafe {
        let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
        if !show {
            if !item.is_null() {
                let _: () = msg_send![bar, removeStatusItem: item];
                let _: () = msg_send![item, release];
                ITEM.with(|i| i.set(std::ptr::null_mut()));
            }
            return;
        }
        let item = if item.is_null() {
            // NSVariableStatusItemLength
            let item: *mut Object = msg_send![bar, statusItemWithLength: -1.0f64];
            let _: () = msg_send![item, retain];
            ITEM.with(|i| i.set(item));
            item
        } else {
            item
        };
        let button: *mut Object = msg_send![item, button];
        let symbol = if unread > 0 { "bell.badge" } else { "bell" };
        let image: *mut Object = msg_send![class!(NSImage),
            imageWithSystemSymbolName: nsstring(symbol)
            accessibilityDescription: nsstring("Notifications")];
        if !image.is_null() {
            let _: () = msg_send![image, setTemplate: true];
            let _: () = msg_send![button, setImage: image];
            // NSImageLeft
            let _: () = msg_send![button, setImagePosition: 2isize];
        }
        let count = if unread > 0 {
            unread.to_string()
        } else {
            String::new()
        };
        let _: () = msg_send![button, setTitle: nsstring(&count)];
        let _: () = msg_send![item, setMenu: menu(unread, recent)];
    }
}

unsafe fn menu(unread: usize, recent: &[Entry]) -> *mut Object {
    unsafe {
        let menu: *mut Object = msg_send![class!(NSMenu), alloc];
        let menu: *mut Object =
            msg_send![menu, initWithTitle: nsstring(crate::channel::app_name())];
        let _: () = msg_send![menu, setAutoenablesItems: false];
        let hint = match unread {
            0 => "No unread notifications".to_string(),
            1 => "1 unread notification".to_string(),
            n => format!("{n} unread notifications"),
        };
        add(menu, &hint, None);
        for e in recent {
            add(menu, &short(&e.text), Some(Pick::Open(e.id)));
        }
        separator(menu);
        add(
            menu,
            &format!("Show {}", crate::channel::app_name()),
            Some(Pick::ShowApp),
        );
        add(menu, "Show Notifications", Some(Pick::ShowNotifications));
        add(
            menu,
            "Jump to Latest Unread",
            (unread > 0).then_some(Pick::JumpToUnread),
        );
        add(
            menu,
            "Mark All Read",
            (unread > 0).then_some(Pick::MarkAllRead),
        );
        add(menu, "Clear All", Some(Pick::ClearAll));
        separator(menu);
        add(menu, "Settings…", Some(Pick::Settings));
        add(
            menu,
            &format!("Quit {}", crate::channel::app_name()),
            Some(Pick::Quit),
        );
        let _: () = msg_send![menu, autorelease];
        menu
    }
}

/// An item that reports `pick`, or a greyed-out one for None.
unsafe fn add(menu: *mut Object, title: &str, pick: Option<Pick>) {
    unsafe {
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item,
            initWithTitle: nsstring(title)
            action: sel!(pick:)
            keyEquivalent: nsstring("")];
        match pick {
            Some(p) => {
                let _: () = msg_send![item, setTarget: target()];
                let _: () = msg_send![item, setTag: tag_of(p)];
            }
            None => {
                let _: () = msg_send![item, setEnabled: false];
            }
        }
        let _: () = msg_send![menu, addItem: item];
        let _: () = msg_send![item, release];
    }
}

unsafe fn separator(menu: *mut Object) {
    unsafe {
        let sep: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: sep];
    }
}

/// One line of at most 60 characters.
fn short(text: &str) -> String {
    let line = crate::one_line(text);
    match line.char_indices().nth(59) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        for p in [
            Pick::ShowApp,
            Pick::Quit,
            Pick::ClearAll,
            Pick::Open(0),
            Pick::Open(42),
        ] {
            assert_eq!(pick_of(tag_of(p)), Some(p));
        }
        assert_eq!(pick_of(0), None);
    }

    #[test]
    fn long_lines_are_cut() {
        assert_eq!(short("a\nb"), "a b");
        let long = "x".repeat(80);
        assert_eq!(short(&long).chars().count(), 60);
        assert!(short(&long).ends_with('…'));
    }
}
