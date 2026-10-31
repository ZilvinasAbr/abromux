#![allow(unexpected_cfgs)]
//! Native confirmation sheets, as cmux shows them: a message, a "Don't ask
//! again" checkbox, and Cancel next to the action. A ticked box is kept in
//! NSUserDefaults under the sheet's key, and only when the user confirms.

use futures::channel::oneshot;
use gpui::Window;
use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::Cell, ffi::CString};

const NS_ALERT_FIRST_BUTTON_RETURN: isize = 1000;

thread_local! {
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

pub struct Confirm<'a> {
    pub message: &'a str,
    pub info: &'a str,
    /// The action's button, e.g. "Close".
    pub button: &'a str,
    pub suppress: &'a str,
    /// NSUserDefaults key that turns the sheet off.
    pub skip_key: &'a str,
}

/// Ask on a sheet over `window`. Resolves to true to go ahead, at once if
/// the user turned this sheet off. While another sheet is up the request is
/// dropped (the receiver errors).
pub fn confirm(window: &Window, c: Confirm) -> oneshot::Receiver<bool> {
    let (tx, rx) = oneshot::channel();
    if skipped(c.skip_key) {
        let _ = tx.send(true);
        return rx;
    }
    let Some(ns_window) = ns_window(window) else {
        let _ = tx.send(true);
        return rx;
    };
    if OPEN.replace(true) {
        return rx;
    }
    let skip_key = c.skip_key.to_string();
    unsafe {
        let alert: *mut Object = msg_send![class!(NSAlert), new];
        let _: () = msg_send![alert, setMessageText: ns_string(c.message)];
        let _: () = msg_send![alert, setInformativeText: ns_string(c.info)];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: ns_string(c.button)];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: ns_string("Cancel")];
        let _: () = msg_send![alert, setShowsSuppressionButton: true];
        let suppression: *mut Object = msg_send![alert, suppressionButton];
        let _: () = msg_send![suppression, setTitle: ns_string(c.suppress)];

        let tx = Cell::new(Some(tx));
        let handler = block::ConcreteBlock::new(move |response: isize| {
            OPEN.set(false);
            let ok = response == NS_ALERT_FIRST_BUTTON_RETURN;
            let state: isize = msg_send![suppression, state];
            if ok && state != 0 {
                set_skipped(&skip_key);
            }
            if let Some(tx) = tx.take() {
                let _ = tx.send(ok);
            }
            let _: () = msg_send![alert, release];
        })
        .copy();
        let _: () =
            msg_send![alert, beginSheetModalForWindow: ns_window completionHandler: &*handler];
    }
    rx
}

fn ns_window(window: &Window) -> Option<*mut Object> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window).ok()?.as_raw() else {
        return None;
    };
    let win: *mut Object = unsafe { msg_send![h.ns_view.as_ptr() as *mut Object, window] };
    (!win.is_null()).then_some(win)
}

fn ns_string(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn skipped(key: &str) -> bool {
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        msg_send![defaults, boolForKey: ns_string(key)]
    }
}

fn set_skipped(key: &str) {
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let _: () = msg_send![defaults, setBool: true forKey: ns_string(key)];
    }
}
