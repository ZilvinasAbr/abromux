//! Passkeys and security keys in browser panes. The page script
//! (`webauthn.js`) hands WebAuthn requests to a native bridge (`webauthn.m`)
//! that runs them with AuthenticationServices. That needs the
//! `com.apple.developer.web-browser.public-key-credential` entitlement; a
//! build without it leaves WebAuthn to WebKit.

use gpui_component::wry::{self, WebViewExtMacOS};
use std::ffi::{CString, c_char, c_void};

const SCRIPT: &str = include_str!("webauthn.js");

unsafe extern "C" {
    fn abromux_webauthn_install(webview: *mut c_void, script: *const c_char);
}

/// Add the bridge to a new webview, before its first navigation.
pub fn install(webview: &wry::WebView) {
    let wk = webview.webview();
    let script = CString::new(SCRIPT).expect("script has no NUL");
    unsafe { abromux_webauthn_install(&*wk as *const _ as *mut c_void, script.as_ptr()) };
}
