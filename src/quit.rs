#![allow(unexpected_cfgs)]
//! ⌘Q asks before quitting, like cmux: a native alert sheet with a
//! "Don't warn again" checkbox. The choice lives in NSUserDefaults.
//! `abromux quit` and the debug `quit` step still quit straight away.

use futures::channel::oneshot;
use gpui::{App, Window};
use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::Cell, ffi::CString};

const SKIP_KEY: &str = "AbroMuxSkipQuitWarning";
const NS_ALERT_FIRST_BUTTON_RETURN: isize = 1000;

thread_local! {
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Quit, after asking unless the user turned the warning off.
pub fn request(window: &Window, cx: &mut App) {
    if skip_warning() {
        return cx.quit();
    }
    let Some(ns_window) = ns_window(window) else { return cx.quit() };
    // A second ⌘Q while the sheet is up does nothing.
    if OPEN.replace(true) {
        return;
    }
    let (tx, rx) = oneshot::channel();
    unsafe {
        let alert: *mut Object = msg_send![class!(NSAlert), new];
        let _: () = msg_send![alert, setMessageText: ns_string("Quit AbroMux?")];
        let _: () = msg_send![alert, setInformativeText: ns_string("This will close all windows and workspaces.")];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: ns_string("Quit")];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: ns_string("Cancel")];
        let _: () = msg_send![alert, setShowsSuppressionButton: true];
        let suppression: *mut Object = msg_send![alert, suppressionButton];
        let _: () = msg_send![suppression, setTitle: ns_string("Don't warn again for Cmd+Q")];

        let tx = Cell::new(Some(tx));
        let handler = block::ConcreteBlock::new(move |response: isize| {
            let state: isize = msg_send![suppression, state];
            if let Some(tx) = tx.take() {
                let _ = tx.send((response == NS_ALERT_FIRST_BUTTON_RETURN, state != 0));
            }
            let _: () = msg_send![alert, release];
        })
        .copy();
        let _: () = msg_send![alert, beginSheetModalForWindow: ns_window completionHandler: &*handler];
    }
    cx.spawn(async move |cx| {
        let answer = rx.await;
        OPEN.set(false);
        let Ok((quit, dont_warn)) = answer else { return };
        if quit {
            // Only a confirmed quit remembers the checkbox, as in cmux.
            if dont_warn {
                set_skip_warning();
            }
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}

fn ns_window(window: &Window) -> Option<*mut Object> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
    let win: *mut Object = unsafe { msg_send![h.ns_view.as_ptr() as *mut Object, window] };
    (!win.is_null()).then_some(win)
}

fn ns_string(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn skip_warning() -> bool {
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        msg_send![defaults, boolForKey: ns_string(SKIP_KEY)]
    }
}

fn set_skip_warning() {
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let _: () = msg_send![defaults, setBool: true forKey: ns_string(SKIP_KEY)];
    }
}
