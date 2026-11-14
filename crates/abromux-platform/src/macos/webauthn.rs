//! The native WebAuthn bridge (`webauthn.m`): runs the page script's
//! WebAuthn requests with AuthenticationServices.

use objc::runtime::Object;
use std::ffi::{CString, c_char, c_void};

unsafe extern "C" {
    fn abromux_webauthn_install(webview: *mut c_void, script: *const c_char);
}

/// Add the bridge and its page `script` to the WKWebView `wk`, before its
/// first navigation.
pub fn install(wk: *mut Object, script: &str) {
    let script = CString::new(script).expect("script has no NUL");
    unsafe { abromux_webauthn_install(wk as *mut c_void, script.as_ptr()) };
}
