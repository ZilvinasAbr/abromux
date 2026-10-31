#![allow(unexpected_cfgs)]
//! macOS notification banners (UserNotifications) and the dock badge.
//! Banners need a bundle identifier, so they only show from AbroMux.app; a
//! bare `cargo run` binary just gets the in-app notifications and badge.

use futures::channel::mpsc;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Object, Sel},
    sel, sel_impl,
};
use std::{ffi::CString, sync::Mutex};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    fn NSLog(format: *mut Object, ...);
}

/// Where clicked banners send their notification id.
static CLICKS: Mutex<Option<mpsc::UnboundedSender<usize>>> = Mutex::new(None);

const ID_PREFIX: &str = "abromux-";

fn nsstring(s: &str) -> *mut Object {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn rust_string(s: *mut Object) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let p: *const std::ffi::c_char = unsafe { msg_send![s, UTF8String] };
    (!p.is_null()).then(|| {
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    })
}

/// Log to the unified log (Console.app, `log show`), where an app opened
/// from Finder can be heard; its stderr goes nowhere.
fn log(msg: &str) {
    unsafe { NSLog(nsstring("abromux: %@"), nsstring(msg)) }
}

/// An NSError's description, or None for nil.
fn error_text(err: *mut Object) -> Option<String> {
    (!err.is_null()).then(|| {
        let text: *mut Object = unsafe { msg_send![err, localizedDescription] };
        rust_string(text).unwrap_or_default()
    })
}

fn bundled() -> bool {
    let bundle: *mut Object = unsafe { msg_send![class!(NSBundle), mainBundle] };
    let id: *mut Object = unsafe { msg_send![bundle, bundleIdentifier] };
    !id.is_null()
}

fn center() -> Option<*mut Object> {
    // currentNotificationCenter throws outside a bundle.
    bundled()
        .then(|| unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] })
}

/// Ask for permission and listen for clicks. The receiver yields the id
/// passed to `post` for each banner the user clicks.
pub fn init() -> mpsc::UnboundedReceiver<usize> {
    let (tx, rx) = mpsc::unbounded();
    *CLICKS.lock().unwrap() = Some(tx);
    let Some(center) = center() else { return rx };
    unsafe {
        let mut decl = ClassDecl::new("AbroMuxNotificationDelegate", class!(NSObject))
            .expect("delegate class");
        decl.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object),
        );
        let delegate: *mut Object = msg_send![decl.register(), new];
        // The center holds its delegate weakly; this one lives for the app.
        let _: () = msg_send![center, setDelegate: delegate];
        let done = block::ConcreteBlock::new(|granted: bool, err: *mut Object| {
            match error_text(err) {
                Some(e) => log(&format!("notification permission failed: {e}")),
                None if !granted => log("notifications are turned off for this app"),
                None => {}
            }
        })
        .copy();
        // UNAuthorizationOptionBadge | Sound | Alert.
        let _: () =
            msg_send![center, requestAuthorizationWithOptions: 0b111u64 completionHandler: &*done];
    }
    rx
}

extern "C" fn did_receive(
    _: &Object,
    _: Sel,
    _center: *mut Object,
    response: *mut Object,
    handler: *mut Object,
) {
    unsafe {
        let notification: *mut Object = msg_send![response, notification];
        let request: *mut Object = msg_send![notification, request];
        let ident: *mut Object = msg_send![request, identifier];
        let id = rust_string(ident).and_then(|s| s.strip_prefix(ID_PREFIX)?.parse().ok());
        if let (Some(id), Some(tx)) = (id, CLICKS.lock().unwrap().as_ref()) {
            let _ = tx.unbounded_send(id);
        }
        if !handler.is_null() {
            (*(handler as *mut block::Block<(), ()>)).call(());
        }
    }
}

/// Show a banner; clicking it reports `id` through `init`'s receiver.
pub fn post(id: usize, title: &str, body: &str) {
    let Some(center) = center() else { return };
    unsafe {
        let content: *mut Object = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: nsstring(title)];
        let _: () = msg_send![content, setBody: nsstring(body)];
        let sound: *mut Object = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![content, setSound: sound];
        let ident = nsstring(&format!("{ID_PREFIX}{id}"));
        let nil: *mut Object = std::ptr::null_mut();
        let request: *mut Object = msg_send![class!(UNNotificationRequest), requestWithIdentifier: ident content: content trigger: nil];
        let done = block::ConcreteBlock::new(|err: *mut Object| {
            if let Some(e) = error_text(err) {
                log(&format!("banner not shown: {e}"));
            }
        })
        .copy();
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: &*done];
        let _: () = msg_send![content, release];
    }
}

/// The dock icon's badge: the unread count, or none for 0.
pub fn set_badge(count: usize) {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let tile: *mut Object = msg_send![app, dockTile];
        let label = if count == 0 {
            std::ptr::null_mut()
        } else {
            nsstring(&count.to_string())
        };
        let _: () = msg_send![tile, setBadgeLabel: label];
    }
}
