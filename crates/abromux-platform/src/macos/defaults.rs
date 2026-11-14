//! NSUserDefaults, read only: where macOS builds kept their prefs before
//! `prefs.json` (see [`crate::prefs::import_once`]).

use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use std::ffi::CString;

/// Whether this process has the app's own defaults: it runs from a bundle,
/// whose identifier names its domain. A bare binary (`cargo run`,
/// scripts/perf.py) gets a domain of its own instead.
pub fn bundled() -> bool {
    unsafe {
        let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
        let id: *mut Object = msg_send![bundle, bundleIdentifier];
        !id.is_null()
    }
}

/// The boolean NSUserDefaults has for `key`; false when unset.
pub fn bool(key: &str) -> bool {
    let Ok(key) = CString::new(key) else {
        return false;
    };
    unsafe {
        let key: *mut Object = msg_send![class!(NSString), stringWithUTF8String: key.as_ptr()];
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        msg_send![defaults, boolForKey: key]
    }
}
